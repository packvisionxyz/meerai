use anyhow::{Context, Result, bail};
use rusqlite::{Connection, TransactionBehavior};

const VERSION: i64 = 1;

pub(crate) fn initialize(conn: &mut Connection) -> Result<()> {
    match version(conn)? {
        VERSION => return Ok(()),
        0 => (),
        other => bail!("unsupported SQLite store schema version {other} (expected {VERSION})"),
    }

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    match version(&tx)? {
        VERSION => (),
        0 => {
            tx.execute_batch(include_str!("v1.sql"))
                .context("initialize SQLite store schema v1")?;
            tx.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
                [VERSION.to_string()],
            )?;
            tx.pragma_update(None, "user_version", VERSION)?;
        }
        other => bail!("unsupported SQLite store schema version {other} (expected {VERSION})"),
    }
    tx.commit()?;
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
