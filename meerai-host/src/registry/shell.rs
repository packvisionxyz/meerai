use super::*;

impl SessionRegistry {
    pub fn begin_user_shell(
        &self,
        session_id: &SessionId,
        command: &str,
        run_id: Option<String>,
    ) -> Result<String, HostError> {
        let mut state = self.lock_state()?;
        let Some(record) = state.sessions.get(session_id) else {
            return Err(HostError::SessionNotFound(session_id.clone()));
        };
        if record.summary.approval_mode == ApprovalMode::Plan {
            return Err(HostError::InvalidInput(
                "shell commands are disabled in plan mode".to_string(),
            ));
        }
        let run_id = run_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| format!("shell-{}", now_millis()));
        if record.summary.shell_runs.iter().any(|run| run.id == run_id) {
            return Err(HostError::InvalidInput(
                "shell run id is already active".to_string(),
            ));
        }
        let entry_id = state.next_entry_id();
        let entry = LiveEntry::User {
            id: entry_id,
            turn_id: None,
            text: format!("!{command}"),
        };
        let record = state.sessions.get_mut(session_id).unwrap();
        record.summary.shell_runs.push(LiveShellRun {
            id: run_id.clone(),
            command: command.to_string(),
            started_at_ms: now_millis(),
        });
        record.summary.user_shell_running = true;
        record.entries.push(entry.clone());
        let summary = record.summary.clone();
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
        Ok(run_id)
    }
    pub fn finish_user_shell(
        &self,
        session_id: &SessionId,
        run_id: &str,
        result: Result<String, String>,
    ) {
        let Ok(mut state) = self.lock_state() else {
            return;
        };
        if !state.sessions.contains_key(session_id) {
            return;
        }
        let entry_id = state.next_entry_id();
        let entry = match result {
            Ok(output) => LiveEntry::System {
                id: entry_id,
                text: output,
            },
            Err(err) => LiveEntry::Error {
                id: entry_id,
                message: format!("shell command failed: {err}"),
            },
        };
        let record = state.sessions.get_mut(session_id).unwrap();
        record.summary.shell_runs.retain(|run| run.id != run_id);
        record.summary.user_shell_running = !record.summary.shell_runs.is_empty();
        record.entries.push(entry.clone());
        let summary = record.summary.clone();
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
    }
    pub fn append_system_entry(&self, session_id: &SessionId, text: String) {
        let Ok(mut state) = self.lock_state() else {
            return;
        };
        if !state.sessions.contains_key(session_id) {
            return;
        }
        let entry = LiveEntry::System {
            id: state.next_entry_id(),
            text,
        };
        state
            .sessions
            .get_mut(session_id)
            .unwrap()
            .entries
            .push(entry.clone());
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::EntryCompleted {
                session_id: session_id.clone(),
                entry,
            },
        );
    }
}
