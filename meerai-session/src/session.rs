use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use anyhow::{Context, Result};
use genai::chat::ChatMessage;
use meerai_core::ToolObservation;
use meerai_core::env::now_millis;
use meerai_store::Store;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::blob::BlobStore;
use crate::compaction::{COMPACTION_INTERRUPTED, open_compactions, open_turns, turn_start};
use crate::event::{
    AbortCause, CompactionResult, LoggedEvent, RewindScope, Seq, SeqRange, SessionEvent,
    TurnEndReason,
};
use crate::ids::{CallId, SessionId, TurnId};
use crate::projection::{ProjectionPolicy, derive_messages, shadowed_ranges};
use crate::{perms, storage};

const LOCK_FILE: &str = "session.lock";
const TOOL_INTERRUPTED: &str = "tool execution interrupted before completion";

pub fn default_sessions_root() -> PathBuf {
    storage::default_root()
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionConfig {
    pub root: PathBuf,
    pub workspace: PathBuf,
}

impl SessionConfig {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            root: default_sessions_root(),
            workspace: workspace.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: SessionId,
    pub cwd: PathBuf,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherited_event_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_session_id: Option<String>,
}

impl SessionMeta {
    fn new(id: SessionId, cwd: PathBuf) -> Self {
        let now = now_millis();
        Self {
            id,
            cwd,
            created_at_ms: now,
            updated_at_ms: now,
            title: None,
            model: None,
            approval_mode: None,
            git_branch: None,
            git_sha: None,
            forked_from: None,
            inherited_event_count: None,
            parent_session_id: None,
            harness: None,
            remote_session_id: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSummary {
    pub id: SessionId,
    pub dir: PathBuf,
    pub title: Option<String>,
    pub model: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub bytes: u64,
    pub parent_session_id: Option<SessionId>,
}

#[derive(Debug, Default)]
struct GitState {
    branch: Option<String>,
    sha: Option<String>,
}

impl GitState {
    async fn detect(workspace: &Path) -> Self {
        let Some(git) = resolve_git_dir(workspace).await else {
            return Self::default();
        };
        let Some(head) = read_text_file(&git.join("HEAD")).await else {
            return Self::default();
        };
        let head = head.trim();
        if let Some(reference) = head.strip_prefix("ref: ") {
            let branch = reference
                .strip_prefix("refs/heads/")
                .unwrap_or(reference)
                .to_string();
            let sha = resolve_git_ref(&git, reference.trim()).await;
            Self {
                branch: Some(branch),
                sha,
            }
        } else if is_git_sha(head) {
            Self {
                branch: None,
                sha: Some(head.to_string()),
            }
        } else {
            Self::default()
        }
    }
}

async fn resolve_git_dir(workspace: &Path) -> Option<PathBuf> {
    let dot_git = workspace.join(".git");
    let meta = fs::metadata(&dot_git).await.ok()?;
    if meta.is_dir() {
        return Some(dot_git);
    }
    if meta.is_file()
        && let Some(raw) = read_text_file(&dot_git).await
        && let Some(gitdir) = raw.trim().strip_prefix("gitdir:")
    {
        let target = PathBuf::from(gitdir.trim());
        return Some(if target.is_absolute() {
            target
        } else {
            workspace.join(target)
        });
    }
    None
}

async fn resolve_git_ref(git: &Path, reference: &str) -> Option<String> {
    if let Some(raw) = read_text_file(&git.join(reference)).await {
        let value = raw.trim();
        if is_git_sha(value) {
            return Some(value.to_string());
        }
    }
    let packed = read_text_file(&git.join("packed-refs")).await?;
    for line in packed.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        let Some((sha, name)) = line.split_once(' ') else {
            continue;
        };
        if name.trim() == reference && is_git_sha(sha) {
            return Some(sha.to_string());
        }
    }
    None
}

async fn read_text_file(path: &Path) -> Option<String> {
    fs::read_to_string(path).await.ok()
}

fn is_git_sha(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Clone)]
pub struct Session {
    inner: Arc<SessionInner>,
}

struct SessionInner {
    id: SessionId,
    dir: Option<PathBuf>,
    config: Option<SessionConfig>,
    store: Store,
    events: RwLock<Vec<LoggedEvent>>,
    meta: RwLock<SessionMeta>,
    next_seq: AtomicU64,
    write_guard: Arc<tokio::sync::Mutex<()>>,
    _lock: Option<SessionLock>,
}

struct SessionLock {
    _file: std::fs::File,
}

impl SessionLock {
    fn acquire(dir: &Path, id: &SessionId) -> Result<Self> {
        let lock_path = dir.join(LOCK_FILE);
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("failed to open lock file {}", lock_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(
                &lock_path,
                std::fs::Permissions::from_mode(meerai_core::fs_perms::FILE_MODE),
            );
        }
        match file.try_lock() {
            Ok(()) => {
                let _ = file.set_len(0);
                let _ = file.seek(SeekFrom::Start(0));
                let _ = writeln!(file, "pid={}", std::process::id());
                let _ = file.flush();
                Ok(Self { _file: file })
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                let holder = read_lock_holder(&lock_path);
                anyhow::bail!("session {id} is already open ({holder})")
            }
            Err(std::fs::TryLockError::Error(err)) => {
                Err(anyhow::Error::from(err)
                    .context(format!("failed to lock {}", lock_path.display())))
            }
        }
    }
}

fn read_lock_holder(lock_path: &Path) -> String {
    match std::fs::read_to_string(lock_path) {
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                "held by another session owner".to_string()
            } else {
                format!("held by {trimmed}")
            }
        }
        Err(_) => "held by another session owner".to_string(),
    }
}

impl Session {
    pub fn memory(id: SessionId, cwd: PathBuf) -> Self {
        let store = Store::open(":memory:").expect("open in-memory session store");
        let meta = SessionMeta::new(id.clone(), cwd.clone());
        store
            .immediate(|tx| storage::insert_session(tx, &cwd, &meta))
            .expect("insert in-memory session");
        Self {
            inner: Arc::new(SessionInner {
                meta: RwLock::new(meta),
                id,
                dir: None,
                config: None,
                store,
                events: RwLock::new(Vec::new()),
                next_seq: AtomicU64::new(1),
                write_guard: Arc::new(tokio::sync::Mutex::new(())),
                _lock: None,
            }),
        }
    }

    pub async fn open(config: &SessionConfig, id: SessionId, cwd: PathBuf) -> Result<Self> {
        Self::open_inner(config, id, cwd, false).await
    }

    pub async fn load_events(config: &SessionConfig, id: &SessionId) -> Result<Vec<LoggedEvent>> {
        Ok(read_sqlite_session(config, id).await?.1)
    }

    pub async fn open_or_create(
        config: &SessionConfig,
        id: SessionId,
        cwd: PathBuf,
    ) -> Result<Self> {
        Self::open_inner(config, id, cwd, true).await
    }

    async fn open_inner(
        config: &SessionConfig,
        id: SessionId,
        cwd: PathBuf,
        create: bool,
    ) -> Result<Self> {
        let dir = session_dir(&config.root, &id)?;
        if !create {
            let root = config.root.clone();
            let id = id.clone();
            storage::blocking(move || {
                let store = storage::read_only(&root)?
                    .with_context(|| format!("session not found: {id}"))?;
                store
                    .read(|conn| storage::meta(conn, &id))?
                    .with_context(|| format!("session not found: {id}"))
            })
            .await?;
        }
        storage::validate_dir(&dir)?;
        perms::create_private_dir(&dir).await?;
        let lock = SessionLock::acquire(&dir, &id)?;
        tighten_existing_files(&dir).await;

        let root = config.root.clone();
        let stored_id = id.clone();
        let (store, meta) = storage::blocking(move || {
            let store = storage::open(&root)?;
            let meta = store.read(|conn| storage::meta(conn, &stored_id))?;
            Ok((store, meta))
        })
        .await?;
        let existed = meta.is_some();
        let mut meta = match meta {
            Some(meta) => meta,
            None if create => SessionMeta::new(id.clone(), cwd),
            None => anyhow::bail!("session not found: {id}"),
        };

        let git = GitState::detect(&config.workspace).await;
        let mut meta_changed = !existed;
        if meta.git_branch.is_none()
            && let Some(branch) = git.branch
        {
            meta.git_branch = Some(branch);
            meta_changed = true;
        }
        if meta.git_sha.is_none()
            && let Some(sha) = git.sha
        {
            meta.git_sha = Some(sha);
            meta_changed = true;
        }
        let stored_meta = meta.clone();
        let workspace = config.workspace.clone();
        let events = {
            let store = store.clone();
            storage::blocking(move || {
                if meta_changed {
                    store.immediate(|tx| {
                        if existed {
                            storage::update_meta(tx, &stored_meta)
                        } else {
                            storage::insert_session(tx, &workspace, &stored_meta)
                        }
                    })?;
                }
                store.read(|conn| storage::events(conn, &stored_meta.id))
            })
            .await?
        };
        let next_seq = events.iter().map(|logged| logged.seq.0).max().unwrap_or(0) + 1;

        let session = Self {
            inner: Arc::new(SessionInner {
                id,
                dir: Some(dir),
                config: Some(config.clone()),
                store,
                events: RwLock::new(events),
                meta: RwLock::new(meta),
                next_seq: AtomicU64::new(next_seq),
                write_guard: Arc::new(tokio::sync::Mutex::new(())),
                _lock: Some(lock),
            }),
        };
        session.heal_dangling_tool_calls().await?;
        session.close_open_turns().await?;
        session.close_open_compactions().await?;
        Ok(session)
    }

