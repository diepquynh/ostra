//! In-place revisions: an `update` replaces the top-level fields it names, and a list whose items
//! carry an `id` is merged by that id instead of replaced, so a revision sends only what changed.

use serde_json::{Map, Value};
use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeError(pub String);

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn id_of(v: &Value) -> Option<String> {
    match v.get("id")? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn keyed(list: &[Value]) -> bool {
    !list.is_empty() && list.iter().all(|v| id_of(v).is_some())
}

/// Order `R2` before `R10` and `AC1.2` before `AC1.10`.
pub(crate) fn natural_cmp(a: &str, b: &str) -> Ordering {
    fn parts(s: &str) -> Vec<(String, u64)> {
        let mut out = vec![];
        let mut text = String::new();
        let mut num: Option<u64> = None;
        for c in s.chars() {
            if let Some(d) = c.to_digit(10) {
                num = Some(num.unwrap_or(0).saturating_mul(10).saturating_add(d as u64));
            } else {
                if let Some(n) = num.take() {
                    out.push((std::mem::take(&mut text), n));
                }
                text.push(c);
            }
        }
        out.push((text, num.unwrap_or(0)));
        out
    }
    parts(a).cmp(&parts(b))
}

/// Merge `update` into `base` and drop every keyed item whose id is in `remove`. Keyed lists end
/// in natural id order, so a new `R12` lands after `R11` wherever it was sent.
pub fn apply_update(base: Option<Value>, update: &Value, remove: &[String]) -> Result<Value, MergeError> {
    let mut doc = match base {
        Some(Value::Object(m)) => m,
        Some(_) => return Err(MergeError("The stored document is not an object.".into())),
        None => Map::new(),
    };
    let Value::Object(update) = update else {
        return Err(MergeError("`update` must be an object of top-level document fields.".into()));
    };
    for (key, new) in update {
        match (doc.get_mut(key), new) {
            (Some(Value::Array(old)), Value::Array(items)) if (keyed(old) || old.is_empty()) && keyed(items) => {
                for item in items {
                    let id = id_of(item);
                    match old.iter_mut().find(|o| id_of(o) == id) {
                        Some(slot) => *slot = item.clone(),
                        None => old.push(item.clone()),
                    }
                }
                old.sort_by(|a, b| natural_cmp(&id_of(a).unwrap_or_default(), &id_of(b).unwrap_or_default()));
            }
            _ => {
                doc.insert(key.clone(), new.clone());
            }
        }
    }
    let mut missing: Vec<&str> = remove.iter().map(String::as_str).collect();
    for value in doc.values_mut() {
        if let Value::Array(list) = value
            && keyed(list)
        {
            list.retain(|v| {
                let id = id_of(v).unwrap_or_default();
                let gone = remove.contains(&id);
                if gone {
                    missing.retain(|m| *m != id);
                }
                !gone
            });
        }
    }
    if !missing.is_empty() {
        return Err(MergeError(format!(
            "Nothing to remove for {}: no list item has that id. Check the ids and call again; nothing was written.",
            missing.join(", ")
        )));
    }
    Ok(Value::Object(doc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn natural_order() {
        let mut ids = vec!["R10", "R2", "R1", "AC1.10", "AC1.2"];
        ids.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(ids, ["AC1.2", "AC1.10", "R1", "R2", "R10"]);
    }

    #[test]
    fn keyed_lists_merge_by_id_and_scalars_replace() {
        let base = json!({"title": "A", "requirements": [{"id": "R1", "t": "one"}, {"id": "R2", "t": "two"}], "notes": ["x"]});
        let up = json!({"title": "B", "requirements": [{"id": "R10", "t": "ten"}, {"id": "R2", "t": "TWO"}], "notes": ["y"]});
        let out = apply_update(Some(base), &up, &[]).unwrap();
        assert_eq!(
            out,
            json!({"title": "B", "requirements": [{"id": "R1", "t": "one"}, {"id": "R2", "t": "TWO"}, {"id": "R10", "t": "ten"}], "notes": ["y"]})
        );
    }

    #[test]
    fn numeric_ids_and_removal() {
        let base = json!({"phases": [{"id": 1}, {"id": 2}, {"id": 3}], "risks": []});
        let out = apply_update(Some(base.clone()), &json!({"phases": [{"id": 2, "n": "x"}]}), &["3".into()]).unwrap();
        assert_eq!(out["phases"], json!([{"id": 1}, {"id": 2, "n": "x"}]));
        let err = apply_update(Some(base), &json!({}), &["R9".into()]).unwrap_err();
        assert!(err.0.contains("R9"), "{err}");
    }

    #[test]
    fn builds_from_nothing() {
        let out = apply_update(None, &json!({"title": "T", "phases": [{"id": 1}]}), &[]).unwrap();
        assert_eq!(out, json!({"title": "T", "phases": [{"id": 1}]}));
    }
}
