use super::*;

impl SessionRegistry {
    pub fn note_tui_user_prompt(
        &self,
        session_id: &SessionId,
        prompt: &str,
        display: Option<&str>,
        steer: bool,
    ) {
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return;
        }
        let display = display
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or(&prompt)
            .to_string();
        let Ok(mut state) = self.lock_state() else {
            return;
        };
        if !state.sessions.contains_key(session_id) {
            return;
        }
        let entry_id = state.next_entry_id();
        let summary = {
            let record = state.sessions.get_mut(session_id).unwrap();
            if record.summary.title.is_none() {
                record.summary.title = normalize_title(&prompt);
            }
            if !steer {
                record.summary.status = SessionStatus::Running;
                record.summary.turn_started_at_ms = Some(now_millis());
                record.turn_started_instant = Some(Instant::now());
            }
            record.entries.push(LiveEntry::User {
                id: entry_id.clone(),
                turn_id: if steer {
                    record.summary.active_turn_id.clone()
                } else {
                    None
                },
                text: display.clone(),
            });
            record.summary.clone()
        };
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::UserPromptSubmitted {
                session_id: session_id.clone(),
                entry_id,
                text: display,
            },
        );
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::SessionUpdated { session: summary },
        );
    }
    pub fn note_tui_mode(&self, session_id: &SessionId, mode: ApprovalMode) {
        let Ok(mut state) = self.lock_state() else {
            return;
        };
        let Some(record) = state.sessions.get_mut(session_id) else {
            return;
        };
        if record.summary.approval_mode == mode {
            return;
        }
        record.summary.approval_mode = mode;
        let session = record.summary.clone();
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::SessionUpdated { session },
        );
    }
    pub fn note_tui_model(&self, session_id: &SessionId, provider: &str, model: &str) {
        let Ok(mut state) = self.lock_state() else {
            return;
        };
        let Some(record) = state.sessions.get_mut(session_id) else {
            return;
        };
        record.summary.provider = provider.to_string();
        record.summary.model = display_model(model);
        let session = record.summary.clone();
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::SessionUpdated { session },
        );
    }
    pub fn note_tui_reset(&self, session_id: &SessionId) {
        let Ok(mut state) = self.lock_state() else {
            return;
        };
        let Some(record) = state.sessions.get_mut(session_id) else {
            return;
        };
        record.entries.clear();
        record.streaming_item_id = None;
        record.streaming_text.clear();
        record.streaming_reasoning.clear();
        record.pending_approvals.clear();
        record.pending_questions.clear();
        record.usage = UsageStatsSnapshot::default();
        record.metrics = TurnMetricsSnapshot::default();
        record.summary.active_turn_id = None;
        record.summary.active_tool = None;
        record.summary.turn_started_at_ms = None;
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::SessionReset {
                session_id: session_id.clone(),
            },
        );
    }
    pub fn set_active_session(&self, session_id: &SessionId) -> Result<(), HostError> {
        let mut state = self.lock_state()?;
        if !state.sessions.contains_key(session_id) {
            return Err(HostError::SessionNotFound(session_id.clone()));
        }
        if state.active_session_id.as_ref() != Some(session_id) {
            state.active_session_id = Some(session_id.clone());
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::SessionActivated {
                    session_id: session_id.clone(),
                },
            );
        }
        Ok(())
    }
    pub async fn close_and_wait_session(&self, session_id: &SessionId) -> Result<(), HostError> {
        let handle = {
            let mut state = self.lock_state()?;
            let Some(record) = state.sessions.remove(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            state.session_order.retain(|id| id != session_id);
            let next_active = if state.active_session_id.as_ref() == Some(session_id) {
                state.active_session_id = state.session_order.first().cloned();
                state.active_session_id.clone()
            } else {
                state.active_session_id.clone()
            };
            let _ = record.cmd_tx.send(AgentCommand::Cancel);
            let _ = record.cmd_tx.send(AgentCommand::Quit);
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::SessionClosed {
                    session_id: session_id.clone(),
                    next_active_session_id: next_active,
                },
            );
            state.pump_handles.remove(session_id)
        };
        if let Some(handle) = handle {
            let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, handle).await;
        }
        Ok(())
    }
    pub fn subscribe(&self, after_seq: Option<u64>) -> SubscribeResult {
        let rx = self.inner.broadcast_tx.subscribe();
        let Ok(state) = self.lock_state() else {
            return SubscribeResult::ResyncRequired { current_seq: 0, rx };
        };
        let Some(after) = after_seq else {
            return SubscribeResult::Replay {
                events: Vec::new(),
                rx,
            };
        };
        if after >= state.current_seq {
            return SubscribeResult::Replay {
                events: Vec::new(),
                rx,
            };
        }
        let Some(oldest) = state.ring.front() else {
            return SubscribeResult::ResyncRequired {
                current_seq: state.current_seq,
                rx,
            };
        };
        if after < oldest.seq.saturating_sub(1) {
            return SubscribeResult::ResyncRequired {
                current_seq: state.current_seq,
                rx,
            };
        }
        let events = state
            .ring
            .iter()
            .filter(|event| event.seq > after)
            .cloned()
            .collect();
        SubscribeResult::Replay { events, rx }
    }
}