    pub fn blob_store(&self) -> Option<BlobStore> {
        Some(BlobStore::new(
            self.inner.store.clone(),
            self.inner.id.clone(),
        ))
    }

    pub async fn read_blob(&self, key: &str) -> Result<Option<String>> {
        if key.contains('/') || key.contains('\\') || key.contains("..") {
            anyhow::bail!("invalid blob key");
        }
        match self.blob_store() {
            Some(blobs) => blobs.get(key),
            None => Ok(None),
        }
    }

    pub fn list_blobs(&self) -> Result<Vec<(String, u64)>> {
        match self.blob_store() {
            Some(blobs) => blobs.list(),
            None => Ok(Vec::new()),
        }
    }

    pub async fn append(&self, event: SessionEvent) -> Result<Seq> {
        let guard = self.inner.write_guard.clone().lock_owned().await;
        let store = self.inner.store.clone();
        let inner = self.inner.clone();
        storage::blocking(move || {
            let _guard = guard;
            let seq = Seq(inner.next_seq.load(Ordering::Relaxed));
            let logged = LoggedEvent { seq, event };
            let updated_meta = matches!(&logged.event, SessionEvent::TurnEnded { .. }).then(|| {
                let mut meta = read_lock(&inner.meta).clone();
                meta.updated_at_ms = now_millis();
                meta
            });
            store.immediate(|tx| {
                storage::insert_event(tx, &inner.id, &logged)?;
                if let Some(meta) = &updated_meta {
                    storage::update_meta(tx, meta)?;
                }
                Ok(())
            })?;
            inner.next_seq.store(seq.0 + 1, Ordering::Relaxed);
            write_lock(&inner.events).push(logged);
            if let Some(meta) = updated_meta {
                *write_lock(&inner.meta) = meta;
            }
            Ok(seq)
        })
        .await
    }

    pub fn events(&self) -> Vec<LoggedEvent> {
        read_lock(&self.inner.events).clone()
    }

    pub fn event_count(&self) -> usize {
        read_lock(&self.inner.events).len()
    }

    pub fn id(&self) -> &SessionId {
        &self.inner.id
    }

    pub fn dir(&self) -> Option<&Path> {
        self.inner.dir.as_deref()
    }

    pub fn snapshot_store(&self) -> Option<crate::SnapshotStore> {
        Some(crate::SnapshotStore::new(
            self.inner.store.clone(),
            self.inner.id.clone(),
        ))
    }

    pub fn config(&self) -> Option<&SessionConfig> {
        self.inner.config.as_ref()
    }

    pub fn meta(&self) -> SessionMeta {
        read_lock(&self.inner.meta).clone()
    }

    pub async fn update_meta(&self, f: impl FnOnce(&mut SessionMeta)) -> Result<()> {
        let guard = self.inner.write_guard.clone().lock_owned().await;
        let mut meta = self.meta();
        f(&mut meta);
        anyhow::ensure!(meta.id == self.inner.id, "cannot change session id");
        meta.updated_at_ms = now_millis();
        let store = self.inner.store.clone();
        let inner = self.inner.clone();
        storage::blocking(move || {
            let _guard = guard;
            store.immediate(|tx| storage::update_meta(tx, &meta))?;
            *write_lock(&inner.meta) = meta;
            Ok(())
        })
        .await
    }

    pub async fn set_title(&self, title: impl Into<String>) -> Result<()> {
        let title = title.into();
        self.update_meta(|meta| meta.title = Some(title)).await
    }

    pub async fn set_model(&self, model: impl Into<String>) -> Result<()> {
        let model = model.into();
        self.update_meta(|meta| meta.model = Some(model)).await
    }

    pub async fn set_approval_mode(&self, approval_mode: impl Into<String>) -> Result<()> {
        let approval_mode = approval_mode.into();
        self.update_meta(|meta| meta.approval_mode = Some(approval_mode))
            .await
    }

    pub async fn set_remote_session(
        &self,
        harness: impl Into<String>,
        remote_session_id: impl Into<String>,
    ) -> Result<()> {
        let harness = harness.into();
        let remote_session_id = remote_session_id.into();
        self.update_meta(|meta| {
            meta.harness = Some(harness);
            meta.remote_session_id = Some(remote_session_id);
        })
        .await
    }

    pub async fn clear_remote_session(&self) -> Result<()> {
        self.update_meta(|meta| {
            meta.harness = None;
            meta.remote_session_id = None;
        })
        .await
    }

    pub async fn flush(&self) -> Result<()> {
        let guard = self.inner.write_guard.clone().lock_owned().await;
        let store = self.inner.store.clone();
        storage::blocking(move || {
            let _guard = guard;
            storage::flush(&store)
        })
        .await
    }

    pub fn derive_messages(&self, policy: &ProjectionPolicy) -> Vec<ChatMessage> {
        let events = self.events();
        derive_messages(&events, policy)
    }

