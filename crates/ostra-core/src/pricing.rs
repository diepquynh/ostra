//! Per-model prices, used to compute `Usage::cost_usd`. Prices come from the models.dev catalog
//! (<https://models.dev/api.json>), which the server fetches and caches on disk and installs here
//! with [`install`]. Until a catalog is installed every cost is 0.

use crate::exec::Usage;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub const MODELS_DEV_URL: &str = "https://models.dev/api.json";

/// Per server-side web search, on both providers.
const WEB_SEARCH_USD: f64 = 0.01;

/// Providers whose own listing wins when resellers list the same model id at other prices.
const FIRST_PARTY: &[&str] = &["anthropic", "openai", "xai", "google", "mistral", "deepseek"];

/// USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    /// 5-minute cache writes.
    pub cache_write: f64,
    pub cache_write_1h: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pricing {
    pub base: Rates,
    /// Rates that replace `base` when a request's prompt is longer than the size, smallest first.
    pub tiers: Vec<(u64, Rates)>,
}

impl Pricing {
    /// The rates for one request whose prompt (input, cache reads, and cache writes) has this many
    /// tokens.
    pub fn rates(&self, prompt_tokens: u64) -> Rates {
        self.tiers.iter().rev().find(|(size, _)| prompt_tokens > *size).map(|(_, r)| *r).unwrap_or(self.base)
    }
}

#[derive(Deserialize)]
struct RawProvider {
    #[serde(default)]
    models: HashMap<String, RawModel>,
}

#[derive(Deserialize)]
struct RawModel {
    cost: Option<RawCost>,
}

#[derive(Deserialize)]
struct RawCost {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
    #[serde(default)]
    tiers: Vec<RawTier>,
}

#[derive(Deserialize)]
struct RawTier {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
    tier: Option<TierBound>,
}

#[derive(Deserialize)]
struct TierBound {
    #[serde(rename = "type")]
    kind: String,
    size: u64,
}

fn rates(provider: &str, input: f64, output: f64, cache_read: Option<f64>, cache_write: Option<f64>) -> Rates {
    let cache_write = cache_write.unwrap_or(input * 1.25);
    // models.dev lists the 5-minute write only. Anthropic prices 1-hour writes at twice the input
    // rate; other providers have one write TTL.
    let cache_write_1h = if provider == "anthropic" { input * 2.0 } else { cache_write };
    Rates { input, output, cache_read: cache_read.unwrap_or(input), cache_write, cache_write_1h }
}

/// Prices by provider and model id.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Catalog {
    providers: HashMap<String, HashMap<String, Pricing>>,
}

impl Catalog {
    /// Parse the models.dev `api.json` document. Models without input and output prices are left
    /// out.
    pub fn from_models_dev(json: &str) -> Result<Catalog, serde_json::Error> {
        let raw: HashMap<String, RawProvider> = serde_json::from_str(json)?;
        let providers = raw
            .into_iter()
            .map(|(provider, p)| {
                let models = p
                    .models
                    .into_iter()
                    .filter_map(|(id, m)| {
                        let c = m.cost?;
                        let base = rates(&provider, c.input?, c.output?, c.cache_read, c.cache_write);
                        let mut tiers: Vec<(u64, Rates)> = c
                            .tiers
                            .iter()
                            .filter_map(|t| {
                                let bound = t.tier.as_ref().filter(|b| b.kind == "context")?;
                                Some((bound.size, rates(&provider, t.input?, t.output?, t.cache_read, t.cache_write)))
                            })
                            .collect();
                        tiers.sort_by_key(|(size, _)| *size);
                        Some((id, Pricing { base, tiers }))
                    })
                    .collect();
                (provider, models)
            })
            .collect();
        Ok(Catalog { providers })
    }

    pub fn is_empty(&self) -> bool {
        self.providers.values().all(HashMap::is_empty)
    }

