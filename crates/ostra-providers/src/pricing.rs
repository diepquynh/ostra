//! Per-model prices in USD per million tokens, used to compute `Usage::cost_usd`.
//!
//! Anthropic rates come from the claude-api reference (cached 2026-06-24). OpenAI GPT-5.6 rates
//! are the standard short-context rates after the 2026-07-30 price change, from third-party
//! trackers; check the official pricing page before relying on them.

use ostra_core::exec::Usage;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pricing {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    /// 5-minute cache writes.
    pub cache_write: f64,
    pub cache_write_1h: f64,
    /// Per server-side web search.
    pub web_search: f64,
}

const fn anthropic(input: f64, output: f64, cache_read: f64) -> Pricing {
    Pricing { input, output, cache_read, cache_write: input * 1.25, cache_write_1h: input * 2.0, web_search: 0.01 }
}

const fn openai(input: f64, output: f64, cache_read: f64) -> Pricing {
    Pricing { input, output, cache_read, cache_write: input * 1.25, cache_write_1h: input * 1.25, web_search: 0.01 }
}

/// Price for a bare model id. Dated snapshot suffixes match their base id.
pub fn price(model: &str) -> Option<Pricing> {
    let table: &[(&str, Pricing)] = &[
        ("claude-fable-5-1", anthropic(10.0, 50.0, 0.25)),
        ("claude-mythos-5-1", anthropic(10.0, 50.0, 1.0)),
        ("claude-fable-5", anthropic(10.0, 50.0, 1.0)),
        ("claude-mythos-5", anthropic(10.0, 50.0, 1.0)),
        ("claude-opus-5-5", anthropic(4.0, 20.0, 0.20)),
        ("claude-opus-5", anthropic(5.0, 25.0, 0.5)),
        ("claude-opus-4-8", anthropic(5.0, 25.0, 0.5)),
        ("claude-opus-4-7", anthropic(5.0, 25.0, 0.5)),
        ("claude-opus-4-6", anthropic(5.0, 25.0, 0.5)),
        ("claude-sonnet-5", anthropic(2.0, 10.0, 0.2)),
        ("claude-sonnet-4-6", anthropic(3.0, 15.0, 0.3)),
        ("claude-haiku-4-5", anthropic(1.0, 5.0, 0.1)),
        ("gpt-5.6-sol", openai(5.0, 30.0, 0.5)),
        ("gpt-5.6-terra", openai(2.0, 12.0, 0.2)),
        ("gpt-5.6-luna", openai(0.2, 1.2, 0.02)),
        ("gpt-5.6", openai(5.0, 30.0, 0.5)),
    ];
    // Longest matching prefix wins, so `claude-opus-5-5` is not priced as `claude-opus-5`.
    table
        .iter()
        .filter(|(id, _)| model == *id || model.strip_prefix(id).is_some_and(|rest| rest.starts_with('-')))
        .max_by_key(|(id, _)| id.len())
        .map(|(_, p)| *p)
}

/// Cost of one response. `input_tokens` excludes cache reads and writes, as both providers'
/// usage is normalized to before this is called. `cache_write_1h_tokens` is the part of
/// `cache_write_tokens` written with the 1-hour TTL; the rest are 5-minute writes.
pub fn cost(model: &str, usage: &Usage, cache_write_1h_tokens: u64, web_searches: u64) -> f64 {
    let Some(p) = price(model) else { return 0.0 };
    let m = 1_000_000.0;
    let write_1h = cache_write_1h_tokens.min(usage.cache_write_tokens);
    usage.input_tokens as f64 * p.input / m
        + usage.output_tokens as f64 * p.output / m
        + usage.cache_read_tokens as f64 * p.cache_read / m
        + (usage.cache_write_tokens - write_1h) as f64 * p.cache_write / m
        + write_1h as f64 * p.cache_write_1h / m
        + web_searches as f64 * p.web_search
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_prefix() {
        assert_eq!(price("claude-opus-5-5").unwrap().input, 4.0);
        assert_eq!(price("claude-opus-5").unwrap().input, 5.0);
        assert_eq!(price("claude-haiku-4-5-20251001").unwrap().input, 1.0);
        assert_eq!(price("claude-fable-5-1").unwrap().cache_read, 0.25);
        assert!(price("claude-opus-50").is_none());
        assert!(price("unknown").is_none());
    }

    #[test]
    fn computes_cost() {
        let u = Usage { input_tokens: 1_000_000, output_tokens: 1_000_000, cache_read_tokens: 1_000_000, ..Default::default() };
        let c = cost("claude-sonnet-5", &u, 0, 1);
        assert!((c - (2.0 + 10.0 + 0.2 + 0.01)).abs() < 1e-9);
    }

    #[test]
    fn cache_writes_priced_by_ttl() {
        let u = Usage { cache_write_tokens: 3_000_000, ..Default::default() };
        let c = cost("claude-opus-5", &u, 1_000_000, 0);
        assert!((c - (2.0 * 6.25 + 10.0)).abs() < 1e-9);
    }
}
