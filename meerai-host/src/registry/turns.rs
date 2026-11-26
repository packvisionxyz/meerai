use super::*;

impl SessionRegistry {
    pub fn submit_prompt(
        &self,
        session_id: &SessionId,
        prompt: String,
        display: Option<String>,
    ) -> Result<(), HostError> {
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return Err(HostError::EmptyPrompt);
        }
        let display = display_text(display, &prompt);

        let cmd_tx = {
            let mut state = self.lock_state()?;
            let Some(record) = state.sessions.get(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            if record.summary.status != SessionStatus::Idle || record.recovery_pending {
                return Err(HostError::Busy);
            }
            if record.cmd_tx.is_closed() {
                return Err(HostError::WorkerClosed);
            }

            let entry_id = state.next_entry_id();
            let (summary, cmd_tx) = {
                let record = state.sessions.get_mut(session_id).unwrap();
                record.summary.status = SessionStatus::Running;
                record.summary.turn_started_at_ms = Some(now_millis());
                record.turn_started_instant = Some(Instant::now());
                if record.summary.title.is_none() {
                    record.summary.title = normalize_title(&prompt);
                }
                record.entries.push(LiveEntry::User {
                    id: entry_id.clone(),
                    turn_id: None,
                    text: display.clone(),
                });
                (record.summary.clone(), record.cmd_tx.clone())
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
            cmd_tx
        };

        cmd_tx
            .send(AgentCommand::SubmitPrompt(prompt))
            .map_err(|_| HostError::WorkerClosed)
    }
    pub fn steer_prompt(
        &self,
        session_id: &SessionId,
        prompt: String,
        display: Option<String>,
    ) -> Result<(), HostError> {
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return Err(HostError::EmptyPrompt);
        }
        let display = display_text(display, &prompt);

        let cmd_tx = {
            let mut state = self.lock_state()?;
            let Some(record) = state.sessions.get(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            if record.is_acp() {
                return Err(HostError::UnsupportedBackend(
                    "steering mid-turn is not supported for ACP sessions".to_string(),
                ));
            }
            if record.summary.status == SessionStatus::Idle {
                return Err(HostError::NotRunning);
            }
            if record.cmd_tx.is_closed() {
                return Err(HostError::WorkerClosed);
            }
            let entry_id = state.next_entry_id();
            let record = state.sessions.get_mut(session_id).unwrap();
            record.entries.push(LiveEntry::User {
                id: entry_id.clone(),
                turn_id: record.summary.active_turn_id.clone(),
                text: display.clone(),
            });
            let cmd_tx = record.cmd_tx.clone();
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::UserPromptSubmitted {
                    session_id: session_id.clone(),
                    entry_id,
                    text: display,
                },
            );
            cmd_tx
        };

        cmd_tx
            .send(AgentCommand::Steer(prompt))
            .map_err(|_| HostError::WorkerClosed)
    }
    pub fn cancel_turn(&self, session_id: &SessionId) -> Result<(), HostError> {
        let state = self.lock_state()?;
        let Some(record) = state.sessions.get(session_id) else {
            return Err(HostError::SessionNotFound(session_id.clone()));
        };
        if record.summary.status == SessionStatus::Idle {
            return Err(HostError::NotRunning);
        }
        record
            .cmd_tx
            .send(AgentCommand::Cancel)
            .map_err(|_| HostError::WorkerClosed)
    }
    pub fn compact(&self, session_id: &SessionId) -> Result<(), HostError> {
        let cmd_tx = {
            let state = self.lock_state()?;
            let Some(record) = state.sessions.get(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            if record.is_acp() {
                return Err(HostError::UnsupportedBackend(
                    "compaction is not supported for ACP sessions".to_string(),
                ));
            }
            if record.summary.status != SessionStatus::Idle {
                return Err(HostError::Busy);
            }
            record.cmd_tx.clone()
        };
        cmd_tx
            .send(AgentCommand::Compact {
                focus: None,
                from_turn_id: None,
            })
            .map_err(|_| HostError::WorkerClosed)
    }
    pub fn reset(&self, session_id: &SessionId) -> Result<(), HostError> {
        let cmd_tx = {
            let mut state = self.lock_state()?;
            let Some(record) = state.sessions.get(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            if record.is_acp() {
                return Err(HostError::UnsupportedBackend(
                    "reset is not supported for ACP sessions".to_string(),
                ));
            }
            if record.summary.status != SessionStatus::Idle {
                return Err(HostError::Busy);
            }
            if record.cmd_tx.is_closed() {
                return Err(HostError::WorkerClosed);
            }
            let record = state.sessions.get_mut(session_id).unwrap();
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
            let cmd_tx = record.cmd_tx.clone();
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::SessionReset {
                    session_id: session_id.clone(),
                },
            );
            cmd_tx
        };
        cmd_tx
            .send(AgentCommand::Reset)
            .map_err(|_| HostError::WorkerClosed)
    }
    pub fn rewind(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        scope: RewindScope,
    ) -> Result<Option<String>, HostError> {
        let (cmd_tx, restored_prompt) = {
            let state = self.lock_state()?;
            let Some(record) = state.sessions.get(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            if record.is_acp() {
                return Err(HostError::UnsupportedBackend(
                    "rewind is not supported for ACP sessions".to_string(),
                ));
            }
            if record.summary.status != SessionStatus::Idle {
                return Err(HostError::Busy);
            }
            let prompt = if matches!(scope, RewindScope::Both | RewindScope::Conversation) {
                record.entries.iter().find_map(|entry| match entry {
                    LiveEntry::User {
                        turn_id: Some(tid),
                        text,
                        ..
                    } if tid == &turn_id => Some(text.clone()),
                    _ => None,
                })
            } else {
                None
            };
            (record.cmd_tx.clone(), prompt)
        };
        cmd_tx
            .send(AgentCommand::Rewind { turn_id, scope })
            .map_err(|_| HostError::WorkerClosed)?;
        Ok(restored_prompt)
    }
}