    /// Price for a model id as a provider reports it, with or without a `provider:` prefix. A
    /// first-party listing wins over resellers, and a dated snapshot (`claude-haiku-4-5-20251001`)
    /// falls back to its base id.
    pub fn price(&self, model: &str) -> Option<&Pricing> {
        let (hint, id) = match model.split_once(':') {
            Some((p, id)) => (Some(p), id),
            None => (None, model),
        };
        let first_party = hint.into_iter().chain(FIRST_PARTY.iter().copied());
        let exact = |p: &str| self.providers.get(p).and_then(|m| m.get(id));
        if let Some(p) = first_party.clone().find_map(exact) {
            return Some(p);
        }
        // Longest matching prefix wins, so `claude-opus-5-5-x` is not priced as `claude-opus-5`.
        let prefixed = first_party
            .filter_map(|p| self.providers.get(p))
            .flat_map(|m| m.iter())
            .filter(|(base, _)| id.strip_prefix(base.as_str()).is_some_and(|rest| rest.starts_with('-')))
            .max_by_key(|(base, _)| base.len())
            .map(|(_, p)| p);
        prefixed.or_else(|| {
            let mut names: Vec<&String> = self.providers.keys().collect();
            names.sort();
            names.into_iter().find_map(|p| self.providers[p].get(id))
        })
    }
}

static CATALOG: RwLock<Option<Arc<Catalog>>> = RwLock::new(None);

/// Make `catalog` the process's prices.
pub fn install(catalog: Catalog) {
    *CATALOG.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(catalog));
}

pub fn catalog() -> Option<Arc<Catalog>> {
    CATALOG.read().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn price(model: &str) -> Option<Pricing> {
    catalog().and_then(|c| c.price(model).cloned())
}

/// Install a sample of real models.dev entries, for tests in any crate.
#[cfg(any(test, feature = "test-prices"))]
pub fn install_test_prices() {
    let sample = include_str!("../tests/fixtures/models-dev-sample.json");
    install(Catalog::from_models_dev(sample).expect("the sample parses"));
}

/// Cost of one response. `input_tokens` excludes cache reads and writes, as every provider's and
/// harness transcript's usage is normalized to before this is called. The part of
/// `cache_write_tokens` not in `cache_write_1h_tokens` is priced as 5-minute writes.
pub fn cost(model: &str, usage: &Usage, web_searches: u64) -> f64 {
    let Some(p) = price(model) else { return 0.0 };
    let r = p.rates(usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens);
    let m = 1_000_000.0;
    let write_1h = usage.cache_write_1h_tokens.min(usage.cache_write_tokens);
    usage.input_tokens as f64 * r.input / m
        + usage.output_tokens as f64 * r.output / m
        + usage.cache_read_tokens as f64 * r.cache_read / m
        + (usage.cache_write_tokens - write_1h) as f64 * r.cache_write / m
        + write_1h as f64 * r.cache_write_1h / m
        + web_searches as f64 * WEB_SEARCH_USD
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Catalog {
        Catalog::from_models_dev(include_str!("../tests/fixtures/models-dev-sample.json")).unwrap()
    }

    #[test]
    fn looks_up_first_party_then_prefix() {
        let c = sample();
        assert_eq!(c.price("claude-opus-5-5").unwrap().base.input, 4.0);
        assert_eq!(c.price("claude-opus-5").unwrap().base.input, 5.0, "anthropic's listing beats a reseller's");
        assert_eq!(c.price("anthropic:claude-opus-5").unwrap().base.input, 5.0);
        assert_eq!(c.price("claude-sonnet-5-20260901").unwrap().base.input, 2.0);
        assert_eq!(c.price("claude-fable-5-1").unwrap().base.cache_read, 0.25);
        assert!(c.price("claude-opus-50").is_none());
        assert!(c.price("unknown").is_none());
    }

    #[test]
    fn derives_one_hour_writes_and_reads_context_tiers() {
        let c = sample();
        let opus = c.price("claude-opus-5").unwrap().base;
        assert_eq!((opus.cache_write, opus.cache_write_1h), (6.25, 10.0));
        let sol = c.price("gpt-5.6-sol").unwrap();
        assert_eq!(sol.rates(272_000).input, sol.base.input);
        assert_eq!(sol.rates(272_001).input, 8.0);
        assert_eq!(sol.base.cache_write_1h, sol.base.cache_write);
    }

    #[test]
    fn prices_cache_writes_by_ttl() {
        install_test_prices();
        let u = Usage { cache_write_tokens: 3_000_000, cache_write_1h_tokens: 1_000_000, ..Default::default() };
        let c = cost("claude-opus-5", &u, 0);
        assert!((c - (2.0 * 6.25 + 10.0)).abs() < 1e-9);
        let u = Usage { input_tokens: 1_000_000, output_tokens: 1_000_000, cache_read_tokens: 1_000_000, ..Default::default() };
        assert!((cost("claude-sonnet-5", &u, 1) - (2.0 + 10.0 + 0.2 + 0.01)).abs() < 1e-9);
    }
}
