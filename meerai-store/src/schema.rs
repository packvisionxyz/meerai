use anyhow::{Context, Result, bail};
use rusqlite::{Connection, TransactionBehavior};

const VERSION: i64 = 2;

pub(crate) fn initialize(conn: &mut Connection) -> Result<()> {
    let current = version(conn)?;
    if current == VERSION {
        return Ok(());
    }
    if current != 0 && current != 1 {
        bail!("unsupported SQLite store schema version {current} (expected {VERSION})");
    }

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    match version(&tx)? {
        VERSION => (),
        0 => {
            tx.execute_batch(include_str!("v1.sql"))
                .context("initialize SQLite store schema")?;
            tx.execute_batch(include_str!("v2.sql"))
                .context("migrate SQLite store schema to v2")?;
            record_version(&tx)?;
        }
        1 => {
            tx.execute_batch(include_str!("v2.sql"))
                .context("migrate SQLite store schema to v2")?;
            record_version(&tx)?;
        }
        other => bail!("unsupported SQLite store schema version {other} (expected {VERSION})"),
    }
    tx.commit()?;
    Ok(())
}

fn record_version(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    tx.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?1)",
        [VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", VERSION)?;
    Ok(())
}

fn version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
}

pub(crate) fn validate(conn: &Connection) -> Result<()> {
    let actual = version(conn)?;
    anyhow::ensure!(
        actual == VERSION,
        "unsupported SQLite store schema version {actual} (expected {VERSION})"
    );
    Ok(())
}
