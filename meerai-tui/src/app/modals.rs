use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use meerai_agents::AgentCommand;
use meerai_agents::events::{ApprovalDecision, GrantScope, QuestionAnswer, RewindScope};
use meerai_client::providers::canonical_provider_id;
use meerai_common::settings::save_settings_to;

use super::text_input::{TextAction, handle_text_key};
use super::*;
use crate::diff::{build_file_diff, list_git_changes, stage_file, unstage_file};
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
        let query = TextInput::new(initial_query.trim());
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
                    _ => match handle_text_key(&mut filter, key, 40, false, false) {
                        TextAction::Submit => {
                            if let Some((provider, status)) = providers.get(selected) {
                                self.open_provider_connect(provider, status);
                            } else {
                                self.modal = Some(ActiveModal::ProviderSelect { filter, selected });
                            }
                        }
                        TextAction::Edited => {
                            selected = 0;
                            self.modal = Some(ActiveModal::ProviderSelect { filter, selected });
                        }
                        TextAction::Ignored => {
                            self.modal = Some(ActiveModal::ProviderSelect { filter, selected });
                        }
                    },
                }
            }
            ActiveModal::AuthMethodSelect {
                provider_id,
                provider_name,
                mut selected,
            } => {
                const AUTH_METHODS: usize = 2;
                match key.code {
                    KeyCode::Esc => self.reopen_provider_select_for(&provider_id),
                    KeyCode::Up | KeyCode::Char('p') if key.code == KeyCode::Up || has_ctrl => {
                        selected = if selected == 0 {
                            AUTH_METHODS - 1
                        } else {
                            selected - 1
                        };
                        self.modal = Some(ActiveModal::AuthMethodSelect {
                            provider_id,
                            provider_name,
                            selected,
                        });
                    }
                    KeyCode::Down | KeyCode::Char('n') if key.code == KeyCode::Down || has_ctrl => {
                        selected = (selected + 1) % AUTH_METHODS;
                        self.modal = Some(ActiveModal::AuthMethodSelect {
                            provider_id,
                            provider_name,
                            selected,
                        });
                    }
                    KeyCode::Enter => self.choose_auth_method(&provider_id, selected),
                    _ => {
                        self.modal = Some(ActiveModal::AuthMethodSelect {
                            provider_id,
                            provider_name,
                            selected,
                        });
                    }
                }
            }
            ActiveModal::ProviderConnection {
                provider_id,
                provider_name,
                status,
                mut selected,
            } => {
                let count = ProviderConnectionAction::options(&status).len();
                match key.code {
                    KeyCode::Esc => self.reopen_provider_select_for(&provider_id),
                    KeyCode::Up | KeyCode::Char('p') if key.code == KeyCode::Up || has_ctrl => {
                        selected = if selected == 0 {
                            count - 1
                        } else {
                            selected - 1
                        };
                        self.modal = Some(ActiveModal::ProviderConnection {
                            provider_id,
                            provider_name,
                            status,
                            selected,
                        });
                    }
                    KeyCode::Down | KeyCode::Char('n') if key.code == KeyCode::Down || has_ctrl => {
                        selected = (selected + 1) % count;
                        self.modal = Some(ActiveModal::ProviderConnection {
                            provider_id,
                            provider_name,
                            status,
                            selected,
                        });
                    }
                    KeyCode::Enter => {
                        self.apply_provider_connection_action(
                            &provider_id,
                            &provider_name,
                            &status,
                            selected,
                        );
                    }
                    _ => {
                        self.modal = Some(ActiveModal::ProviderConnection {
                            provider_id,
                            provider_name,
                            status,
                            selected,
                        });
                    }
                }
            }
            ActiveModal::ChatGptLogin {
                provider_id,
                provider_name,
                mut url,
                mut input,
                mut error,
                mut busy,
            } => {
                match key.code {
                    KeyCode::Esc => {
                        self.cancel_chatgpt_login();
                        return true;
                    }
                    KeyCode::Char('y') if has_ctrl => {
                        if let Some(link) = url.as_deref() {
                            self.copy_text_to_clipboard(link);
                            self.status_notice =
                                Some("Authorize URL copied to clipboard".to_string());
                        }
                    }
                    _ => match handle_text_key(&mut input, key, 64, false, false) {
                        TextAction::Submit => {
                            if error.is_some() {
                                self.login_input = None;
                                self.login_cancel = None;
                                self.pending_login_request = Some(provider_id.clone());
                                input.clear();
                                error = None;
                                busy = true;
                                url = None;
                            } else {
                                let trimmed = input.trim().to_string();
                                if !trimmed.is_empty()
                                    && let Some(tx) = &self.login_input
                                {
                                    let _ = tx.send(trimmed);
                                    input.clear();
                                    error = None;
                                    busy = true;
                                }
                            }
                        }
                        TextAction::Edited => {
                            error = None;
                        }
                        TextAction::Ignored => {}
                    },
                }
                self.modal = Some(ActiveModal::ChatGptLogin {
                    provider_id,
                    provider_name,
                    url,
                    input,
                    error,
                    busy,
                });
            }
            ActiveModal::ApiKeyInput {
                provider_id,
                provider_name,
                mut api_key,
                mut base_url,
                mut editing_base_url,
                mut existing_key_status,
                mut error,
            } => {
                let action = match key.code {
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
                            filter: TextInput::default(),
                            selected,
                        });
                        return true;
                    }
                    KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => {
                        editing_base_url = !editing_base_url;
                        TextAction::Ignored
                    }
                    KeyCode::Char('d') if has_ctrl => {
                        match self.secrets.delete_credential(&provider_id) {
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
                                    Some(format!("Removed credential for {provider_id}"));
                            }
                            Err(err) => {
                                error = Some(format!("{err:#}"));
                            }
                        }
                        TextAction::Edited
                    }
                    _ => {
                        let field = if editing_base_url {
                            &mut base_url
                        } else {
                            &mut api_key
                        };
                        handle_text_key(field, key, 64, false, false)
                    }
                };
                if action == TextAction::Submit {
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
                    return true;
                }
                if action == TextAction::Edited {
                    error = None;
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
            ActiveModal::ModelInput {
                mut input,
                mut selected,
                mut error,
            } => {
                let matches = self.model_candidates(&input);
                let highlighted = selected.and_then(|index| matches.get(index)).cloned();
                match key.code {
                    KeyCode::Esc => {
                        self.modal = None;
                    }
                    KeyCode::Char('u') if has_ctrl => {
                        input.clear();
                        selected = None;
                        error = None;
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected,
                            error,
                        });
                    }
                    KeyCode::Char('d') if has_ctrl => {
                        let target = highlighted.or_else(|| {
                            let typed = input.trim();
                            (!typed.is_empty()).then(|| typed.to_string())
                        });
                        if let Some(model) = target {
                            let provider = self.tab().active_provider.clone();
                            let (target_provider, target_model) = self.resolve_model_target(&model);
                            let provider_id = if target_provider.is_empty() {
                                provider
                            } else {
                                target_provider
                            };
                            self.settings
                                .forget_provider_model(&provider_id, &target_model);
                            self.persist_settings();
                            input.clear();
                            selected = None;
                            error = None;
                            self.status_notice =
                                Some(format!("Removed {provider_id}/{target_model}"));
                        }
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected,
                            error,
                        });
                    }
                    KeyCode::Up | KeyCode::Char('p') if key.code == KeyCode::Up || has_ctrl => {
                        if !matches.is_empty() {
                            selected = Some(match selected {
                                None | Some(0) => matches.len() - 1,
                                Some(index) => index.min(matches.len() - 1).saturating_sub(1),
                            });
                            error = None;
                        }
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected,
                            error,
                        });
                    }
                    KeyCode::Down | KeyCode::Char('n') if key.code == KeyCode::Down || has_ctrl => {
                        if !matches.is_empty() {
                            selected = Some(match selected {
                                None => 0,
                                Some(index) => (index + 1) % matches.len(),
                            });
                            error = None;
                        }
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected,
                            error,
                        });
                    }
                    KeyCode::Tab => {
                        if let Some(model) = highlighted.or_else(|| matches.first().cloned()) {
                            input.set_text(model);
                            selected = Some(0);
                            error = None;
                        }
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected,
                            error,
                        });
                    }
                    _ => match handle_text_key(&mut input, key, 60, false, false) {
                        TextAction::Submit => {
                            let typed = input.trim().to_string();
                            let target = if let Some(model) = highlighted {
                                Some(model)
                            } else if !typed.is_empty() {
                                Some(typed)
                            } else {
                                let provider = self.tab().active_provider.clone();
                                self.current_or_default_model(&provider)
                            };
                            match target {
                                Some(model) => {
                                    let (target_provider, target_model) =
                                        self.resolve_model_target(&model);
                                    if target_model.is_empty() {
                                        self.modal = Some(ActiveModal::ModelInput {
                                            input,
                                            selected,
                                            error: Some("Model name is required".to_string()),
                                        });
                                    } else {
                                        self.select_provider_and_model(
                                            &target_provider,
                                            &target_model,
                                        );
                                        self.modal = None;
                                    }
                                }
                                None => {
                                    self.modal = Some(ActiveModal::ModelInput {
                                        input,
                                        selected,
                                        error: Some("Model name is required".to_string()),
                                    });
                                }
                            }
                        }
                        TextAction::Edited => {
                            selected = None;
                            error = None;
                            self.modal = Some(ActiveModal::ModelInput {
                                input,
                                selected,
                                error,
                            });
                        }
                        TextAction::Ignored => {
                            self.modal = Some(ActiveModal::ModelInput {
                                input,
                                selected,
                                error,
                            });
                        }
                    },
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
                        self.workspace.as_ref(),
                        &self.workspace_root,
                        mode,
                        &workspace_files,
                        &query,
                        &mut results,
                        &mut selected,
                        &mut preview,
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
            }
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
                        _ => match handle_text_key(&mut reason, key, 60, true, true) {
                            TextAction::Submit => {
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
                            TextAction::Edited | TextAction::Ignored => {}
                        },
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
                        self.status_notice = Some(
                            "Shell running in background; output will appear in the transcript"
                                .into(),
                        );
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
                    run_id,
                    command,
                    output,
                    running,
                    ..
                }) = self.modal.take()
                {
                    self.modal = Some(ActiveModal::Terminal {
                        run_id,
                        command,
                        output,
                        running,
                        scroll,
                    });
                }
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
                let width = self.terminal_width.saturating_sub(12).max(20) as usize;
                let mut submit = false;
                if editing_custom {
                    match key.code {
                        KeyCode::Esc => {
                            editing_custom = false;
                        }
                        KeyCode::Tab => {
                            if last {
                                submit = true;
                            } else {
                                current += 1;
                                focus = 0;
                                editing_custom = false;
                            }
                        }
                        KeyCode::BackTab => {
                            if current > 0 {
                                current -= 1;
                                focus = 0;
                                editing_custom = false;
                            }
                        }
                        KeyCode::Up
                            if !key.modifiers.intersects(
                                KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT,
                            ) =>
                        {
                            if !custom[current].move_up(width) {
                                editing_custom = false;
                                focus = focus.saturating_sub(1);
                            }
                        }
                        KeyCode::Down
                            if !key.modifiers.intersects(
                                KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT,
                            ) =>
                        {
                            if !custom[current].move_down(width) {
                                editing_custom = false;
                            }
                        }
                        _ => match handle_text_key(&mut custom[current], key, width, true, true) {
                            TextAction::Submit => submit = true,
                            TextAction::Edited => {
                                if !multiple {
                                    answers[current].clear();
                                }
                            }
                            TextAction::Ignored => {}
                        },
                    }
                } else {
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
                        KeyCode::Up | KeyCode::Char('k')
                            if key.code == KeyCode::Up || !has_ctrl =>
                        {
                            focus = focus.saturating_sub(1);
                        }
                        KeyCode::Down | KeyCode::Char('j')
                            if key.code == KeyCode::Down || !has_ctrl =>
                        {
                            focus = (focus + 1).min(other);
                        }
                        KeyCode::Char(' ') if focus < other => {
                            if multiple {
                                if let Some(index) =
                                    answers[current].iter().position(|i| *i == focus)
                                {
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
                            editing_custom = true;
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
                        KeyCode::Enter if focus == other => {
                            editing_custom = true;
                        }
                        KeyCode::Enter => {
                            if focus < other && answers[current].is_empty() {
                                answers[current] = vec![focus];
                            }
                            submit = true;
                        }
                        KeyCode::Tab | KeyCode::Right => {
                            if !last {
                                current += 1;
                                focus = 0;
                                editing_custom = false;
                            } else {
                                submit = true;
                            }
                        }
                        KeyCode::BackTab | KeyCode::Left => {
                            if current > 0 {
                                current -= 1;
                                focus = 0;
                                editing_custom = false;
                            }
                        }
                        _ => {
                            if focus == other {
                                editing_custom = true;
                                match handle_text_key(&mut custom[current], key, width, true, true)
                                {
                                    TextAction::Submit => submit = true,
                                    TextAction::Edited => {
                                        if !multiple {
                                            answers[current].clear();
                                        }
                                    }
                                    TextAction::Ignored => {}
                                }
                            }
                        }
                    }
                }
                if submit {
                    if focus < other && answers[current].is_empty() && !editing_custom {
                        answers[current] = vec![focus];
                    }
                    if last {
                        let collected = collect_question_answers(&request, &answers, &custom);
                        self.record_question_answer(&request.request_id, Some(collected.clone()));
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
                        filter.backspace();
                        selected = 0;
                        self.modal = Some(ActiveModal::CommandPalette { filter, selected });
                    }
                    _ => match handle_text_key(&mut filter, key, 48, false, false) {
                        TextAction::Submit => {
                            if let Some(command) = commands.get(selected) {
                                let action = command.action;
                                self.modal = None;
                                self.run_palette_action(action);
                            } else {
                                self.modal = Some(ActiveModal::CommandPalette { filter, selected });
                            }
                        }
                        TextAction::Edited => {
                            selected = 0;
                            self.modal = Some(ActiveModal::CommandPalette { filter, selected });
                        }
                        TextAction::Ignored => {
                            self.modal = Some(ActiveModal::CommandPalette { filter, selected });
                        }
                    },
                }
            }
            ActiveModal::QueuedEdit {
                index,
                mut input,
                mut error,
            } => {
                let width = if self.queued_editor_width == 0 {
                    56
                } else {
                    self.queued_editor_width as usize
                };
                let mut keep_open = true;
                match key.code {
                    KeyCode::Esc => {
                        self.modal = None;
                        keep_open = false;
                    }
                    _ => match handle_text_key(&mut input, key, width, true, true) {
                        TextAction::Submit => {
                            let text = input.trim().to_string();
                            if text.is_empty() {
                                error = Some("Message is required".to_string());
                            } else {
                                let updated = {
                                    let tab = self.tab_mut();
                                    match tab.queued.get_mut(index) {
                                        Some(slot) => {
                                            *slot = text.clone();
                                            true
                                        }
                                        None => false,
                                    }
                                };
                                if updated {
                                    self.status_notice = Some("Queued message updated".to_string());
                                }
                                self.modal = None;
                                keep_open = false;
                            }
                        }
                        TextAction::Edited => {
                            error = None;
                        }
                        TextAction::Ignored => {}
                    },
                }
                if keep_open {
                    self.modal = Some(ActiveModal::QueuedEdit {
                        index,
                        input,
                        error,
                    });
                }
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
            }
            ActiveModal::Context(mut panel) => {
                if panel.focus_input.is_some() {
                    match key.code {
                        KeyCode::Esc => {
                            panel.focus_input = None;
                            panel.error = None;
                        }
                        _ => {
                            let action = match panel.focus_input.as_mut() {
                                Some(input) => handle_text_key(input, key, 56, false, false),
                                None => TextAction::Ignored,
                            };
                            if action == TextAction::Submit {
                                let focus = panel
                                    .focus_input
                                    .take()
                                    .map(|input| input.text)
                                    .unwrap_or_default();
                                let from_turn_id = panel
                                    .breakdown
                                    .as_ref()
                                    .and_then(|breakdown| {
                                        breakdown.turn_ids.get(panel.selected_turn)
                                    })
                                    .map(|turn| turn.to_string());
                                queue_modal_host_action(
                                    self,
                                    meerai_host::HostAction::Summarize {
                                        focus: (!focus.trim().is_empty())
                                            .then(|| focus.trim().to_string()),
                                        from_turn_id,
                                    },
                                );
                            }
                        }
                    }
                    self.modal = Some(ActiveModal::Context(panel));
                    return true;
                }
                if panel.blob_content.is_some() {
                    match key.code {
                        KeyCode::Esc => {
                            panel.blob_key = None;
                            panel.blob_content = None;
                            panel.blob_scroll = 0;
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            panel.blob_scroll = panel.blob_scroll.saturating_add(1);
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            panel.blob_scroll = panel.blob_scroll.saturating_sub(1);
                        }
                        KeyCode::Char('g') => panel.blob_scroll = 0,
                        _ => {}
                    }
                    self.modal = Some(ActiveModal::Context(panel));
                    return true;
                }
                let turn_count = panel
                    .breakdown
                    .as_ref()
                    .map_or(0, |breakdown| breakdown.turn_ids.len());
                let blob_count = panel
                    .breakdown
                    .as_ref()
                    .map_or(0, |breakdown| breakdown.spilled_blobs.len());
                match key.code {
                    KeyCode::Esc => self.modal = None,
                    KeyCode::Down | KeyCode::Char('j') => {
                        if turn_count > 0 {
                            panel.selected_turn = (panel.selected_turn + 1) % turn_count;
                        }
                        if blob_count > 0 {
                            panel.selected_blob = (panel.selected_blob + 1) % blob_count;
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if turn_count > 0 {
                            panel.selected_turn = panel.selected_turn.saturating_sub(1);
                        }
                        if blob_count > 0 {
                            panel.selected_blob = panel.selected_blob.saturating_sub(1);
                        }
                    }
                    KeyCode::Char('g') => {
                        panel.scroll = 0;
                        panel.selected_turn = 0;
                        panel.selected_blob = 0;
                    }
                    KeyCode::Char('r') => {
                        let _ = self.tab().cmd_tx.send(AgentCommand::InspectContext);
                    }
                    KeyCode::Char('p') => {
                        if let Some(turn) = panel
                            .breakdown
                            .as_ref()
                            .and_then(|breakdown| breakdown.turn_ids.get(panel.selected_turn))
                        {
                            let pinned = panel
                                .breakdown
                                .as_ref()
                                .is_some_and(|breakdown| breakdown.pinned_turn_ids.contains(turn));
                            queue_modal_host_action(
                                self,
                                meerai_host::HostAction::PinTurn {
                                    turn_id: turn.to_string(),
                                    pinned: !pinned,
                                },
                            );
                        }
                    }
                    KeyCode::Char('s') => {
                        if let Some(turn) = panel
                            .breakdown
                            .as_ref()
                            .and_then(|breakdown| breakdown.turn_ids.get(panel.selected_turn))
                        {
                            queue_modal_host_action(
                                self,
                                meerai_host::HostAction::Summarize {
                                    focus: None,
                                    from_turn_id: Some(turn.to_string()),
                                },
                            );
                        }
                    }
                    KeyCode::Char('f') => panel.focus_input = Some(TextInput::default()),
                    KeyCode::Char('b') => {
                        if let Some(key) = panel
                            .breakdown
                            .as_ref()
                            .and_then(|breakdown| breakdown.spilled_blobs.get(panel.selected_blob))
                        {
                            let tab_id = self.tab().tab_id;
                            self.pending_session_actions
                                .push(PendingSessionAction::LoadBlob {
                                    tab_id,
                                    key: key.clone(),
                                });
                        }
                    }
                    KeyCode::Char('t') => {
                        let current = panel
                            .breakdown
                            .as_ref()
                            .map_or(4, |breakdown| breakdown.verbatim_window_turns);
                        let next = match current {
                            0..=3 => 4,
                            4..=7 => 8,
                            8..=15 => 16,
                            _ => 4,
                        };
                        queue_modal_host_action(
                            self,
                            meerai_host::HostAction::SetContextPolicy {
                                verbatim_window_turns: Some(next),
                                reasoning_pruning: None,
                                clear_old_tool_results: None,
                            },
                        );
                    }
                    KeyCode::Char('c') => {
                        let next = !panel
                            .breakdown
                            .as_ref()
                            .is_some_and(|breakdown| breakdown.reasoning_pruning);
                        queue_modal_host_action(
                            self,
                            meerai_host::HostAction::SetContextPolicy {
                                verbatim_window_turns: None,
                                reasoning_pruning: Some(next),
                                clear_old_tool_results: None,
                            },
                        );
                    }
                    KeyCode::Char('C') => {
                        let next = !panel
                            .breakdown
                            .as_ref()
                            .is_some_and(|breakdown| breakdown.clear_old_tool_results);
                        queue_modal_host_action(
                            self,
                            meerai_host::HostAction::SetContextPolicy {
                                verbatim_window_turns: None,
                                reasoning_pruning: None,
                                clear_old_tool_results: Some(next),
                            },
                        );
                    }
                    _ => {}
                }
                self.modal = Some(ActiveModal::Context(panel));
            }
            ActiveModal::Loop { state, mut scroll } => {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => self.modal = None,
                    KeyCode::Down | KeyCode::Char('j') => {
                        scroll = scroll.saturating_add(1);
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        scroll = scroll.saturating_sub(1);
                    }
                    KeyCode::PageDown => {
                        scroll = scroll.saturating_add(10);
                    }
                    KeyCode::PageUp => {
                        scroll = scroll.saturating_sub(10);
                    }
                    KeyCode::Home | KeyCode::Char('g') => scroll = 0,
                    _ => {}
                }
                if self.modal.is_some() {
                    self.modal = Some(ActiveModal::Loop { state, scroll });
                }
            }
            ActiveModal::Memory {
                entries,
                query,
                mut selected,
                loading,
                error,
            } => {
                let count = entries.len();
                match key.code {
                    KeyCode::Esc => self.modal = None,
                    KeyCode::Down | KeyCode::Char('j') if count > 0 => {
                        selected = (selected + 1) % count;
                        self.modal = Some(ActiveModal::Memory {
                            entries,
                            query,
                            selected,
                            loading,
                            error,
                        });
                    }
                    KeyCode::Up | KeyCode::Char('k') if count > 0 => {
                        selected = if selected == 0 {
                            count - 1
                        } else {
                            selected - 1
                        };
                        self.modal = Some(ActiveModal::Memory {
                            entries,
                            query,
                            selected,
                            loading,
                            error,
                        });
                    }
                    KeyCode::Char('r') => {
                        self.pending_session_actions
                            .push(PendingSessionAction::LoadMemories {
                                query: (!query.is_empty()).then(|| query.clone()),
                            });
                        self.modal = Some(ActiveModal::Memory {
                            entries,
                            query,
                            selected,
                            loading: true,
                            error: None,
                        });
                    }
                    KeyCode::Char('d') | KeyCode::Char('p') if count > 0 => {
                        let Some(memory) = entries.get(selected) else {
                            self.modal = Some(ActiveModal::Memory {
                                entries,
                                query,
                                selected,
                                loading,
                                error,
                            });
                            return true;
                        };
                        let action = if key.code == KeyCode::Char('d') {
                            MemoryModalAction::Delete
                        } else {
                            MemoryModalAction::TogglePin
                        };
                        self.pending_session_actions
                            .push(PendingSessionAction::MemoryAction {
                                action,
                                id: memory.id.clone(),
                            });
                        self.modal = Some(ActiveModal::Memory {
                            entries,
                            query,
                            selected,
                            loading: true,
                            error: None,
                        });
                    }
                    _ => {
                        self.modal = Some(ActiveModal::Memory {
                            entries,
                            query,
                            selected,
                            loading,
                            error,
                        });
                    }
                }
            }
            ActiveModal::Routing {
                routes,
                mut selected,
                loading,
                error,
            } => {
                let count = routes.len();
                match key.code {
                    KeyCode::Esc => self.modal = None,
                    KeyCode::Down | KeyCode::Char('j') if count > 0 => {
                        selected = (selected + 1) % count;
                        self.modal = Some(ActiveModal::Routing {
                            routes,
                            selected,
                            loading,
                            error,
                        });
                    }
                    KeyCode::Up | KeyCode::Char('k') if count > 0 => {
                        selected = if selected == 0 {
                            count - 1
                        } else {
                            selected - 1
                        };
                        self.modal = Some(ActiveModal::Routing {
                            routes,
                            selected,
                            loading,
                            error,
                        });
                    }
                    KeyCode::Char('d') if count > 0 => {
                        let task_type = routes[selected].task_type.as_key();
                        self.pending_session_actions
                            .push(PendingSessionAction::RouteAction { task_type });
                        self.modal = Some(ActiveModal::Routing {
                            routes,
                            selected,
                            loading: true,
                            error: None,
                        });
                    }
                    KeyCode::Char('r') => {
                        self.pending_session_actions
                            .push(PendingSessionAction::LoadRoutes);
                        self.modal = Some(ActiveModal::Routing {
                            routes,
                            selected,
                            loading: true,
                            error: None,
                        });
                    }
                    _ => {
                        self.modal = Some(ActiveModal::Routing {
                            routes,
                            selected,
                            loading,
                            error,
                        });
                    }
                }
            }
            ActiveModal::Packs {
                packs,
                definitions,
                tab,
                mut selected,
                loading,
                error,
            } => {
                let active_len = match tab {
                    PackTab::Packs => packs.len(),
                    PackTab::Definitions => definitions.agents.len() + definitions.skills.len(),
                };
                match key.code {
                    KeyCode::Esc => self.modal = None,
                    KeyCode::Tab | KeyCode::Char('1') | KeyCode::Char('2') => {
                        let tab = match key.code {
                            KeyCode::Char('1') => PackTab::Packs,
                            KeyCode::Char('2') => PackTab::Definitions,
                            _ if tab == PackTab::Packs => PackTab::Definitions,
                            _ => PackTab::Packs,
                        };
                        self.modal = Some(ActiveModal::Packs {
                            packs,
                            definitions,
                            tab,
                            selected: 0,
                            loading,
                            error,
                        });
                    }
                    KeyCode::Down | KeyCode::Char('j') if active_len > 0 => {
                        selected = (selected + 1) % active_len;
                        self.modal = Some(ActiveModal::Packs {
                            packs,
                            definitions,
                            tab,
                            selected,
                            loading,
                            error,
                        });
                    }
                    KeyCode::Up | KeyCode::Char('k') if active_len > 0 => {
                        selected = if selected == 0 {
                            active_len - 1
                        } else {
                            selected - 1
                        };
                        self.modal = Some(ActiveModal::Packs {
                            packs,
                            definitions,
                            tab,
                            selected,
                            loading,
                            error,
                        });
                    }
                    KeyCode::Char('e') | KeyCode::Char('t') | KeyCode::Char('x')
                        if tab == PackTab::Packs && !packs.is_empty() =>
                    {
                        let Some(pack) = packs.get(selected) else {
                            self.modal = Some(ActiveModal::Packs {
                                packs,
                                definitions,
                                tab,
                                selected,
                                loading,
                                error,
                            });
                            return true;
                        };
                        let action = match key.code {
                            KeyCode::Char('e') => PackModalAction::ToggleEnabled,
                            KeyCode::Char('t') => PackModalAction::ToggleTrusted,
                            _ => PackModalAction::Remove,
                        };
                        self.pending_session_actions
                            .push(PendingSessionAction::PackAction {
                                action,
                                id: pack.id.clone(),
                            });
                        self.modal = Some(ActiveModal::Packs {
                            packs,
                            definitions,
                            tab,
                            selected,
                            loading: true,
                            error: None,
                        });
                    }
                    KeyCode::Char('r') => {
                        self.pending_session_actions
                            .push(PendingSessionAction::LoadPacks);
                        self.pending_session_actions
                            .push(PendingSessionAction::LoadDefinitions);
                        self.modal = Some(ActiveModal::Packs {
                            packs,
                            definitions,
                            tab,
                            selected,
                            loading: true,
                            error: None,
                        });
                    }
                    _ => {
                        self.modal = Some(ActiveModal::Packs {
                            packs,
                            definitions,
                            tab,
                            selected,
                            loading,
                            error,
                        });
                    }
                }
            }
            ActiveModal::Sampling {
                mut effort,
                mut temperature,
                mut field,
                error,
            } => {
                if key.code == KeyCode::Esc {
                    self.modal = None;
                    return true;
                }
                if matches!(key.code, KeyCode::Tab | KeyCode::Down) && field == 0 {
                    field = 1;
                    self.modal = Some(ActiveModal::Sampling {
                        effort,
                        temperature,
                        field,
                        error: None,
                    });
                    return true;
                }
                if matches!(key.code, KeyCode::BackTab | KeyCode::Up) && field == 1 {
                    field = 0;
                    self.modal = Some(ActiveModal::Sampling {
                        effort,
                        temperature,
                        field,
                        error: None,
                    });
                    return true;
                }
                if field == 0 {
                    match key.code {
                        KeyCode::Left | KeyCode::Right => {
                            let allowed = self.active_model_effort_levels();
                            let current = effort.unwrap_or_default().clamp_to(&allowed);
                            effort = Some(match key.code {
                                KeyCode::Left => current.previous_in(&allowed),
                                _ => current.next_in(&allowed),
                            });
                            self.modal = Some(ActiveModal::Sampling {
                                effort,
                                temperature,
                                field,
                                error: None,
                            });
                        }
                        KeyCode::Char('l' | 'm' | 'h' | 'x') => {
                            let allowed = self.active_model_effort_levels();
                            let picked = match key.code {
                                KeyCode::Char('l') => EffortLevel::Low,
                                KeyCode::Char('h') => EffortLevel::High,
                                KeyCode::Char('x') => EffortLevel::ExtraHigh,
                                _ => EffortLevel::Medium,
                            };
                            effort = Some(picked.clamp_to(&allowed));
                            self.modal = Some(ActiveModal::Sampling {
                                effort,
                                temperature,
                                field,
                                error: None,
                            });
                        }
                        KeyCode::Enter => match self.apply_sampling(effort, &temperature) {
                            Ok(()) => self.modal = None,
                            Err(message) => {
                                self.modal = Some(ActiveModal::Sampling {
                                    effort,
                                    temperature,
                                    field,
                                    error: Some(message),
                                });
                            }
                        },
                        _ => {
                            self.modal = Some(ActiveModal::Sampling {
                                effort,
                                temperature,
                                field,
                                error,
                            });
                        }
                    }
                } else {
                    let action = match key.code {
                        KeyCode::Left | KeyCode::Right
                            if !key.modifiers.intersects(
                                KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT,
                            ) =>
                        {
                            let allowed = self.active_model_effort_levels();
                            let current = effort.unwrap_or_default().clamp_to(&allowed);
                            effort = Some(match key.code {
                                KeyCode::Left => current.previous_in(&allowed),
                                _ => current.next_in(&allowed),
                            });
                            TextAction::Edited
                        }
                        KeyCode::Char('l' | 'm' | 'h' | 'x')
                            if !typing_effort_keyword(&temperature) =>
                        {
                            let allowed = self.active_model_effort_levels();
                            let picked = match key.code {
                                KeyCode::Char('l') => EffortLevel::Low,
                                KeyCode::Char('h') => EffortLevel::High,
                                KeyCode::Char('x') => EffortLevel::ExtraHigh,
                                _ => EffortLevel::Medium,
                            };
                            effort = Some(picked.clamp_to(&allowed));
                            TextAction::Edited
                        }
                        _ => handle_text_key(&mut temperature, key, 16, false, false),
                    };
                    temperature
                        .text
                        .retain(|c| c.is_ascii_alphanumeric() || c == '.');
                    temperature.cursor = temperature.cursor.min(temperature.text.len());
                    match action {
                        TextAction::Submit => match self.apply_sampling(effort, &temperature) {
                            Ok(()) => self.modal = None,
                            Err(message) => {
                                self.modal = Some(ActiveModal::Sampling {
                                    effort,
                                    temperature,
                                    field,
                                    error: Some(message),
                                });
                            }
                        },
                        TextAction::Edited | TextAction::Ignored => {
                            self.modal = Some(ActiveModal::Sampling {
                                effort,
                                temperature,
                                field,
                                error: None,
                            });
                        }
                    }
                }
            }
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
            (ActiveModal::AuthMethodSelect { provider_id, .. }, ModalHitKind::Close) => {
                self.reopen_provider_select_for(&provider_id);
            }
            (ActiveModal::AuthMethodSelect { provider_id, .. }, ModalHitKind::ListItem(idx)) => {
                self.choose_auth_method(&provider_id, idx);
            }
            (ActiveModal::ProviderConnection { provider_id, .. }, ModalHitKind::Close) => {
                self.reopen_provider_select_for(&provider_id);
            }
            (
                ActiveModal::ProviderConnection {
                    provider_id,
                    provider_name,
                    status,
                    ..
                },
                ModalHitKind::ListItem(idx),
            ) => {
                self.apply_provider_connection_action(&provider_id, &provider_name, &status, idx);
            }
            (ActiveModal::ChatGptLogin { .. }, ModalHitKind::Close) => {
                self.cancel_chatgpt_login();
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
                    input, selected, ..
                },
                ModalHitKind::ListItem(idx),
            ) => {
                let candidates = self.model_candidates(&input);
                if let Some(model) = candidates.get(idx) {
                    let (target_provider, target_model) = self.resolve_model_target(model);
                    self.select_provider_and_model(&target_provider, &target_model);
                    self.modal = None;
                } else {
                    self.modal = Some(ActiveModal::ModelInput {
                        input,
                        selected,
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
            (
                ActiveModal::ServeDialog {
                    port_input,
                    secret_input,
                    error,
                    busy,
                    ..
                },
                ModalHitKind::ServeField(target_secret),
            ) => {
                self.modal = Some(ActiveModal::ServeDialog {
                    port_input,
                    secret_input,
                    editing_secret: target_secret,
                    error,
                    busy,
                });
            }
            (ActiveModal::ServeDialog { busy, .. }, ModalHitKind::Close) => {
                if !busy {
                    self.modal = None;
                }
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
            (
                ActiveModal::Sampling {
                    temperature,
                    field,
                    error,
                    ..
                },
                ModalHitKind::EffortTier(tier),
            ) => {
                self.modal = Some(ActiveModal::Sampling {
                    effort: Some(tier),
                    temperature,
                    field,
                    error,
                });
            }
            (ActiveModal::Runs { kind, .. }, ModalHitKind::ListItem(index)) => {
                self.open_run_detail(kind, index);
            }
            (ActiveModal::Runs { .. } | ActiveModal::RunDetail { .. }, ModalHitKind::Close) => {
                self.modal = None;
            }
            (ActiveModal::Terminal { running, .. }, ModalHitKind::Close) => {
                self.modal = None;
                if running {
                    self.status_notice = Some(
                        "Shell running in background; output will appear in the transcript".into(),
                    );
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
            filter: TextInput::default(),
            selected,
        });
    }

    fn activate_provider_at(&mut self, filter: &str, selected: usize) {
        let providers = self.filtered_providers(filter);
        if let Some((provider, status)) = providers.get(selected) {
            self.open_provider_connect(provider, status);
        } else {
            self.modal = Some(ActiveModal::ProviderSelect {
                filter: TextInput::new(filter),
                selected,
            });
        }
    }

    fn open_provider_connect(&mut self, provider: &ProviderInfo, status: &KeyStatus) {
        if status.is_configured() && provider.auth.requires_credential() {
            self.modal = Some(ActiveModal::ProviderConnection {
                provider_id: provider.id.clone(),
                provider_name: provider.display_name.clone(),
                status: status.clone(),
                selected: 0,
            });
            return;
        }
        match provider.auth {
            ProviderAuthKind::None => {
                self.activate_provider_only(&provider.id);
                self.open_model_input();
            }
            ProviderAuthKind::ApiKey => self.open_api_key_input(provider, status),
            ProviderAuthKind::OAuth => self.begin_chatgpt_login(provider),
            ProviderAuthKind::Either => {
                self.modal = Some(ActiveModal::AuthMethodSelect {
                    provider_id: provider.id.clone(),
                    provider_name: provider.display_name.clone(),
                    selected: 0,
                });
            }
        }
    }

    fn apply_provider_connection_action(
        &mut self,
        provider_id: &str,
        provider_name: &str,
        status: &KeyStatus,
        selected: usize,
    ) {
        let Some(action) = ProviderConnectionAction::options(status)
            .get(selected)
            .copied()
        else {
            return;
        };
        match action {
            ProviderConnectionAction::Next => {
                self.activate_provider_only(provider_id);
                self.open_model_input();
            }
            ProviderConnectionAction::Update => {
                let provider = {
                    let rt = self.runtime.read().unwrap();
                    rt.catalog.provider(provider_id).cloned()
                };
                let Some(provider) = provider else {
                    self.modal = None;
                    return;
                };
                match provider.auth {
                    ProviderAuthKind::ApiKey => self.open_api_key_input(&provider, status),
                    ProviderAuthKind::OAuth => self.begin_chatgpt_login(&provider),
                    ProviderAuthKind::Either => {
                        self.modal = Some(ActiveModal::AuthMethodSelect {
                            provider_id: provider.id.clone(),
                            provider_name: provider.display_name.clone(),
                            selected: 0,
                        });
                    }
                    ProviderAuthKind::None => self.open_model_input(),
                }
            }
            ProviderConnectionAction::Remove => match self.secrets.delete_credential(provider_id) {
                Ok(()) => {
                    self.invalidate_key_status();
                    self.status_notice = Some(format!("Removed credential for {provider_id}"));
                    self.reopen_provider_select_for(provider_id);
                }
                Err(error) => {
                    self.status_notice = Some(format!("Failed to remove credential: {error:#}"));
                    self.modal = Some(ActiveModal::ProviderConnection {
                        provider_id: provider_id.to_string(),
                        provider_name: provider_name.to_string(),
                        status: status.clone(),
                        selected,
                    });
                }
            },
        }
    }

    fn open_api_key_input(&mut self, provider: &ProviderInfo, status: &KeyStatus) {
        let saved_url = {
            let rt = self.runtime.read().unwrap();
            rt.base_urls.get(&provider.id).cloned().unwrap_or_default()
        };
        self.modal = Some(ActiveModal::ApiKeyInput {
            provider_id: provider.id.clone(),
            provider_name: provider.display_name.clone(),
            api_key: TextInput::default(),
            base_url: TextInput::new(saved_url),
            editing_base_url: false,
            existing_key_status: status.clone(),
            error: None,
        });
    }

    fn choose_auth_method(&mut self, provider_id: &str, selected: usize) {
        if selected == 0 {
            let provider = {
                let rt = self.runtime.read().unwrap();
                rt.catalog.provider(provider_id).cloned()
            };
            let Some(provider) = provider else {
                self.modal = None;
                return;
            };
            self.begin_chatgpt_login(&provider);
            return;
        }
        let provider = {
            let rt = self.runtime.read().unwrap();
            rt.catalog.provider(provider_id).cloned()
        };
        let Some(provider) = provider else {
            self.modal = None;
            return;
        };
        let status = self.cached_key_status(&provider);
        self.open_api_key_input(&provider, &status);
    }

    fn begin_chatgpt_login(&mut self, provider: &ProviderInfo) {
        self.activate_provider_only(&provider.id);
        self.modal = Some(ActiveModal::ChatGptLogin {
            provider_id: provider.id.clone(),
            provider_name: provider.display_name.clone(),
            url: None,
            input: TextInput::default(),
            error: None,
            busy: true,
        });
        self.pending_login_request = Some(provider.id.clone());
    }

    pub fn apply_login_started(&mut self, url: String) {
        if let Some(ActiveModal::ChatGptLogin {
            url: slot, busy, ..
        }) = &mut self.modal
        {
            *slot = Some(url);
            *busy = false;
        }
    }

    pub fn apply_login_failed(&mut self, message: String) {
        self.login_input = None;
        self.login_cancel = None;
        self.status_notice = Some(message.clone());
        if let Some(ActiveModal::ChatGptLogin { error, busy, .. }) = &mut self.modal {
            *error = Some(message);
            *busy = false;
        }
    }

    pub fn apply_login_succeeded(&mut self, provider_id: &str, account: Option<String>) {
        self.login_input = None;
        self.login_cancel = None;
        self.modal = None;
        self.invalidate_key_status();
        self.reload_settings_from_disk();
        self.activate_provider_only(provider_id);
        self.settings.active_provider = Some(provider_id.to_string());
        self.open_model_input();
        self.status_notice = Some(match account {
            Some(account) => format!("Signed in with ChatGPT as {account}"),
            None => "Signed in with ChatGPT".to_string(),
        });
    }

    fn cancel_chatgpt_login(&mut self) {
        self.pending_login_request = None;
        self.login_input = None;
        self.login_cancel = None;
        self.modal = None;
        self.status_notice = Some("ChatGPT sign-in cancelled".to_string());
    }

    fn reload_settings_from_disk(&mut self) {
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
            filter: TextInput::default(),
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
            PaletteAction::ToggleChanges => {
                self.toggle_changes_panel();
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

    pub(super) fn toggle_diagnose_panel(&mut self) {
        if self.is_sidebar_visible() {
            self.sidebar_diagnose_open = !self.sidebar_diagnose_open;
        } else {
            self.show_sidebar = true;
            self.sidebar_diagnose_open = true;
            if self.terminal_width < MIN_SIDEBAR_WIDTH {
                self.status_notice = Some(format!(
                    "Diagnose needs a terminal width >= {MIN_SIDEBAR_WIDTH} cols"
                ));
            }
        }
    }

    pub(super) fn toggle_changes_panel(&mut self) {
        if self.is_sidebar_visible() {
            self.sidebar_changes_open = !self.sidebar_changes_open;
        } else {
            self.show_sidebar = true;
            self.sidebar_changes_open = true;
            if self.terminal_width < MIN_SIDEBAR_WIDTH {
                self.status_notice = Some(format!(
                    "File Changes needs a terminal width >= {MIN_SIDEBAR_WIDTH} cols"
                ));
            }
        }
    }

    pub(super) fn click_sidebar_diagnose_at(&mut self, column: u16, row: u16) -> bool {
        if self.modal.is_some() || !self.is_sidebar_visible() {
            return false;
        }
        let area = self.sidebar_diagnose_head_area;
        if area.width == 0
            || area.height == 0
            || column < area.x
            || column >= area.x + area.width
            || row < area.y
            || row >= area.y + area.height
        {
            return false;
        }
        self.sidebar_diagnose_open = !self.sidebar_diagnose_open;
        true
    }

    pub(super) fn click_sidebar_changes_at(&mut self, column: u16, row: u16) -> bool {
        if self.modal.is_some() || !self.is_sidebar_visible() {
            return false;
        }
        let area = self.sidebar_changes_head_area;
        if area.width == 0
            || area.height == 0
            || column < area.x
            || column >= area.x + area.width
            || row < area.y
            || row >= area.y + area.height
        {
            return false;
        }
        self.sidebar_changes_open = !self.sidebar_changes_open;
        true
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

        let context_window_tokens = self.model_context_window(&canonical, &next_model);
        let effort_levels = {
            let rt = self.runtime.read().unwrap();
            rt.catalog.effort_levels_for(&canonical, &next_model)
        };

        self.tab_mut().model = next_model.clone();
        self.settings.active_model = (!next_model.is_empty()).then_some(next_model);
        self.sync_completion_models();
        self.persist_settings();
        if let Some(spec) = spec {
            let _ = self.tab().cmd_tx.send(AgentCommand::SetModel {
                model: spec,
                provider: Some(canonical.clone()),
                context_window_tokens,
                effort_levels,
            });
        }
        self.send_plan_usage(&canonical);
    }

    pub(super) fn send_plan_usage(&self, provider_id: &str) {
        let provider = {
            let rt = self.runtime.read().unwrap();
            rt.catalog.provider(provider_id).cloned()
        };
        let plan_usage = provider.is_some_and(|provider| {
            matches!(
                self.cached_key_status(&provider),
                KeyStatus::OAuth {
                    needs_relogin: false,
                    ..
                }
            )
        });
        if let Ok(mut runtime) = self.runtime.write() {
            runtime.plan_usage = plan_usage;
        }
        let _ = self
            .tab()
            .cmd_tx
            .send(AgentCommand::SetPlanUsage { plan_usage });
    }

    pub(super) fn select_provider_and_model(&mut self, provider_id: &str, model_id: &str) {
        let canonical = canonical_provider_id(provider_id);
        let clean_model = model_id.trim().to_string();
        self.tab_mut().active_provider = canonical.clone();
        self.tab_mut().model = clean_model.clone();
        self.settings
            .record_provider_model(&canonical, &clean_model);
        self.settings.active_model = Some(clean_model.clone());

        let spec = {
            let mut rt = self.runtime.write().unwrap();
            rt.active_provider = canonical.clone();
            rt.catalog.format_genai_spec(&canonical, &clean_model)
        };

        self.sync_completion_models();
        self.persist_settings();
        let context_window_tokens = self.model_context_window(&canonical, &clean_model);
        let effort_levels = {
            let rt = self.runtime.read().unwrap();
            rt.catalog.effort_levels_for(&canonical, &clean_model)
        };
        let _ = self.tab().cmd_tx.send(AgentCommand::SetModel {
            model: spec.clone(),
            provider: Some(canonical.clone()),
            context_window_tokens,
            effort_levels,
        });
        self.send_plan_usage(&canonical);
        let session_id = {
            let tab = self.tab_mut();
            tab.transcript.push(TranscriptEntry::System {
                text: format!("Active model: {canonical}/{clean_model}"),
            });
            tab.session_id.clone()
        };
        if let Some(reg) = &self.registry {
            reg.note_tui_model(&session_id, &canonical, &spec);
        }
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
            filter: TextInput::default(),
            selected,
        });
    }

    pub(super) fn open_model_input(&mut self) {
        self.modal = Some(ActiveModal::ModelInput {
            input: TextInput::default(),
            selected: None,
            error: None,
        });
    }
}

impl AppState {
    fn stage_diff_path(&mut self, path: &str, stage: bool) -> Result<Option<usize>, String> {
        if stage {
            stage_file(&self.workspace_root, path)?;
        } else {
            unstage_file(&self.workspace_root, path)?;
        }
        self.file_changes = list_git_changes(&self.workspace_root);
        Ok(self
            .file_changes
            .iter()
            .position(|entry| entry.path == path))
    }
}

fn collect_question_answers(
    request: &QuestionRequest,
    answers: &[Vec<usize>],
    custom: &[TextInput],
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

fn queue_modal_host_action(app: &mut AppState, action: meerai_host::HostAction) {
    let tab_id = app.tab().tab_id;
    app.pending_session_actions
        .push(PendingSessionAction::HostAction { tab_id, action });
}

fn subagent_spec_from_panel(panel: &SubagentSpawnPanel) -> Option<meerai_agents::SubagentSpec> {
    let max_steps = parse_optional(&panel.steps)?;
    let max_tokens = parse_optional(&panel.tokens)?;
    let max_cost_usd = if panel.cost.trim().is_empty() {
        None
    } else {
        Some(panel.cost.trim().parse::<f64>().ok()?)
    };
    let allowed_tools = if panel.tools.trim().is_empty() {
        None
    } else {
        let tools: Vec<String> = panel
            .tools
            .split(',')
            .map(str::trim)
            .filter(|tool| !tool.is_empty())
            .map(str::to_string)
            .collect();
        (!tools.is_empty()).then_some(tools)
    };
    Some(meerai_agents::SubagentSpec {
        name: (!panel.name.trim().is_empty()).then(|| panel.name.trim().to_string()),
        agent: panel.agent.trim().to_string(),
        task: panel.task.trim().to_string(),
        model: (!panel.model.trim().is_empty()).then(|| panel.model.trim().to_string()),
        approval_mode: panel.mode,
        allowed_tools,
        max_steps,
        max_tokens,
        max_cost_usd,
    })
}

fn parse_optional<T: std::str::FromStr>(raw: &str) -> Option<Option<T>> {
    let raw = raw.trim();
    if raw.is_empty() {
        Some(None)
    } else {
        raw.parse().ok().map(Some)
    }
}

fn typing_effort_keyword(temperature: &str) -> bool {
    temperature
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
}
