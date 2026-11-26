use serde::{Deserialize, Deserializer};
use serde_json::Value;

fn parse_number<T>(value: Value) -> Result<T, String>
where
    T: std::str::FromStr + TryFrom<u64>,
    <T as std::str::FromStr>::Err: std::fmt::Display,
    <T as TryFrom<u64>>::Error: std::fmt::Display,
{
    match value {
        Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| format!("expected a non-negative integer, got {number}"))
            .and_then(|number| {
                T::try_from(number).map_err(|error| format!("integer out of range: {error}"))
            }),
        Value::String(text) => text
            .trim()
            .parse::<T>()
            .map_err(|error| format!("expected a non-negative integer: {error}")),
        other => Err(format!("expected a non-negative integer, got {other}")),
    }
}

fn number<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: std::str::FromStr + TryFrom<u64>,
    <T as std::str::FromStr>::Err: std::fmt::Display,
    <T as TryFrom<u64>>::Error: std::fmt::Display,
{
    parse_number(Value::deserialize(deserializer)?).map_err(serde::de::Error::custom)
}

fn option_number<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: std::str::FromStr + TryFrom<u64>,
    <T as std::str::FromStr>::Err: std::fmt::Display,
    <T as TryFrom<u64>>::Error: std::fmt::Display,
{
    match Option::<Value>::deserialize(deserializer)? {
        None | Some(Value::Null) => Ok(None),
        Some(value) => parse_number(value)
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

pub fn usize_value<'de, D: Deserializer<'de>>(d: D) -> Result<usize, D::Error> {
    number(d)
}
pub fn option_usize_value<'de, D: Deserializer<'de>>(d: D) -> Result<Option<usize>, D::Error> {
    option_number(d)
}
pub fn u64_value<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    number(d)
}
pub fn option_u64_value<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    option_number(d)
}
pub fn u32_value<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    number(d)
}
pub fn option_u32_value<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    option_number(d)
}

fn is_argument_wrapper(key: &str) -> bool {
    matches!(
        key,
        "arguments"
            | "args"
            | "input"
            | "parameters"
            | "params"
            | "function"
            | "tool"
            | "tool_input"
            | "tool_call"
            | "tool_args"
            | "payload"
            | "json"
    )
}

fn nested_value(value: &Value) -> Option<Value> {
    match value {
        Value::Object(_) | Value::Array(_) | Value::String(_) => Some(value.clone()),
        _ => None,
    }
}

fn repair_invalid_json_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.peek() {
            Some('"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' | 'u') => {
                out.push('\\');
                let Some(escaped) = chars.next() else {
                    break;
                };
                out.push(escaped);
            }
            Some(_) | None => out.push_str("\\\\"),
        }
    }
    out
}

pub fn parse_tool_arguments(text: &str) -> Option<Value> {
    let mut candidate = text.trim().to_string();
    for _ in 0..32 {
        if let Ok(value) = serde_json::from_str::<Value>(&candidate) {
            return Some(value);
        }
        let repaired = repair_invalid_json_escapes(&candidate);
        if repaired != candidate
            && let Ok(value) = serde_json::from_str::<Value>(&repaired)
        {
            return Some(value);
        }
        if !candidate.contains("\\\"") {
            break;
        }
        candidate = candidate.replace("\\\"", "\"");
    }
    None
}

/// Unwrap common argument wrappers: a JSON string that contains JSON, a
/// single-key object like `{"arguments": ...}`, and escaped text such as
/// `{\"command\": ...}`. Models emit all of these.
pub fn normalize_tool_arguments(value: &Value) -> Value {
    let mut current = value.clone();
    for _ in 0..64 {
        let next = match &current {
            Value::String(text) => {
                parse_tool_arguments(text).and_then(|inner| nested_value(&inner))
            }
            Value::Object(map) if map.len() == 1 => {
                let Some((key, inner)) = map.iter().next() else {
                    break;
                };
                if is_argument_wrapper(key) {
                    nested_value(inner)
                } else {
                    None
                }
            }
            Value::Object(map) if map.len() <= 3 => map
                .get("arguments")
                .or_else(|| map.get("args"))
                .or_else(|| map.get("tool_input"))
                .or_else(|| map.get("tool_args"))
                .and_then(nested_value),
            _ => None,
        };
        match next {
            Some(next) => current = next,
            None => break,
        }
    }
    current
}

pub fn option_bool_value<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<Value>::deserialize(deserializer)? {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(value)),
        Some(Value::String(text)) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Ok(Some(true)),
            "false" | "0" | "no" => Ok(Some(false)),
            other => Err(serde::de::Error::custom(format!(
                "expected a boolean, got {other}"
            ))),
        },
        Some(other) => Err(serde::de::Error::custom(format!(
            "expected a boolean, got {other}"
        ))),
    }
}

pub fn coerce_to_schema(value: &mut Value, schema: &Value) -> bool {
    coerce(value, schema, 0)
}

