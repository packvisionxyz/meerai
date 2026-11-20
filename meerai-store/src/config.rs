use anyhow::{Context, Result};
use meerai_core::env::now_millis;
use rusqlite::{OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};

use crate::Store;

#[derive(Clone, Debug)]
pub struct ConfigRepo {
    store: Store,
}

impl Store {
    pub fn config(&self) -> ConfigRepo {
        ConfigRepo {
            store: self.clone(),
        }
    }
}

impl ConfigRepo {
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let json: Option<String> = self.store.read(|conn| {
            Ok(conn
                .query_row(
                    "SELECT json_value FROM user_config WHERE key = ?1",
                    [key],
                    |row| row.get(0),
                )
                .optional()?)
        })?;
        json.map(|json| {
            serde_json::from_str(&json).with_context(|| format!("decode config key {key:?}"))
        })
        .transpose()
    }

    pub fn set<T: Serialize + ?Sized>(&self, key: &str, value: &T) -> Result<()> {
        let json =
            serde_json::to_string(value).with_context(|| format!("encode config key {key:?}"))?;
        self.store.immediate(|tx| {
            tx.execute(
                "INSERT INTO user_config (key, json_value, updated_at_ms) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET
                    json_value = excluded.json_value, updated_at_ms = excluded.updated_at_ms",
                params![key, json, i64::try_from(now_millis())?],
            )?;
            Ok(())
        })
    }
}
