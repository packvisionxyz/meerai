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
                out.push(chars.next().expect("peeked escape"));
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
                let (key, inner) = map.iter().next().expect("single entry");
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