fn coerce(value: &mut Value, schema: &Value, depth: usize) -> bool {
    if depth > 16 {
        return false;
    }
    let Some(object) = schema.as_object() else {
        return false;
    };
    let mut changed = false;

    if let Some(types) = object.get("type") {
        let types: Vec<&str> = match types {
            Value::String(text) => vec![text.as_str()],
            Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        if !types.is_empty()
            && !type_matches(value, &types)
            && let Some(next) = coerce_scalar(value, &types)
        {
            *value = next;
            changed = true;
        }
    }

    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(options)) = object.get(key) {
            for option in options {
                let mut candidate = value.clone();
                if coerce(&mut candidate, option, depth + 1)
                    && schema_type_matches(&candidate, option)
                {
                    *value = candidate;
                    changed = true;
                    break;
                }
            }
        }
    }
    if let Some(Value::Array(all)) = object.get("allOf") {
        for option in all {
            changed |= coerce(value, option, depth + 1);
        }
    }

    match value {
        Value::Object(map) => {
            if let Some(Value::Object(properties)) = object.get("properties") {
                for (key, property_schema) in properties {
                    let Some(child) = map.get_mut(key) else {
                        continue;
                    };
                    if child.is_null() && !schema_allows_null(property_schema) {
                        map.remove(key);
                        changed = true;
                        continue;
                    }
                    changed |= coerce(child, property_schema, depth + 1);
                }
            }
            if let Some(additional) = object.get("additionalProperties")
                && additional.is_object()
            {
                for child in map.values_mut() {
                    changed |= coerce(child, additional, depth + 1);
                }
            }
        }
        Value::Array(items) => {
            if let Some(item_schema) = object.get("items") {
                for child in items {
                    changed |= coerce(child, item_schema, depth + 1);
                }
            }
        }
        _ => {}
    }

    changed
}

fn type_matches(value: &Value, types: &[&str]) -> bool {
    types.iter().any(|kind| match *kind {
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => true,
    })
}

fn coerce_scalar(value: &Value, types: &[&str]) -> Option<Value> {
    let wants = |kind: &str| types.contains(&kind);
    if wants("string") {
        return match value {
            Value::Number(number) => Some(Value::String(number.to_string())),
            Value::Bool(boolean) => Some(Value::String(boolean.to_string())),
            _ => None,
        };
    }
    if wants("integer") {
        if let Value::String(text) = value {
            let trimmed = text.trim();
            return trimmed
                .parse::<i64>()
                .map(Value::from)
                .ok()
                .or_else(|| trimmed.parse::<u64>().map(Value::from).ok());
        }
        return None;
    }
    if wants("number") {
        if let Value::String(text) = value {
            return text
                .trim()
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number);
        }
        return None;
    }
    if wants("boolean") {
        return match value {
            Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
                "true" | "yes" | "1" => Some(Value::Bool(true)),
                "false" | "no" | "0" => Some(Value::Bool(false)),
                _ => None,
            },
            Value::Number(number) if number.as_i64() == Some(1) => Some(Value::Bool(true)),
            Value::Number(number) if number.as_i64() == Some(0) => Some(Value::Bool(false)),
            _ => None,
        };
    }
    if wants("array") && !value.is_null() {
        return Some(Value::Array(vec![value.clone()]));
    }
    None
}

fn schema_allows_null(schema: &Value) -> bool {
    match schema.get("type") {
        Some(Value::String(kind)) => kind == "null",
        Some(Value::Array(types)) => types.iter().any(|kind| kind == "null"),
        _ => ["anyOf", "oneOf"].iter().any(|key| {
            schema
                .get(*key)
                .and_then(Value::as_array)
                .is_some_and(|options| options.iter().any(schema_allows_null))
        }),
    }
}

