//! Provider credentials entered in the browser. They live in the registry database, next to the
//! push keys, so they never reach `config.toml` or a response body.

use crate::app::Shared;
use ostra_core::api::ProviderCredentialsEdit;
use ostra_core::config::{SavedCredentials, ValidationIssue};
use ostra_store::{RegistryDb, StoreError};
use std::collections::BTreeMap;

const PREFIX: &str = "provider_credentials:";

pub fn load_all(registry: &RegistryDb) -> Result<BTreeMap<String, SavedCredentials>, StoreError> {
    let mut out = BTreeMap::new();
    for (key, bytes) in registry.kv_scan(PREFIX)? {
        match serde_json::from_slice(&bytes) {
            Ok(saved) => {
                out.insert(key[PREFIX.len()..].to_string(), saved);
            }
            Err(e) => tracing::warn!("ignoring unreadable saved credentials under {key}: {e}"),
        }
    }
    Ok(out)
}

pub fn load(registry: &RegistryDb, provider: &str) -> Result<SavedCredentials, StoreError> {
    Ok(load_all(registry)?.remove(provider).unwrap_or_default())
}

pub fn save(registry: &RegistryDb, provider: &str, saved: &SavedCredentials) -> Result<(), StoreError> {
    let key = format!("{PREFIX}{provider}");
    if saved.is_empty() {
        registry.kv_delete(&key)?;
    } else {
        registry.kv_set(&key, &serde_json::to_vec(saved).expect("credentials serialize"))?;
    }
    Ok(())
}

fn issue(path: &str, message: impl Into<String>) -> ValidationIssue {
    ValidationIssue { path: path.into(), message: message.into() }
}

/// `None` keeps the field, an empty string clears it, and anything else replaces it.
fn merge(field: &mut Option<String>, edit: &Option<String>) {
    if let Some(value) = edit {
        let value = value.trim();
        *field = (!value.is_empty()).then(|| value.to_string());
    }
}

/// Apply an edit on top of what is saved, and list every problem with the result.
pub fn apply(mut saved: SavedCredentials, edit: &ProviderCredentialsEdit) -> Result<SavedCredentials, Vec<ValidationIssue>> {
    merge(&mut saved.base_url, &edit.base_url);
    merge(&mut saved.api_key, &edit.api_key);
    merge(&mut saved.auth_token, &edit.auth_token);
    let mut issues = vec![];
    if let Some(url) = &saved.base_url {
        match url::Url::parse(url) {
            Ok(u) if matches!(u.scheme(), "http" | "https") && u.host().is_some() => {}
            _ => issues.push(issue("base_url", "Enter the base URL as http:// or https:// followed by a host, for example https://api.anthropic.com.")),
        }
    }
    for (path, value) in [("api_key", &saved.api_key), ("auth_token", &saved.auth_token)] {
        if value.as_deref().is_some_and(|v| v.chars().any(|c| c.is_whitespace() || c.is_control())) {
            issues.push(issue(path, "Paste the value without spaces or line breaks, because it is sent as an HTTP header."));
        }
    }
    if issues.is_empty() { Ok(saved) } else { Err(issues) }
}

impl Shared {
    /// Rebuild the providers from the current global config and the saved credentials.
    pub fn reload_providers(&self) -> Result<(), StoreError> {
        let saved = load_all(&self.registry)?;
        self.providers.reload(&self.global(), &saved);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_merge_and_clear() {
        let saved = SavedCredentials { base_url: Some("https://a.example".into()), api_key: Some("k".into()), auth_token: None };
        let edit = ProviderCredentialsEdit { base_url: Some(" ".into()), auth_token: Some(" t ".into()), ..Default::default() };
        let out = apply(saved, &edit).unwrap();
        assert_eq!(out.base_url, None);
        assert_eq!(out.api_key.as_deref(), Some("k"));
        assert_eq!(out.auth_token.as_deref(), Some("t"));
    }

    #[test]
    fn bad_values_are_issues() {
        let edit = ProviderCredentialsEdit { base_url: Some("api.example".into()), api_key: Some("a b".into()), ..Default::default() };
        let issues = apply(SavedCredentials::default(), &edit).unwrap_err();
        let paths: Vec<_> = issues.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(paths, ["base_url", "api_key"]);
    }

    #[test]
    fn round_trips_through_the_registry() {
        let reg = RegistryDb::open_in_memory().unwrap();
        let saved = SavedCredentials { api_key: Some("k".into()), ..Default::default() };
        save(&reg, "anthropic", &saved).unwrap();
        assert_eq!(load(&reg, "anthropic").unwrap(), saved);
        save(&reg, "anthropic", &SavedCredentials::default()).unwrap();
        assert!(load_all(&reg).unwrap().is_empty());
    }
}
