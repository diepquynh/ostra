use crate::StoreError;
use chrono::{DateTime, Utc};
use serde::{Serialize, de::DeserializeOwned};

pub(crate) fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub(crate) fn parse_time(s: &str) -> Result<DateTime<Utc>, StoreError> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| StoreError::Invalid(format!("bad timestamp `{s}`: {e}")))
}

pub(crate) fn parse_time_opt(s: Option<&str>) -> Result<Option<DateTime<Utc>>, StoreError> {
    s.map(parse_time).transpose()
}

/// A unit-variant enum's serde name, for TEXT columns.
pub(crate) fn enum_str<T: Serialize>(value: &T) -> Result<String, StoreError> {
    match serde_json::to_value(value)? {
        serde_json::Value::String(s) => Ok(s),
        other => Err(StoreError::Invalid(format!(
            "expected a string enum, got {other}"
        ))),
    }
}

pub(crate) fn enum_from<T: DeserializeOwned>(s: &str) -> Result<T, StoreError> {
    Ok(serde_json::from_value(serde_json::Value::String(
        s.to_string(),
    ))?)
}
