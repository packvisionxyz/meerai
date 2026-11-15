use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use meerai_agents::AgentCommand;
use meerai_agents::events::{ApprovalDecision, GrantScope};
use meerai_common::settings::save_settings_to;
use meerai_providers::providers::canonical_provider_id;

use super::*;
use crate::diff::build_file_diff;
use crate::search::{
    SearchMatch, SearchMode, SearchPreview, build_search_preview, discover_workspace_files,
    search_content, search_files,
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
        });
    }

    pub fn open_search_modal(&mut self, mode: SearchMode, initial_query: &str) {
        let workspace_files = discover_workspace_files(&self.workspace_root);
        let query = initial_query.trim().to_string();
        let mut results = Vec::new();
        let mut selected = 0;
        let mut preview = None;
        Self::refresh_search(
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

    pub(super) fn refresh_search(
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
            SearchMode::Content => search_content(workspace_root, workspace_files, query),
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
                            let provider_info = {
                                let rt = self.runtime.read().unwrap();
                                rt.catalog.provider(&provider_id).cloned()
                            };
                            if let Some(info) = provider_info {
                                existing_key_status =
                                    check_key_status(&info, self.secrets.as_ref());
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
                        } else if let Some(existing) =
                            self.current_or_default_model(&self.active_provider)
                        {
                            let provider = self.active_provider.clone();
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
            } => {
                let max_scroll = diff.rows.len().saturating_sub(1);
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => {
                        self.modal = None;
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        scroll = (scroll + 1).min(max_scroll);
                        self.modal = Some(ActiveModal::DiffView {
                            file_index,
                            diff,
                            scroll,
                        });
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        scroll = scroll.saturating_sub(1);
                        self.modal = Some(ActiveModal::DiffView {
                            file_index,
                            diff,
                            scroll,
                        });
                    }
                    KeyCode::PageDown | KeyCode::Char('d')
                        if key.code == KeyCode::PageDown || has_ctrl =>
                    {
                        let step = (self.viewport_height / 2).max(5);
                        scroll = (scroll + step).min(max_scroll);
                        self.modal = Some(ActiveModal::DiffView {
                            file_index,
                            diff,
                            scroll,
                        });
                    }
                    KeyCode::PageUp | KeyCode::Char('u')
                        if key.code == KeyCode::PageUp || has_ctrl =>
                    {
                        let step = (self.viewport_height / 2).max(5);
                        scroll = scroll.saturating_sub(step);
                        self.modal = Some(ActiveModal::DiffView {
                            file_index,
                            diff,
                            scroll,
                        });
                    }
                    KeyCode::Home | KeyCode::Char('g') => {
                        scroll = 0;
                        self.modal = Some(ActiveModal::DiffView {
                            file_index,
                            diff,
                            scroll,
                        });
                    }
                    KeyCode::End | KeyCode::Char('G') => {
                        scroll = max_scroll;
                        self.modal = Some(ActiveModal::DiffView {
                            file_index,
                            diff,
                            scroll,
                        });
                    }
                    KeyCode::Tab | KeyCode::Char(']') => {
                        if !self.file_changes.is_empty() {
                            file_index = (file_index + 1) % self.file_changes.len();
                            let entry = &self.file_changes[file_index];
                            diff =
                                build_file_diff(&self.workspace_root, &entry.path, &entry.status);
                            scroll = 0;
                        }
                        self.modal = Some(ActiveModal::DiffView {
                            file_index,
                            diff,
                            scroll,
                        });
                    }
                    KeyCode::BackTab | KeyCode::Char('[') => {
                        if !self.file_changes.is_empty() {
                            file_index = if file_index == 0 {
                                self.file_changes.len() - 1
                            } else {
                                file_index - 1
                            };
                            let entry = &self.file_changes[file_index];
                            diff =
                                build_file_diff(&self.workspace_root, &entry.path, &entry.status);
                            scroll = 0;
                        }
                        self.modal = Some(ActiveModal::DiffView {
                            file_index,
                            diff,
                            scroll,
                        });
                    }
                    _ => {
                        self.modal = Some(ActiveModal::DiffView {
                            file_index,
                            diff,
                            scroll,
                        });
                    }
                }
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
                        let step = (self.viewport_height / 2).max(5);
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
                        let step = (self.viewport_height / 2).max(5);
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
                                let _ = self.cmd_tx.send(AgentCommand::Approve {
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
                        let _ = self.cmd_tx.send(AgentCommand::Approve {
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
        }

        true
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
        let switching_provider = self.active_provider != canonical;
        self.active_provider = canonical.clone();
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
                    (!switching_provider && !self.model.trim().is_empty())
                        .then(|| self.model.clone())
                })
                .unwrap_or_default();

            let spec = (!candidate.is_empty())
                .then(|| rt.catalog.format_genai_spec(&canonical, &candidate));
            (spec, candidate)
        };

        self.model = next_model.clone();
        self.settings.active_model = (!next_model.is_empty()).then_some(next_model);
        self.sync_completion_models();
        self.persist_settings();
        if let Some(spec) = spec {
            let _ = self.cmd_tx.send(AgentCommand::SetModel(spec));
        }
    }

    pub(super) fn select_provider_and_model(&mut self, provider_id: &str, model_id: &str) {
        let canonical = canonical_provider_id(provider_id);
        let clean_model = model_id.trim().to_string();
        self.active_provider = canonical.clone();
        self.model = clean_model.clone();
        self.settings
            .record_provider_model(&canonical, &clean_model);

        let spec = {
            let mut rt = self.runtime.write().unwrap();
            rt.active_provider = canonical.clone();
            rt.catalog.format_genai_spec(&canonical, &clean_model)
        };

        self.sync_completion_models();
        self.persist_settings();
        let _ = self.cmd_tx.send(AgentCommand::SetModel(spec));
        self.transcript.push(TranscriptEntry::System {
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
        let selected = {
            let rt = self.runtime.read().unwrap();
            rt.catalog
                .providers()
                .iter()
                .position(|p| p.id == self.active_provider)
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
