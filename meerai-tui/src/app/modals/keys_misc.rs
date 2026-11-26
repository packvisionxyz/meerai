use super::*;

impl AppState {
    pub(crate) fn keys_misc(
        &mut self,
        modal: ActiveModal,
        key: KeyEvent,
        _has_ctrl: bool,
        _has_alt: bool,
    ) -> bool {
        match modal {
            ActiveModal::VaultReset { mut input, error } => {
                if key.code == KeyCode::Esc {
                    self.modal = None;
                    return true;
                }
                match handle_text_key(&mut input, key, 24, false, false) {
                    TextAction::Submit => {
                        if input.trim() != "reset" {
                            self.modal = Some(ActiveModal::VaultReset {
                                input,
                                error: Some("Type reset exactly to confirm.".to_string()),
                            });
                        } else {
                            match self.secrets.reset_vault() {
                                Ok(removed) => {
                                    self.modal = None;
                                    self.tab_mut().auto_follow = true;
                                    self.tab_mut().transcript.push(TranscriptEntry::System {
                                        text: format!(
                                            "Vault reset: removed {removed} saved credentials. Run /connect to log in again."
                                        ),
                                    });
                                }
                                Err(error) => {
                                    self.modal = Some(ActiveModal::VaultReset {
                                        input,
                                        error: Some(format!("{error:#}")),
                                    });
                                }
                            }
                        }
                    }
                    TextAction::Edited => {
                        self.modal = Some(ActiveModal::VaultReset { input, error: None });
                    }
                    TextAction::Ignored => {
                        self.modal = Some(ActiveModal::VaultReset { input, error });
                    }
                }
                true
            }
            ActiveModal::SubagentSpawn(mut panel) => {
                const FIELDS: usize = 9;
                let width = self.terminal_width.saturating_sub(16).max(20) as usize;
                match key.code {
                    KeyCode::Esc => {
                        self.modal = None;
                        return true;
                    }
                    KeyCode::Tab => {
                        panel.field = (panel.field + 1) % FIELDS;
                        panel.error = None;
                    }
                    KeyCode::BackTab => {
                        panel.field = if panel.field == 0 {
                            FIELDS - 1
                        } else {
                            panel.field - 1
                        };
                        panel.error = None;
                    }
                    KeyCode::Down if panel.field != 2 => {
                        panel.field = (panel.field + 1) % FIELDS;
                        panel.error = None;
                    }
                    KeyCode::Up if panel.field != 2 => {
                        panel.field = if panel.field == 0 {
                            FIELDS - 1
                        } else {
                            panel.field - 1
                        };
                        panel.error = None;
                    }
                    KeyCode::Char('m') if panel.field == 4 => {
                        panel.mode = match panel.mode {
                            None => Some(ApprovalMode::Plan),
                            Some(ApprovalMode::Plan) => Some(ApprovalMode::Ask),
                            Some(ApprovalMode::Ask) => Some(ApprovalMode::Auto),
                            Some(ApprovalMode::Auto) => None,
                        };
                        panel.error = None;
                    }
                    _ if panel.field == 4 => {}
                    _ => {
                        let multiline = panel.field == 2;
                        let field = match panel.field {
                            0 => &mut panel.name,
                            1 => &mut panel.agent,
                            2 => &mut panel.task,
                            3 => &mut panel.model,
                            5 => &mut panel.steps,
                            6 => &mut panel.tokens,
                            7 => &mut panel.cost,
                            _ => &mut panel.tools,
                        };
                        match handle_text_key(field, key, width, multiline, multiline) {
                            TextAction::Submit => {
                                if panel.task.trim().is_empty() {
                                    panel.error = Some("Task must not be empty.".to_string());
                                } else if let Some(spec) = subagent_spec_from_panel(&panel) {
                                    let _ = self
                                        .tab()
                                        .cmd_tx
                                        .send(AgentCommand::SpawnSubagent { spec });
                                    self.status_notice = Some("Subagent started.".to_string());
                                    self.modal = None;
                                    return true;
                                } else {
                                    panel.error = Some(
                                        "Steps and tokens must be integers; cost must be a number."
                                            .to_string(),
                                    );
                                }
                            }
                            TextAction::Edited => {
                                panel.error = None;
                            }
                            TextAction::Ignored => {}
                        }
                    }
                }
                self.modal = Some(ActiveModal::SubagentSpawn(panel));
                true
            }
            _ => false,
        }
    }
}
