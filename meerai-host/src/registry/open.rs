use super::*;

impl SessionRegistry {
    pub(super) fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, RegistryState>, HostError> {
        self.inner.state.lock().map_err(|_| HostError::Poisoned)
    }
    pub fn new() -> Arc<Self> {
        let (broadcast_tx, _) = broadcast::channel(CAPACITY);
        let (agent_broadcast_tx, _) = broadcast::channel(CAPACITY);
        Arc::new(Self {
            inner: Arc::new(RegistryInner {
                state: Mutex::new(RegistryState::new()),
                broadcast_tx,
                agent_broadcast_tx,
            }),
        })
    }
    pub fn register_opened(&self, opened: OpenedSession) -> LiveSessionSummary {
        self.register_opened_with_activation(opened, false)
    }
    pub fn register_opened_active(&self, opened: OpenedSession) -> LiveSessionSummary {
        self.register_opened_with_activation(opened, true)
    }
    fn register_opened_with_activation(
        &self,
        opened: OpenedSession,
        activate: bool,
    ) -> LiveSessionSummary {
        let replay = replay_from_events(&opened.events);
        let turn_ids = collect_user_turn_ids(&opened.events);
        let mut turn_iter = turn_ids.into_iter();
        let cumulative = session_usage_from_events(&opened.events);
        let metrics = cumulative_metrics_from_usage(cumulative.as_ref());

        let session_id = opened.session_id;
        let summary = LiveSessionSummary {
            session_id: session_id.clone(),
            title: opened.title,
            backend: opened.backend,
            provider: opened.provider,
            model: opened.model,
            approval_mode: opened.approval_mode,
            status: SessionStatus::Idle,
            active_turn_id: None,
            active_tool: None,
            turn_started_at_ms: None,
            user_shell_running: false,
            transient: opened.transient,
            shell_runs: Vec::new(),
        };

        let Ok(mut state) = self.lock_state() else {
            return summary;
        };
        let entries = replay
            .into_iter()
            .map(|entry| replay_entry_to_live(entry, &mut turn_iter, &mut state))
            .collect();

        let record = SessionRecord {
            summary: summary.clone(),
            entries,
            streaming_item_id: None,
            streaming_text: String::new(),
            streaming_reasoning: String::new(),
            cmd_tx: opened.cmd_tx,
            pending_approvals: VecDeque::new(),
            pending_questions: VecDeque::new(),
            usage: UsageStatsSnapshot::default(),
            metrics,
            context: None,
            loop_state: last_loop_from_events(&opened.events),
            turn_started_instant: None,
            tool_items: HashMap::new(),
            recovery_pending: false,
        };

        if !state.sessions.contains_key(&session_id) {
            state.session_order.push(session_id.clone());
        }
        state.sessions.insert(session_id.clone(), record);
        let became_active = if activate || state.active_session_id.is_none() {
            let changed = state.active_session_id.as_ref() != Some(&session_id);
            state.active_session_id = Some(session_id.clone());
            changed
        } else {
            false
        };
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::SessionOpened {
                session: summary.clone(),
            },
        );
        if became_active && activate {
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::SessionActivated {
                    session_id: session_id.clone(),
                },
            );
        }

        let inner = Arc::clone(&self.inner);
        let event_rx = opened.event_rx;
        let spawned_id = session_id.clone();
        let handle = tokio::spawn(async move {
            run_pump(inner, spawned_id, event_rx).await;
        });
        state.pump_handles.insert(session_id, handle);

        summary
    }
    pub fn context_for(&self, id: &SessionId) -> Option<meerai_session::ContextBreakdown> {
        let state = self.lock_state().ok()?;
        state
            .sessions
            .get(id)
            .and_then(|record| record.context.clone())
    }
    pub fn snapshot(&self) -> RegistrySnapshot {
        let state = match self.lock_state() {
            Ok(state) => state,
            Err(_) => return RegistrySnapshot::poisoned(),
        };
        let sessions = state
            .session_order
            .iter()
            .filter_map(|id| state.sessions.get(id).map(|r| r.summary.clone()))
            .collect();
        let active_session = state
            .active_session_id
            .as_ref()
            .and_then(|id| state.sessions.get(id).map(SessionRecord::to_detail));
        RegistrySnapshot {
            event_id: state.current_seq,
            active_session_id: state.active_session_id.clone(),
            sessions,
            active_session,
            file_changes: state.file_changes.clone(),
            poisoned: false,
        }
    }
    pub fn has_session(&self, id: &SessionId) -> bool {
        self.lock_state()
            .map(|state| state.sessions.contains_key(id))
            .unwrap_or(false)
    }
    pub fn session_detail(&self, id: &SessionId) -> Option<LiveSessionDetail> {
        let state = self.lock_state().ok()?;
        state.sessions.get(id).map(SessionRecord::to_detail)
    }
    pub fn session_summary(&self, id: &SessionId) -> Option<LiveSessionSummary> {
        let state = self.lock_state().ok()?;
        state.sessions.get(id).map(|r| r.summary.clone())
    }
    pub fn cmd_tx(&self, id: &SessionId) -> Option<mpsc::UnboundedSender<AgentCommand>> {
        let state = self.lock_state().ok()?;
        state.sessions.get(id).map(|r| r.cmd_tx.clone())
    }
    pub fn subscribe_agent_events(&self) -> broadcast::Receiver<(SessionId, AgentEvent)> {
        self.inner.agent_broadcast_tx.subscribe()
    }
}
