use super::*;

impl SessionRegistry {
    pub fn emit_reload(&self, outcome: &HostReloadOutcome) {
        let Ok(mut state) = self.lock_state() else {
            return;
        };
        for record in state.sessions.values() {
            if !record.is_acp() {
                let _ = record.cmd_tx.send(AgentCommand::Reload {
                    rules: outcome.payload.rules.clone(),
                    context: outcome.payload.context.clone(),
                    effort_levels: None,
                    system_prompt: outcome.payload.system_prompt.clone(),
                });
                let _ = record
                    .cmd_tx
                    .send(AgentCommand::SetHooks(outcome.payload.hooks.clone()));
            }
        }
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::Reloaded {
                host_info: Box::new(outcome.host_info.clone()),
                warnings: outcome.warnings.clone(),
            },
        );
    }
    pub fn emit_file_changes(&self, changes: Vec<FileChangeEntry>) {
        let Ok(mut state) = self.lock_state() else {
            return;
        };
        if state.file_changes == changes {
            return;
        }
        state.file_changes = changes.clone();
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::FileChangesUpdated { changes },
        );
    }
    pub fn emit_providers_updated(&self) {
        let Ok(mut state) = self.lock_state() else {
            return;
        };
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::ProvidersUpdated,
        );
    }
    pub fn emit_chatgpt_login(&self, status: ChatGptLoginStatus) {
        let Ok(mut state) = self.lock_state() else {
            return;
        };
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::ChatGptLoginUpdated(status),
        );
    }
    pub async fn shutdown_all(&self) {
        let handles = {
            let Ok(mut state) = self.lock_state() else {
                return;
            };
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::HostDisconnected,
            );
            for record in state.sessions.values() {
                let _ = record.cmd_tx.send(AgentCommand::Cancel);
                let _ = record.cmd_tx.send(AgentCommand::Quit);
            }
            std::mem::take(&mut state.pump_handles)
        };

        let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, async {
            for handle in handles.into_values() {
                let _ = handle.await;
            }
        })
        .await;
    }
}
