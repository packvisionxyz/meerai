use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use meerai_agents::AgentCommand;
use meerai_agents::events::{ApprovalDecision, GrantScope, QuestionAnswer, RewindScope};
use meerai_client::providers::canonical_provider_id;
use meerai_common::settings::save_settings_to;

use super::*;
use crate::diff::build_file_diff;
use crate::search::{
    SearchMatch, SearchMode, SearchPreview, build_search_preview, search_content, search_files,
};

impl AppState {
    pub fn open_diff_modal(&mut self, file_index: usize) {
        let Some(entry) = self.file_changes.get(file_index) else {
            return;
        };
        let diff = build_file_diff(&self.workspace_root, &entry.path, &entry.status);
        self.modal = Some(ActiveModal::DiffView {
            file_index,
            diff,
            scroll: 0,
            list: DiffListState::Closed,
        });
    }

    pub fn open_search_modal(&mut self, mode: SearchMode, initial_query: &str) {
        let workspace_files = self.workspace.list_files();
        let query = initial_query.trim().to_string();
        let mut results = Vec::new();
        let mut selected = 0;
        let mut preview = None;
        Self::refresh_search(
            self.workspace.as_ref(),
            &self.workspace_root,
            mode,
            &workspace_files,
            &query,
            &mut results,
            &mut selected,
            &mut preview,
        );
        self.modal = Some(ActiveModal::Search {
            query,
            mode,
            workspace_files,
            results,
            selected,
            preview,
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn refresh_search(
        workspace: &dyn crate::workspace::WorkspaceQuery,
        workspace_root: &Path,
        mode: SearchMode,
        workspace_files: &[String],
        query: &str,
        results: &mut Vec<SearchMatch>,
        selected: &mut usize,
        preview: &mut Option<SearchPreview>,
    ) {
        *results = match mode {
            SearchMode::Files => search_files(workspace_files, query),
            SearchMode::Content => search_content(workspace, workspace_files, query),
        };
        *selected = 0;
        *preview = results
            .first()
            .map(|item| build_search_preview(workspace_root, item, 20));
    }

    pub(super) fn handle_modal_key(&mut self, key: KeyEvent) -> bool {
        let Some(modal) = self.modal.take() else {
            return false;
        };
        let has_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let has_alt = key.modifiers.contains(KeyModifiers::ALT);

        match modal {
            ActiveModal::ProviderSelect {
                mut filter,
                mut selected,
            } => {
                let providers = self.filtered_providers(&filter);
                match key.code {
                    KeyCode::Esc => {
                        self.modal = None;
                    }
                    KeyCode::Up | KeyCode::Char('p') if key.code == KeyCode::Up || has_ctrl => {
                        if !providers.is_empty() {
                            selected = if selected == 0 {
                                providers.len() - 1
                            } else {
                                selected - 1
                            };
                        }
                        self.modal = Some(ActiveModal::ProviderSelect { filter, selected });
                    }
                    KeyCode::Down | KeyCode::Char('n') if key.code == KeyCode::Down || has_ctrl => {
                        if !providers.is_empty() {
                            selected = (selected + 1) % providers.len();
                        }
                        self.modal = Some(ActiveModal::ProviderSelect { filter, selected });
                    }
                    KeyCode::Backspace => {
                        filter.pop();
                        selected = 0;
                        self.modal = Some(ActiveModal::ProviderSelect { filter, selected });
                    }
                    KeyCode::Enter => {
                        if let Some((provider, status)) = providers.get(selected) {
                            if !provider.requires_api_key {
                                self.activate_provider_only(&provider.id);
                                self.open_model_input();
                            } else {
                                let saved_url = {
                                    let rt = self.runtime.read().unwrap();
                                    rt.base_urls.get(&provider.id).cloned().unwrap_or_default()
                                };
                                self.modal = Some(ActiveModal::ApiKeyInput {
                                    provider_id: provider.id.clone(),
                                    provider_name: provider.display_name.clone(),
                                    api_key: String::new(),
                                    base_url: saved_url,
                                    editing_base_url: false,
                                    existing_key_status: status.clone(),
                                    error: None,
                                });
                            }
                        } else {
                            self.modal = Some(ActiveModal::ProviderSelect { filter, selected });
                        }
                    }
                    KeyCode::Char(c) if !has_ctrl && !has_alt => {
                        filter.push(c);
                        selected = 0;
                        self.modal = Some(ActiveModal::ProviderSelect { filter, selected });
                    }
                    _ => {
                        self.modal = Some(ActiveModal::ProviderSelect { filter, selected });
                    }
                }
            }
            ActiveModal::ApiKeyInput {
                provider_id,
                provider_name,
                mut api_key,
                mut base_url,
                mut editing_base_url,
                mut existing_key_status,
                mut error,
            } => match key.code {
                KeyCode::Esc => {
                    let selected = {
                        let rt = self.runtime.read().unwrap();
                        rt.catalog
                            .providers()
                            .iter()
                            .position(|p| p.id == provider_id)
                            .unwrap_or(0)
                    };
                    self.modal = Some(ActiveModal::ProviderSelect {
                        filter: String::new(),
                        selected,
                    });
                }
                KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => {
                    editing_base_url = !editing_base_url;
                    self.modal = Some(ActiveModal::ApiKeyInput {
                        provider_id,
                        provider_name,
                        api_key,
                        base_url,
                        editing_base_url,
                        existing_key_status,
                        error,
                    });
                }
                KeyCode::Char('d') if has_ctrl => {
                    match self.secrets.delete_key(&provider_id) {
                        Ok(()) => {
                            api_key.clear();
                            self.invalidate_key_status();
                            let provider_info = {
                                let rt = self.runtime.read().unwrap();
                                rt.catalog.provider(&provider_id).cloned()
                            };
                            if let Some(info) = provider_info {
                                existing_key_status = self.cached_key_status(&info);
                            } else {
                                existing_key_status = KeyStatus::Missing;
                            }
                            error = None;
                            self.status_notice =
                                Some(format!("Removed stored key for {provider_id}"));
                        }
                        Err(err) => {
                            error = Some(format!("{err:#}"));
                        }
                    }
                    self.modal = Some(ActiveModal::ApiKeyInput {
                        provider_id,
                        provider_name,
                        api_key,
                        base_url,
                        editing_base_url,
                        existing_key_status,
                        error,
                    });
                }
                KeyCode::Backspace => {
                    if editing_base_url {
                        base_url.pop();
                    } else {
                        api_key.pop();
                    }
                    error = None;
                    self.modal = Some(ActiveModal::ApiKeyInput {
                        provider_id,
                        provider_name,
                        api_key,
                        base_url,
                        editing_base_url,
                        existing_key_status,
                        error,
                    });
                }
                KeyCode::Enter => {
                    let trimmed_key = api_key.trim();
                    if !trimmed_key.is_empty() {
                        if let Err(err) = self.secrets.set_key(&provider_id, trimmed_key) {
                            self.modal = Some(ActiveModal::ApiKeyInput {
                                provider_id,
                                provider_name,
                                api_key,
                                base_url,
                                editing_base_url,
                                existing_key_status,
                                error: Some(format!("{err:#}")),
                            });
                            return true;
                        }
                        self.invalidate_key_status();
                    } else if !existing_key_status.is_configured() {
                        self.modal = Some(ActiveModal::ApiKeyInput {
                            provider_id,
                            provider_name,
                            api_key,
                            base_url,
                            editing_base_url,
                            existing_key_status,
                            error: Some("API key is required".to_string()),
                        });
                        return true;
                    }

                    let clean_url = base_url.trim().to_string();
                    {
                        let mut rt = self.runtime.write().unwrap();
                        if clean_url.is_empty() {
                            rt.base_urls.remove(&provider_id);
                        } else {
                            rt.base_urls.insert(provider_id.clone(), clean_url.clone());
                        }
                    }
                    self.settings.set_provider_base_url(
                        &provider_id,
                        (!clean_url.is_empty()).then_some(clean_url),
                    );

                    self.activate_provider_only(&provider_id);
                    self.status_notice = Some(format!("Connected {provider_id}"));
                    self.open_model_input();
                }
                KeyCode::Char(c) if !has_ctrl && !has_alt => {
                    if editing_base_url {
                        base_url.push(c);
                    } else {
                        api_key.push(c);
                    }
                    error = None;
                    self.modal = Some(ActiveModal::ApiKeyInput {
                        provider_id,
                        provider_name,
                        api_key,
                        base_url,
                        editing_base_url,
                        existing_key_status,
                        error,
                    });
                }
                _ => {
                    self.modal = Some(ActiveModal::ApiKeyInput {
                        provider_id,
                        provider_name,
                        api_key,
                        base_url,
                        editing_base_url,
                        existing_key_status,
                        error,
                    });
                }
            },
            ActiveModal::ModelInput {
                mut input,
                mut selected_recent,
                mut error,
            } => {
                let recent = self.recent_models_for_active_provider();
                match key.code {
                    KeyCode::Esc => {
                        self.modal = None;
                    }
                    KeyCode::Char('u') if has_ctrl => {
                        input.clear();
                        selected_recent = None;
                        error = None;
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected_recent,
                            error,
                        });
                    }
                    KeyCode::Up | KeyCode::Char('p') if key.code == KeyCode::Up || has_ctrl => {
                        if !recent.is_empty() {
                            let next = match selected_recent {
                                None | Some(0) => recent.len() - 1,
                                Some(idx) => idx.saturating_sub(1),
                            };
                            input = recent[next].clone();
                            selected_recent = Some(next);
                            error = None;
                        }
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected_recent,
                            error,
                        });
                    }
                    KeyCode::Down | KeyCode::Char('n') if key.code == KeyCode::Down || has_ctrl => {
                        if !recent.is_empty() {
                            let next = match selected_recent {
                                None => 0,
                                Some(idx) => (idx + 1) % recent.len(),
                            };
                            input = recent[next].clone();
                            selected_recent = Some(next);
                            error = None;
                        }
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected_recent,
                            error,
                        });
                    }
                    KeyCode::Backspace => {
                        input.pop();
                        selected_recent = None;
                        error = None;
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected_recent,
                            error,
                        });
                    }
                    KeyCode::Enter => {
                        let candidate = input.trim();
                        if !candidate.is_empty() {
                            let (target_provider, target_model) =
                                self.resolve_model_target(candidate);
                            if target_model.is_empty() {
                                self.modal = Some(ActiveModal::ModelInput {
                                    input,
                                    selected_recent,
                                    error: Some("Model name is required".to_string()),
                                });
                            } else {
                                self.select_provider_and_model(&target_provider, &target_model);
                                self.modal = None;
                            }
                        } else if let Some(existing) = {
                            let provider = self.tab().active_provider.clone();
                            self.current_or_default_model(&provider)
                        } {
                            let provider = self.tab().active_provider.clone();
                            self.select_provider_and_model(&provider, &existing);
                            self.modal = None;
                        } else {
                            self.modal = Some(ActiveModal::ModelInput {
                                input,
                                selected_recent,
                                error: Some("Model name is required".to_string()),
                            });
                        }
                    }
                    KeyCode::Char(c) if !has_ctrl && !has_alt => {
                        input.push(c);
                        selected_recent = None;
                        error = None;
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected_recent,
                            error,
                        });
                    }
                    _ => {
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected_recent,
                            error,
                        });
                    }
                }
            }
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
            }
            ActiveModal::Search {
                mut query,
                mut mode,
                workspace_files,
                mut results,
                mut selected,
                mut preview,
            } => match key.code {
                KeyCode::Esc => {
                    self.modal = None;
                }
                KeyCode::Tab | KeyCode::BackTab => {
                    mode = mode.toggle();
                    Self::refresh_search(
                        self.workspace.as_ref(),
                        &self.workspace_root,
                        mode,
                        &workspace_files,
                        &query,
                        &mut results,
                        &mut selected,
                        &mut preview,
                    );
                    self.modal = Some(ActiveModal::Search {
                        query,
                        mode,
                        workspace_files,
                        results,
                        selected,
                        preview,
                    });
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
                    self.modal = Some(ActiveModal::Search {
                        query,
                        mode,
                        workspace_files,
                        results,
                        selected,
                        preview,
                    });
                }
                KeyCode::Down | KeyCode::Char('n') if key.code == KeyCode::Down || has_ctrl => {
                    if !results.is_empty() {
                        selected = (selected + 1) % results.len();
                        preview = results
                            .get(selected)
                            .map(|item| build_search_preview(&self.workspace_root, item, 20));
                    }
                    self.modal = Some(ActiveModal::Search {
                        query,
                        mode,
                        workspace_files,
                        results,
                        selected,
                        preview,
                    });
                }
                KeyCode::PageDown | KeyCode::Char('d')
                    if key.code == KeyCode::PageDown || has_ctrl =>
                {
                    if let Some(prev) = preview.as_mut() {
                        let step = (self.tab().viewport_height / 2).max(5);
                        let max_scroll = prev.lines.len().saturating_sub(6);
                        prev.scroll = prev.scroll.saturating_add(step).min(max_scroll);
                    }
                    self.modal = Some(ActiveModal::Search {
                        query,
                        mode,
                        workspace_files,
                        results,
                        selected,
                        preview,
                    });
                }
                KeyCode::PageUp => {
                    if let Some(prev) = preview.as_mut() {
                        let step = (self.tab().viewport_height / 2).max(5);
                        prev.scroll = prev.scroll.saturating_sub(step);
                    }
                    self.modal = Some(ActiveModal::Search {
                        query,
                        mode,
                        workspace_files,
                        results,
                        selected,
                        preview,
                    });
                }
                KeyCode::Char('u') if has_ctrl => {
                    query.clear();
                    Self::refresh_search(
                        self.workspace.as_ref(),
                        &self.workspace_root,
                        mode,
                        &workspace_files,
                        &query,
                        &mut results,
                        &mut selected,
                        &mut preview,
                    );
                    self.modal = Some(ActiveModal::Search {
                        query,
                        mode,
                        workspace_files,
                        results,
                        selected,
                        preview,
                    });
                }
                KeyCode::Backspace => {
                    query.pop();
                    Self::refresh_search(
                        self.workspace.as_ref(),
                        &self.workspace_root,
                        mode,
                        &workspace_files,
                        &query,
                        &mut results,
                        &mut selected,
                        &mut preview,
                    );
                    self.modal = Some(ActiveModal::Search {
                        query,
                        mode,
                        workspace_files,
                        results,
                        selected,
                        preview,
                    });
                }
                KeyCode::Enter => {
                    if let Some(item) = results.get(selected) {
                        let snippet = match item.line {
                            Some(line) => format!("@{}#L{line} ", item.path),
                            None => format!("@{} ", item.path),
                        };
                        self.insert_str(&snippet);
                        self.mode = InputMode::Insert;
                        self.completion.reset();
                        self.modal = None;
                    } else {
                        self.modal = Some(ActiveModal::Search {
                            query,
                            mode,
                            workspace_files,
                            results,
                            selected,
                            preview,
                        });
                    }
                }
                KeyCode::Char(c) if !has_ctrl && !has_alt => {
                    query.push(c);
                    Self::refresh_search(
                        self.workspace.as_ref(),
                        &self.workspace_root,
                        mode,
                        &workspace_files,
                        &query,
                        &mut results,
                        &mut selected,
                        &mut preview,
                    );
                    self.modal = Some(ActiveModal::Search {
                        query,
                        mode,
                        workspace_files,
                        results,
                        selected,
                        preview,
                    });
                }
                _ => {
                    self.modal = Some(ActiveModal::Search {
                        query,
                        mode,
                        workspace_files,
                        results,
                        selected,
                        preview,
                    });
                }
            },
            ActiveModal::Approval {
                queue,
                mut reason,
                mut editing_reason,
            } => {
                let request_id = queue.first().map(|request| request.request_id.clone());
                if editing_reason {
                    match key.code {
                        KeyCode::Esc => {
                            reason.clear();
                            editing_reason = false;
                        }
                        KeyCode::Enter => {
                            if let Some(request_id) = request_id {
                                let text = reason.trim().to_string();
                                let decision = if text.is_empty() {
                                    ApprovalDecision::Decline
                                } else {
                                    ApprovalDecision::DeclineWithReason(text)
                                };
                                let _ = self.tab().cmd_tx.send(AgentCommand::Approve {
                                    request_id,
                                    decision,
                                    scope: None,
                                });
                            }
                            reason.clear();
                            editing_reason = false;
                        }
                        KeyCode::Backspace => {
                            reason.pop();
                        }
                        KeyCode::Char(c) if !has_ctrl && !has_alt => {
                            reason.push(c);
                        }
                        _ => {}
                    }
                } else {
                    let action = match key.code {
                        KeyCode::Char('y') | KeyCode::Enter => {
                            Some((ApprovalDecision::Accept, None))
                        }
                        KeyCode::Char('t') => {
                            Some((ApprovalDecision::Accept, Some(GrantScope::Turn)))
                        }
                        KeyCode::Char('a') => Some((ApprovalDecision::AcceptForSession, None)),
                        KeyCode::Char('n') => Some((ApprovalDecision::Decline, None)),
                        KeyCode::Char('r') => {
                            reason.clear();
                            editing_reason = true;
                            None
                        }
                        KeyCode::Esc => Some((ApprovalDecision::Cancel, None)),
                        _ => None,
                    };
                    if let Some((decision, scope)) = action
                        && let Some(request_id) = request_id
                    {
                        let _ = self.tab().cmd_tx.send(AgentCommand::Approve {
                            request_id,
                            decision,
                            scope,
                        });
                    }
                }
                self.modal = Some(ActiveModal::Approval {
                    queue,
                    reason,
                    editing_reason,
                });
            }
            ActiveModal::Terminal {
                output, mut scroll, ..
            } => {
                let max = output
                    .as_deref()
                    .map(|body| body.lines().count().saturating_sub(1))
                    .unwrap_or(0);
                match key.code {
                    KeyCode::Esc => {
                        self.modal = None;
                        self.status_notice =
                            Some("Shell running in background; output will appear here".into());
                        return true;
                    }
                    KeyCode::Char('q') => {
                        self.modal = None;
                        return true;
                    }
                    KeyCode::Char('b') if output.is_some() => {
                        self.modal = None;
                        return true;
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        scroll = (scroll + 1).min(max);
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        scroll = scroll.saturating_sub(1);
                    }
                    KeyCode::PageDown | KeyCode::Char('d') if key.code == KeyCode::PageDown => {
                        scroll = (scroll + 10).min(max);
                    }
                    KeyCode::PageUp | KeyCode::Char('u') if key.code == KeyCode::PageUp => {
                        scroll = scroll.saturating_sub(10);
                    }
                    _ => {}
                }
                if let Some(ActiveModal::Terminal {
                    command,
                    output,
                    running,
                    ..
                }) = self.modal.take()
                {
                    self.modal = Some(ActiveModal::Terminal {
                        command,
                        output,
                        running,
                        scroll,
                    });
                }
            }
            ActiveModal::Question {
                request,
                mut answers,
                mut custom,
                mut editing_custom,
                mut current,
                mut focus,
            } => {
                let prompt = &request.questions[current];
                let last = current + 1 >= request.questions.len();
                let other = prompt.options.len();
                let multiple = prompt.multiple;
                match key.code {
                    KeyCode::Esc => {
                        self.record_question_answer(&request.request_id, None);
                        let _ = self.tab().cmd_tx.send(AgentCommand::AnswerQuestion {
                            request_id: request.request_id.clone(),
                            answers: None,
                        });
                        self.modal = None;
                        self.open_pending_question();
                        return true;
                    }
                    KeyCode::Up => {
                        editing_custom = false;
                        focus = focus.saturating_sub(1);
                    }
                    KeyCode::Down => {
                        editing_custom = false;
                        focus = (focus + 1).min(other);
                    }
                    KeyCode::Char('k') if !editing_custom => {
                        editing_custom = false;
                        focus = focus.saturating_sub(1);
                    }
                    KeyCode::Char('j') if !editing_custom => {
                        editing_custom = false;
                        focus = (focus + 1).min(other);
                    }
                    KeyCode::Char(' ') if focus < other => {
                        if multiple {
                            if let Some(index) = answers[current].iter().position(|i| *i == focus) {
                                answers[current].remove(index);
                            } else {
                                answers[current].push(focus);
                                answers[current].sort_unstable();
                            }
                        } else {
                            answers[current] = vec![focus];
                            custom[current].clear();
                        }
                    }
                    KeyCode::Char(' ') => {
                        if editing_custom {
                            if !multiple {
                                answers[current].clear();
                            }
                            custom[current].push(' ');
                        } else {
                            editing_custom = true;
                        }
                    }
                    KeyCode::Char(c) if focus == other => {
                        editing_custom = true;
                        if !multiple {
                            answers[current].clear();
                        }
                        custom[current].push(c);
                    }
                    KeyCode::Backspace if focus == other => {
                        editing_custom = true;
                        custom[current].pop();
                    }
                    KeyCode::Tab | KeyCode::Right => {
                        if !last {
                            current += 1;
                            focus = 0;
                            editing_custom = false;
                        }
                    }
                    KeyCode::Left => {
                        if current > 0 {
                            current -= 1;
                            focus = 0;
                            editing_custom = false;
                        }
                    }
                    KeyCode::Char(c @ '1'..='9') if focus < other => {
                        let index = c.to_digit(10).unwrap_or(1) as usize - 1;
                        if index < prompt.options.len() {
                            focus = index;
                            if multiple {
                                if !answers[current].contains(&index) {
                                    answers[current].push(index);
                                    answers[current].sort_unstable();
                                }
                            } else {
                                answers[current] = vec![index];
                                custom[current].clear();
                            }
                        }
                    }
                    KeyCode::Enter if focus == other && !editing_custom => {
                        editing_custom = true;
                    }
                    KeyCode::Enter if focus == other && custom[current].trim().is_empty() => {}
                    KeyCode::Enter => {
                        if focus < other && answers[current].is_empty() {
                            answers[current] = vec![focus];
                        }
                        if last {
                            let collected = collect_question_answers(&request, &answers, &custom);
                            self.record_question_answer(
                                &request.request_id,
                                Some(collected.clone()),
                            );
                            let _ = self.tab().cmd_tx.send(AgentCommand::AnswerQuestion {
                                request_id: request.request_id.clone(),
                                answers: Some(collected),
                            });
                            self.modal = None;
                            self.open_pending_question();
                            return true;
                        }
                        current += 1;
                        focus = 0;
                        editing_custom = false;
                    }
                    _ => {}
                }
                self.modal = Some(ActiveModal::Question {
                    request,
                    answers,
                    custom,
                    editing_custom,
                    current,
                    focus,
                });
            }
            ActiveModal::RewindMenu {
                turn_id,
                mut selected,
            } => match key.code {
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
            },
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
                    KeyCode::Backspace => {
                        filter.pop();
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
                    KeyCode::Enter => {
                        if let Some((session_id, open_tab)) = filtered_ids.get(selected).cloned() {
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
                    KeyCode::Char(c) if !has_ctrl && !has_alt => {
                        filter.push(c);
                        selected = 0;
                        self.modal = Some(ActiveModal::SessionSelect {
                            filter,
                            selected,
                            sessions,
                            loading,
                            error,
                        });
                    }
                    _ => {
                        self.modal = Some(ActiveModal::SessionSelect {
                            filter,
                            selected,
                            sessions,
                            loading,
                            error,
                        });
                    }
                }
            }
            ActiveModal::UserMessageActions {
                entry_idx,
                mut selected,
            } => {
                let actions = [UserAction::Rewind, UserAction::Copy, UserAction::Fork];
                let run = |state: &mut Self, action| {
                    state.modal = None;
                    state.run_user_action(entry_idx, action);
                };
                match key.code {
                    KeyCode::Esc => {
                        self.modal = None;
                    }
                    KeyCode::Up | KeyCode::Char('p') if key.code == KeyCode::Up || has_ctrl => {
                        selected = if selected == 0 { 2 } else { selected - 1 };
                        self.modal = Some(ActiveModal::UserMessageActions {
                            entry_idx,
                            selected,
                        });
                    }
                    KeyCode::Down | KeyCode::Char('n') if key.code == KeyCode::Down || has_ctrl => {
                        selected = (selected + 1) % 3;
                        self.modal = Some(ActiveModal::UserMessageActions {
                            entry_idx,
                            selected,
                        });
                    }
                    KeyCode::Enter => run(self, actions[selected.min(2)]),
                    KeyCode::Char('w') if !has_ctrl => run(self, UserAction::Rewind),
                    KeyCode::Char('c') if !has_ctrl => run(self, UserAction::Copy),
                    KeyCode::Char('f') if !has_ctrl => run(self, UserAction::Fork),
                    _ => {
                        self.modal = Some(ActiveModal::UserMessageActions {
                            entry_idx,
                            selected,
                        });
                    }
                }
            }
            ActiveModal::CommandPalette {
                mut filter,
                mut selected,
            } => {
                let commands = filtered_commands(&filter);
                match key.code {
                    KeyCode::Esc => {
                        self.modal = None;
                    }
                    KeyCode::Up | KeyCode::Char('p') if key.code == KeyCode::Up || has_ctrl => {
                        if !commands.is_empty() {
                            selected = if selected == 0 {
                                commands.len() - 1
                            } else {
                                selected - 1
                            };
                        }
                        self.modal = Some(ActiveModal::CommandPalette { filter, selected });
                    }
                    KeyCode::Down | KeyCode::Char('n') if key.code == KeyCode::Down || has_ctrl => {
                        if !commands.is_empty() {
                            selected = (selected + 1) % commands.len();
                        }
                        self.modal = Some(ActiveModal::CommandPalette { filter, selected });
                    }
                    KeyCode::Backspace => {
                        filter.pop();
                        selected = 0;
                        self.modal = Some(ActiveModal::CommandPalette { filter, selected });
                    }
                    KeyCode::Enter => {
                        if let Some(command) = commands.get(selected) {
                            let action = command.action;
                            self.modal = None;
                            self.run_palette_action(action);
                        } else {
                            self.modal = Some(ActiveModal::CommandPalette { filter, selected });
                        }
                    }
                    KeyCode::Char(c) if !has_ctrl && !has_alt => {
                        filter.push(c);
                        selected = 0;
                        self.modal = Some(ActiveModal::CommandPalette { filter, selected });
                    }
                    _ => {
                        self.modal = Some(ActiveModal::CommandPalette { filter, selected });
                    }
                }
            }
        }

        true
    }

    pub(super) fn modal_hit_at(&self, col: u16, row: u16) -> Option<ModalHitKind> {
        self.modal_hit_zones
            .iter()
            .rev()
            .find(|zone| {
                let r = zone.rect;
                r.width > 0
                    && r.height > 0
                    && col >= r.x
                    && col < r.x + r.width
                    && row >= r.y
                    && row < r.y + r.height
            })
            .map(|zone| zone.kind)
    }

    pub(super) fn handle_modal_mouse_move(&mut self, col: u16, row: u16) -> bool {
        let hit = self.modal_hit_at(col, row);
        if hit == self.hovered_modal_hit {
            return false;
        }
        self.hovered_modal_hit = hit;
        true
    }

    pub(super) fn handle_modal_click(&mut self, col: u16, row: u16) -> bool {
        let Some(hit) = self.modal_hit_at(col, row) else {
            return false;
        };
        let Some(modal) = self.modal.take() else {
            return false;
        };
        match (modal, hit) {
            (ActiveModal::ProviderSelect { filter, .. }, ModalHitKind::Close) => {
                let _ = filter;
                self.modal = None;
            }
            (ActiveModal::ProviderSelect { filter, .. }, ModalHitKind::ListItem(idx)) => {
                self.activate_provider_at(&filter, idx);
            }
            (
                ActiveModal::ApiKeyInput {
                    provider_id,
                    provider_name,
                    api_key,
                    base_url,
                    existing_key_status,
                    error,
                    ..
                },
                ModalHitKind::ApiKeyField(target_base_url),
            ) => {
                self.modal = Some(ActiveModal::ApiKeyInput {
                    provider_id,
                    provider_name,
                    api_key,
                    base_url,
                    editing_base_url: target_base_url,
                    existing_key_status,
                    error,
                });
            }
            (ActiveModal::ApiKeyInput { provider_id, .. }, ModalHitKind::Close) => {
                self.reopen_provider_select_for(&provider_id);
            }
            (ActiveModal::ModelInput { .. }, ModalHitKind::Close) => {
                self.modal = None;
            }
            (
                ActiveModal::ModelInput {
                    input,
                    selected_recent,
                    ..
                },
                ModalHitKind::ListItem(idx),
            ) => {
                let recent = self.recent_models_for_active_provider();
                if let Some(model) = recent.get(idx) {
                    let (target_provider, target_model) = self.resolve_model_target(model);
                    self.select_provider_and_model(&target_provider, &target_model);
                    self.modal = None;
                } else {
                    self.modal = Some(ActiveModal::ModelInput {
                        input,
                        selected_recent,
                        error: None,
                    });
                }
            }
            (ActiveModal::DiffView { .. }, ModalHitKind::Close) => {
                self.modal = None;
            }
            (
                ActiveModal::DiffView {
                    file_index,
                    diff,
                    scroll,
                    list,
                },
                ModalHitKind::ListItem(idx),
            ) => {
                let opened = self
                    .file_changes
                    .get(idx)
                    .map(|entry| build_file_diff(&self.workspace_root, &entry.path, &entry.status));
                self.modal = Some(match opened {
                    Some(diff) => ActiveModal::DiffView {
                        file_index: idx,
                        diff,
                        scroll: 0,
                        list: DiffListState::Open { selected: idx },
                    },
                    None => ActiveModal::DiffView {
                        file_index,
                        diff,
                        scroll,
                        list,
                    },
                });
            }
            (ActiveModal::Search { .. }, ModalHitKind::Close) => {
                self.modal = None;
            }
            (
                ActiveModal::Search {
                    query,
                    mut mode,
                    workspace_files,
                    mut results,
                    mut selected,
                    mut preview,
                },
                ModalHitKind::SearchModeToggle,
            ) => {
                mode = mode.toggle();
                Self::refresh_search(
                    self.workspace.as_ref(),
                    &self.workspace_root,
                    mode,
                    &workspace_files,
                    &query,
                    &mut results,
                    &mut selected,
                    &mut preview,
                );
                self.modal = Some(ActiveModal::Search {
                    query,
                    mode,
                    workspace_files,
                    results,
                    selected,
                    preview,
                });
            }
            (
                ActiveModal::Search {
                    query,
                    mode,
                    workspace_files,
                    results,
                    selected,
                    preview,
                },
                ModalHitKind::ListItem(idx),
            ) => {
                if selected == idx {
                    if let Some(item) = results.get(idx) {
                        let snippet = match item.line {
                            Some(line) => format!("@{}#L{line} ", item.path),
                            None => format!("@{} ", item.path),
                        };
                        self.insert_str(&snippet);
                        self.mode = InputMode::Insert;
                        self.completion.reset();
                        self.modal = None;
                    }
                } else {
                    let next_selected = idx.min(results.len().saturating_sub(1));
                    let preview = results
                        .get(idx)
                        .map(|item| build_search_preview(&self.workspace_root, item, 20))
                        .or(preview);
                    self.modal = Some(ActiveModal::Search {
                        query,
                        mode,
                        workspace_files,
                        results,
                        selected: next_selected,
                        preview,
                    });
                }
            }
            (ActiveModal::CommandPalette { .. }, ModalHitKind::Close) => {
                self.modal = None;
            }
            (ActiveModal::CommandPalette { filter, .. }, ModalHitKind::ListItem(idx)) => {
                let commands = filtered_commands(&filter);
                if let Some(command) = commands.get(idx) {
                    let action = command.action;
                    self.modal = None;
                    self.run_palette_action(action);
                } else {
                    self.modal = Some(ActiveModal::CommandPalette {
                        filter,
                        selected: 0,
                    });
                }
            }
            (ActiveModal::SessionSelect { .. }, ModalHitKind::Close) => {
                self.session_delete_confirm = None;
                self.modal = None;
            }
            (
                ActiveModal::SessionSelect {
                    filter,
                    sessions,
                    loading,
                    error,
                    ..
                },
                ModalHitKind::ListItem(idx),
            ) => {
                let filtered_ids: Vec<(meerai_session::SessionId, Option<usize>)> =
                    filter_session_summaries(&sessions, &filter)
                        .into_iter()
                        .map(|summary| {
                            let open_tab = self.tab_index_for_session(&summary.id);
                            (summary.id.clone(), open_tab)
                        })
                        .collect();
                if let Some((session_id, open_tab)) = filtered_ids.get(idx).cloned() {
                    self.session_delete_confirm = None;
                    self.modal = None;
                    self.activate_session_choice(session_id, open_tab);
                } else {
                    self.modal = Some(ActiveModal::SessionSelect {
                        filter,
                        selected: idx,
                        sessions,
                        loading,
                        error,
                    });
                }
            }
            (ActiveModal::UserMessageActions { .. }, ModalHitKind::Close) => {
                self.modal = None;
            }
            (ActiveModal::UserMessageActions { entry_idx, .. }, ModalHitKind::ListItem(idx)) => {
                let actions = [UserAction::Rewind, UserAction::Copy, UserAction::Fork];
                if let Some(&action) = actions.get(idx) {
                    self.modal = None;
                    self.run_user_action(entry_idx, action);
                }
            }
            (ActiveModal::RewindMenu { .. }, ModalHitKind::Close) => {
                self.modal = None;
            }
            (ActiveModal::RewindMenu { turn_id, .. }, ModalHitKind::ListItem(idx)) => {
                self.apply_rewind_choice(turn_id, idx);
            }
            (ActiveModal::Terminal { running, .. }, ModalHitKind::Close) => {
                self.modal = None;
                if running {
                    self.status_notice =
                        Some("Shell running in background; output will appear here".into());
                }
            }
            (
                ActiveModal::Question {
                    request,
                    mut answers,
                    custom,
                    current,
                    ..
                },
                ModalHitKind::ListItem(idx),
            ) => {
                let prompt = &request.questions[current];
                let other = prompt.options.len();
                if idx < other {
                    if prompt.multiple {
                        if let Some(pos) = answers[current].iter().position(|i| *i == idx) {
                            answers[current].remove(pos);
                        } else {
                            answers[current].push(idx);
                            answers[current].sort_unstable();
                        }
                    } else {
                        answers[current] = vec![idx];
                    }
                    self.modal = Some(ActiveModal::Question {
                        request,
                        answers,
                        custom,
                        editing_custom: false,
                        current,
                        focus: idx,
                    });
                } else {
                    self.modal = Some(ActiveModal::Question {
                        request,
                        answers,
                        custom,
                        editing_custom: true,
                        current,
                        focus: other,
                    });
                }
            }
            (
                ActiveModal::Approval {
                    queue,
                    mut reason,
                    mut editing_reason,
                },
                ModalHitKind::ApprovalAction(action),
            ) => {
                let request_id = queue.first().map(|req| req.request_id.clone());
                let decision = match action {
                    ApprovalClickAction::Accept => Some((ApprovalDecision::Accept, None)),
                    ApprovalClickAction::AcceptTurn => {
                        Some((ApprovalDecision::Accept, Some(GrantScope::Turn)))
                    }
                    ApprovalClickAction::AcceptSession => {
                        Some((ApprovalDecision::AcceptForSession, None))
                    }
                    ApprovalClickAction::Decline => Some((ApprovalDecision::Decline, None)),
                    ApprovalClickAction::DeclineReason => {
                        reason.clear();
                        editing_reason = true;
                        None
                    }
                    ApprovalClickAction::Cancel => Some((ApprovalDecision::Cancel, None)),
                };
                if let Some((decision, scope)) = decision
                    && let Some(request_id) = request_id
                {
                    let _ = self.tab().cmd_tx.send(AgentCommand::Approve {
                        request_id,
                        decision,
                        scope,
                    });
                }
                self.modal = Some(ActiveModal::Approval {
                    queue,
                    reason,
                    editing_reason,
                });
            }
            (other, _) => {
                self.modal = Some(other);
                return false;
            }
        }
        true
    }

    fn reopen_provider_select_for(&mut self, provider_id: &str) {
        let selected = {
            let rt = self.runtime.read().unwrap();
            rt.catalog
                .providers()
                .iter()
                .position(|p| p.id == provider_id)
                .unwrap_or(0)
        };
        self.modal = Some(ActiveModal::ProviderSelect {
            filter: String::new(),
            selected,
        });
    }

    fn activate_provider_at(&mut self, filter: &str, selected: usize) {
        let providers = self.filtered_providers(filter);
        if let Some((provider, status)) = providers.get(selected) {
            if !provider.requires_api_key {
                self.activate_provider_only(&provider.id);
                self.open_model_input();
            } else {
                let saved_url = {
                    let rt = self.runtime.read().unwrap();
                    rt.base_urls.get(&provider.id).cloned().unwrap_or_default()
                };
                self.modal = Some(ActiveModal::ApiKeyInput {
                    provider_id: provider.id.clone(),
                    provider_name: provider.display_name.clone(),
                    api_key: String::new(),
                    base_url: saved_url,
                    editing_base_url: false,
                    existing_key_status: status.clone(),
                    error: None,
                });
            }
        } else {
            self.modal = Some(ActiveModal::ProviderSelect {
                filter: filter.to_string(),
                selected,
            });
        }
    }

    fn activate_session_choice(
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

    fn apply_rewind_choice(&mut self, turn_id: TurnId, selected: usize) {
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

    pub(super) fn open_command_palette(&mut self) {
        self.modal = Some(ActiveModal::CommandPalette {
            filter: String::new(),
            selected: 0,
        });
    }

    fn run_palette_action(&mut self, action: PaletteAction) {
        match action {
            PaletteAction::Slash(command) => {
                self.handle_slash_command(command);
            }
            PaletteAction::CopyReply => {
                if !self.copy_latest_reply() {
                    self.status_notice = Some("Nothing to copy".to_string());
                }
            }
            PaletteAction::NextTab => {
                self.next_tab();
            }
            PaletteAction::PreviousTab => {
                self.prev_tab();
            }
            PaletteAction::MoveTabLeft => {
                let active = self.active_tab;
                if active > 0 {
                    self.move_tab(active, active - 1);
                }
            }
            PaletteAction::MoveTabRight => {
                let active = self.active_tab;
                if active + 1 < self.tabs.len() {
                    self.move_tab(active, active + 1);
                }
            }
            PaletteAction::CycleApproval => {
                self.cycle_approval_mode();
            }
        }
    }

    pub(super) fn toggle_sidebar(&mut self) {
        self.show_sidebar = !self.show_sidebar;
        if self.show_sidebar && self.terminal_width < MIN_SIDEBAR_WIDTH {
            self.status_notice = Some(format!(
                "Sidebar hidden until terminal width >= {MIN_SIDEBAR_WIDTH} cols"
            ));
        }
    }

    pub(super) fn activate_provider_only(&mut self, provider_id: &str) {
        let canonical = canonical_provider_id(provider_id);
        let current_model = self.tab().model.clone();
        let switching_provider = self.tab().active_provider != canonical;
        self.tab_mut().active_provider = canonical.clone();
        self.settings.active_provider = Some(canonical.clone());

        let (spec, next_model) = {
            let mut rt = self.runtime.write().unwrap();
            rt.active_provider = canonical.clone();

            let candidate = self
                .settings
                .provider_model(&canonical)
                .map(str::to_string)
                .or_else(|| rt.catalog.default_model_for_provider(&canonical))
                .or_else(|| {
                    (!switching_provider && !current_model.trim().is_empty())
                        .then_some(current_model)
                })
                .unwrap_or_default();

            let spec = (!candidate.is_empty())
                .then(|| rt.catalog.format_genai_spec(&canonical, &candidate));
            (spec, candidate)
        };

        self.tab_mut().model = next_model.clone();
        self.settings.active_model = (!next_model.is_empty()).then_some(next_model);
        self.sync_completion_models();
        self.persist_settings();
        if let Some(spec) = spec {
            let _ = self.tab().cmd_tx.send(AgentCommand::SetModel(spec));
        }
    }

    pub(super) fn select_provider_and_model(&mut self, provider_id: &str, model_id: &str) {
        let canonical = canonical_provider_id(provider_id);
        let clean_model = model_id.trim().to_string();
        self.tab_mut().active_provider = canonical.clone();
        self.tab_mut().model = clean_model.clone();
        self.settings
            .record_provider_model(&canonical, &clean_model);

        let spec = {
            let mut rt = self.runtime.write().unwrap();
            rt.active_provider = canonical.clone();
            rt.catalog.format_genai_spec(&canonical, &clean_model)
        };

        self.sync_completion_models();
        self.persist_settings();
        let _ = self.tab().cmd_tx.send(AgentCommand::SetModel(spec));
        self.tab_mut().transcript.push(TranscriptEntry::System {
            text: format!("Active model: {canonical}/{clean_model}"),
        });
    }

    pub(super) fn persist_settings(&mut self) {
        if let Some(dir) = &self.config_dir
            && let Err(err) = save_settings_to(dir, &self.settings)
        {
            self.status_notice = Some(format!("Failed to save settings: {err:#}"));
        }
    }

    pub(super) fn open_provider_select(&mut self) {
        let active_provider = self.tab().active_provider.clone();
        let selected = {
            let rt = self.runtime.read().unwrap();
            rt.catalog
                .providers()
                .iter()
                .position(|p| p.id == active_provider)
                .unwrap_or(0)
        };
        self.modal = Some(ActiveModal::ProviderSelect {
            filter: String::new(),
            selected,
        });
    }

    pub(super) fn open_model_input(&mut self) {
        self.modal = Some(ActiveModal::ModelInput {
            input: String::new(),
            selected_recent: None,
            error: None,
        });
    }
}

fn collect_question_answers(
    request: &QuestionRequest,
    answers: &[Vec<usize>],
    custom: &[String],
) -> Vec<QuestionAnswer> {
    request
        .questions
        .iter()
        .enumerate()
        .map(|(index, prompt)| {
            let mut selected: Vec<String> = answers[index]
                .iter()
                .filter_map(|i| prompt.options.get(*i))
                .map(|option| option.label.clone())
                .collect();
            let text = custom[index].trim();
            if !text.is_empty() {
                selected.push(text.to_string());
            }
            QuestionAnswer {
                header: prompt.header.clone(),
                selected,
            }
        })
        .collect()
}
