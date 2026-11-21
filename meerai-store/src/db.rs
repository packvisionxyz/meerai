use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, ensure};
use meerai_core::env::data_dir;
use rusqlite::{Connection, ErrorCode, OpenFlags, Transaction, TransactionBehavior};

use crate::{files, schema};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub struct Store {
    connection: Arc<Mutex<Connection>>,
    path: Option<Arc<PathBuf>>,
}

impl Drop for Store {
    fn drop(&mut self) {
        // The last handle runs SQLITE_FCNTL_PERSIST_WAL through SQLite's own close path. Doing it
        // after the connection is gone would need a second open, which releases the record locks.
        if let Some(path) = self.path.as_ref()
            && Arc::strong_count(&self.connection) == 1
        {
            let _ = files::harden_sidecars(path);
        }
    }
}

impl Store {
    pub fn default_path() -> Result<std::path::PathBuf> {
        if let Some(path) = std::env::var_os("MEERAI_DB_PATH") {
            return Ok(path.into());
        }
        Ok(data_dir()
            .context("no user data directory; set MEERAI_DB_PATH")?
            .join("meerai.db"))
    }

    pub fn open_default() -> Result<Self> {
        let path = Self::default_path()?;
        if std::env::var_os("MEERAI_DB_PATH").is_none() {
            files::prepare_default_dir(path.parent().context("missing store directory")?)?;
        }
        Self::open(path)
    }

    pub fn open_existing_read_only(path: impl AsRef<Path>) -> Result<Option<Self>> {
        let path = path.as_ref();
        if !files::validate_existing(path)? {
            return Ok(None);
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::from_bits_retain(rusqlite::ffi::SQLITE_OPEN_NOFOLLOW),
        )
        .with_context(|| format!("open read-only SQLite store at {}", path.display()))?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        schema::validate(&conn)?;
        Ok(Some(Self {
            connection: Arc::new(Mutex::new(conn)),
            path: Some(Arc::new(path.to_path_buf())),
        }))
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if path == Path::new(":memory:") {
            let conn = Connection::open_in_memory()?;
            return Self::from_connection(conn, false, None);
        }
        let result: Result<Self> = (|| {
            let path = files::prepare(path)?;
            let conn = Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX
                    | OpenFlags::from_bits_retain(rusqlite::ffi::SQLITE_OPEN_NOFOLLOW),
            )?;
            let store = Self::from_connection(conn, true, Some(path.clone()))?;
            files::harden_sidecars(&path)?;
            Ok(store)
        })();
        result.with_context(|| format!("open SQLite store at {}", path.display()))
    }

    // Clones share one connection. Callbacks must not re-enter the same store.
    pub fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.lock()?;
        f(&conn)
    }

    pub fn immediate<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = f(&tx)?;
        tx.commit()?;
        Ok(value)
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| anyhow!("SQLite store connection lock poisoned"))
    }

    fn from_connection(mut conn: Connection, disk: bool, path: Option<PathBuf>) -> Result<Self> {
        conn.busy_timeout(BUSY_TIMEOUT)?;
        if disk {
            enable_wal(&conn)?;
        }
        conn.execute_batch(
            "PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA recursive_triggers = ON;",
        )?;
        schema::initialize(&mut conn)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(conn)),
            path: path.map(Arc::new),
        })
    }
}

fn enable_wal(conn: &Connection) -> Result<()> {
    let start = Instant::now();
    loop {
        let result = conn.query_row("PRAGMA journal_mode = WAL", [], |row| {
            row.get::<_, String>(0)
        });
        match result {
            Ok(mode) => {
                ensure!(mode == "wal", "SQLite refused WAL mode (mode is {mode})");
                return Ok(());
            }
            // The journal-mode lock upgrade can return BUSY without calling SQLite's busy handler.
            Err(err)
                if err.sqlite_error_code() == Some(ErrorCode::DatabaseBusy)
                    && start.elapsed() < BUSY_TIMEOUT =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(err) => return Err(err).context("enable SQLite WAL mode"),
        }
    }
}