    pub fn search_events(&self, query: &str, limit: usize) -> Result<Vec<LoggedEvent>> {
        let Some(fts) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let limit = i64::try_from(limit.max(1))?;
        self.inner.store.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT e.seq, e.payload_json
                 FROM session_events_fts
                 JOIN session_events e
                   ON e.session_id = session_events_fts.session_id
                  AND e.seq = session_events_fts.seq
                 WHERE session_events_fts.session_id = ?1
                   AND session_events_fts MATCH ?2
                 ORDER BY e.seq
                 LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![self.inner.id.as_str(), fts, limit], |row| {
                let seq = Seq(row.get(0)?);
                let raw: String = row.get(1)?;
                Ok((seq, raw))
            })?;
            let mut events = Vec::new();
            for row in rows {
                let (seq, raw) = row?;
                let event = serde_json::from_str(&raw)
                    .with_context(|| format!("decode session event {}", seq.0))?;
                events.push(LoggedEvent { seq, event });
            }
            Ok(events)
        })
    }

    pub async fn record_usage(
        &self,
        turn_id: Option<&TurnId>,
        provider: &str,
        model: &str,
        task_type: Option<&str>,
        usage: &crate::event::Usage,
        est_cost_usd: Option<f64>,
    ) -> Result<()> {
        let entry = meerai_store::UsageEntry {
            session_id: self.inner.id.as_str().to_string(),
            parent_session_id: self
                .meta()
                .parent_session_id
                .map(|id| id.as_str().to_string()),
            turn_id: turn_id.map(|turn| turn.as_str().to_string()),
            agent_id: None,
            task_type: task_type.map(str::to_string),
            provider: provider.to_string(),
            model: model.to_string(),
            prompt_tokens: usage.prompt_tokens.unwrap_or(0).max(0) as u64,
            completion_tokens: usage.completion_tokens.unwrap_or(0).max(0) as u64,
            cached_tokens: usage.cached_tokens.unwrap_or(0).max(0) as u64,
            est_cost_usd,
            ts_ms: now_millis(),
        };
        let store = self.inner.store.clone();
        storage::blocking(move || store.usage().record(&entry)).await
    }

    pub async fn fork(
        config: &SessionConfig,
        source: &Session,
        boundary_turn: &TurnId,
    ) -> Result<Session> {
        let events = source.events();
        let meta = source.meta();
        Self::fork_from_parts(config, source.id(), &meta, &events, Some(boundary_turn)).await
    }

    pub async fn fork_stored(
        config: &SessionConfig,
        source_id: &SessionId,
        boundary_turn: Option<&TurnId>,
    ) -> Result<Session> {
        let (meta, events) = read_sqlite_session(config, source_id).await?;
        Self::fork_from_parts(config, source_id, &meta, &events, boundary_turn).await
    }

    async fn fork_from_parts(
        config: &SessionConfig,
        source_id: &SessionId,
        source_meta: &SessionMeta,
        events: &[LoggedEvent],
        boundary_turn: Option<&TurnId>,
    ) -> Result<Session> {
        let resolved_boundary = match boundary_turn {
            Some(turn) => turn.clone(),
            None => latest_closed_turn_in_events(events).ok_or_else(|| {
                anyhow::anyhow!("session has no closed turns to fork: {source_id}")
            })?,
        };
        let prefix = fork_prefix(events, &resolved_boundary)?;
        let inherited_event_count = prefix.len() as u64;
        let approval_mode = source_meta.approval_mode.clone();
        let cwd = source_meta.cwd.clone();

        let session = Self::open_or_create(config, SessionId::new(), cwd).await?;
        if let Err(error) = Self::fill_forked(
            &session,
            source_id,
            inherited_event_count,
            &prefix,
            approval_mode,
        )
        .await
        {
            discard_session(&session).await;
            return Err(error);
        }

        Ok(session)
    }

    async fn fill_forked(
        session: &Session,
        source_id: &SessionId,
        inherited_event_count: u64,
        prefix: &[LoggedEvent],
        approval_mode: Option<String>,
    ) -> Result<()> {
        session.append_preserved(prefix).await?;
        session
            .update_meta(|meta| {
                meta.forked_from = Some(source_id.clone());
                meta.inherited_event_count = Some(inherited_event_count);
                meta.approval_mode = approval_mode;
            })
            .await
    }

    pub fn latest_closed_turn(&self) -> Option<TurnId> {
        latest_closed_turn_in_events(&self.events())
    }

    pub async fn fork_latest(config: &SessionConfig, source: &Session) -> Result<Session> {
        let events = source.events();
        let meta = source.meta();
        Self::fork_from_parts(config, source.id(), &meta, &events, None).await
    }

    pub async fn create_child(
        &self,
        model: Option<String>,
        title: Option<String>,
    ) -> Result<Session> {
        let parent_id = self.id().clone();
        let cwd = self.meta().cwd;
        let child = match self.config() {
            Some(config) => Self::open_or_create(config, SessionId::new(), cwd).await?,
            None => Self::memory(SessionId::new(), cwd),
        };
        if let Err(error) = Self::link_child(&child, parent_id, model, title).await {
            discard_session(&child).await;
            return Err(error);
        }
        Ok(child)
    }

    async fn link_child(
        child: &Session,
        parent_id: SessionId,
        model: Option<String>,
        title: Option<String>,
    ) -> Result<()> {
        child
            .update_meta(|meta| {
                meta.parent_session_id = Some(parent_id);
                if model.is_some() {
                    meta.model = model;
                }
                if title.is_some() {
                    meta.title = title;
                }
            })
            .await
    }

    pub async fn rewind(
        &self,
        turn_id: &TurnId,
        scope: RewindScope,
        restored: Vec<PathBuf>,
    ) -> Result<()> {
        let events = self.events();
        let start = turn_start(&events, turn_id)
            .ok_or_else(|| anyhow::anyhow!("unknown turn: {turn_id}"))?;
        let close_conversation = matches!(scope, RewindScope::Both | RewindScope::Conversation);
        let closing: Vec<SessionEvent> = if close_conversation {
            open_turns(&events)
                .into_iter()
                .map(|open| SessionEvent::TurnEnded {
                    turn_id: open,
                    reason: TurnEndReason::Aborted {
                        cause: AbortCause::User,
                    },
                })
                .collect()
        } else {
            Vec::new()
        };
        let replaced = close_conversation.then(|| SeqRange {
            start,
            end_inclusive: Seq(
                events.last().map_or(start.0, |logged| logged.seq.0) + closing.len() as u64
            ),
        });
        // One batch: the closing turns and the rewind marker land together or not at all.
        self.append_batch(
            &closing,
            Some(SessionEvent::RewindApplied {
                turn_id: turn_id.clone(),
                scope,
                restored,
                replaced,
            }),
        )
        .await
    }

    async fn append_batch(&self, first: &[SessionEvent], last: Option<SessionEvent>) -> Result<()> {
        let guard = self.inner.write_guard.clone().lock_owned().await;
        let store = self.inner.store.clone();
        let inner = self.inner.clone();
        let first = first.to_vec();
        storage::blocking(move || {
            let _guard = guard;
            let mut next = inner.next_seq.load(Ordering::Relaxed);
            let mut logged = Vec::with_capacity(first.len() + usize::from(last.is_some()));
            for event in first.into_iter().chain(last) {
                logged.push(LoggedEvent {
                    seq: Seq(next),
                    event,
                });
                next += 1;
            }
            let mut meta = None;
            if logged
                .iter()
                .any(|event| matches!(event.event, SessionEvent::TurnEnded { .. }))
            {
                let mut updated = read_lock(&inner.meta).clone();
                updated.updated_at_ms = now_millis();
                meta = Some(updated);
            }
            store.immediate(|tx| {
                for event in &logged {
                    storage::insert_event(tx, &inner.id, event)?;
                }
                if let Some(meta) = &meta {
                    storage::update_meta(tx, meta)?;
                }
                Ok(())
            })?;
            inner.next_seq.store(next, Ordering::Relaxed);
            write_lock(&inner.events).extend(logged);
            if let Some(meta) = meta {
                *write_lock(&inner.meta) = meta;
            }
            Ok(())
        })
        .await
    }

    async fn close_open_turns(&self) -> Result<()> {
        for turn_id in open_turns(&self.events()) {
            self.append(SessionEvent::TurnEnded {
                turn_id,
                reason: TurnEndReason::Interrupted,
            })
            .await?;
        }
        Ok(())
    }

    async fn close_open_compactions(&self) -> Result<()> {
        for compaction_id in open_compactions(&self.events()) {
            self.append(SessionEvent::CompactionEnded {
                compaction_id,
                result: CompactionResult::Failed {
                    error: COMPACTION_INTERRUPTED.to_string(),
                },
            })
            .await?;
        }
        Ok(())
    }

    pub async fn heal_dangling_tool_calls(&self) -> Result<()> {
        let events = self.events();
        let shadowed = shadowed_ranges(&events);
        let mut pending: Vec<CallId> = Vec::new();
        for logged in &events {
            if shadowed.iter().any(|range| range.contains(logged.seq)) {
                continue;
            }
            match &logged.event {
                SessionEvent::AssistantMessage { tool_calls, .. } => {
                    pending = tool_calls
                        .iter()
                        .map(|call| CallId::from(call.id.as_str()))
                        .collect();
                }
                SessionEvent::ToolResult { call_id, .. } => {
                    pending.retain(|id| id != call_id);
                }
                _ => {}
            }
        }
        for call_id in pending {
            self.append(SessionEvent::ToolResult {
                call_id,
                output: ToolObservation::error(TOOL_INTERRUPTED),
            })
            .await?;
        }
        Ok(())
    }

    async fn append_preserved(&self, events: &[LoggedEvent]) -> Result<()> {
        let guard = self.inner.write_guard.clone().lock_owned().await;
        let store = self.inner.store.clone();
        let session = self.clone();
        let events = events.to_vec();
        storage::blocking(move || {
            let _guard = guard;
            store.immediate(|tx| {
                for event in &events {
                    storage::insert_event(tx, session.id(), event)?;
                }
                Ok(())
            })?;
            write_lock(&session.inner.events).extend_from_slice(&events);
            session.advance_next_seq(&events);
            Ok(())
        })
        .await
    }

    fn advance_next_seq(&self, events: &[LoggedEvent]) {
        if let Some(max) = events.iter().map(|logged| logged.seq.0).max() {
            self.inner.next_seq.fetch_max(max + 1, Ordering::Relaxed);
        }
    }
}

