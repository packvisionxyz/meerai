use super::*;

impl AppState {
    pub(crate) fn keys_panels(
        &mut self,
        modal: ActiveModal,
        key: KeyEvent,
        has_ctrl: bool,
        has_alt: bool,
    ) -> bool {
        match modal {
            ActiveModal::DiffView {
                mut file_index,
                mut diff,
                mut scroll,
                mut list,
            } => {
                let max_scroll = diff.rows.len().saturating_sub(1);
                let file_count = self.file_changes.len();
                let list_selected = match list {
                    DiffListState::Open { selected } => {
                        Some(selected.min(file_count.saturating_sub(1)))
                    }
                    DiffListState::Closed => None,
                };
                match key.code {
                    KeyCode::Char('f') if !has_ctrl && !has_alt => {
                        list = if list_selected.is_some() {
                            DiffListState::Closed
                        } else {
                            DiffListState::Open {
                                selected: file_index,
                            }
                        };
                    }
                    KeyCode::Esc if list_selected.is_some() => {
                        list = DiffListState::Closed;
                    }
                    KeyCode::Esc | KeyCode::Char('q') => {
                        self.modal = None;
                        return true;
                    }
                    KeyCode::Down | KeyCode::Char('j') => match list_selected {
                        Some(selected) => {
                            list = DiffListState::Open {
                                selected: (selected + 1).min(file_count.saturating_sub(1)),
                            };
                        }
                        None => {
                            scroll = (scroll + 1).min(max_scroll);
                        }
                    },
                    KeyCode::Up | KeyCode::Char('k') => match list_selected {
                        Some(selected) => {
                            list = DiffListState::Open {
                                selected: selected.saturating_sub(1),
                            };
                        }
                        None => {
                            scroll = scroll.saturating_sub(1);
                        }
                    },
                    KeyCode::Enter => {
                        if let Some(selected) = list_selected
                            && let Some(entry) = self.file_changes.get(selected)
                        {
                            file_index = selected;
                            diff =
                                build_file_diff(&self.workspace_root, &entry.path, &entry.status);
                            scroll = 0;
                        }
                    }
                    KeyCode::PageDown | KeyCode::Char('d')
                        if key.code == KeyCode::PageDown || has_ctrl =>
                    {
                        let step = (self.tab().viewport_height / 2).max(5);
                        scroll = (scroll + step).min(max_scroll);
                    }
                    KeyCode::PageUp | KeyCode::Char('u')
                        if key.code == KeyCode::PageUp || has_ctrl =>
                    {
                        let step = (self.tab().viewport_height / 2).max(5);
                        scroll = scroll.saturating_sub(step);
                    }
                    KeyCode::Home | KeyCode::Char('g') => {
                        scroll = 0;
                    }
                    KeyCode::End | KeyCode::Char('G') => {
                        scroll = max_scroll;
                    }
                    KeyCode::Char('s') if !has_ctrl && !has_alt => {
                        let target = list_selected
                            .unwrap_or(file_index)
                            .min(file_count.saturating_sub(1));
                        if let Some(entry) = self.file_changes.get(target).cloned() {
                            match self.stage_diff_path(&entry.path, true) {
                                Ok(Some(idx)) => {
                                    file_index = idx;
                                    let entry = &self.file_changes[idx];
                                    diff = build_file_diff(
                                        &self.workspace_root,
                                        &entry.path,
                                        &entry.status,
                                    );
                                    scroll = 0;
                                    if list_selected.is_some() {
                                        list = DiffListState::Open { selected: idx };
                                    }
                                    self.status_notice = Some(format!("Staged {}", entry.path));
                                }
                                Ok(None) => {
                                    if self.file_changes.is_empty() {
                                        self.modal = None;
                                        self.status_notice = Some(format!("Staged {}", entry.path));
                                        return true;
                                    }
                                    file_index =
                                        target.min(self.file_changes.len().saturating_sub(1));
                                    let next = &self.file_changes[file_index];
                                    diff = build_file_diff(
                                        &self.workspace_root,
                                        &next.path,
                                        &next.status,
                                    );
                                    scroll = 0;
                                    if list_selected.is_some() {
                                        list = DiffListState::Open {
                                            selected: file_index,
                                        };
                                    }
                                    self.status_notice = Some(format!("Staged {}", entry.path));
                                }
                                Err(err) => {
                                    self.status_notice = Some(format!("Stage failed: {err}"));
                                }
                            }
                        }
                    }
                    KeyCode::Char('u') if !has_ctrl && !has_alt => {
                        let target = list_selected
                            .unwrap_or(file_index)
                            .min(file_count.saturating_sub(1));
                        if let Some(entry) = self.file_changes.get(target).cloned() {
                            match self.stage_diff_path(&entry.path, false) {
                                Ok(Some(idx)) => {
                                    file_index = idx;
                                    let entry = &self.file_changes[idx];
                                    diff = build_file_diff(
                                        &self.workspace_root,
                                        &entry.path,
                                        &entry.status,
                                    );
                                    scroll = 0;
                                    if list_selected.is_some() {
                                        list = DiffListState::Open { selected: idx };
                                    }
                                    self.status_notice = Some(format!("Unstaged {}", entry.path));
                                }
                                Ok(None) => {
                                    if self.file_changes.is_empty() {
                                        self.modal = None;
                                        self.status_notice =
                                            Some(format!("Unstaged {}", entry.path));
                                        return true;
                                    }
                                    file_index =
                                        target.min(self.file_changes.len().saturating_sub(1));
                                    let next = &self.file_changes[file_index];
                                    diff = build_file_diff(
                                        &self.workspace_root,
                                        &next.path,
                                        &next.status,
                                    );
                                    scroll = 0;
                                    if list_selected.is_some() {
                                        list = DiffListState::Open {
                                            selected: file_index,
                                        };
                                    }
                                    self.status_notice = Some(format!("Unstaged {}", entry.path));
                                }
                                Err(err) => {
                                    self.status_notice = Some(format!("Unstage failed: {err}"));
                                }
                            }
                        }
                    }
                    KeyCode::Right | KeyCode::Char(']') if file_count > 0 => {
                        file_index = (file_index + 1) % file_count;
                        let entry = &self.file_changes[file_index];
                        diff = build_file_diff(&self.workspace_root, &entry.path, &entry.status);
                        scroll = 0;
                        if list_selected.is_some() {
                            list = DiffListState::Open {
                                selected: file_index,
                            };
                        }
                    }
                    KeyCode::Left | KeyCode::Char('[') if file_count > 0 => {
                        file_index = if file_index == 0 {
                            file_count - 1
                        } else {
                            file_index - 1
                        };
                        let entry = &self.file_changes[file_index];
                        diff = build_file_diff(&self.workspace_root, &entry.path, &entry.status);
                        scroll = 0;
                        if list_selected.is_some() {
                            list = DiffListState::Open {
                                selected: file_index,
                            };
                        }
                    }
                    _ => {}
                }
                self.modal = Some(ActiveModal::DiffView {
                    file_index,
                    diff,
                    scroll,
                    list,
                });
                true
            }
            ActiveModal::Search {
                mut query,
                mut mode,
                workspace_files,
                mut results,
                mut selected,
                mut preview,
            } => {
                let mut changed = true;
                match key.code {
                    KeyCode::Esc => {
                        self.modal = None;
                        return true;
                    }
                    KeyCode::Tab | KeyCode::BackTab => {
                        mode = mode.toggle();
                    }
                    KeyCode::Up | KeyCode::Char('p') if key.code == KeyCode::Up || has_ctrl => {
                        if !results.is_empty() {
                            selected = if selected == 0 {
                                results.len() - 1
                            } else {
                                selected - 1
                            };
                            preview = results
                                .get(selected)
                                .map(|item| build_search_preview(&self.workspace_root, item, 20));
                        }
                        changed = false;
                    }
                    KeyCode::Down | KeyCode::Char('n') if key.code == KeyCode::Down || has_ctrl => {
                        if !results.is_empty() {
                            selected = (selected + 1) % results.len();
                            preview = results
                                .get(selected)
                                .map(|item| build_search_preview(&self.workspace_root, item, 20));
                        }
                        changed = false;
                    }
                    KeyCode::PageDown | KeyCode::Char('d')
                        if key.code == KeyCode::PageDown || has_ctrl =>
                    {
                        if let Some(prev) = preview.as_mut() {
                            let step = (self.tab().viewport_height / 2).max(5);
                            let max_scroll = prev.lines.len().saturating_sub(6);
                            prev.scroll = prev.scroll.saturating_add(step).min(max_scroll);
                        }
                        changed = false;
                    }
                    KeyCode::PageUp => {
                        if let Some(prev) = preview.as_mut() {
                            let step = (self.tab().viewport_height / 2).max(5);
                            prev.scroll = prev.scroll.saturating_sub(step);
                        }
                        changed = false;
                    }
                    _ => match handle_text_key(&mut query, key, 56, false, false) {
                        TextAction::Submit => {
                            if let Some(item) = results.get(selected) {
                                let snippet = match item.line {
                                    Some(line) => format!("@{}#L{line} ", item.path),
                                    None => format!("@{} ", item.path),
                                };
                                self.insert_str(&snippet);
                                self.mode = InputMode::Insert;
                                self.completion.reset();
                                self.modal = None;
                                return true;
                            }
                        }
                        TextAction::Edited => {}
                        TextAction::Ignored => changed = false,
                    },
                }
                if key.code == KeyCode::Tab || key.code == KeyCode::BackTab || changed {
                    Self::refresh_search(
                        SearchSources {
                            workspace: self.workspace.as_ref(),
                            workspace_root: &self.workspace_root,
                            mode,
                            workspace_files: &workspace_files,
                            query: &query,
                        },
                        SearchOutput {
                            results: &mut results,
                            selected: &mut selected,
                            preview: &mut preview,
                        },
                    );
                }
                self.modal = Some(ActiveModal::Search {
                    query,
                    mode,
                    workspace_files,
                    results,
                    selected,
                    preview,
                });
                true
            }
            ActiveModal::Runs { kind, mut selected } => {
                let count = self.run_count(kind);
                match key.code {
                    KeyCode::Esc => {
                        self.modal = None;
                        return true;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        selected = selected.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if count > 0 {
                            selected = (selected + 1).min(count - 1);
                        }
                    }
                    KeyCode::Enter => {
                        self.open_run_detail(kind, selected);
                        return true;
                    }
                    _ => {}
                }
                self.modal = Some(ActiveModal::Runs { kind, selected });
                true
            }
            ActiveModal::RunDetail {
                run,
                selected,
                mut scroll,
            } => {
                let max = self.run_detail_max_scroll;
                match key.code {
                    KeyCode::Esc => {
                        match &run {
                            RunTarget::Subagent(_) => self.modal = None,
                            RunTarget::Shell(_) => {
                                self.modal = Some(ActiveModal::Runs {
                                    kind: run.kind(),
                                    selected,
                                });
                            }
                        }
                        return true;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        scroll = scroll.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        scroll = (scroll + 1).min(max);
                    }
                    KeyCode::PageUp => {
                        scroll = scroll.saturating_sub(10);
                    }
                    KeyCode::PageDown => {
                        scroll = (scroll + 10).min(max);
                    }
                    KeyCode::Home | KeyCode::Char('g') => {
                        scroll = 0;
                    }
                    KeyCode::End | KeyCode::Char('G') => {
                        scroll = max;
                    }
                    _ => {}
                }
                self.modal = Some(ActiveModal::RunDetail {
                    run,
                    selected,
                    scroll,
                });
                true
            }
            ActiveModal::RewindMenu {
                turn_id,
                mut selected,
            } => {
                match key.code {
                    KeyCode::Esc => {
                        self.modal = None;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        selected = selected.saturating_sub(1);
                        self.modal = Some(ActiveModal::RewindMenu { turn_id, selected });
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        selected = (selected + 1).min(1);
                        self.modal = Some(ActiveModal::RewindMenu { turn_id, selected });
                    }
                    KeyCode::Enter => {
                        self.apply_rewind_choice(turn_id, selected);
                        return true;
                    }
                    _ => {
                        self.modal = Some(ActiveModal::RewindMenu { turn_id, selected });
                    }
                }
                true
            }
            ActiveModal::SessionSelect {
                mut filter,
                mut selected,
                sessions,
                mut loading,
                mut error,
            } => {
                let filtered_ids: Vec<(meerai_session::SessionId, Option<usize>)> =
                    filter_session_summaries(&sessions, &filter)
                        .into_iter()
                        .map(|summary| {
                            let open_tab = self.tab_index_for_session(&summary.id);
                            (summary.id.clone(), open_tab)
                        })
                        .collect();

                match key.code {
                    KeyCode::Esc => {
                        self.session_delete_confirm = None;
                        self.modal = None;
                    }
                    KeyCode::Up | KeyCode::Char('p') if key.code == KeyCode::Up || has_ctrl => {
                        if !filtered_ids.is_empty() {
                            selected = if selected == 0 {
                                filtered_ids.len() - 1
                            } else {
                                selected - 1
                            };
                        }
                        self.modal = Some(ActiveModal::SessionSelect {
                            filter,
                            selected,
                            sessions,
                            loading,
                            error,
                        });
                    }
                    KeyCode::Down | KeyCode::Char('n') if key.code == KeyCode::Down || has_ctrl => {
                        if !filtered_ids.is_empty() {
                            selected = (selected + 1) % filtered_ids.len();
                        }
                        self.modal = Some(ActiveModal::SessionSelect {
                            filter,
                            selected,
                            sessions,
                            loading,
                            error,
                        });
                    }
                    KeyCode::Char('d') if has_ctrl => {
                        if let Some((session_id, _)) = filtered_ids.get(selected) {
                            if self.session_delete_confirm.as_ref() == Some(session_id) {
                                self.session_delete_confirm = None;
                                loading = true;
                                error = None;
                                self.pending_session_actions.push(
                                    PendingSessionAction::DeleteSession {
                                        session_id: session_id.clone(),
                                    },
                                );
                            } else {
                                self.session_delete_confirm = Some(session_id.clone());
                                error = None;
                            }
                        }
                        self.modal = Some(ActiveModal::SessionSelect {
                            filter,
                            selected,
                            sessions,
                            loading,
                            error,
                        });
                    }
                    _ => match handle_text_key(&mut filter, key, 48, false, false) {
                        TextAction::Submit => {
                            if let Some((session_id, open_tab)) =
                                filtered_ids.get(selected).cloned()
                            {
                                self.modal = None;
                                if let Some(tab_idx) = open_tab {
                                    self.switch_to_tab(tab_idx);
                                    self.status_notice =
                                        Some(format!("Switched to tab {}", tab_idx + 1));
                                } else if self.session_opener.is_some() {
                                    self.status_notice =
                                        Some(format!("Opening session {session_id}..."));
                                    self.pending_session_actions
                                        .push(PendingSessionAction::OpenSessionId(session_id));
                                } else {
                                    self.tab_mut().transcript.push(TranscriptEntry::Error {
                                        text: "Session switching is not available in this context."
                                            .to_string(),
                                        expanded: false,
                                    });
                                }
                            } else {
                                self.modal = Some(ActiveModal::SessionSelect {
                                    filter,
                                    selected,
                                    sessions,
                                    loading,
                                    error,
                                });
                            }
                        }
                        TextAction::Edited => {
                            selected = 0;
                            self.session_delete_confirm = None;
                            self.modal = Some(ActiveModal::SessionSelect {
                                filter,
                                selected,
                                sessions,
                                loading,
                                error,
                            });
                        }
                        TextAction::Ignored => {
                            self.modal = Some(ActiveModal::SessionSelect {
                                filter,
                                selected,
                                sessions,
                                loading,
                                error,
                            });
                        }
                    },
                }
                true
            }
            ActiveModal::ServeDialog {
                mut port_input,
                mut secret_input,
                mut editing_secret,
                mut error,
                busy,
            } => {
                if busy {
                    self.modal = Some(ActiveModal::ServeDialog {
                        port_input,
                        secret_input,
                        editing_secret,
                        error,
                        busy,
                    });
                    return true;
                }
                match key.code {
                    KeyCode::Esc => {
                        self.modal = None;
                    }
                    KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => {
                        editing_secret = !editing_secret;
                        self.modal = Some(ActiveModal::ServeDialog {
                            port_input,
                            secret_input,
                            editing_secret,
                            error,
                            busy: false,
                        });
                    }
                    KeyCode::Char('u') if has_ctrl => {
                        if editing_secret {
                            secret_input.clear();
                        } else {
                            port_input.clear();
                        }
                        error = None;
                        self.modal = Some(ActiveModal::ServeDialog {
                            port_input,
                            secret_input,
                            editing_secret,
                            error,
                            busy: false,
                        });
                    }
                    _ => {
                        let action = if editing_secret {
                            let action = handle_text_key(&mut secret_input, key, 40, false, false);
                            secret_input.text.retain(|c| !c.is_control());
                            secret_input.cursor = secret_input.cursor.min(secret_input.text.len());
                            action
                        } else {
                            let action = handle_text_key(&mut port_input, key, 12, false, false);
                            port_input.text.retain(|c| c.is_ascii_digit());
                            port_input.cursor = port_input.cursor.min(port_input.text.len());
                            action
                        };
                        match action {
                            TextAction::Submit => {
                                let port_trimmed = port_input.trim();
                                let parsed_port =
                                    if port_trimmed.is_empty() {
                                        Ok(None)
                                    } else {
                                        match port_trimmed.parse::<u16>() {
                                            Ok(p) if p > 0 => Ok(Some(p)),
                                            _ => Err("Port must be a number between 1 and 65535"
                                                .to_string()),
                                        }
                                    };
                                let secret_opt = if secret_input.is_empty() {
                                    Ok(None)
                                } else if secret_input.chars().count() < 8 {
                                    Err("Password must be at least 8 characters (or leave blank for default)"
                                        .to_string())
                                } else if secret_input.chars().any(char::is_control)
                                    || secret_input.trim().is_empty()
                                {
                                    Err(
                                        "Password cannot be whitespace or contain control characters"
                                            .to_string(),
                                    )
                                } else {
                                    Ok(Some(secret_input.text.clone()))
                                };
                                match (parsed_port, secret_opt) {
                                    (Ok(port), Ok(secret)) => {
                                        self.pending_session_actions.push(
                                            PendingSessionAction::StartWebServer { port, secret },
                                        );
                                        self.status_notice =
                                            Some("Starting web UI server...".to_string());
                                        self.modal = Some(ActiveModal::ServeDialog {
                                            port_input,
                                            secret_input: TextInput::default(),
                                            editing_secret,
                                            error: None,
                                            busy: true,
                                        });
                                    }
                                    (Err(msg), _) | (_, Err(msg)) => {
                                        self.modal = Some(ActiveModal::ServeDialog {
                                            port_input,
                                            secret_input,
                                            editing_secret,
                                            error: Some(msg),
                                            busy: false,
                                        });
                                    }
                                }
                            }
                            TextAction::Edited => {
                                error = None;
                                self.modal = Some(ActiveModal::ServeDialog {
                                    port_input,
                                    secret_input,
                                    editing_secret,
                                    error,
                                    busy: false,
                                });
                            }
                            TextAction::Ignored => {
                                self.modal = Some(ActiveModal::ServeDialog {
                                    port_input,
                                    secret_input,
                                    editing_secret,
                                    error,
                                    busy: false,
                                });
                            }
                        }
                    }
                }
                true
            }
            _ => false,
        }
    }
}
