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
    fn rejects_non_numeric_values() {
        assert!(serde_json::from_str::<Args>(r#"{"offset": "abc", "line": 1}"#).is_err());
        assert!(serde_json::from_str::<Args>(r#"{"offset": -1, "line": 1}"#).is_err());
        assert!(serde_json::from_str::<Args>(r#"{"line": 1, "flag": "maybe"}"#).is_err());
    }
}
