use std::path::Path;

use anyhow::{Result, bail};
use meerai_store::Store;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Settings {
    model: String,
}

#[test]
fn typed_config_persists_and_upserts() -> Result<()> {
    let dir = TempDir::new()?;
    let path = dir.path().join("store.db");
    let store = Store::open(&path)?;
    let config = store.clone().config();
    assert_eq!(config.get::<Settings>("settings")?, None);
    config.set(
        "settings",
        &Settings {
            model: "first".into(),
        },
    )?;
    assert_eq!(
        config.get::<Settings>("settings")?,
        Some(Settings {
            model: "first".into(),
        }),
    );
    assert!(config.get::<String>("settings").is_err());
    config.set("settings", "replacement")?;
    drop(config);
    drop(store);
    assert_eq!(
        Store::open(&path)?.config().get::<String>("settings")?,
        Some("replacement".into()),
    );
    Ok(())
}

#[test]
fn initializes_v2_and_refuses_unknown_versions() -> Result<()> {
    let dir = TempDir::new()?;
    let path = dir.path().join("store.db");
    let store = Store::open(&path)?;
    store.read(|conn| {
        let journal: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        assert_eq!(journal, "wal");
        for (pragma, expected) in [
            ("user_version", 2),
            ("synchronous", 1),
            ("foreign_keys", 1),
            ("busy_timeout", 5000),
            ("recursive_triggers", 1),
        ] {
            let value: i64 = conn.pragma_query_value(None, pragma, |row| row.get(0))?;
            assert_eq!(value, expected, "{pragma}");
        }
        let version: String = conn.query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(version, "2");
        conn.pragma_update(None, "user_version", 42)?;
        Ok(())
    })?;
    drop(store);
    let Err(err) = Store::open(path) else {
        panic!("unknown schema was accepted");
    };
    assert!(format!("{err:#}").contains("unsupported SQLite store schema version 42"));
    Ok(())
}

#[test]
fn migrates_v1_sessions_to_v2_with_a_provider_column() -> Result<()> {
    let dir = TempDir::new()?;
    let path = dir.path().join("store.db");
    drop(Store::open(&path)?);
    let conn = Connection::open(&path)?;
    conn.execute("ALTER TABLE sessions DROP COLUMN provider", [])?;
    conn.pragma_update(None, "user_version", 1)?;
    conn.execute(
        "UPDATE meta SET value = '1' WHERE key = 'schema_version'",
        [],
    )?;
    drop(conn);
    let store = Store::open(&path)?;
    store.read(|conn| {
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        assert_eq!(version, 2);
        conn.execute("SELECT provider FROM sessions LIMIT 0", [])?;
        Ok(())
    })?;
    Ok(())
}

#[test]
fn wal_reader_sees_only_committed_changes_and_failed_writes_roll_back() -> Result<()> {
    let dir = TempDir::new()?;
    let path = dir.path().join("store.db");
    let writer = Store::open(&path)?;
    writer.config().set("state", "old")?;
    let reader = Store::open(&path)?;
    writer.immediate(|tx| {
        tx.execute("UPDATE user_config SET json_value = '\"new\"'", [])?;
        assert_eq!(
            reader.config().get::<String>("state")?.as_deref(),
            Some("old")
        );
        Ok(())
    })?;
    assert_eq!(
        reader.config().get::<String>("state")?.as_deref(),
        Some("new")
    );
    let result = writer.immediate::<()>(|tx| {
        tx.execute("DELETE FROM user_config", [])?;
        bail!("abort write");
    });
    assert_eq!(result.unwrap_err().to_string(), "abort write");
    assert_eq!(
        reader.config().get::<String>("state")?.as_deref(),
        Some("new")
    );
    Ok(())
}

fn count(conn: &Connection, query: &str) -> Result<i64> {
    Ok(conn.query_row(query, [], |row| row.get(0))?)
}

