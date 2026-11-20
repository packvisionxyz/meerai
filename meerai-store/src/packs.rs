use anyhow::{Context, Result, ensure};
use meerai_core::env::now_millis;
use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::Store;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackSourceKind {
    Git,
    Local,
}

impl PackSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Git => "git",
            Self::Local => "local",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "git" => Ok(Self::Git),
            "local" => Ok(Self::Local),
            other => anyhow::bail!("unknown pack source kind: {other}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPack {
    pub id: String,
    pub source_kind: PackSourceKind,
    pub source_uri: String,
    pub pinned_rev: String,
    pub manifest_json: String,
    pub enabled: bool,
    pub trusted: bool,
    pub installed_at_ms: u64,
    pub updated_at_ms: u64,
}

impl Store {
    pub fn packs(&self) -> PackRepo {
        PackRepo {
            store: self.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PackRepo {
    store: Store,
}

impl PackRepo {
    pub fn list(&self) -> Result<Vec<InstalledPack>> {
        self.store.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, source_kind, source_uri, pinned_rev, manifest_json, enabled, trusted,
                        installed_at_ms, updated_at_ms
                 FROM installed_packs ORDER BY id",
            )?;
            let rows = stmt.query_map([], pack_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn get(&self, id: &str) -> Result<Option<InstalledPack>> {
        self.store.read(|conn| {
            Ok(conn
                .query_row(
                    "SELECT id, source_kind, source_uri, pinned_rev, manifest_json, enabled,
                            trusted, installed_at_ms, updated_at_ms
                     FROM installed_packs WHERE id = ?1",
                    [id],
                    pack_row,
                )
                .optional()?)
        })
    }

    pub fn upsert(&self, pack: &InstalledPack) -> Result<()> {
        validate_pack(pack)?;
        let now = now_millis();
        let stored = pack.clone();
        self.store.immediate(|tx| {
            tx.execute(
                "INSERT INTO installed_packs
                 (id, source_kind, source_uri, pinned_rev, manifest_json, enabled, trusted,
                  installed_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(id) DO UPDATE SET
                    source_kind = excluded.source_kind,
                    source_uri = excluded.source_uri,
                    pinned_rev = excluded.pinned_rev,
                    manifest_json = excluded.manifest_json,
                    enabled = excluded.enabled,
                    trusted = excluded.trusted,
                    updated_at_ms = excluded.updated_at_ms",
                params![
                    stored.id,
                    stored.source_kind.as_str(),
                    stored.source_uri,
                    stored.pinned_rev,
                    stored.manifest_json,
                    stored.enabled,
                    stored.trusted,
                    i64::try_from(stored.installed_at_ms)?,
                    i64::try_from(now)?,
                ],
            )?;
            Ok(())
        })
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<bool> {
        self.set_flag(id, "enabled", enabled)
    }

    pub fn set_trusted(&self, id: &str, trusted: bool) -> Result<bool> {
        self.set_flag(id, "trusted", trusted)
    }

    pub fn pin(&self, id: &str, rev: &str) -> Result<bool> {
        let rev = rev.trim();
        ensure!(!rev.is_empty(), "pack revision must not be empty");
        let now = i64::try_from(now_millis())?;
        self.store.immediate(|tx| {
            let updated = tx.execute(
                "UPDATE installed_packs SET pinned_rev = ?1, updated_at_ms = ?2 WHERE id = ?3",
                params![rev, now, id],
            )?;
            Ok(updated == 1)
        })
    }

    pub fn remove(&self, id: &str) -> Result<bool> {
        self.store.immediate(|tx| {
            let removed = tx.execute("DELETE FROM installed_packs WHERE id = ?1", [id])?;
            Ok(removed == 1)
        })
    }

    fn set_flag(&self, id: &str, column: &str, value: bool) -> Result<bool> {
        let sql = match column {
            "enabled" => {
                "UPDATE installed_packs SET enabled = ?1, updated_at_ms = ?2 WHERE id = ?3"
            }
            "trusted" => {
                "UPDATE installed_packs SET trusted = ?1, updated_at_ms = ?2 WHERE id = ?3"
            }
            other => anyhow::bail!("unknown pack flag: {other}"),
        };
        let now = i64::try_from(now_millis())?;
        self.store.immediate(|tx| {
            let updated = tx.execute(sql, params![value, now, id])?;
            Ok(updated == 1)
        })
    }
}

fn validate_pack(pack: &InstalledPack) -> Result<()> {
    ensure!(!pack.id.trim().is_empty(), "pack id must not be empty");
    ensure!(
        !pack.source_uri.trim().is_empty(),
        "pack source must not be empty"
    );
    ensure!(
        !pack.pinned_rev.trim().is_empty(),
        "pack revision must not be empty"
    );
    if !pack.manifest_json.trim().is_empty() {
        serde_json::from_str::<serde_json::Value>(&pack.manifest_json)
            .context("pack manifest is not valid JSON")?;
    }
    Ok(())
}

fn pack_row(row: &Row<'_>) -> rusqlite::Result<InstalledPack> {
    let kind: String = row.get(1)?;
    let source_kind = PackSourceKind::parse(&kind).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown pack source kind: {kind}"),
            )),
        )
    })?;
    let installed: i64 = row.get(7)?;
    let updated: i64 = row.get(8)?;
    Ok(InstalledPack {
        id: row.get(0)?,
        source_kind,
        source_uri: row.get(2)?,
        pinned_rev: row.get(3)?,
        manifest_json: row.get(4)?,
        enabled: row.get(5)?,
        trusted: row.get(6)?,
        installed_at_ms: installed.max(0) as u64,
        updated_at_ms: updated.max(0) as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> (tempfile::TempDir, PackRepo) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("packs.db")).unwrap();
        let repo = store.packs();
        (dir, repo)
    }

    fn pack(id: &str) -> InstalledPack {
        InstalledPack {
            id: id.to_string(),
            source_kind: PackSourceKind::Git,
            source_uri: "https://example.com/pack.git".to_string(),
            pinned_rev: "abc123".to_string(),
            manifest_json: r#"{"name":"pack"}"#.to_string(),
            enabled: true,
            trusted: false,
            installed_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    #[test]
    fn upsert_and_list_packs() {
        let (_dir, repo) = repo();
        repo.upsert(&pack("beta")).unwrap();
        repo.upsert(&pack("alpha")).unwrap();
        let packs = repo.list().unwrap();
        assert_eq!(packs.len(), 2);
        assert_eq!(packs[0].id, "alpha");
        assert_eq!(packs[1].id, "beta");
        let loaded = repo.get("beta").unwrap().unwrap();
        assert_eq!(loaded.source_kind, PackSourceKind::Git);
        assert!(loaded.enabled);
        assert!(!loaded.trusted);
    }

    #[test]
    fn toggles_trust_enable_and_pin() {
        let (_dir, repo) = repo();
        repo.upsert(&pack("p")).unwrap();
        assert!(repo.set_enabled("p", false).unwrap());
        assert!(repo.set_trusted("p", true).unwrap());
        assert!(repo.pin("p", "def456").unwrap());
        assert!(!repo.pin("missing", "def456").unwrap());
        let loaded = repo.get("p").unwrap().unwrap();
        assert!(!loaded.enabled);
        assert!(loaded.trusted);
        assert_eq!(loaded.pinned_rev, "def456");
        assert!(repo.remove("p").unwrap());
        assert!(!repo.remove("p").unwrap());
    }

    #[test]
    fn rejects_invalid_packs() {
        let (_dir, repo) = repo();
        let mut bad = pack("p");
        bad.id = " ".to_string();
        assert!(repo.upsert(&bad).is_err());

        let mut bad_json = pack("p");
        bad_json.manifest_json = "{".to_string();
        assert!(repo.upsert(&bad_json).is_err());
    }
}