fn fts_query(query: &str) -> Option<String> {
    let tokens: Vec<String> = query
        .split_whitespace()
        .filter(|token| !matches!(*token, "AND" | "OR" | "NOT" | "NEAR"))
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect();
    (!tokens.is_empty()).then(|| tokens.join(" "))
}

// A half-built fork or child must not survive in the session list. Removal is best effort: the
// original error is always the one reported.
async fn discard_session(session: &Session) {
    if let Some(config) = session.config() {
        let root = config.root.clone();
        let id = session.id().clone();
        let _ = storage::blocking(move || delete_session(&root, &id)).await;
    }
}

fn latest_closed_turn_in_events(events: &[LoggedEvent]) -> Option<TurnId> {
    events.iter().rev().find_map(|logged| match &logged.event {
        SessionEvent::TurnEnded { turn_id, .. } => Some(turn_id.clone()),
        _ => None,
    })
}

fn fork_prefix(events: &[LoggedEvent], boundary_turn: &TurnId) -> Result<Vec<LoggedEvent>> {
    let start = events
        .iter()
        .position(|logged| {
            matches!(&logged.event, SessionEvent::TurnStarted { turn_id, .. } if turn_id == boundary_turn)
        })
        .ok_or_else(|| anyhow::anyhow!("unknown turn: {boundary_turn}"))?;
    let end = events
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, logged)| {
            matches!(&logged.event, SessionEvent::TurnEnded { turn_id, .. } if turn_id == boundary_turn)
        })
        .map(|(index, _)| index)
        .ok_or_else(|| anyhow::anyhow!("turn {boundary_turn} is still open"))?;

    let prefix = &events[..=end];
    if !open_turns(prefix).is_empty() {
        anyhow::bail!("cannot fork: prefix ends with an open turn");
    }
    Ok(prefix.to_vec())
}

fn session_dir(root: &Path, id: &SessionId) -> Result<PathBuf> {
    storage::validate_id(id)?;
    Ok(root.join(id.as_str()))
}

async fn read_sqlite_session(
    config: &SessionConfig,
    id: &SessionId,
) -> Result<(SessionMeta, Vec<LoggedEvent>)> {
    let root = config.root.clone();
    let id = id.clone();
    storage::blocking(move || {
        let store =
            storage::read_only(&root)?.with_context(|| format!("session not found: {id}"))?;
        store.read(|conn| {
            let tx = conn.unchecked_transaction()?;
            let meta =
                storage::meta(&tx, &id)?.with_context(|| format!("session not found: {id}"))?;
            let events = storage::events(&tx, &id)?;
            tx.commit()?;
            Ok((meta, events))
        })
    })
    .await
}

async fn tighten_existing_files(dir: &Path) {
    let _ = perms::tighten_file(&dir.join(LOCK_FILE)).await;
}

