use super::*;

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
    pub(super) async fn open_inner(
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
}
