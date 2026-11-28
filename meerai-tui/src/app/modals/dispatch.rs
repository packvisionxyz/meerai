use super::*;

impl AppState {
    pub(crate) fn handle_modal_key(&mut self, key: KeyEvent) -> bool {
        let Some(modal) = self.modal.take() else {
            return false;
        };
        let has_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let has_alt = key.modifiers.contains(KeyModifiers::ALT);

        match modal {
            ActiveModal::ProviderSelect { .. }
            | ActiveModal::AuthMethodSelect { .. }
            | ActiveModal::ProviderConnection { .. }
            | ActiveModal::ChatGptLogin { .. }
            | ActiveModal::ApiKeyInput { .. }
            | ActiveModal::ModelInput { .. } => self.keys_provider(modal, key, has_ctrl, has_alt),
            ActiveModal::Approval { .. }
            | ActiveModal::Question { .. }
            | ActiveModal::UserMessageActions { .. }
            | ActiveModal::CommandPalette { .. }
            | ActiveModal::QueuedEdit { .. }
            | ActiveModal::Terminal { .. } => self.keys_chat(modal, key, has_ctrl, has_alt),
            ActiveModal::DiffView { .. }
            | ActiveModal::Search { .. }
            | ActiveModal::Runs { .. }
            | ActiveModal::McpServers { .. }
            | ActiveModal::RunDetail { .. }
            | ActiveModal::RewindMenu { .. }
            | ActiveModal::SessionSelect { .. }
            | ActiveModal::ServeDialog { .. } => self.keys_panels(modal, key, has_ctrl, has_alt),
            ActiveModal::Context(_)
            | ActiveModal::Loop { .. }
            | ActiveModal::Memory { .. }
            | ActiveModal::Routing { .. }
            | ActiveModal::Packs { .. }
            | ActiveModal::Sampling { .. } => self.keys_insight(modal, key, has_ctrl, has_alt),
            ActiveModal::VaultReset { .. } | ActiveModal::SubagentSpawn(_) => {
                self.keys_misc(modal, key, has_ctrl, has_alt)
            }
        }
    }
    pub(crate) fn modal_hit_at(&self, col: u16, row: u16) -> Option<ModalHitKind> {
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
    pub(crate) fn handle_modal_mouse_move(&mut self, col: u16, row: u16) -> bool {
        let hit = self.modal_hit_at(col, row);
        if hit == self.hovered_modal_hit {
            return false;
        }
        self.hovered_modal_hit = hit;
        true
    }
    pub(crate) fn handle_modal_click(&mut self, col: u16, row: u16) -> bool {
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
            (
                ActiveModal::Runs { .. }
                | ActiveModal::McpServers { .. }
                | ActiveModal::RunDetail { .. },
                ModalHitKind::Close,
            ) => {
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
    pub(crate) fn toggle_sidebar(&mut self) {
        self.show_sidebar = !self.show_sidebar;
        if self.show_sidebar && self.terminal_width < MIN_SIDEBAR_WIDTH {
            self.status_notice = Some(format!(
                "Sidebar hidden until terminal width >= {MIN_SIDEBAR_WIDTH} cols"
            ));
        }
    }
    pub(crate) fn toggle_diagnose_panel(&mut self) {
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
    pub(crate) fn toggle_changes_panel(&mut self) {
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
    pub(crate) fn click_sidebar_diagnose_at(&mut self, column: u16, row: u16) -> bool {
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
    pub(crate) fn click_sidebar_changes_at(&mut self, column: u16, row: u16) -> bool {
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
    pub(crate) fn send_plan_usage(&self, provider_id: &str) {
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
    pub(crate) fn open_model_input(&mut self) {
        self.modal = Some(ActiveModal::ModelInput {
            input: TextInput::default(),
            selected: None,
            error: None,
        });
    }
}
