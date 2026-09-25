//! Tool arguments as models send them. Weaker models often pass an object or array argument as a
//! string holding its JSON; [`coerce_json_strings`] parses those back before validation.

use serde_json::Value;

fn allows_string(schema: &Value) -> bool {
    let typed = |t: &Value| match t {
        Value::String(s) => s == "string",
        Value::Array(a) => a.iter().any(|x| x == "string"),
        _ => false,
    };
    schema.get("type").is_some_and(typed)
        || schema.get("enum").is_some()
        || schema.get("const").is_some_and(Value::is_string)
        || ["anyOf", "oneOf"].iter().any(|k| {
            schema
                .get(*k)
                .and_then(Value::as_array)
                .is_some_and(|v| v.iter().any(allows_string))
        })
}

/// Parse each top-level argument that `schema` types as something other than a string but that
/// arrived as a string of JSON object or array text. Returns whether anything changed.
pub fn coerce_json_strings(input: &mut Value, schema: &Value) -> bool {
    let (Some(args), Some(props)) = (
        input.as_object_mut(),
        schema.get("properties").and_then(Value::as_object),
    ) else {
        return false;
    };
    let mut changed = false;
    for (key, prop) in props {
        let Some(Value::String(text)) = args.get(key) else {
            continue;
        };
        if allows_string(prop) {
            continue;
        }
        let t = text.trim();
        if !(t.starts_with('{') || t.starts_with('[')) {
            continue;
        }
        if let Ok(v @ (Value::Object(_) | Value::Array(_))) = serde_json::from_str::<Value>(t) {
            args.insert(key.clone(), v);
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stringified_objects_and_arrays_are_parsed_strings_are_kept() {
        let schema = json!({"type": "object", "properties": {
            "document": {"$ref": "#/$defs/Document"},
            "paths": {"type": "array", "items": {"type": "string"}},
            "path": {"type": "string"},
            "note": {"anyOf": [{"type": "string"}, {"type": "null"}]},
            "bad": {"type": "object"}
        }});
        let mut input = json!({
            "document": "{\n  \"title\": \"T\"\n}",
            "paths": " [\"a.ts\"]",
            "path": "{\"looks\": \"like json\"}",
            "note": "[1]",
            "bad": "{not json"
        });
        assert!(coerce_json_strings(&mut input, &schema));
        assert_eq!(
            input,
            json!({
                "document": {"title": "T"},
                "paths": ["a.ts"],
                "path": "{\"looks\": \"like json\"}",
                "note": "[1]",
                "bad": "{not json"
            })
        );
        assert!(!coerce_json_strings(&mut input, &schema));
    }
}
