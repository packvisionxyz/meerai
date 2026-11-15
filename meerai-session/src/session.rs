use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use anyhow::{Context, Result};
use genai::chat::ChatMessage;
use meerai_core::env::{home_dir, now_millis};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::blob::{BLOB_DIR_NAME, BlobStore};
use crate::compaction::{COMPACTION_INTERRUPTED, open_compactions, open_turns, turn_start};
use crate::event::{
    AbortCause, CompactionResult, LoggedEvent, Seq, SeqRange, SessionEvent, TurnEndReason,
};
use crate::ids::{SessionId, TurnId};
use crate::projection::{ProjectionPolicy, derive_messages};
use crate::{perms, storage};

const META_FILE: &str = "meta.json";

pub const DEFAULT_ROTATION_THRESHOLD_BYTES: u64 = 8 * 1024 * 1024;

pub fn default_sessions_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("MEERAI_SESSION_DIR") {
        return PathBuf::from(dir);
    }
    match dirs::data_dir() {
        Some(dir) => dir.join("meerai").join("sessions"),
        None => home_fallback(),
    }
}

fn home_fallback() -> PathBuf {
    match home_dir() {
        Some(home) => home
            .join(".local")
            .join("share")
            .join("meerai")
            .join("sessions"),
        None => PathBuf::from(".local")
            .join("share")
            .join("meerai")
            .join("sessions"),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionConfig {
    pub root: PathBuf,
    pub workspace: PathBuf,
    pub project_local: bool,
}

impl SessionConfig {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            root: default_sessions_root(),
            workspace: workspace.into(),
            project_local: false,
        }
    }

    fn effective_root(&self) -> PathBuf {
        if self.project_local {
            self.workspace.join(".meerai").join("sessions")
        } else {
            self.root.clone()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: SessionId,
    pub title: Option<String>,
    pub cwd: PathBuf,
    pub git_branch: Option<String>,
    pub git_sha: Option<String>,
    pub model: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub forked_from: Option<SessionId>,
    pub inherited_event_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<SessionId>,
}

impl SessionMeta {
    fn new(id: SessionId, cwd: PathBuf) -> Self {
        let now = now_millis();
        Self {
            id,
            title: None,
            cwd,
            git_branch: None,
            git_sha: None,
            model: None,
            created_at_ms: now,
            updated_at_ms: now,
            forked_from: None,
            inherited_event_count: None,
            parent_session_id: None,
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
    events: RwLock<Vec<LoggedEvent>>,
    meta: RwLock<SessionMeta>,
    next_seq: AtomicU64,
    generation: AtomicU64,
    active_size: AtomicU64,
    rotation_threshold: AtomicU64,
    write_guard: tokio::sync::Mutex<()>,
}

impl Session {
    pub fn memory(id: SessionId, cwd: PathBuf) -> Self {
        Self {
            inner: Arc::new(SessionInner {
                meta: RwLock::new(SessionMeta::new(id.clone(), cwd)),
                id,
                dir: None,
                config: None,
                events: RwLock::new(Vec::new()),
                next_seq: AtomicU64::new(1),
                generation: AtomicU64::new(0),
                active_size: AtomicU64::new(0),
                rotation_threshold: AtomicU64::new(DEFAULT_ROTATION_THRESHOLD_BYTES),
                write_guard: tokio::sync::Mutex::new(()),
            }),
        }
    }

    pub async fn open(config: &SessionConfig, id: SessionId, cwd: PathBuf) -> Result<Self> {
        Self::open_inner(config, id, cwd, false).await
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
        let dir = session_dir(&config.effective_root(), &id)?;
        if create {
            perms::create_private_dir(&dir).await?;
            if config.project_local {
                let root = config.effective_root();
                let ignore = root.join(".gitignore");
                if fs::metadata(&ignore).await.is_err() {
                    let _ = fs::write(ignore, "*\n!.gitignore\n").await;
                }
            }
        } else if path_is_dir(&dir).await? {
            perms::tighten_dir(&dir).await?;
        } else {
            anyhow::bail!("session not found: {id}");
        }
        tighten_existing_files(&dir).await;

        let meta_path = dir.join(META_FILE);
        let (mut meta, existed) = match fs::read_to_string(&meta_path).await {
            Ok(raw) => {
                let meta = serde_json::from_str(&raw)
                    .with_context(|| format!("failed to parse {}", meta_path.display()))?;
                (meta, true)
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                (SessionMeta::new(id.clone(), cwd), false)
            }
            Err(err) => {
                return Err(anyhow::Error::from(err)
                    .context(format!("failed to read {}", meta_path.display())));
            }
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
        if meta_changed {
            write_meta(&dir, &meta).await?;
        }

        let log = storage::load(&dir).await?;
        let next_seq = log
            .events
            .iter()
            .map(|logged| logged.seq.0)
            .max()
            .unwrap_or(0)
            + 1;

        let session = Self {
            inner: Arc::new(SessionInner {
                id,
                dir: Some(dir),
                config: Some(config.clone()),
                events: RwLock::new(log.events),
                meta: RwLock::new(meta),
                next_seq: AtomicU64::new(next_seq),
                generation: AtomicU64::new(log.active_generation),
                active_size: AtomicU64::new(log.active_size),
                rotation_threshold: AtomicU64::new(DEFAULT_ROTATION_THRESHOLD_BYTES),
                write_guard: tokio::sync::Mutex::new(()),
            }),
        };
        session.close_open_turns().await?;
        session.close_open_compactions().await?;
        Ok(session)
    }

    pub fn rotation_threshold(&self) -> u64 {
        self.inner.rotation_threshold.load(Ordering::Relaxed)
    }

    pub fn set_rotation_threshold(&self, bytes: u64) {
        self.inner
            .rotation_threshold
            .store(bytes, Ordering::Relaxed);
    }

    pub fn blob_store(&self) -> Option<BlobStore> {
        self.inner.dir.as_ref().map(BlobStore::for_session)
    }

    pub async fn append(&self, event: SessionEvent) -> Result<Seq> {
        let _guard = self.inner.write_guard.lock().await;
        let seq = Seq(self.inner.next_seq.fetch_add(1, Ordering::Relaxed));
        let logged = LoggedEvent { seq, event };

        if let Some(dir) = &self.inner.dir {
            let dir = dir.clone();
            self.persist(&dir, &logged).await?;
        }

        let turn_ended = matches!(&logged.event, SessionEvent::TurnEnded { .. });
        write_lock(&self.inner.events).push(logged);

        if turn_ended && let Some(dir) = &self.inner.dir {
            let _ = self.touch_meta(dir).await;
        }
        Ok(seq)
    }

    pub fn events(&self) -> Vec<LoggedEvent> {
        read_lock(&self.inner.events).clone()
    }

    pub fn id(&self) -> &SessionId {
        &self.inner.id
    }

    pub fn dir(&self) -> Option<&Path> {
        self.inner.dir.as_deref()
    }

    pub fn config(&self) -> Option<&SessionConfig> {
        self.inner.config.as_ref()
    }

    pub fn meta(&self) -> SessionMeta {
        read_lock(&self.inner.meta).clone()
    }

    pub async fn update_meta(&self, f: impl FnOnce(&mut SessionMeta)) -> Result<()> {
        let meta = {
            let mut meta = write_lock(&self.inner.meta);
            f(&mut meta);
            meta.updated_at_ms = now_millis();
            meta.clone()
        };

        if let Some(dir) = &self.inner.dir {
            write_meta(dir, &meta).await?;
        }
        Ok(())
    }

    async fn touch_meta(&self, dir: &Path) -> Result<()> {
        let meta = {
            let mut meta = write_lock(&self.inner.meta);
            meta.updated_at_ms = now_millis();
            meta.clone()
        };
        write_meta(dir, &meta).await
    }

    pub async fn set_title(&self, title: impl Into<String>) -> Result<()> {
        let title = title.into();
        self.update_meta(|meta| meta.title = Some(title)).await
    }

    pub async fn set_model(&self, model: impl Into<String>) -> Result<()> {
        let model = model.into();
        self.update_meta(|meta| meta.model = Some(model)).await
    }

    pub async fn flush(&self) -> Result<()> {
        let Some(dir) = &self.inner.dir else {
            return Ok(());
        };
        let generation = self.inner.generation.load(Ordering::Relaxed);
        let path = dir.join(storage::active_name(generation));

        match fs::OpenOptions::new().append(true).open(&path).await {
            Ok(file) => file
                .sync_all()
                .await
                .with_context(|| format!("failed to sync {}", path.display())),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
            Err(err) => {
                Err(anyhow::Error::from(err).context(format!("failed to open {}", path.display())))
            }
        }
    }

    pub fn derive_messages(&self, policy: &ProjectionPolicy) -> Vec<ChatMessage> {
        let events = self.events();
        derive_messages(&events, policy)
    }

    pub async fn fork(
        config: &SessionConfig,
        source: &Session,
        boundary_turn: &TurnId,
    ) -> Result<Session> {
        let prefix = fork_prefix(&source.events(), boundary_turn)?;
        let forked_from = source.id().clone();
        let cwd = source.meta().cwd;
        let inherited_event_count = prefix.len() as u64;

        let session = Self::open_or_create(config, SessionId::new(), cwd).await?;
        session.append_preserved(&prefix).await?;
        session
            .update_meta(|meta| {
                meta.forked_from = Some(forked_from);
                meta.inherited_event_count = Some(inherited_event_count);
            })
            .await?;

        Ok(session)
    }

    pub fn latest_closed_turn(&self) -> Option<TurnId> {
        self.events()
            .into_iter()
            .rev()
            .find_map(|logged| match logged.event {
                SessionEvent::TurnEnded { turn_id, .. } => Some(turn_id),
                _ => None,
            })
    }

    pub async fn fork_latest(config: &SessionConfig, source: &Session) -> Result<Session> {
        let boundary = source.latest_closed_turn().ok_or_else(|| {
            anyhow::anyhow!("session has no closed turns to fork: {}", source.id())
        })?;
        Self::fork(config, source, &boundary).await
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
            .await?;
        Ok(child)
    }

    pub async fn revert(&self, turn_id: &TurnId) -> Result<()> {
        let events = self.events();
        let start = turn_start(&events, turn_id)
            .ok_or_else(|| anyhow::anyhow!("unknown turn: {turn_id}"))?;

        for open in open_turns(&events) {
            self.append(SessionEvent::TurnEnded {
                turn_id: open,
                reason: TurnEndReason::Aborted {
                    cause: AbortCause::User,
                },
            })
            .await?;
        }

        let end_inclusive = self
            .events()
            .last()
            .map(|logged| logged.seq)
            .unwrap_or(start);

        self.append(SessionEvent::RevertApplied {
            turn_id: turn_id.clone(),
            replaced: SeqRange {
                start,
                end_inclusive,
            },
        })
        .await
        .map(|_| ())
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

    async fn persist(&self, dir: &Path, logged: &LoggedEvent) -> Result<()> {
        let mut line = serde_json::to_string(logged)
            .with_context(|| format!("failed to serialize event {}", logged.seq.0))?;
        line.push('\n');
        let bytes = line.as_bytes();

        let threshold = self.inner.rotation_threshold.load(Ordering::Relaxed);
        let size = self.inner.active_size.load(Ordering::Relaxed);
        if size > 0 && size.saturating_add(bytes.len() as u64) > threshold {
            let generation = self.inner.generation.load(Ordering::Relaxed);
            storage::seal(dir, generation).await?;
            self.inner
                .generation
                .store(generation + 1, Ordering::Relaxed);
            self.inner.active_size.store(0, Ordering::Relaxed);
        }

        let generation = self.inner.generation.load(Ordering::Relaxed);
        let path = dir.join(storage::active_name(generation));
        let mut file = perms::append_options()
            .open(&path)
            .await
            .with_context(|| format!("failed to open {}", path.display()))?;
        file.write_all(bytes)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
        file.flush()
            .await
            .with_context(|| format!("failed to flush {}", path.display()))?;
        drop(file);
        perms::tighten_file(&path).await?;
        self.inner
            .active_size
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);

        Ok(())
    }

    async fn append_preserved(&self, events: &[LoggedEvent]) -> Result<()> {
        let Some(dir) = &self.inner.dir else {
            write_lock(&self.inner.events).extend_from_slice(events);
            self.advance_next_seq(events);
            return Ok(());
        };

        let _guard = self.inner.write_guard.lock().await;
        let generation = self.inner.generation.load(Ordering::Relaxed);
        let path = dir.join(storage::active_name(generation));
        let mut file = perms::append_options()
            .open(&path)
            .await
            .with_context(|| format!("failed to open {}", path.display()))?;

        let mut written = 0u64;
        for logged in events {
            let mut line = serde_json::to_string(logged)
                .with_context(|| format!("failed to serialize event {}", logged.seq.0))?;
            line.push('\n');
            file.write_all(line.as_bytes())
                .await
                .with_context(|| format!("failed to write {}", path.display()))?;
            written += line.len() as u64;
        }
        file.flush()
            .await
            .with_context(|| format!("failed to flush {}", path.display()))?;
        file.sync_all()
            .await
            .with_context(|| format!("failed to sync {}", path.display()))?;
        drop(file);
        perms::tighten_file(&path).await?;
        self.inner.active_size.fetch_add(written, Ordering::Relaxed);

        write_lock(&self.inner.events).extend_from_slice(events);
        self.advance_next_seq(events);
        Ok(())
    }

    fn advance_next_seq(&self, events: &[LoggedEvent]) {
        if let Some(max) = events.iter().map(|logged| logged.seq.0).max() {
            self.inner.next_seq.fetch_max(max + 1, Ordering::Relaxed);
        }
    }
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
    let name = id.as_str();
    if !crate::ids::is_safe_segment(name) {
        anyhow::bail!("invalid session id: {name}");
    }
    Ok(root.join(name))
}

async fn path_is_dir(path: &Path) -> Result<bool> {
    match fs::metadata(path).await {
        Ok(metadata) => Ok(metadata.is_dir()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
        Err(err) => {
            Err(anyhow::Error::from(err).context(format!("failed to inspect {}", path.display())))
        }
    }
}

async fn tighten_existing_files(dir: &Path) {
    let Ok(mut entries) = fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let path = entry.path();
        let Ok(file_type) = entry.file_type().await else {
            continue;
        };
        if file_type.is_dir() {
            if name == BLOB_DIR_NAME {
                let _ = perms::tighten_dir(&path).await;
                tighten_blob_files(&path).await;
            }
            continue;
        }
        if name == META_FILE || storage::is_log_file_name(name) {
            let _ = perms::tighten_file(&path).await;
        }
    }
}

async fn tighten_blob_files(dir: &Path) {
    let Ok(mut entries) = fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(file_type) = entry.file_type().await else {
            continue;
        };
        if file_type.is_file() {
            let _ = perms::tighten_file(&entry.path()).await;
        }
    }
}

pub fn list_sessions(root: &Path) -> Result<Vec<SessionSummary>> {
    let mut summaries = Vec::new();
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(summaries),
        Err(err) => {
            return Err(
                anyhow::Error::from(err).context(format!("failed to list {}", root.display()))
            );
        }
    };

    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(meta) = read_meta(&dir) else {
            continue;
        };
        let bytes = dir_size(&dir);
        summaries.push(SessionSummary {
            id: meta.id,
            dir,
            title: meta.title,
            model: meta.model,
            created_at_ms: meta.created_at_ms,
            updated_at_ms: meta.updated_at_ms,
            bytes,
            parent_session_id: meta.parent_session_id,
        });
    }

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

pub fn delete_session(root: &Path, id: &SessionId) -> Result<bool> {
    let dir = session_dir(root, id)?;
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
        Err(err) => {
            Err(anyhow::Error::from(err).context(format!("failed to delete {}", dir.display())))
        }
    }
}

fn read_meta(dir: &Path) -> Option<SessionMeta> {
    let raw = std::fs::read_to_string(dir.join(META_FILE)).ok()?;
    serde_json::from_str(&raw).ok()
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(entries) = std::fs::read_dir(path) else {
        return total;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            total += dir_size(&entry.path());
        } else {
            total += metadata.len();
        }
    }
    total
}

async fn write_meta(dir: &Path, meta: &SessionMeta) -> Result<()> {
    let path = dir.join(META_FILE);
    let raw = serde_json::to_string_pretty(meta)
        .with_context(|| format!("failed to serialize {}", path.display()))?;
    perms::write_private(&path, format!("{raw}\n").as_bytes())
        .await
        .with_context(|| format!("failed to write {}", path.display()))
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
    use crate::event::{CompactionTrigger, ContentBlock, InputSource, TurnTrigger};
    use crate::ids::CompactionId;
    use std::time::Duration;

    const GIT_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("meerai-session-{label}-{}", SessionId::new()))
    }

    fn config(root: &Path) -> SessionConfig {
        SessionConfig {
            root: root.to_path_buf(),
            workspace: root.to_path_buf(),
            project_local: false,
        }
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

    async fn event_count(dir: &Path) -> usize {
        let mut entries = fs::read_dir(dir).await.unwrap();
        let mut count = 0;
        while let Some(entry) = entries.next_entry().await.unwrap() {
            if entry.metadata().await.unwrap().is_dir() {
                count += 1;
            }
        }
        count
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
        assert!(expected_dir.join(META_FILE).exists());

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
    async fn project_local_sessions_are_gitignored() {
        let workspace = temp_root("project-local-ignore");
        let mut config = SessionConfig::new(&workspace);
        config.project_local = true;

        Session::open_or_create(&config, SessionId::new(), workspace.clone())
            .await
            .unwrap();

        let ignore = workspace
            .join(".meerai")
            .join("sessions")
            .join(".gitignore");
        let raw = fs::read_to_string(ignore).await.unwrap();
        assert_eq!(raw, "*\n!.gitignore\n");
    }

    #[tokio::test]
    async fn project_local_config_uses_workspace_directory() {
        let workspace = temp_root("project");
        let root = temp_root("project-root");
        let config = SessionConfig {
            root,
            workspace: workspace.clone(),
            project_local: true,
        };
        let id = SessionId::new();

        let session = Session::open_or_create(&config, id.clone(), workspace.clone())
            .await
            .unwrap();

        let expected_dir = workspace.join(".meerai").join("sessions").join(id.as_str());
        assert_eq!(session.dir(), Some(expected_dir.as_path()));
        assert!(expected_dir.join(META_FILE).exists());

        fs::remove_dir_all(&workspace).await.unwrap();
    }

    #[tokio::test]
    async fn update_meta_rewrites_meta_file() {
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

        let raw = fs::read_to_string(root.join(id.as_str()).join(META_FILE))
            .await
            .unwrap();
        let persisted: SessionMeta = serde_json::from_str(&raw).unwrap();
        assert_eq!(persisted.title.as_deref(), Some("Renamed"));

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn skips_malformed_log_lines() {
        let root = temp_root("malformed");
        let config = config(&root);
        let id = SessionId::new();
        let session = Session::open_or_create(&config, id.clone(), PathBuf::from("/tmp/workspace"))
            .await
            .unwrap();
        session.append(user("kept")).await.unwrap();

        let log_path = root.join(id.as_str()).join(storage::active_name(
            session.inner.generation.load(Ordering::Relaxed),
        ));
        let mut raw = fs::read_to_string(&log_path).await.unwrap();
        raw.push_str("{ this is not an event }\n");
        fs::write(&log_path, raw).await.unwrap();

        let reopened =
            Session::open_or_create(&config, id.clone(), PathBuf::from("/tmp/workspace"))
                .await
                .unwrap();

        assert_eq!(reopened.events().len(), 1);
        assert_eq!(reopened.append(user("new")).await.unwrap(), Seq(2));

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
    async fn rotates_generations_and_reads_all() {
        let root = temp_root("rotation");
        let config = config(&root);
        let id = SessionId::new();
        let session = Session::open_or_create(&config, id.clone(), PathBuf::from("/tmp/workspace"))
            .await
            .unwrap();
        session.set_rotation_threshold(200);

        for index in 0..40 {
            session
                .append(user(&format!("message {index}")))
                .await
                .unwrap();
        }
        session.flush().await.unwrap();

        let dir = session.dir().unwrap().to_path_buf();
        let mut sealed = Vec::new();
        let mut active = Vec::new();
        let mut entries = fs::read_dir(&dir).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".jsonl.zstd") {
                sealed.push(entry.path());
            } else if name.ends_with(".jsonl") {
                active.push(entry.path());
            }
        }
        sealed.sort();
        assert!(
            !sealed.is_empty(),
            "expected at least one sealed generation"
        );
        assert_eq!(active.len(), 1, "exactly one active generation");

        let first_sealed = fs::read(&sealed[0]).await.unwrap();

        let reopened =
            Session::open_or_create(&config, id.clone(), PathBuf::from("/tmp/workspace"))
                .await
                .unwrap();
        let events = reopened.events();
        assert_eq!(events.len(), 40);
        for (index, logged) in events.iter().enumerate() {
            assert_eq!(logged.seq, Seq(index as u64 + 1));
        }
        assert_eq!(reopened.append(user("after")).await.unwrap(), Seq(41));

        let first_sealed_after = fs::read(&sealed[0]).await.unwrap();
        assert_eq!(
            first_sealed, first_sealed_after,
            "committed generations are never rewritten"
        );

        fs::remove_dir_all(&root).await.unwrap();
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

        let before = event_count(&root).await;
        let result = Session::fork(&config, &source, &open).await;
        assert!(result.is_err());
        assert_eq!(
            event_count(&root).await,
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
    async fn revert_shadows_projected_range() {
        let root = temp_root("revert");
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

        session.revert(&second).await.unwrap();

        let events = session.events();
        assert_eq!(events.len(), 7);
        match &events[6].event {
            SessionEvent::RevertApplied { turn_id, replaced } => {
                assert_eq!(turn_id, &second);
                assert_eq!(replaced.start, Seq(4));
                assert_eq!(replaced.end_inclusive, Seq(6));
            }
            other => panic!("expected RevertApplied, got {other:?}"),
        }

        let messages = session.derive_messages(&ProjectionPolicy::default());
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content.first_text(), Some("one"));

        let reopened = Session::open_or_create(
            &config,
            session.id().clone(),
            PathBuf::from("/tmp/workspace"),
        )
        .await
        .unwrap();
        assert_eq!(reopened.events().len(), 7, "revert never deletes events");
        assert_eq!(
            reopened.derive_messages(&ProjectionPolicy::default()).len(),
            1
        );

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn revert_closes_open_turn_before_shadowing() {
        let session = Session::memory(SessionId::new(), PathBuf::from("/tmp/workspace"));
        let turn = TurnId::new();
        session.append(started(&turn)).await.unwrap();
        session.append(user("abandoned")).await.unwrap();

        session.revert(&turn).await.unwrap();

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
            SessionEvent::RevertApplied { replaced, .. } => {
                assert_eq!(replaced.start, Seq(1));
                assert_eq!(replaced.end_inclusive, Seq(3));
            }
            other => panic!("expected RevertApplied, got {other:?}"),
        }
        assert!(
            session
                .derive_messages(&ProjectionPolicy::default())
                .is_empty()
        );
    }

    #[tokio::test]
    async fn revert_rejects_unknown_turn() {
        let session = Session::memory(SessionId::new(), PathBuf::from("/tmp/workspace"));
        session.append(user("hello")).await.unwrap();

        assert!(session.revert(&TurnId::from("missing")).await.is_err());
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
        session.set_rotation_threshold(150);
        for index in 0..12 {
            session
                .append(user(&format!("permission check {index}")))
                .await
                .unwrap();
        }
        session.flush().await.unwrap();

        let dir = session.dir().unwrap().to_path_buf();
        assert_eq!(mode_of(&dir), 0o700);
        assert_eq!(mode_of(&dir.join(META_FILE)), 0o600);
        assert_eq!(
            mode_of(&dir.join(storage::active_name(
                session.inner.generation.load(Ordering::Relaxed),
            ))),
            0o600
        );
        assert_eq!(mode_of(&dir.join(storage::sealed_name(0))), 0o600);

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn blob_files_use_private_permissions() {
        let root = temp_root("blob-perms");
        std::fs::create_dir_all(&root).unwrap();
        let blobs = BlobStore::for_session(&root);

        let path = blobs.put("spilled output").unwrap();

        assert_eq!(mode_of(&path), 0o600);
        assert_eq!(mode_of(blobs.dir()), 0o700);

        std::fs::remove_dir_all(&root).unwrap();
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
            project_local: false,
        };
        let session = Session::open_or_create(&config, SessionId::new(), workspace.clone())
            .await
            .unwrap();

        assert_eq!(session.meta().git_branch.as_deref(), Some("main"));
        assert_eq!(session.meta().git_sha.as_deref(), Some(GIT_SHA));

        let raw = fs::read_to_string(root.join(session.id().as_str()).join(META_FILE))
            .await
            .unwrap();
        let persisted: SessionMeta = serde_json::from_str(&raw).unwrap();
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
            project_local: false,
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
            project_local: false,
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
            project_local: false,
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

        let raw = fs::read_to_string(root.join(session.id().as_str()).join(META_FILE))
            .await
            .unwrap();
        let persisted: SessionMeta = serde_json::from_str(&raw).unwrap();
        assert_eq!(persisted.title.as_deref(), Some("Fix the parser"));
        assert_eq!(persisted.model.as_deref(), Some("gpt-4o-mini"));
        assert_eq!(persisted.updated_at_ms, modeled.updated_at_ms);

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn lists_sessions_newest_first_and_skips_malformed() {
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

        std::fs::create_dir_all(root.join("broken")).unwrap();
        std::fs::write(root.join("broken").join(META_FILE), "not json").unwrap();

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

        let raw = fs::read_to_string(root.join(older.id().as_str()).join(META_FILE))
            .await
            .unwrap();
        let persisted: SessionMeta = serde_json::from_str(&raw).unwrap();
        assert_eq!(persisted.updated_at_ms, older.meta().updated_at_ms);

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn open_retightens_legacy_world_readable_files() {
        let root = temp_root("legacy-perms");
        let config = config(&root);
        let id = SessionId::new();
        let session = Session::open_or_create(&config, id.clone(), PathBuf::from("/tmp/workspace"))
            .await
            .unwrap();
        session.set_rotation_threshold(150);
        for index in 0..12 {
            session
                .append(user(&format!("legacy permission {index}")))
                .await
                .unwrap();
        }
        session.flush().await.unwrap();

        let blobs = session.blob_store().unwrap();
        let blob_path = blobs.put("spilled").unwrap();
        let dir = session.dir().unwrap().to_path_buf();
        drop(session);

        let loose = |path: &Path| {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
        };

        let mut files = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            if entry.file_type().unwrap().is_dir() {
                continue;
            }
            loose(&entry.path());
            files.push(entry.path());
        }
        loose(&blob_path);

        assert!(files.iter().any(|path| path.ends_with(META_FILE)));
        assert!(
            files
                .iter()
                .any(|path| path.extension().is_some_and(|ext| ext == "zstd"))
        );
        assert!(
            files
                .iter()
                .any(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        );

        let reopened = Session::open(&config, id, PathBuf::from("/tmp/workspace"))
            .await
            .unwrap();
        assert_eq!(mode_of(reopened.dir().unwrap()), 0o700);
        for path in &files {
            assert_eq!(mode_of(path), 0o600, "{}", path.display());
        }
        assert_eq!(mode_of(&blob_path), 0o600);
        assert_eq!(mode_of(blobs.dir()), 0o700);

        fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn create_child_links_parent_and_latest_session_skips_children() {
        let root = temp_root("child-session");
        let config = SessionConfig {
            root: root.clone(),
            workspace: root.clone(),
            project_local: false,
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
}
