use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result, ensure};
use meerai_core::env::now_millis;
use meerai_store::Store;
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::{LoggedEvent, Seq, SessionId, SessionMeta, SessionSummary};

pub(crate) fn default_root() -> PathBuf {
    let Some(db) = std::env::var_os("MEERAI_DB_PATH").map(PathBuf::from) else {
        return meerai_core::env::sessions_dir();
    };
    if db == Path::new(":memory:") {
        static ROOT: OnceLock<PathBuf> = OnceLock::new();
        return ROOT
            .get_or_init(|| std::env::temp_dir().join(SessionId::new().as_str()))
            .clone();
    }
    let mut root = db.into_os_string();
    root.push(".sessions");
    root.into()
}

pub(crate) fn db_path(root: &Path) -> PathBuf {
    if root == default_root()
        && (std::env::var_os("MEERAI_SESSION_DIR").is_none()
            || std::env::var_os("MEERAI_DB_PATH").is_some())
        && let Ok(path) = Store::default_path()
    {
        return path;
    }
    root.join("meerai.db")
}

fn memory_store() -> Result<Store> {
    static STORE: OnceLock<Store> = OnceLock::new();
    if let Some(store) = STORE.get() {
        return Ok(store.clone());
    }
    Ok(STORE
        .get_or_init({
            let store = Store::open(":memory:")?;
            move || store
        })
        .clone())
}

pub(crate) fn open(root: &Path) -> Result<Store> {
    let path = db_path(root);
    if path == Path::new(":memory:") {
        return memory_store();
    }
    if Store::default_path().is_ok_and(|default| default == path) {
        return Store::open_default();
    }
    Store::open(path)
}

pub(crate) fn read_only(root: &Path) -> Result<Option<Store>> {
    let path = db_path(root);
    if path == Path::new(":memory:") {
        return memory_store().map(Some);
    }
    let parent = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = match parent.canonicalize() {
        Ok(parent) => parent,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("resolve store directory {}", parent.display()));
        }
    };
    Store::open_existing_read_only(
        parent.join(path.file_name().context("missing database file name")?),
    )
}

pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .context("session storage task failed")?
}

pub(crate) fn validate_id(id: &SessionId) -> Result<()> {
    ensure!(
        crate::ids::is_safe_segment(id.as_str()),
        "invalid session id: {id}"
    );
    Ok(())
}

pub(crate) fn validate_dir(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => ensure!(meta.is_dir(), "not a session directory: {}", path.display()),
        Err(err) if err.kind() == ErrorKind::NotFound => (),
        Err(err) => return Err(err.into()),
    }
    Ok(())
}

fn meta_row(row: &Row<'_>) -> rusqlite::Result<SessionMeta> {
    Ok(SessionMeta {
        id: SessionId::from_raw(row.get::<_, String>(0)?),
        cwd: PathBuf::from(row.get::<_, String>(1)?),
        created_at_ms: row.get(2)?,
        updated_at_ms: row.get(3)?,
        title: row.get(4)?,
        provider: row.get(5)?,
        model: row.get(6)?,
        approval_mode: row.get(7)?,
        git_branch: row.get(8)?,
        git_sha: row.get(9)?,
        forked_from: row.get::<_, Option<String>>(10)?.map(SessionId::from_raw),
        inherited_event_count: row.get(11)?,
        parent_session_id: row.get::<_, Option<String>>(12)?.map(SessionId::from_raw),
        harness: row.get(13)?,
        remote_session_id: row.get(14)?,
    })
}

pub(crate) fn meta(conn: &Connection, id: &SessionId) -> Result<Option<SessionMeta>> {
    Ok(conn
        .query_row(
            "SELECT id, cwd, created_at_ms, updated_at_ms, title, provider, model, approval_mode,
         git_branch, git_sha, forked_from, inherited_event_count, parent_session_id, harness,
         remote_session_id
         FROM sessions WHERE id = ?1",
            [id.as_str()],
            meta_row,
        )
        .optional()?)
}

pub(crate) fn insert_session(conn: &Connection, project: &Path, meta: &SessionMeta) -> Result<()> {
    conn.execute(
        "INSERT INTO sessions (id, project_root, cwd, created_at_ms, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            meta.id.as_str(),
            project.to_str().context("project path is not UTF-8")?,
            meta.cwd.to_str().context("session cwd is not UTF-8")?,
            meta.created_at_ms,
            meta.updated_at_ms
        ],
    )?;
    update_meta(conn, meta)
}

