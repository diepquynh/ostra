//! In-place revisions: an `update` replaces the top-level fields it names, and a list whose items
//! carry an `id` is merged by that id instead of replaced. A merged item takes the fields the
//! update sends and keeps the rest, at every level, so a revision sends only what changed: one
//! step's `action` is `{"phases": [{"id": 2, "steps": [{"id": "2.3", "action": "..."}]}]}`.

use serde_json::{Map, Value};
use std::cmp::Ordering;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeError(pub String);

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The merged document, and the nested items it kept that the update did not send, such as
/// `` `2` steps: 2.4, 2.5 ``, so the agent sees what a partial update left in place.
#[derive(Debug, Clone, PartialEq)]
pub struct Merged {
    pub value: Value,
    pub kept: Vec<String>,
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

fn merge_list(old: &mut Vec<Value>, items: &[Value], kept: &mut Vec<String>) {
    for item in items {
        let id = id_of(item);
        match old.iter_mut().find(|o| id_of(o) == id) {
            Some(slot) => merge_item(slot, item, kept),
            None => old.push(item.clone()),
        }
    }
    old.sort_by(|a, b| natural_cmp(&id_of(a).unwrap_or_default(), &id_of(b).unwrap_or_default()));
}

/// A stored item takes each field the update sends. A keyed list inside it merges by id the same
/// way, and the stored ids the update left out are reported in `kept`.
fn merge_item(slot: &mut Value, item: &Value, kept: &mut Vec<String>) {
    let (Value::Object(stored), Value::Object(new)) = (&mut *slot, item) else {
        *slot = item.clone();
        return;
    };
    let owner = id_of(item).unwrap_or_default();
    for (key, value) in new {
        match (stored.get_mut(key), value) {
            (Some(Value::Array(old)), Value::Array(items)) if keyed(old) && keyed(items) => {
                let sent: HashSet<String> = items.iter().filter_map(id_of).collect();
                let left: Vec<String> = old
                    .iter()
                    .filter_map(id_of)
                    .filter(|id| !sent.contains(id))
                    .collect();
                if !left.is_empty() {
                    kept.push(format!("`{owner}` {key}: {}", left.join(", ")));
                }
                merge_list(old, items, kept);
            }
            _ => {
                stored.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Merge `update` into `base` and drop every keyed item whose id is in `remove`. Keyed lists end
/// in natural id order, so a new `R12` lands after `R11` wherever it was sent.
pub fn apply_update(
    base: Option<Value>,
    update: &Value,
    remove: &[String],
) -> Result<Merged, MergeError> {
    let mut doc = match base {
        Some(Value::Object(m)) => m,
        Some(_) => return Err(MergeError("The stored document is not an object.".into())),
        None => Map::new(),
    };
    let Value::Object(update) = update else {
        return Err(MergeError(
            "`update` must be an object of top-level document fields.".into(),
        ));
    };
    let mut kept = vec![];
    for (key, new) in update {
        match (doc.get_mut(key), new) {
            (Some(Value::Array(old)), Value::Array(items))
                if (keyed(old) || old.is_empty()) && keyed(items) =>
            {
                merge_list(old, items, &mut kept);
            }
            _ => {
                doc.insert(key.clone(), new.clone());
            }
        }
    }
    let mut missing = vec![];
    for id in remove {
        if remove_id(&mut doc, id)? == 0 {
            missing.push(id.as_str());
        }
    }
    if !missing.is_empty() {
        return Err(MergeError(format!(
            "Nothing to remove for {}: no list item has that id. Check the ids and call again; nothing was written.",
            missing.join(", ")
        )));
    }
    Ok(Merged {
        value: Value::Object(doc),
        kept,
    })
}

/// Remove the items with `id` from the keyed lists directly inside `v`, and count them.
fn remove_direct(v: &mut Value, id: &str) -> usize {
    let Value::Object(m) = v else { return 0 };
    let mut n = 0;
    for value in m.values_mut() {
        if let Value::Array(list) = value
            && keyed(list)
        {
            let before = list.len();
            list.retain(|i| id_of(i).as_deref() != Some(id));
            n += before - list.len();
        }
    }
    n
}

/// Run `f` on every keyed item below `v`, at any depth, whose id is `owner`, and sum its counts.
fn for_items(v: &mut Value, owner: &str, f: &mut dyn FnMut(&mut Value) -> usize) -> usize {
    let Value::Object(m) = v else { return 0 };
    let mut n = 0;
    for value in m.values_mut() {
        let Value::Array(list) = value else { continue };
        if !keyed(list) {
            continue;
        }
        for item in list.iter_mut() {
            if id_of(item).as_deref() == Some(owner) {
                n += f(item);
            }
            n += for_items(item, owner, f);
        }
    }
    n
}

/// The ids of the items whose keyed lists hold an item with `id`, below the top level.
fn holders(v: &Value, id: &str, out: &mut Vec<String>) {
    let Value::Object(m) = v else { return };
    for value in m.values() {
        let Value::Array(list) = value else { continue };
        if !keyed(list) {
            continue;
        }
        for item in list {
            let owner = id_of(item).unwrap_or_default();
            if let Value::Object(fields) = item {
                let holds = fields.values().any(|f| {
                    matches!(f, Value::Array(l) if keyed(l) && l.iter().any(|i| id_of(i).as_deref() == Some(id)))
                });
                if holds && !out.contains(&owner) {
                    out.push(owner);
                }
            }
            holders(item, id, out);
        }
    }
}

/// Remove `id` and count what went. A top-level item goes as before. Otherwise the id must sit in
/// one item only, or be named `{parent id}/{id}`, such as `2.3/E1`.
fn remove_id(doc: &mut Map<String, Value>, id: &str) -> Result<usize, MergeError> {
    let mut root = Value::Object(std::mem::take(doc));
    let result = if let Some((parent, child)) = id.split_once('/') {
        Ok(for_items(&mut root, parent, &mut |item| {
            remove_direct(item, child)
        }))
    } else {
        match remove_direct(&mut root, id) {
            0 => {
                let mut at = vec![];
                holders(&root, id, &mut at);
                match at.as_slice() {
                    [] => Ok(0),
                    [owner] => {
                        let owner = owner.clone();
                        Ok(for_items(&mut root, &owner, &mut |item| {
                            remove_direct(item, id)
                        }))
                    }
                    several => Err(MergeError(format!(
                        "`{id}` is in several places, under {}. Name the one to remove as `{{parent id}}/{id}`, for example `{}/{id}`; nothing was written.",
                        several
                            .iter()
                            .map(|o| format!("`{o}`"))
                            .collect::<Vec<_>>()
                            .join(", "),
                        several[0]
                    ))),
                }
            }
            n => Ok(n),
        }
    };
    if let Value::Object(m) = root {
        *doc = m;
    }
    result
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
            out.value,
            json!({"title": "B", "requirements": [{"id": "R1", "t": "one"}, {"id": "R2", "t": "TWO"}, {"id": "R10", "t": "ten"}], "notes": ["y"]})
        );
        assert!(out.kept.is_empty(), "{:?}", out.kept);
    }

    #[test]
    fn numeric_ids_and_removal() {
        let base = json!({"phases": [{"id": 1}, {"id": 2}, {"id": 3}], "risks": []});
        let out = apply_update(
            Some(base.clone()),
            &json!({"phases": [{"id": 2, "n": "x"}]}),
            &["3".into()],
        )
        .unwrap();
        assert_eq!(out.value["phases"], json!([{"id": 1}, {"id": 2, "n": "x"}]));
        let err = apply_update(Some(base), &json!({}), &["R9".into()]).unwrap_err();
        assert!(err.0.contains("R9"), "{err}");
    }

    #[test]
    fn builds_from_nothing() {
        let out = apply_update(None, &json!({"title": "T", "phases": [{"id": 1}]}), &[]).unwrap();
        assert_eq!(out.value, json!({"title": "T", "phases": [{"id": 1}]}));
    }

    fn plan() -> Value {
        json!({"phases": [
            {"id": 1, "name": "data", "steps": [{"id": "1.1", "action": "a"}]},
            {"id": 2, "name": "service", "constraints": [{"id": "E1", "rule": "r"}], "steps": [
                {"id": "2.1", "action": "b", "binding_rules": [{"id": "E1", "rule": "r"}]},
                {"id": "2.2", "action": "c", "read_first": ["x.rs"]}
            ]}
        ]})
    }

    #[test]
    fn a_keyed_item_takes_only_the_fields_sent_at_every_level() {
        let up = json!({"phases": [{"id": 2, "steps": [{"id": "2.2", "action": "C"}, {"id": "2.3", "action": "d"}]}]});
        let out = apply_update(Some(plan()), &up, &[]).unwrap();
        let phase = &out.value["phases"][1];
        assert_eq!(phase["name"], "service");
        assert_eq!(phase["constraints"][0]["id"], "E1");
        let steps = phase["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0]["action"], "b");
        assert_eq!(steps[1]["action"], "C");
        assert_eq!(steps[1]["read_first"], json!(["x.rs"]));
        assert_eq!(steps[2]["id"], "2.3");
        assert_eq!(out.kept, vec!["`2` steps: 2.1".to_string()]);
        // A list without ids is a field like any other: the update's list replaces it.
        let up = json!({"phases": [{"id": 2, "steps": [{"id": "2.2", "read_first": ["y.rs"]}]}]});
        let out = apply_update(Some(plan()), &up, &[]).unwrap();
        assert_eq!(
            out.value["phases"][1]["steps"][1]["read_first"],
            json!(["y.rs"])
        );
    }

    #[test]
    fn nested_items_are_removed_by_id_or_by_parent_and_id() {
        let out = apply_update(Some(plan()), &json!({}), &["2.2".into()]).unwrap();
        assert_eq!(out.value["phases"][1]["steps"].as_array().unwrap().len(), 1);
        // `E1` sits in phase 2's constraints and in step 2.1's binding rules.
        let err = apply_update(Some(plan()), &json!({}), &["E1".into()]).unwrap_err();
        assert!(
            err.0.contains("under `2`, `2.1`") && err.0.contains("`2/E1`"),
            "{err}"
        );
        let out = apply_update(Some(plan()), &json!({}), &["2.1/E1".into()]).unwrap();
        assert_eq!(
            out.value["phases"][1]["steps"][0]["binding_rules"],
            json!([])
        );
        assert_eq!(out.value["phases"][1]["constraints"][0]["id"], "E1");
        // A top-level id goes as it always did, without looking further down.
        let out = apply_update(Some(plan()), &json!({}), &["1".into()]).unwrap();
        assert_eq!(out.value["phases"].as_array().unwrap().len(), 1);
        let err = apply_update(Some(plan()), &json!({}), &["9/E1".into()]).unwrap_err();
        assert!(err.0.contains("9/E1"), "{err}");
    }
}
