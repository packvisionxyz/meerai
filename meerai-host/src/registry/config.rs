use super::*;

impl SessionRegistry {
    pub fn set_model(
        &self,
        session_id: &SessionId,
        provider: String,
        display_model: String,
        command_model: String,
        context_window_tokens: Option<u64>,
        effort_levels: Option<Vec<meerai_common::settings::EffortLevel>>,
    ) -> Result<(), HostError> {
        let cmd_tx = {
            let mut state = self.lock_state()?;
            let Some(record) = state.sessions.get_mut(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            if record.is_acp() {
                return Err(HostError::UnsupportedBackend(
                    "changing models is not supported for ACP sessions".to_string(),
                ));
            }
            if record.summary.status != SessionStatus::Idle {
                return Err(HostError::Busy);
            }
            record.summary.provider = provider.clone();
            record.summary.model = display_model.clone();
            let summary = record.summary.clone();
            let cmd_tx = record.cmd_tx.clone();
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::SessionUpdated { session: summary },
            );
            cmd_tx
        };
        cmd_tx
            .send(AgentCommand::SetModel {
                model: command_model,
                provider: Some(provider),
                context_window_tokens,
                effort_levels,
            })
            .map_err(|_| HostError::WorkerClosed)
    }
    pub fn set_approval_mode(
        &self,
        session_id: &SessionId,
        mode: ApprovalMode,
    ) -> Result<(), HostError> {
        let cmd_tx = {
            let mut state = self.lock_state()?;
            let Some(record) = state.sessions.get_mut(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            if record.is_acp() {
                return Err(HostError::UnsupportedBackend(
                    "changing approval mode is not supported for ACP sessions".to_string(),
                ));
            }
            record.summary.approval_mode = mode;
            let summary = record.summary.clone();
            let cmd_tx = record.cmd_tx.clone();
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::SessionUpdated { session: summary },
            );
            cmd_tx
        };
        cmd_tx
            .send(AgentCommand::SetPermissionMode(mode))
            .map_err(|_| HostError::WorkerClosed)
    }
    pub fn apply_agent(
        &self,
        session_id: &SessionId,
        app: ResolvedAgentApplication,
    ) -> Result<(), HostError> {
        let (cmd_tx, notice) = {
            let mut state = self.lock_state()?;
            let Some(record) = state.sessions.get(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            if record.is_acp() {
                return Err(HostError::UnsupportedBackend(
                    "agents are not supported for ACP sessions".to_string(),
                ));
            }
            if record.summary.status != SessionStatus::Idle {
                return Err(HostError::Busy);
            }
            let entry_id = state.next_entry_id();
            let notice = match &app.model {
                Some(model) => format!("Applied agent @{} (model: {model})", app.name),
                None => format!("Applied agent @{}", app.name),
            };
            let entry = LiveEntry::System {
                id: entry_id,
                text: notice.clone(),
            };
            let record = state.sessions.get_mut(session_id).unwrap();
            if let Some(model) = &app.model {
                record.summary.model = display_model(model);
            }
            record.entries.push(entry.clone());
            let summary = record.summary.clone();
            let cmd_tx = record.cmd_tx.clone();
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::EntryCompleted {
                    session_id: session_id.clone(),
                    entry,
                },
            );
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::SessionUpdated { session: summary },
            );
            (cmd_tx, notice)
        };
        let _ = notice;
        cmd_tx
            .send(AgentCommand::ApplyAgent {
                name: app.name,
                instructions: app.instructions,
                model: app.model,
                context_window_tokens: app.context_window_tokens,
                effort_levels: app.effort_levels,
            })
            .map_err(|_| HostError::WorkerClosed)
    }
    pub fn apply_skill(
        &self,
        session_id: &SessionId,
        app: ResolvedSkillApplication,
    ) -> Result<(), HostError> {
        let cmd_tx = {
            let mut state = self.lock_state()?;
            let Some(record) = state.sessions.get(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            if record.is_acp() {
                return Err(HostError::UnsupportedBackend(
                    "skills are not supported for ACP sessions".to_string(),
                ));
            }
            if record.summary.status != SessionStatus::Idle {
                return Err(HostError::Busy);
            }
            let entry_id = state.next_entry_id();
            let entry = LiveEntry::System {
                id: entry_id,
                text: format!("Loaded skill @{}", app.name),
            };
            let record = state.sessions.get_mut(session_id).unwrap();
            record.entries.push(entry.clone());
            let cmd_tx = record.cmd_tx.clone();
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::EntryCompleted {
                    session_id: session_id.clone(),
                    entry,
                },
            );
            cmd_tx
        };
        cmd_tx
            .send(AgentCommand::ApplySkill {
                name: app.name,
                instructions: app.instructions,
            })
            .map_err(|_| HostError::WorkerClosed)
    }
}