fn schema_type_matches(value: &Value, schema: &Value) -> bool {
    match schema.get("type") {
        Some(Value::String(kind)) => type_matches(value, &[kind.as_str()]),
        Some(Value::Array(types)) => {
            let types: Vec<&str> = types.iter().filter_map(Value::as_str).collect();
            type_matches(value, &types)
        }
        _ => {
            ["anyOf", "oneOf"].iter().any(|key| {
                schema
                    .get(*key)
                    .and_then(Value::as_array)
                    .is_some_and(|options| {
                        options
                            .iter()
                            .any(|option| schema_type_matches(value, option))
                    })
            }) || schema.get("type").is_none()
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct Args {
        #[serde(default, deserialize_with = "super::option_usize_value")]
        offset: Option<usize>,
        #[serde(deserialize_with = "super::u32_value")]
        line: u32,
        #[serde(default, deserialize_with = "super::option_bool_value")]
        flag: Option<bool>,
    }

    #[test]
    fn accepts_numbers_and_numeric_strings() {
        let args: Args =
            serde_json::from_str(r#"{"offset": "560", "line": "12", "flag": "true"}"#).unwrap();
        assert_eq!(args.offset, Some(560));
        assert_eq!(args.line, 12);
        assert_eq!(args.flag, Some(true));

        let args: Args = serde_json::from_str(r#"{"offset": 560, "line": 12}"#).unwrap();
        assert_eq!(args.offset, Some(560));
        assert_eq!(args.line, 12);
        assert_eq!(args.flag, None);

        let args: Args = serde_json::from_str(r#"{"line": 1}"#).unwrap();
        assert_eq!(args.offset, None);
    }

    #[test]
    fn coerces_argument_types_to_the_schema() {
        use serde_json::json;
        let schema = json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer" },
                "ratio": { "type": "number" },
                "flag": { "type": "boolean" },
                "name": { "type": "string" },
                "paths": { "type": "array", "items": { "type": "string" } }
            }
        });
        let mut value = json!({
            "limit": "20",
            "ratio": "1.5",
            "flag": "true",
            "name": 42,
            "paths": "a"
        });
        assert!(super::coerce_to_schema(&mut value, &schema));
        assert_eq!(
            value,
            json!({
                "limit": 20,
                "ratio": 1.5,
                "flag": true,
                "name": "42",
                "paths": ["a"]
            })
        );
    }

    #[test]
    fn coercion_removes_null_optional_properties() {
        use serde_json::json;
        let schema = json!({
            "type": "object",
            "properties": {
                "required": { "type": "integer" },
                "optional": { "type": ["integer", "null"] }
            }
        });
        let mut value = json!({ "required": null, "optional": null });
        assert!(super::coerce_to_schema(&mut value, &schema));
        assert_eq!(value, json!({ "optional": null }));
    }

    #[test]
    fn coercion_leaves_valid_values_unchanged() {
        use serde_json::json;
        let schema = json!({
            "type": "object",
            "properties": { "count": { "type": "integer" } }
        });
        let mut value = json!({ "count": 1 });
        assert!(!super::coerce_to_schema(&mut value, &schema));
        assert_eq!(value, json!({ "count": 1 }));
    }

    #[test]
    fn unwraps_double_encoded_and_wrapped_arguments() {
        use serde_json::json;
        let inner = json!({"command": "cargo test"});
        let wrapped = json!({"arguments": inner.to_string()});
        assert_eq!(super::normalize_tool_arguments(&wrapped), inner);

        let double =
            serde_json::Value::String(serde_json::Value::String(inner.to_string()).to_string());
        assert_eq!(super::normalize_tool_arguments(&double), inner);

        let plain = json!({"path": "a.txt"});
        assert_eq!(super::normalize_tool_arguments(&plain), plain);
    }

    #[test]
    fn unwraps_deeply_escaped_arguments() {
        use serde_json::json;
        let inner = json!({"command": "git diff HEAD --stat"});
        let mut nested = serde_json::Value::String(inner.to_string());
        for _ in 0..4 {
            nested = serde_json::Value::String(nested.to_string());
        }
        assert_eq!(super::normalize_tool_arguments(&nested), inner);

        let escaped = r#"{\"command\":\"git diff HEAD --stat\"}"#;
        assert_eq!(
            super::normalize_tool_arguments(&serde_json::Value::String(escaped.to_string())),
            inner
        );

        let double_escaped = escaped.replace('"', "\\\"");
        assert_eq!(
            super::normalize_tool_arguments(&serde_json::Value::String(double_escaped)),
            inner
        );
    }

    #[test]
    fn unwraps_more_than_eight_wrapper_layers() {
        use serde_json::json;
        let inner = json!({"command": "wc -l src/*.rs | sort -n"});
        let mut wrapped = inner.to_string();
        for _ in 0..16 {
            wrapped = json!({ "arguments": wrapped }).to_string();
        }
        assert_eq!(
            super::normalize_tool_arguments(&serde_json::Value::String(wrapped)),
            inner
        );
    }

    #[test]
    fn parses_arguments_from_raw_escaped_text() {
        assert_eq!(
            super::parse_tool_arguments(r#"{\"command\":\"ls\"}"#),
            Some(serde_json::json!({"command": "ls"}))
        );
        assert_eq!(super::parse_tool_arguments("not json"), None);
    }

    #[test]
    fn repairs_invalid_backslash_escapes_in_arguments() {
        let plain = r#"{"command":"grep -n 'a\|b' src/app.rs"}"#;
        assert_eq!(
            super::parse_tool_arguments(plain),
            Some(serde_json::json!({"command": "grep -n 'a\\|b' src/app.rs"}))
        );

        let quoted = r#""{\"command\":\"grep -n 'a\|b' src/app.rs\"}""#;
        assert_eq!(
            super::normalize_tool_arguments(&serde_json::Value::String(quoted.to_string())),
            serde_json::json!({"command": "grep -n 'a\\|b' src/app.rs"})
        );
    }

    #[test]
    fn rejects_non_numeric_values() {
        assert!(serde_json::from_str::<Args>(r#"{"offset": "abc", "line": 1}"#).is_err());
        assert!(serde_json::from_str::<Args>(r#"{"offset": -1, "line": 1}"#).is_err());
        assert!(serde_json::from_str::<Args>(r#"{"line": 1, "flag": "maybe"}"#).is_err());
    }
}
