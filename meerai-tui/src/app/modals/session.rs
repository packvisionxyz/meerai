use super::*;

impl AppState {
    pub(crate) fn reload_settings_from_disk(&mut self) {
        let Some(dir) = self.config_dir.clone() else {
            return;
        };
        let Ok(loaded) = meerai_common::settings::load_settings_from(&dir) else {
            return;
        };
        let provider = self.tab().active_provider.clone();
        let model = self.tab().model.clone();
        self.settings = loaded;
        self.settings.active_provider = Some(provider);
        if !model.is_empty() {
            self.settings.active_model = Some(model);
        }
    }
    pub(crate) fn activate_session_choice(
        &mut self,
        session_id: meerai_session::SessionId,
        open_tab: Option<usize>,
    ) {
        if let Some(tab_idx) = open_tab {
            self.switch_to_tab(tab_idx);
            self.status_notice = Some(format!("Switched to tab {}", tab_idx + 1));
        } else if self.session_opener.is_some() {
            self.status_notice = Some(format!("Opening session {session_id}..."));
            self.pending_session_actions
                .push(PendingSessionAction::OpenSessionId(session_id));
        } else {
            self.tab_mut().transcript.push(TranscriptEntry::Error {
                text: "Session switching is not available in this context.".to_string(),
                expanded: false,
            });
        }
    }
    pub(crate) fn apply_rewind_choice(&mut self, turn_id: TurnId, selected: usize) {
        let scope = match selected {
            0 => RewindScope::Both,
            _ => RewindScope::Code,
        };
        if scope == RewindScope::Both
            && let Some(index) = self
                .tab()
                .user_turns
                .iter()
                .find_map(|(index, turn)| (turn == &turn_id).then_some(*index))
        {
            let message = match self.tab().transcript.get(index) {
                Some(TranscriptEntry::User { text, .. }) => Some(text.clone()),
                _ => None,
            };
            let tab = self.tab_mut();
            tab.transcript.truncate(index);
            tab.user_turns.retain(|idx, _| *idx < index);
            tab.turn_activity.retain(|idx, _| *idx < index);
            tab.is_running = false;
            tab.turn_id = None;
            tab.queued.clear();
            tab.auto_follow = true;
            if let Some(message) = message {
                self.restore_composer_message(message);
            }
        }
        let _ = self
            .tab()
            .cmd_tx
            .send(AgentCommand::Rewind { turn_id, scope });
        self.modal = None;
        self.status_notice = Some(match scope {
            RewindScope::Both => {
                "Rewinding code and conversation; message restored to the composer".to_string()
            }
            _ => "Rewinding code; conversation kept".to_string(),
        });
    }
    pub(crate) fn persist_settings(&mut self) {
        if let Some(dir) = &self.config_dir
            && let Err(err) = save_settings_to(dir, &self.settings)
        {
            self.status_notice = Some(format!("Failed to save settings: {err:#}"));
        }
    }
}
