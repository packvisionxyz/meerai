use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

fn next_raw(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.as_nanos() % 0x1_0000_0000) as u64)
        .unwrap_or(0);
    format!("{prefix}-{nanos:08x}-{n:x}")
}

macro_rules! id_type {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new() -> Self {
                Self(next_raw($prefix))
            }

            pub fn from_raw(raw: impl Into<String>) -> Self {
                Self(raw.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn parse(raw: impl AsRef<str>) -> anyhow::Result<Self> {
                let raw = raw.as_ref();
                if !is_safe_segment(raw) {
                    anyhow::bail!("invalid session id: {raw}");
                }
                Ok(Self(raw.to_string()))
            }
        }

        impl std::str::FromStr for $name {
            type Err = anyhow::Error;

            fn from_str(raw: &str) -> Result<Self, Self::Err> {
                Self::parse(raw)
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_string())
            }
        }
    };
}

id_type!(SessionId, "ses");
id_type!(TurnId, "turn");
id_type!(StepId, "step");
id_type!(ItemId, "item");
id_type!(CallId, "call");
id_type!(RequestId, "req");
id_type!(CompactionId, "cmp");
id_type!(CheckpointId, "ckpt");

pub(crate) fn is_safe_segment(raw: &str) -> bool {
    !raw.is_empty() && raw != "." && raw != ".." && !raw.contains(['/', '\\', ':'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_prefixed() {
        let a = TurnId::new();
        let b = TurnId::new();
        assert_ne!(a, b);
        assert!(a.as_str().starts_with("turn-"));
    }

    #[test]
    fn session_ids_parse_safely() {
        let id: SessionId = "ses-abc".parse().unwrap();
        assert_eq!(id.as_str(), "ses-abc");
        assert_eq!(SessionId::parse("ses-xyz").unwrap().as_str(), "ses-xyz");

        for raw in ["", ".", "..", "a/b", "a\\b", "C:", "C:\\ws", "x:y"] {
            assert!(SessionId::parse(raw).is_err(), "{raw:?}");
            assert!(raw.parse::<SessionId>().is_err(), "{raw:?}");
        }

        assert!(SessionId::parse("C:").is_err());
        assert!(TurnId::parse("D:").is_err());
        assert!(CompactionId::parse("session-1").is_ok());
    }
}