pub(crate) fn update_meta(conn: &Connection, meta: &SessionMeta) -> Result<()> {
    let count = conn.execute(
        "UPDATE sessions SET cwd = ?2, created_at_ms = ?3, updated_at_ms = ?4, title = ?5,
         provider = ?6, model = ?7, approval_mode = ?8, git_branch = ?9, git_sha = ?10,
         forked_from = ?11, inherited_event_count = ?12, parent_session_id = ?13, harness = ?14,
         remote_session_id = ?15
         WHERE id = ?1",
        params![
            meta.id.as_str(),
            meta.cwd.to_str().context("session cwd is not UTF-8")?,
            meta.created_at_ms,
            meta.updated_at_ms,
            meta.title,
            meta.provider,
            meta.model,
            meta.approval_mode,
            meta.git_branch,
            meta.git_sha,
            meta.forked_from.as_ref().map(SessionId::as_str),
            meta.inherited_event_count,
            meta.parent_session_id.as_ref().map(SessionId::as_str),
            meta.harness,
            meta.remote_session_id
        ],
    )?;
    ensure!(count == 1, "session not found: {}", meta.id);
    Ok(())
}

pub(crate) fn events(conn: &Connection, id: &SessionId) -> Result<Vec<LoggedEvent>> {
    let mut stmt = conn.prepare(
        "SELECT seq, payload_json FROM session_events WHERE session_id = ?1 ORDER BY seq",
    )?;
    read_events(&mut stmt, [id.as_str()])
}

fn read_events(
    stmt: &mut rusqlite::Statement<'_>,
    params: impl rusqlite::Params,
) -> Result<Vec<LoggedEvent>> {
    let mut rows = stmt.query(params)?;
    let mut events = Vec::new();
    while let Some(row) = rows.next()? {
        let seq = Seq(row.get(0)?);
        let raw: String = row.get(1)?;
        let event = serde_json::from_str(&raw)
            .with_context(|| format!("decode session event {}", seq.0))?;
        events.push(LoggedEvent { seq, event });
    }
    Ok(events)
}

pub(crate) fn insert_event(conn: &Connection, id: &SessionId, logged: &LoggedEvent) -> Result<()> {
    let payload = serde_json::to_string(&logged.event)?;
    conn.execute(
        "INSERT INTO session_events (session_id, seq, ts_ms, event_type, turn_id, payload_json)
         VALUES (?1, ?2, ?3, json_extract(?4, '$.type'), json_extract(?4, '$.turn_id'), ?4)",
        params![id.as_str(), logged.seq.0, now_millis(), payload],
    )?;
    Ok(())
}

pub(crate) fn list(conn: &Connection, root: &Path) -> Result<Vec<SessionSummary>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, model, created_at_ms, updated_at_ms, parent_session_id,
         COALESCE((SELECT SUM(length(CAST(payload_json AS BLOB))) FROM session_events WHERE session_id = s.id), 0)
         + COALESCE((SELECT SUM(length(content_zstd)) FROM session_blobs WHERE session_id = s.id), 0)
         + COALESCE((SELECT SUM(length(content_zstd)) FROM session_snapshots WHERE session_id = s.id), 0)
         FROM sessions AS s ORDER BY updated_at_ms DESC, id",
    )?;
    let rows = stmt.query_map([], |row| {
        let id = SessionId::from_raw(row.get::<_, String>(0)?);
        Ok(SessionSummary {
            dir: root.join(id.as_str()),
            id,
            title: row.get(1)?,
            model: row.get(2)?,
            created_at_ms: row.get(3)?,
            updated_at_ms: row.get(4)?,
            parent_session_id: row.get::<_, Option<String>>(5)?.map(SessionId::from_raw),
            bytes: row.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn has_user_input(conn: &Connection, id: &SessionId) -> Result<bool> {
    let value: Option<bool> = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM session_events WHERE session_id = ?1 AND event_type = 'user_message'
         AND json_extract(payload_json, '$.source') = 'user') FROM sessions WHERE id = ?1",
        [id.as_str()], |row| row.get(0),
    ).optional()?;
    value.with_context(|| format!("session not found: {id}"))
}

pub(crate) fn delete(conn: &Connection, id: &SessionId) -> Result<bool> {
    Ok(conn.execute("DELETE FROM sessions WHERE id = ?1", [id.as_str()])? != 0)
}

pub(crate) fn flush(store: &Store) -> Result<()> {
    store.read(|conn| {
        let (busy, _, _): (i64, i64, i64) =
            conn.query_row("PRAGMA wal_checkpoint(FULL)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
        ensure!(busy == 0, "session database checkpoint is busy");
        Ok(())
    })
}