pub fn list_sessions(root: &Path) -> Result<Vec<SessionSummary>> {
    let Some(store) = storage::read_only(root)? else {
        return Ok(Vec::new());
    };
    let mut summaries = store.read(|conn| storage::list(conn, root))?;
    summaries.sort_by(|left, right| {
        right
            .updated_at_ms
            .cmp(&left.updated_at_ms)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(summaries)
}

pub fn latest_session(root: &Path) -> Result<Option<SessionSummary>> {
    let sessions = list_sessions(root)?;
    Ok(sessions
        .iter()
        .find(|summary| summary.parent_session_id.is_none())
        .cloned()
        .or_else(|| sessions.into_iter().next()))
}

pub async fn read_session_blob(
    config: &SessionConfig,
    id: &SessionId,
    key: &str,
) -> Result<Option<String>> {
    if key.contains('/') || key.contains('\\') || key.contains("..") {
        anyhow::bail!("invalid blob key");
    }
    let root = config.root.clone();
    let id = id.clone();
    let key = key.to_string();
    storage::blocking(move || {
        let Some(store) = storage::read_only(&root)? else {
            return Ok(None);
        };
        BlobStore::new(store, id).get(&key)
    })
    .await
}

pub async fn session_has_user_input(dir: &Path) -> Result<bool> {
    let root = dir.parent().context("missing sessions root")?.to_owned();
    let name = dir
        .file_name()
        .and_then(|name| name.to_str())
        .context("invalid session directory")?;
    let id = SessionId::from(name);
    storage::validate_id(&id)?;
    storage::blocking(move || {
        let store =
            storage::read_only(&root)?.with_context(|| format!("session not found: {id}"))?;
        store.read(|conn| storage::has_user_input(conn, &id))
    })
    .await
}

pub fn delete_session(root: &Path, id: &SessionId) -> Result<bool> {
    let dir = session_dir(root, id)?;
    let Some(store) = storage::read_only(root)? else {
        return Ok(false);
    };
    if store.read(|conn| storage::meta(conn, id))?.is_none() {
        return Ok(false);
    }
    storage::validate_dir(&dir)?;
    perms::create_private_dir_blocking(&dir)?;
    let lock = SessionLock::acquire(&dir, id)?;
    #[cfg(windows)]
    drop(lock);
    // Remove the directory before the row: a failure must not report a deleted session that still
    // has files. The reverse order destroys the row and then fails to remove the files.
    let removed_dir = match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => {
            Err(anyhow::Error::from(err).context(format!("failed to delete {}", dir.display())))
        }
    };
    #[cfg(not(windows))]
    let removed_dir = {
        drop(lock);
        removed_dir
    };
    let removed_dir = removed_dir?;
    let store = storage::open(root)?;
    let deleted = store.immediate(|tx| storage::delete(tx, id))?;
    Ok(deleted || removed_dir)
}

fn read_lock<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_lock<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compaction::has_open_compaction;
    use crate::event::{
        CompactionTrigger, ContentBlock, InputSource, StreamDigest, TurnTrigger, Usage,
    };
    use crate::ids::CompactionId;
    use meerai_core::ObservationStatus;
    use std::time::Duration;

    const GIT_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("meerai-session-{label}-{}", SessionId::new()))
    }

    fn config(root: &Path) -> SessionConfig {
        SessionConfig {
            root: root.to_path_buf(),
            workspace: root.to_path_buf(),
        }
    }

    fn persisted_meta(root: &Path, id: &SessionId) -> SessionMeta {
        let store = storage::read_only(root).unwrap().unwrap();
        store.read(|conn| storage::meta(conn, id)).unwrap().unwrap()
    }

    fn user(text: &str) -> SessionEvent {
        SessionEvent::UserMessage {
            source: InputSource::User,
            content: vec![ContentBlock::Text(text.to_string())],
        }
    }

    fn started(turn_id: &TurnId) -> SessionEvent {
        SessionEvent::TurnStarted {
            turn_id: turn_id.clone(),
            trigger: TurnTrigger::UserPrompt,
        }
    }

    fn ended(turn_id: &TurnId) -> SessionEvent {
        SessionEvent::TurnEnded {
            turn_id: turn_id.clone(),
            reason: TurnEndReason::Completed,
        }
    }

    #[tokio::test]
    async fn memory_session_appends_in_memory() {
        let session = Session::memory(SessionId::new(), PathBuf::from("/tmp/workspace"));

        let first = session.append(user("hello")).await.unwrap();
        let second = session.append(user("again")).await.unwrap();

        assert_eq!(first, Seq(1));
        assert_eq!(second, Seq(2));
        assert_eq!(session.events().len(), 2);
        assert!(session.dir().is_none());
        assert_eq!(session.meta().cwd, PathBuf::from("/tmp/workspace"));
        session.set_model("gpt-5-mini").await.unwrap();
        assert_eq!(session.meta().model.as_deref(), Some("gpt-5-mini"));
    }

    #[tokio::test]
    async fn open_or_create_persists_and_reloads() {
        let root = temp_root("reload");
        let config = config(&root);
        let id = SessionId::new();
        let cwd = PathBuf::from("/tmp/workspace");

        let session = Session::open_or_create(&config, id.clone(), cwd.clone())
            .await
            .unwrap();
        session.append(user("hello")).await.unwrap();
        session.append(user("again")).await.unwrap();
        session.flush().await.unwrap();

        let expected_dir = root.join(id.as_str());
        assert_eq!(session.dir(), Some(expected_dir.as_path()));
        assert!(expected_dir.join(LOCK_FILE).exists());
        assert!(!expected_dir.join("meta.json").exists());
        assert!(session_has_user_input(&expected_dir).await.unwrap());
        drop(session);

        let reopened = Session::open_or_create(&config, id.clone(), cwd.clone())
            .await
            .unwrap();
        assert_eq!(reopened.events().len(), 2);
        assert_eq!(reopened.events()[0].seq, Seq(1));
        assert_eq!(reopened.meta().cwd, cwd);

        let next = reopened.append(user("third")).await.unwrap();
        assert_eq!(next, Seq(3));

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn update_meta_persists_in_sqlite() {
        let root = temp_root("meta");
        let config = config(&root);
        let id = SessionId::new();
        let session = Session::open_or_create(&config, id.clone(), PathBuf::from("/tmp/workspace"))
            .await
            .unwrap();

        session
            .update_meta(|meta| {
                meta.title = Some("Renamed".to_string());
                meta.updated_at_ms = 0;
            })
            .await
            .unwrap();

        assert_eq!(session.meta().title.as_deref(), Some("Renamed"));
        assert!(session.meta().updated_at_ms > 0);
        assert!(session.meta().updated_at_ms >= session.meta().created_at_ms);

        let persisted = persisted_meta(&root, &id);
        assert_eq!(persisted.title.as_deref(), Some("Renamed"));

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[test]
    fn session_meta_deserializes_without_optional_fields() {
        let meta: SessionMeta = serde_json::from_str(
            r#"{"id":"ses-minimal","cwd":"/tmp/workspace","created_at_ms":1,"updated_at_ms":2}"#,
        )
        .unwrap();

        assert_eq!(meta.id.as_str(), "ses-minimal");
        assert_eq!(meta.title, None);
        assert_eq!(meta.harness, None);
        assert_eq!(meta.remote_session_id, None);
    }

    #[tokio::test]
    async fn remote_session_fields_round_trip() {
        let root = temp_root("remote-meta");
        let config = config(&root);
        let id = SessionId::new();
        let session = Session::open_or_create(&config, id.clone(), PathBuf::from("/tmp/workspace"))
            .await
            .unwrap();

        session
            .set_remote_session("acp:agy", "remote-session-1")
            .await
            .unwrap();

        let meta = session.meta();
        assert_eq!(meta.harness.as_deref(), Some("acp:agy"));
        assert_eq!(meta.remote_session_id.as_deref(), Some("remote-session-1"));

        let persisted = persisted_meta(&root, &id);
        assert_eq!(persisted.harness.as_deref(), Some("acp:agy"));
        assert_eq!(
            persisted.remote_session_id.as_deref(),
            Some("remote-session-1")
        );

        session.clear_remote_session().await.unwrap();
        assert_eq!(session.meta().harness, None);
        assert_eq!(session.meta().remote_session_id, None);

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn derive_messages_projects_logged_events() {
        let session = Session::memory(SessionId::new(), PathBuf::from("/tmp/workspace"));
        session.append(user("hello")).await.unwrap();

        let messages = session.derive_messages(&ProjectionPolicy::default());

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content.first_text(), Some("hello"));
    }

    #[tokio::test]
    async fn rejects_unsafe_session_ids() {
        let root = temp_root("unsafe");
        let config = config(&root);

        for raw in ["", ".", "..", "a/b", "a\\b", "C:", "C:\\ws"] {
            let id = SessionId::from_raw(raw);
            let result =
                Session::open_or_create(&config, id, PathBuf::from("/tmp/workspace")).await;
            assert!(result.is_err(), "expected error for session id {raw:?}");
        }

        let _ = fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn recovery_closes_open_turn_with_interrupted() {
        let root = temp_root("recovery");
        let config = config(&root);
        let id = SessionId::new();
        let turn = TurnId::new();

        let session = Session::open_or_create(&config, id.clone(), PathBuf::from("/tmp/workspace"))
            .await
            .unwrap();
        session.append(started(&turn)).await.unwrap();
        session.append(user("pending work")).await.unwrap();
        session.flush().await.unwrap();
        drop(session);

        let reopened =
            Session::open_or_create(&config, id.clone(), PathBuf::from("/tmp/workspace"))
                .await
                .unwrap();
        let events = reopened.events();
        assert_eq!(events.len(), 3);
        match &events[2].event {
            SessionEvent::TurnEnded { turn_id, reason } => {
                assert_eq!(turn_id, &turn);
                assert_eq!(reason, &TurnEndReason::Interrupted);
            }
            other => panic!("expected synthesized TurnEnded, got {other:?}"),
        }
        drop(reopened);

        let reopened_again = Session::open_or_create(&config, id, PathBuf::from("/tmp/workspace"))
            .await
            .unwrap();
        assert_eq!(
            reopened_again.events().len(),
            3,
            "recovery must not repeat on later opens"
        );

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn recovery_heals_dangling_tool_calls() {
        let root = temp_root("heal-tools");
        let config = config(&root);
        let id = SessionId::new();
        let turn = TurnId::new();

        let session = Session::open_or_create(&config, id.clone(), PathBuf::from("/tmp/workspace"))
            .await
            .unwrap();
        session.append(started(&turn)).await.unwrap();
        session.append(user("pending work")).await.unwrap();
        session
            .append(SessionEvent::AssistantMessage {
                provider: "gemini".to_string(),
                model: "gemini-2.5-pro".to_string(),
                content: Vec::new(),
                tool_calls: vec![meerai_core::ToolCall {
                    id: "call-1".to_string(),
                    name: "code-read_file".to_string(),
                    args: "{}".to_string(),
                }],
                call_signatures: vec![Some(vec!["sig-1".to_string()])],
                usage: Usage::default(),
                stream_digest: StreamDigest::default(),
            })
            .await
            .unwrap();
        session.flush().await.unwrap();
        drop(session);

        let reopened =
            Session::open_or_create(&config, id.clone(), PathBuf::from("/tmp/workspace"))
                .await
                .unwrap();
        let events = reopened.events();
        let (call_id, output) = events
            .iter()
            .find_map(|logged| match &logged.event {
                SessionEvent::ToolResult { call_id, output } => {
                    Some((call_id.clone(), output.clone()))
                }
                _ => None,
            })
            .expect("synthetic tool result");
        assert_eq!(call_id.as_str(), "call-1");
        assert_eq!(output.status, ObservationStatus::Error);
        assert!(output.stderr.contains("interrupted"));
        assert!(matches!(
            events.last().map(|logged| &logged.event),
            Some(SessionEvent::TurnEnded {
                reason: TurnEndReason::Interrupted,
                ..
            })
        ));
        assert!(matches!(
            events
                .get(events.len().saturating_sub(2))
                .map(|logged| &logged.event),
            Some(SessionEvent::ToolResult { .. })
        ));
        let event_len = events.len();
        drop(reopened);

        let reopened_again = Session::open_or_create(&config, id, PathBuf::from("/tmp/workspace"))
            .await
            .unwrap();
        assert_eq!(
            reopened_again.events().len(),
            event_len,
            "healing must not repeat on later opens"
        );

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn heal_dangling_tool_calls_is_public_and_idempotent() {
        let session = Session::memory(SessionId::new(), PathBuf::from("/tmp/workspace"));
        let turn = TurnId::new();
        session.append(started(&turn)).await.unwrap();
        session.append(user("pending work")).await.unwrap();
        session.append(assistant_with_call("call-1")).await.unwrap();

        session.heal_dangling_tool_calls().await.unwrap();
        session.heal_dangling_tool_calls().await.unwrap();

        let results = session
            .events()
            .iter()
            .filter(|logged| matches!(logged.event, SessionEvent::ToolResult { .. }))
            .count();
        assert_eq!(results, 1, "live repair must not duplicate results");
    }

    #[tokio::test]
    async fn load_events_never_heals_a_live_session() {
        let root = temp_root("read-only-load");
        let config = config(&root);
        let id = SessionId::new();
        let turn = TurnId::new();

        let session = Session::open_or_create(&config, id.clone(), PathBuf::from("/tmp/workspace"))
            .await
            .unwrap();
        session.append(started(&turn)).await.unwrap();
        session.append(user("pending work")).await.unwrap();
        session.append(assistant_with_call("call-1")).await.unwrap();
        session.flush().await.unwrap();
        drop(session);

        let loaded = Session::load_events(&config, &id).await.unwrap();
        assert_eq!(
            loaded.len(),
            3,
            "a read-only load must not append recovery events"
        );
        assert!(
            loaded
                .iter()
                .all(|logged| !matches!(logged.event, SessionEvent::ToolResult { .. })),
            "a read-only load must not synthesize tool results"
        );

        let reopened = Session::open_or_create(&config, id, PathBuf::from("/tmp/workspace"))
            .await
            .unwrap();
        assert_eq!(
            reopened.events().len(),
            5,
            "a recovery open still heals the dangling call"
        );

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn healing_skips_rewound_calls() {
        let root = temp_root("heal-rewound");
        let config = config(&root);
        let id = SessionId::new();
        let turn = TurnId::new();

        let session = Session::open_or_create(&config, id.clone(), PathBuf::from("/tmp/workspace"))
            .await
            .unwrap();
        let start = session.append(started(&turn)).await.unwrap();
        session.append(user("pending work")).await.unwrap();
        let end_inclusive = session.append(assistant_with_call("call-1")).await.unwrap();
        session
            .append(SessionEvent::RewindApplied {
                turn_id: turn,
                scope: RewindScope::Conversation,
                restored: Vec::new(),
                replaced: Some(SeqRange {
                    start,
                    end_inclusive,
                }),
            })
            .await
            .unwrap();
        session.flush().await.unwrap();
        drop(session);

        let reopened = Session::open_or_create(&config, id, PathBuf::from("/tmp/workspace"))
            .await
            .unwrap();
        assert!(
            reopened
                .events()
                .iter()
                .all(|logged| !matches!(logged.event, SessionEvent::ToolResult { .. })),
            "rewound calls must not be healed"
        );

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn fork_copies_healed_tool_results() {
        let root = temp_root("fork-healed");
        let config = config(&root);
        let id = SessionId::new();
        let turn = TurnId::new();

        let session = Session::open_or_create(&config, id.clone(), PathBuf::from("/tmp/workspace"))
            .await
            .unwrap();
        session.append(started(&turn)).await.unwrap();
        session.append(user("pending work")).await.unwrap();
        session.append(assistant_with_call("call-1")).await.unwrap();
        session.flush().await.unwrap();
        drop(session);

        let source = Session::open_or_create(&config, id, PathBuf::from("/tmp/workspace"))
            .await
            .unwrap();
        let fork = Session::fork_latest(&config, &source).await.unwrap();
        assert!(
            fork.events().iter().any(|logged| matches!(
                &logged.event,
                SessionEvent::ToolResult { call_id, .. } if call_id.as_str() == "call-1"
            )),
            "a fork must copy the healed result that sits before the interrupted turn end"
        );

        fs::remove_dir_all(&root).await.unwrap();
    }

    fn assistant_with_call(call_id: &str) -> SessionEvent {
        SessionEvent::AssistantMessage {
            provider: "gemini".to_string(),
            model: "gemini-2.5-pro".to_string(),
            content: Vec::new(),
            tool_calls: vec![meerai_core::ToolCall {
                id: call_id.to_string(),
                name: "code-read_file".to_string(),
                args: "{}".to_string(),
            }],
            call_signatures: vec![Some(vec!["sig-1".to_string()])],
            usage: Usage::default(),
            stream_digest: StreamDigest::default(),
        }
    }

    #[tokio::test]
    async fn fork_copies_events_through_boundary_turn() {
        let root = temp_root("fork");
        let config = config(&root);
        let source =
            Session::open_or_create(&config, SessionId::new(), PathBuf::from("/tmp/workspace"))
                .await
                .unwrap();
        let first = TurnId::new();
        let second = TurnId::new();

        source.append(started(&first)).await.unwrap();
        source.append(user("one")).await.unwrap();
        source.append(ended(&first)).await.unwrap();
        source.append(started(&second)).await.unwrap();
        source.append(user("two")).await.unwrap();
        source.append(ended(&second)).await.unwrap();

        let forked = Session::fork(&config, &source, &first).await.unwrap();

        assert_ne!(forked.id(), source.id());
        assert_eq!(forked.events().len(), 3);
        assert_eq!(forked.meta().forked_from.as_ref(), Some(source.id()));
        assert_eq!(forked.meta().inherited_event_count, Some(3));
        assert_eq!(forked.append(user("new")).await.unwrap(), Seq(4));

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn fork_rejects_open_turn_without_partial_write() {
        let root = temp_root("fork-open");
        let config = config(&root);
        let source =
            Session::open_or_create(&config, SessionId::new(), PathBuf::from("/tmp/workspace"))
                .await
                .unwrap();
        let closed = TurnId::new();
        let open = TurnId::new();

        source.append(started(&closed)).await.unwrap();
        source.append(user("done")).await.unwrap();
        source.append(ended(&closed)).await.unwrap();
        source.append(started(&open)).await.unwrap();
        source.append(user("still running")).await.unwrap();

        let before = list_sessions(&root).unwrap().len();
        let result = Session::fork(&config, &source, &open).await;
        assert!(result.is_err());
        assert_eq!(
            list_sessions(&root).unwrap().len(),
            before,
            "no partial session written"
        );

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn fork_does_not_copy_open_tail() {
        let root = temp_root("fork-tail");
        let config = config(&root);
        let source =
            Session::open_or_create(&config, SessionId::new(), PathBuf::from("/tmp/workspace"))
                .await
                .unwrap();
        let closed = TurnId::new();
        let open = TurnId::new();

        source.append(started(&closed)).await.unwrap();
        source.append(user("done")).await.unwrap();
        source.append(ended(&closed)).await.unwrap();
        source.append(started(&open)).await.unwrap();
        source.append(user("still running")).await.unwrap();

        let forked = Session::fork(&config, &source, &closed).await.unwrap();

        assert_eq!(forked.events().len(), 3);
        assert_eq!(forked.meta().inherited_event_count, Some(3));
        assert!(forked.events().iter().all(|logged| {
            !matches!(&logged.event, SessionEvent::TurnStarted { turn_id, .. } if turn_id == &open)
        }));

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn fork_rejects_unknown_turn() {
        let root = temp_root("fork-unknown");
        let config = config(&root);
        let source =
            Session::open_or_create(&config, SessionId::new(), PathBuf::from("/tmp/workspace"))
                .await
                .unwrap();
        let turn = TurnId::new();
        source.append(started(&turn)).await.unwrap();
        source.append(ended(&turn)).await.unwrap();

        let result = Session::fork(&config, &source, &TurnId::from("missing")).await;
        assert!(result.is_err());

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn approval_mode_persists_in_meta() {
        let root = temp_root("approval-mode");
        let config = config(&root);
        let workspace = PathBuf::from("/tmp/workspace");
        let session = Session::open_or_create(&config, SessionId::new(), workspace.clone())
            .await
            .unwrap();
        assert_eq!(session.meta().approval_mode, None);

        session.set_approval_mode("auto").await.unwrap();
        let id = session.id().clone();
        drop(session);

        let reopened = Session::open(&config, id, workspace).await.unwrap();
        assert_eq!(reopened.meta().approval_mode.as_deref(), Some("auto"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn rewind_shadows_projected_range() {
        let root = temp_root("rewind");
        let config = config(&root);
        let session =
            Session::open_or_create(&config, SessionId::new(), PathBuf::from("/tmp/workspace"))
                .await
                .unwrap();
        let first = TurnId::new();
        let second = TurnId::new();

        session.append(started(&first)).await.unwrap();
        session.append(user("one")).await.unwrap();
        session.append(ended(&first)).await.unwrap();
        session.append(started(&second)).await.unwrap();
        session.append(user("two")).await.unwrap();
        session.append(ended(&second)).await.unwrap();

        session
            .rewind(&second, RewindScope::Conversation, Vec::new())
            .await
            .unwrap();

        let events = session.events();
        assert_eq!(events.len(), 7);
        match &events[6].event {
            SessionEvent::RewindApplied {
                turn_id,
                scope,
                replaced: Some(replaced),
                ..
            } => {
                assert_eq!(turn_id, &second);
                assert_eq!(*scope, RewindScope::Conversation);
                assert_eq!(replaced.start, Seq(4));
                assert_eq!(replaced.end_inclusive, Seq(6));
            }
            other => panic!("expected RewindApplied, got {other:?}"),
        }

        let messages = session.derive_messages(&ProjectionPolicy::default());
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content.first_text(), Some("one"));
        let session_id = session.id().clone();
        drop(session);

        let reopened =
            Session::open_or_create(&config, session_id, PathBuf::from("/tmp/workspace"))
                .await
                .unwrap();
        assert_eq!(reopened.events().len(), 7, "rewind never deletes events");
        assert_eq!(
            reopened.derive_messages(&ProjectionPolicy::default()).len(),
            1
        );

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn rewind_closes_open_turn_before_shadowing() {
        let session = Session::memory(SessionId::new(), PathBuf::from("/tmp/workspace"));
        let turn = TurnId::new();
        session.append(started(&turn)).await.unwrap();
        session.append(user("abandoned")).await.unwrap();

        session
            .rewind(&turn, RewindScope::Conversation, Vec::new())
            .await
            .unwrap();

        let events = session.events();
        assert_eq!(events.len(), 4);
        assert!(matches!(
            &events[2].event,
            SessionEvent::TurnEnded {
                reason: TurnEndReason::Aborted {
                    cause: AbortCause::User
                },
                ..
            }
        ));
        match &events[3].event {
            SessionEvent::RewindApplied {
                scope,
                replaced: Some(replaced),
                ..
            } => {
                assert_eq!(*scope, RewindScope::Conversation);
                assert_eq!(replaced.start, Seq(1));
                assert_eq!(replaced.end_inclusive, Seq(3));
            }
            other => panic!("expected RewindApplied, got {other:?}"),
        }
        assert!(
            session
                .derive_messages(&ProjectionPolicy::default())
                .is_empty()
        );
    }

    #[tokio::test]
    async fn rewind_rejects_unknown_conversation_turn() {
        let session = Session::memory(SessionId::new(), PathBuf::from("/tmp/workspace"));
        session.append(user("hello")).await.unwrap();

        assert!(
            session
                .rewind(
                    &TurnId::from("missing"),
                    RewindScope::Conversation,
                    Vec::new()
                )
                .await
                .is_err()
        );
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;

        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn persisted_session_uses_private_permissions() {
        let root = temp_root("perms");
        let config = config(&root);
        let session =
            Session::open_or_create(&config, SessionId::new(), PathBuf::from("/tmp/workspace"))
                .await
                .unwrap();
        session.append(user("permission check")).await.unwrap();
        session.flush().await.unwrap();

        let dir = session.dir().unwrap();
        assert_eq!(mode_of(dir), 0o700);
        assert_eq!(mode_of(&dir.join(LOCK_FILE)), 0o600);
        assert_eq!(mode_of(&storage::db_path(&root)), 0o600);

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn detects_git_branch_and_sha_from_loose_ref() {
        let root = temp_root("git-root");
        let workspace = temp_root("git-ws");
        let git = workspace.join(".git");
        fs::create_dir_all(git.join("refs").join("heads"))
            .await
            .unwrap();
        fs::write(git.join("HEAD"), b"ref: refs/heads/main\n")
            .await
            .unwrap();
        fs::write(git.join("refs").join("heads").join("main"), GIT_SHA)
            .await
            .unwrap();

        let config = SessionConfig {
            root: root.clone(),
            workspace: workspace.clone(),
        };
        let session = Session::open_or_create(&config, SessionId::new(), workspace.clone())
            .await
            .unwrap();

        assert_eq!(session.meta().git_branch.as_deref(), Some("main"));
        assert_eq!(session.meta().git_sha.as_deref(), Some(GIT_SHA));

        let persisted = persisted_meta(&root, session.id());
        assert_eq!(persisted.git_branch.as_deref(), Some("main"));
        assert_eq!(persisted.git_sha.as_deref(), Some(GIT_SHA));

        fs::remove_dir_all(&root).await.unwrap();
        fs::remove_dir_all(&workspace).await.unwrap();
    }

    #[tokio::test]
    async fn resolves_git_sha_from_packed_refs() {
        let root = temp_root("packed-root");
        let workspace = temp_root("packed-ws");
        let git = workspace.join(".git");
        fs::create_dir_all(&git).await.unwrap();
        fs::write(git.join("HEAD"), b"ref: refs/heads/feature\n")
            .await
            .unwrap();
        fs::write(
            git.join("packed-refs"),
            format!(
                "# pack-refs with: peeled\n{GIT_SHA} refs/heads/feature\n\
                 fedcba9876543210fedcba9876543210fedcba98 refs/heads/other\n"
            ),
        )
        .await
        .unwrap();

        let config = SessionConfig {
            root: root.clone(),
            workspace: workspace.clone(),
        };
        let session = Session::open_or_create(&config, SessionId::new(), workspace.clone())
            .await
            .unwrap();

        assert_eq!(session.meta().git_branch.as_deref(), Some("feature"));
        assert_eq!(session.meta().git_sha.as_deref(), Some(GIT_SHA));

        fs::remove_dir_all(&root).await.unwrap();
        fs::remove_dir_all(&workspace).await.unwrap();
    }

    #[tokio::test]
    async fn detached_head_is_its_own_sha() {
        let root = temp_root("detached-root");
        let workspace = temp_root("detached-ws");
        let git = workspace.join(".git");
        fs::create_dir_all(&git).await.unwrap();
        fs::write(git.join("HEAD"), format!("{GIT_SHA}\n"))
            .await
            .unwrap();

        let config = SessionConfig {
            root: root.clone(),
            workspace: workspace.clone(),
        };
        let session = Session::open_or_create(&config, SessionId::new(), workspace.clone())
            .await
            .unwrap();

        assert!(session.meta().git_branch.is_none());
        assert_eq!(session.meta().git_sha.as_deref(), Some(GIT_SHA));

        fs::remove_dir_all(&root).await.unwrap();
        fs::remove_dir_all(&workspace).await.unwrap();
    }

    #[tokio::test]
    async fn missing_git_leaves_meta_git_fields_empty() {
        let root = temp_root("no-git-root");
        let workspace = temp_root("no-git-ws");
        fs::create_dir_all(&workspace).await.unwrap();

        let config = SessionConfig {
            root: root.clone(),
            workspace: workspace.clone(),
        };
        let session = Session::open_or_create(&config, SessionId::new(), workspace.clone())
            .await
            .unwrap();

        assert!(session.meta().git_branch.is_none());
        assert!(session.meta().git_sha.is_none());

        fs::remove_dir_all(&root).await.unwrap();
        fs::remove_dir_all(&workspace).await.unwrap();
    }

    #[tokio::test]
    async fn set_title_and_set_model_persist_and_bump_updated_at() {
        let root = temp_root("meta-fields");
        let config = config(&root);
        let session =
            Session::open_or_create(&config, SessionId::new(), PathBuf::from("/tmp/workspace"))
                .await
                .unwrap();
        let created_at = session.meta().created_at_ms;

        tokio::time::sleep(Duration::from_millis(2)).await;
        session.set_title("Fix the parser").await.unwrap();
        let titled = session.meta();
        assert_eq!(titled.title.as_deref(), Some("Fix the parser"));
        assert!(titled.updated_at_ms > created_at);

        tokio::time::sleep(Duration::from_millis(2)).await;
        session.set_model("gpt-4o-mini").await.unwrap();
        let modeled = session.meta();
        assert_eq!(modeled.model.as_deref(), Some("gpt-4o-mini"));
        assert!(modeled.updated_at_ms > titled.updated_at_ms);

        let persisted = persisted_meta(&root, session.id());
        assert_eq!(persisted.title.as_deref(), Some("Fix the parser"));
        assert_eq!(persisted.model.as_deref(), Some("gpt-4o-mini"));
        assert_eq!(persisted.updated_at_ms, modeled.updated_at_ms);

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn lists_sessions_newest_first() {
        let root = temp_root("list");
        let config = config(&root);
        let first =
            Session::open_or_create(&config, SessionId::new(), PathBuf::from("/tmp/workspace"))
                .await
                .unwrap();
        first.set_title("first").await.unwrap();

        tokio::time::sleep(Duration::from_millis(2)).await;
        let second =
            Session::open_or_create(&config, SessionId::new(), PathBuf::from("/tmp/workspace"))
                .await
                .unwrap();
        second.set_model("gpt-4o-mini").await.unwrap();
        second.append(user("listed")).await.unwrap();

        let listed = list_sessions(&root).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(&listed[0].id, second.id());
        assert_eq!(listed[0].model.as_deref(), Some("gpt-4o-mini"));
        assert!(listed[0].dir.ends_with(second.id().as_str()));
        assert!(listed[0].bytes > 0);
        assert_eq!(&listed[1].id, first.id());
        assert_eq!(listed[1].title.as_deref(), Some("first"));

        let latest = latest_session(&root).unwrap().unwrap();
        assert_eq!(&latest.id, second.id());

        fs::remove_dir_all(&root).await.unwrap();
        assert!(latest_session(&root).unwrap().is_none());
    }

    #[tokio::test]
    async fn deletes_sessions_and_reports_absence() {
        let root = temp_root("delete");
        let config = config(&root);
        let session =
            Session::open_or_create(&config, SessionId::new(), PathBuf::from("/tmp/workspace"))
                .await
                .unwrap();
        let id = session.id().clone();
        drop(session);

        assert!(delete_session(&root, &id).unwrap());
        assert!(!root.join(id.as_str()).exists());
        assert!(!delete_session(&root, &id).unwrap());
        assert!(delete_session(&root, &SessionId::from("../escape")).is_err());
        assert!(delete_session(&root, &SessionId::from("C:")).is_err());
        assert!(!root.join("C:").exists());

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn open_requires_an_existing_session_directory() {
        let root = temp_root("open");
        let config = config(&root);
        let id = SessionId::new();

        assert!(
            Session::open(&config, id.clone(), PathBuf::from("/tmp/workspace"))
                .await
                .is_err()
        );
        assert!(!root.join(id.as_str()).exists());

        let created = Session::open_or_create(&config, id.clone(), PathBuf::from("/tmp/workspace"))
            .await
            .unwrap();
        created.append(user("hello")).await.unwrap();
        drop(created);

        let opened = Session::open(&config, id.clone(), PathBuf::from("/tmp/workspace"))
            .await
            .unwrap();
        assert_eq!(opened.events().len(), 1);
        assert_eq!(opened.events()[0].seq, Seq(1));

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn recovery_closes_open_compaction_bracket() {
        let root = temp_root("compaction-recovery");
        let config = config(&root);
        let id = SessionId::new();
        let compaction_id = CompactionId::from("cmp-open");

        let session = Session::open_or_create(&config, id.clone(), PathBuf::from("/tmp/workspace"))
            .await
            .unwrap();
        session
            .append(SessionEvent::CompactionStarted {
                compaction_id: compaction_id.clone(),
                trigger: CompactionTrigger::Manual,
            })
            .await
            .unwrap();
        drop(session);

        let reopened =
            Session::open_or_create(&config, id.clone(), PathBuf::from("/tmp/workspace"))
                .await
                .unwrap();
        let events = reopened.events();
        assert_eq!(events.len(), 2);
        match &events[1].event {
            SessionEvent::CompactionEnded {
                compaction_id: ended_id,
                result,
            } => {
                assert_eq!(ended_id, &compaction_id);
                assert_eq!(
                    result,
                    &CompactionResult::Failed {
                        error: COMPACTION_INTERRUPTED.to_string()
                    }
                );
            }
            other => panic!("expected synthesized CompactionEnded, got {other:?}"),
        }
        assert!(!has_open_compaction(&events));
        drop(reopened);

        let reopened_again = Session::open(&config, id, PathBuf::from("/tmp/workspace"))
            .await
            .unwrap();
        assert_eq!(
            reopened_again.events().len(),
            2,
            "compaction recovery must not repeat on later opens"
        );

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn turn_end_refreshes_updated_at_for_listing() {
        let root = temp_root("touch");
        let config = config(&root);
        let older =
            Session::open_or_create(&config, SessionId::new(), PathBuf::from("/tmp/workspace"))
                .await
                .unwrap();

        tokio::time::sleep(Duration::from_millis(2)).await;
        let newer =
            Session::open_or_create(&config, SessionId::new(), PathBuf::from("/tmp/workspace"))
                .await
                .unwrap();
        assert_eq!(&latest_session(&root).unwrap().unwrap().id, newer.id());

        tokio::time::sleep(Duration::from_millis(2)).await;
        let turn = TurnId::new();
        older.append(started(&turn)).await.unwrap();
        older.append(ended(&turn)).await.unwrap();

        assert_eq!(&latest_session(&root).unwrap().unwrap().id, older.id());
        assert!(older.meta().updated_at_ms > newer.meta().updated_at_ms);

        let persisted = persisted_meta(&root, older.id());
        assert_eq!(persisted.updated_at_ms, older.meta().updated_at_ms);

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn create_child_links_parent_and_latest_session_skips_children() {
        let root = temp_root("child-session");
        let config = SessionConfig {
            root: root.clone(),
            workspace: root.clone(),
        };
        let parent_id = SessionId::from_raw("parent-1");
        let parent = Session::open_or_create(&config, parent_id.clone(), root.clone())
            .await
            .unwrap();
        let child = parent
            .create_child(Some("small-model".to_string()), Some("explore".to_string()))
            .await
            .unwrap();

        assert_eq!(child.meta().parent_session_id, Some(parent_id.clone()));
        assert_eq!(child.meta().model.as_deref(), Some("small-model"));
        assert_eq!(child.meta().title.as_deref(), Some("explore"));

        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        child
            .append(SessionEvent::TurnEnded {
                turn_id: TurnId::from_raw("t-child"),
                reason: TurnEndReason::Completed,
            })
            .await
            .unwrap();

        let latest = latest_session(&root).unwrap().unwrap();
        assert_eq!(latest.id, parent_id);

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn open_fails_while_session_lock_is_held_and_succeeds_after_drop() {
        let root = temp_root("lock-exclusive");
        let config = config(&root);
        let id = SessionId::new();
        let owner = Session::open_or_create(&config, id.clone(), root.clone())
            .await
            .unwrap();

        let err = match Session::open(&config, id.clone(), root.clone()).await {
            Ok(_) => panic!("expected lock contention error"),
            Err(err) => err,
        };
        let message = err.to_string();
        assert!(message.contains("already open"), "{message}");
        assert!(
            message.contains(&format!("pid={}", std::process::id())),
            "{message}"
        );

        drop(owner);
        let reopened = Session::open(&config, id, root.clone()).await.unwrap();
        drop(reopened);
        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn fork_stored_succeeds_while_source_session_is_locked() {
        let root = temp_root("fork-stored-locked");
        let config = config(&root);
        let source_id = SessionId::new();
        let turn_id = TurnId::from_raw("turn-1");

        let source = Session::open_or_create(&config, source_id.clone(), root.clone())
            .await
            .unwrap();
        source.append(started(&turn_id)).await.unwrap();
        source.append(user("hello")).await.unwrap();
        source.append(ended(&turn_id)).await.unwrap();
        source.flush().await.unwrap();

        let forked = Session::fork_stored(&config, &source_id, None)
            .await
            .unwrap();
        assert_ne!(forked.id(), &source_id);
        assert_eq!(forked.meta().forked_from.as_ref(), Some(&source_id));
        assert_eq!(forked.events().len(), 3);

        drop(forked);
        drop(source);
        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn delete_session_fails_while_session_lock_is_held() {
        let root = temp_root("delete-locked");
        let config = config(&root);
        let id = SessionId::new();
        let owner = Session::open_or_create(&config, id.clone(), root.clone())
            .await
            .unwrap();

        let err = delete_session(&root, &id).unwrap_err().to_string();
        assert!(err.contains("already open"), "{err}");
        assert!(root.join(id.as_str()).exists());

        drop(owner);
        assert!(delete_session(&root, &id).unwrap());
        assert!(!root.join(id.as_str()).exists());

        let _ = fs::remove_dir_all(&root).await;
    }
}