#[test]
fn fts_tracks_replacements_updates_and_cascades() -> Result<()> {
    let store = Store::open(":memory:")?;
    store.immediate(|tx| {
        tx.execute_batch(
            r#"INSERT INTO sessions (id, project_root, cwd, created_at_ms, updated_at_ms)
                 VALUES ('s', '/project', '/project', 0, 0);
               INSERT INTO session_events VALUES
                 ('s', 1, 0, 'user_message', NULL, '{"text":"SQLite \u03bb zebra"}');
               INSERT INTO project_memories (id, project_root, kind, title, body,
                     source_session_id, created_at_ms, updated_at_ms)
                 VALUES ('m', '/project', 'fact', 'Database', 'A persistent memory', 's', 0, 0);
               INSERT INTO session_blobs VALUES ('s', 'blob', 1, x'00', 0);
               INSERT INTO session_snapshots VALUES ('s', 'turn', 'file', 0, NULL, 0);
               INSERT INTO usage_ledger (session_id, provider, model, prompt_tokens,
                     completion_tokens, cached_tokens, ts_ms) VALUES ('s', 'p', 'm', 1, 1, 0, 0);"#,
        )?;
        Ok(())
    })?;
    store.read(|conn| {
        assert_eq!(count(conn, "SELECT count(*) FROM session_events_fts WHERE session_events_fts MATCH 'λ zebra'")?, 1);
        assert_eq!(count(conn, "SELECT count(*) FROM project_memories_fts WHERE project_memories_fts MATCH 'Database memory'")?, 1);
        Ok(())
    })?;
    store.immediate(|tx| {
        tx.execute_batch(
            "INSERT OR REPLACE INTO session_events VALUES
                ('s', 1, 0, 'user_message', NULL, '{\"text\":\"leopard\"}');
             UPDATE session_events SET seq = 2 WHERE session_id = 's' AND seq = 1;
             UPDATE project_memories SET id = 'moved', title = 'Architecture', body = 'Revised';",
        )?;
        assert_eq!(count(tx, "SELECT count(*) FROM session_events_fts")?, 1);
        assert_eq!(count(tx, "SELECT count(*) FROM session_events_fts WHERE seq = 2 AND session_events_fts MATCH 'leopard'")?, 1);
        assert_eq!(count(tx, "SELECT count(*) FROM project_memories_fts WHERE project_memories_fts MATCH 'Database'")?, 0);
        assert_eq!(count(tx, "SELECT count(*) FROM project_memories_fts WHERE id = 'moved' AND project_memories_fts MATCH 'Revised'")?, 1);
        tx.execute("DELETE FROM sessions WHERE id = 's'", [])?;
        for table in ["session_events", "session_events_fts", "session_blobs", "session_snapshots", "usage_ledger"] {
            assert_eq!(count(tx, &format!("SELECT count(*) FROM {table}"))?, 0, "{table}");
        }
        assert_eq!(count(tx, "SELECT count(*) FROM project_memories_fts")?, 1);
        tx.execute("DELETE FROM project_memories WHERE id = 'moved'", [])?;
        assert_eq!(count(tx, "SELECT count(*) FROM project_memories_fts")?, 0);
        Ok(())
    })?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn unix_store_files_are_private_but_existing_custom_parent_is_unchanged() -> Result<()> {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn mode(path: &Path) -> Result<u32> {
        Ok(fs::metadata(path)?.permissions().mode() & 0o777)
    }

    let dir = TempDir::new()?;
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755))?;
    let shared_path = dir.path().join("shared.db");
    let shared = Store::open(&shared_path)?;
    assert_eq!(mode(dir.path())?, 0o755);
    assert_eq!(mode(&shared_path)?, 0o600);
    drop(shared);

    let path = dir.path().join("new/nested/store.db");
    let store = Store::open(&path)?;
    assert_eq!(mode(&dir.path().join("new"))?, 0o700);
    assert_eq!(mode(&dir.path().join("new/nested"))?, 0o700);
    store.config().set("key", "value")?;
    for name in ["store.db", "store.db-wal", "store.db-shm"] {
        let file = path.with_file_name(name);
        assert_eq!(mode(&file)?, 0o600, "{name}");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644))?;
    }
    let reopened = Store::open(&path)?;
    for name in ["store.db", "store.db-wal", "store.db-shm"] {
        assert_eq!(mode(&path.with_file_name(name))?, 0o600, "{name}");
    }
    assert_eq!(
        reopened.config().get::<String>("key")?.as_deref(),
        Some("value")
    );
    assert_eq!(mode(dir.path())?, 0o755);
    Ok(())
}

#[test]
fn closing_the_store_hardens_sidecars_created_late() -> Result<()> {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &Path) -> Result<u32> {
        Ok(fs::metadata(path)?.permissions().mode() & 0o777)
    }

    let dir = TempDir::new()?;
    let path = dir.path().join("store.db");
    let store = Store::open(&path)?;
    store.config().set("key", "value")?;
    for name in ["store.db-wal", "store.db-shm"] {
        let file = path.with_file_name(name);
        if file.exists() {
            fs::set_permissions(&file, fs::Permissions::from_mode(0o644))?;
        }
    }
    drop(store);
    for name in ["store.db", "store.db-wal", "store.db-shm"] {
        let file = path.with_file_name(name);
        if file.exists() {
            assert_eq!(mode(&file)?, 0o600, "{name}");
        }
    }
    Ok(())
}

#[test]
fn simultaneous_first_opens_initialize_once() -> Result<()> {
    use std::sync::Barrier;

    let dir = TempDir::new()?;
    let path = dir.path().join("new/store.db");
    let barrier = Barrier::new(4);
    std::thread::scope(|scope| -> Result<()> {
        let threads: Vec<_> = (0..4)
            .map(|i| {
                let barrier = &barrier;
                let path = &path;
                scope.spawn(move || {
                    barrier.wait();
                    Store::open(path)?.config().set(&i.to_string(), &i)
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("opener thread panicked")?;
        }
        Ok(())
    })?;
    let config = Store::open(path)?.config();
    for i in 0..4 {
        assert_eq!(config.get::<i32>(&i.to_string())?, Some(i));
    }
    Ok(())
}
