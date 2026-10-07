//! A JSON Schema subset for the shapes users declare (custom agent `data`, plugin inputs):
//! `type`, `properties`, `required`, `items`, `enum`, `minItems`, `maxItems`, and
//! `additionalProperties: false`. Keywords outside the subset are ignored, never refused.

use serde_json::Value;

/// Every way `value` breaks `schema`, each naming the path it was found at.
pub fn check(schema: &Value, value: &Value, path: &str) -> Vec<String> {
    let mut out = vec![];
    walk(schema, value, path, &mut out);
    out
}

/// Keywords a declared schema may use. A schema with others still loads, but they check nothing.
pub const KEYWORDS: [&str; 10] = [
    "type",
    "properties",
    "required",
    "items",
    "enum",
    "minItems",
    "maxItems",
    "additionalProperties",
    "description",
    "title",
];

/// Rule CA3: a declared schema must be an object whose `type` keywords name JSON types.
pub fn validate_schema(schema: &Value) -> Result<(), String> {
    let Some(obj) = schema.as_object() else {
        return Err("Declare the schema as a JSON object.".into());
    };
    if let Some(t) = obj.get("type") {
        let names: Vec<&str> = match t {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        if names.is_empty() || names.iter().any(|n| !TYPES.contains(n)) {
            return Err(format!(
                "Give `type` as one of {}: `{t}` is not.",
                TYPES.join(", ")
            ));
        }
    }
    if let Some(props) = obj.get("properties").and_then(Value::as_object) {
        for (k, v) in props {
            validate_schema(v).map_err(|e| format!("{k}: {e}"))?;
        }
    }
    if let Some(items) = obj.get("items") {
        validate_schema(items).map_err(|e| format!("items: {e}"))?;
    }
    Ok(())
}

const TYPES: [&str; 7] = [
    "object", "array", "string", "number", "integer", "boolean", "null",
];

fn type_ok(t: &str, v: &Value) -> bool {
    match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "number" => v.is_number(),
        "integer" => v.is_i64() || v.is_u64() || v.as_f64().is_some_and(|f| f.fract() == 0.0),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        _ => true,
    }
}

fn walk(schema: &Value, value: &Value, path: &str, out: &mut Vec<String>) {
    let Some(s) = schema.as_object() else {
        return;
    };
    if let Some(t) = s.get("type") {
        let ok = match t {
            Value::String(t) => type_ok(t, value),
            Value::Array(ts) => ts
                .iter()
                .filter_map(Value::as_str)
                .any(|t| type_ok(t, value)),
            _ => true,
        };
        if !ok {
            out.push(format!("`{path}` must be of type {t}"));
            return;
        }
    }
    if let Some(Value::Array(allowed)) = s.get("enum")
        && !allowed.contains(value)
    {
        out.push(format!(
            "`{path}` must be one of {}",
            Value::Array(allowed.clone())
        ));
    }
    if let Value::Object(map) = value {
        if let Some(Value::Array(req)) = s.get("required") {
            for r in req.iter().filter_map(Value::as_str) {
                if !map.contains_key(r) {
                    out.push(format!("`{path}.{r}` is required"));
                }
            }
        }
        let props = s.get("properties").and_then(Value::as_object);
        for (k, v) in map {
            match props.and_then(|p| p.get(k)) {
                Some(sub) => walk(sub, v, &format!("{path}.{k}"), out),
                None if s.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    out.push(format!("`{path}.{k}` is not allowed"))
                }
                None => {}
            }
        }
    }
    if let Value::Array(items) = value {
        let len = items.len() as u64;
        if let Some(min) = s.get("minItems").and_then(Value::as_u64)
            && len < min
        {
            out.push(format!("`{path}` needs at least {min} items"));
        }
        if let Some(max) = s.get("maxItems").and_then(Value::as_u64)
            && len > max
        {
            out.push(format!("`{path}` allows at most {max} items"));
        }
        if let Some(sub) = s.get("items") {
            for (i, v) in items.iter().enumerate() {
                walk(sub, v, &format!("{path}[{i}]"), out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn checks_the_subset() {
        let s = json!({"type": "object", "required": ["a"], "additionalProperties": false,
            "properties": {"a": {"type": "array", "minItems": 1, "items": {"enum": ["x", "y"]}}}});
        assert!(check(&s, &json!({"a": ["x"]}), "data").is_empty());
        assert_eq!(check(&s, &json!({}), "data"), vec!["`data.a` is required"]);
        assert_eq!(check(&s, &json!({"a": []}), "data").len(), 1);
        assert_eq!(check(&s, &json!({"a": ["z"], "b": 1}), "data").len(), 2);
        assert!(check(&json!({"type": "integer"}), &json!(2.0), "n").is_empty());
        assert!(validate_schema(&json!({"type": "obj"})).is_err());
        assert!(validate_schema(&json!({"type": ["string", "null"]})).is_ok());
    }
}
