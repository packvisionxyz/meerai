use super::*;

impl AppState {
    pub fn handle_terminal_event(&mut self, event: Event) -> bool {
        match event {
            Event::Key(key) => self.handle_key_event(key),
            Event::Paste(text) => {
                let workspace = self.workspace.clone();
                if let Some(modal) = &mut self.modal {
                    let clean = text.replace(['\r', '\n'], "");
                    match modal {
                        ActiveModal::Terminal { .. } => {}
                        ActiveModal::ProviderSelect { filter, selected } => {
                            filter.insert_str(&clean);
                            *selected = 0;
                        }
                        ActiveModal::ApiKeyInput {
                            api_key,
                            base_url,
                            editing_base_url,
                            error,
                            ..
                        } => {
                            if *editing_base_url {
                                base_url.insert_str(&clean);
                            } else {
                                api_key.insert_str(&clean);
                            }
                            *error = None;
                        }
                        ActiveModal::ModelInput {
                            input,
                            selected,
                            error,
                        } => {
                            input.insert_str(&clean);
                            *selected = None;
                            *error = None;
                        }
                        ActiveModal::ChatGptLogin { input, error, .. } => {
                            input.insert_str(&clean);
                            *error = None;
                        }
                        ActiveModal::DiffView { .. }
                        | ActiveModal::AuthMethodSelect { .. }
                        | ActiveModal::ProviderConnection { .. }
                        | ActiveModal::Loop { .. }
                        | ActiveModal::Memory { .. }
                        | ActiveModal::Routing { .. }
                        | ActiveModal::Packs { .. }
                        | ActiveModal::Runs { .. }
                        | ActiveModal::RunDetail { .. }
                        | ActiveModal::RewindMenu { .. } => {}
                        ActiveModal::Context(panel) => {
                            if let Some(input) = panel.focus_input.as_mut() {
                                input.insert_str(&clean);
                            }
                        }
                        ActiveModal::VaultReset { input, error } => {
                            input.insert_str(&clean);
                            *error = None;
                        }
                        ActiveModal::Sampling {
                            temperature, error, ..
                        } => {
                            temperature.insert_str(&clean);
                            *error = None;
                        }
                        ActiveModal::SubagentSpawn(panel) => {
                            match panel.field {
                                0 => panel.name.insert_str(&clean),
                                1 => panel.agent.insert_str(&clean),
                                2 => panel.task.insert_str(&text),
                                3 => panel.model.insert_str(&clean),
                                5 => panel.steps.insert_str(&clean),
                                6 => panel.tokens.insert_str(&clean),
                                7 => panel.cost.insert_str(&clean),
                                8 => panel.tools.insert_str(&clean),
                                _ => {}
                            }
                            panel.error = None;
                        }
                        ActiveModal::QueuedEdit { input, error, .. } => {
                            input.insert_str(&text);
                            *error = None;
                        }
                        ActiveModal::Approval {
                            reason,
                            editing_reason,
                            ..
                        } => {
                            if *editing_reason {
                                reason.insert_str(&text);
                            }
                        }
                        ActiveModal::Question {
                            custom,
                            editing_custom,
                            current,
                            focus,
                            ..
                        } => {
                            let other = custom.len();
                            if *editing_custom || *focus == other {
                                if let Some(input) = custom.get_mut(*current) {
                                    input.insert_str(&text);
                                }
                                *editing_custom = true;
                            }
                        }
                        ActiveModal::Search {
                            query,
                            mode,
                            workspace_files,
                            results,
                            selected,
                            preview,
                        } => {
                            query.insert_str(&clean);
                            Self::refresh_search(
                                SearchSources {
                                    workspace: workspace.as_ref(),
                                    workspace_root: &self.workspace_root,
                                    mode: *mode,
                                    workspace_files,
                                    query,
                                },
                                SearchOutput {
                                    results,
                                    selected,
                                    preview,
                                },
                            );
                        }
                        ActiveModal::SessionSelect {
                            filter, selected, ..
                        } => {
                            filter.insert_str(&clean);
                            *selected = 0;
                        }
                        ActiveModal::CommandPalette { filter, selected } => {
                            filter.insert_str(&clean);
                            *selected = 0;
                        }
                        ActiveModal::UserMessageActions { .. } => {}
                        ActiveModal::ServeDialog {
                            port_input,
                            secret_input,
                            editing_secret,
                            error,
                            busy,
                        } => {
                            if !*busy {
                                if *editing_secret {
                                    secret_input.insert_str(&clean);
                                } else {
                                    port_input.extend(clean.chars().filter(char::is_ascii_digit));
                                }
                                *error = None;
                            }
                        }
                    }
                    return true;
                }
                if self.mode == InputMode::Insert {
                    self.insert_str(&text);
                    self.update_completion();
                    return true;
                }
                false
            }
            Event::Mouse(mouse) => self.handle_mouse_event(mouse),
            Event::Resize(w, _) => {
                self.terminal_width = w;
                true
            }
            _ => false,
        }
    }
    pub fn handle_mouse_event(&mut self, mouse: MouseEvent) -> bool {
        let pos = ScreenPos {
            col: mouse.column,
            row: mouse.row,
        };
        if matches!(
            mouse.kind,
            MouseEventKind::Down(_) | MouseEventKind::Drag(_) | MouseEventKind::Moved
        ) {
            self.last_mouse_pos = Some(pos);
        }

        if self.modal.is_some() {
            self.tab_drag = None;
            if self.handle_subagent_selection_mouse(
                mouse.kind,
                mouse.column,
                mouse.row,
                mouse.modifiers,
            ) {
                return true;
            }
            return match mouse.kind {
                MouseEventKind::Moved => self.handle_modal_mouse_move(mouse.column, mouse.row),
                MouseEventKind::Down(MouseButton::Left) => {
                    self.handle_modal_click(mouse.column, mouse.row)
                }
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                    self.handle_modal_scroll(mouse.kind)
                }
                _ => false,
            };
        }

        if mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && let Some(hit) = self.status_hit_at(mouse.column, mouse.row)
        {
            match hit {
                StatusHitKind::Run(kind) => self.open_runs(kind),
                StatusHitKind::Update => {
                    self.handle_slash_command("/update");
                }
                StatusHitKind::WebServer => {
                    if let Some(server) = self.web_server.clone() {
                        self.copy_text_to_clipboard(&server.url);
                        self.status_notice = Some(format!("Copied {}", server.url));
                    }
                }
            }
            return true;
        }

        if self.anchor_rail_drag
            || (mouse.kind == MouseEventKind::Down(MouseButton::Left)
                && rect_contains(self.anchor_rail_area, mouse.column, mouse.row))
        {
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    self.anchor_rail_drag = true;
                    self.jump_from_anchor_rail(mouse.row, true);
                    return true;
                }
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.jump_from_anchor_rail(mouse.row, false);
                    return true;
                }
                MouseEventKind::Up(_) => {
                    self.anchor_rail_drag = false;
                    return true;
                }
                _ => {}
            }
        }

        if let Some(drag) = self.tab_drag {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Moved => {
                    self.tab_drag = Some(TabDrag {
                        current_x: mouse.column,
                        moved: drag.moved || drag.start_x.abs_diff(mouse.column) > 2,
                        ..drag
                    });
                    return true;
                }
                MouseEventKind::Up(button) => {
                    self.tab_drag = None;
                    if drag.moved {
                        let drop = self.tab_drop_index(mouse.column);
                        self.selection = None;
                        self.doc_selection = None;
                        self.move_tab(drag.from, drop);
                        return true;
                    }
                    if button != MouseButton::Left {
                        return true;
                    }
                }
                _ => {}
            }
        }

        if mouse.kind == MouseEventKind::Moved {
            if let Some(anchor) = self.composer_drag_anchor {
                self.extend_composer_drag(anchor, mouse.column, mouse.row);
                return true;
            }
            let hovered_entry = self.hovered_entry_at(pos);
            let hovered_tab = self.tab_hit_at(mouse.column, mouse.row);
            let hovered_file = self.hovered_file_change_at(mouse.column, mouse.row);
            let hovered_completion = self.completion_hit_at(mouse.column, mouse.row);
            let hovered_rail = self.anchor_rail_hover_row(mouse.column, mouse.row);
            let hovered_queued = self.queued_hit_at(mouse.column, mouse.row);
            let hovered_status = self.status_hit_at(mouse.column, mouse.row);
            let hovered_sidebar = self.sidebar_hit_at(mouse.column, mouse.row);
            let changed = hovered_entry != self.hovered_entry
                || hovered_tab != self.hovered_tab
                || hovered_file != self.hovered_file_change
                || hovered_completion != self.hovered_completion
                || hovered_rail != self.anchor_rail_hover
                || hovered_queued != self.hovered_queued
                || hovered_status != self.hovered_status
                || hovered_sidebar != self.hovered_sidebar;
            self.hovered_entry = hovered_entry;
            self.hovered_tab = hovered_tab;
            self.hovered_file_change = hovered_file;
            self.hovered_completion = hovered_completion;
            self.anchor_rail_hover = hovered_rail;
            self.hovered_queued = hovered_queued;
            self.hovered_status = hovered_status;
            self.hovered_sidebar = hovered_sidebar;
            return changed;
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.modal.is_none() && self.click_completion_at(mouse.column, mouse.row) {
                    self.selection = None;
                    self.doc_selection = None;
                    return true;
                }
                if self.modal.is_none() && self.click_tab_at(mouse.column, mouse.row) {
                    self.selection = None;
                    self.doc_selection = None;
                    return true;
                }
                if self.click_sidebar_diagnose_at(mouse.column, mouse.row) {
                    self.selection = None;
                    self.doc_selection = None;
                    return true;
                }
                if self.click_sidebar_changes_at(mouse.column, mouse.row) {
                    self.selection = None;
                    self.doc_selection = None;
                    return true;
                }
                if self.modal.is_none() && self.click_file_change_at(mouse.column, mouse.row) {
                    self.selection = None;
                    self.doc_selection = None;
                    return true;
                }
                if self.modal.is_none() && self.click_path_at(mouse.column, mouse.row) {
                    self.selection = None;
                    self.doc_selection = None;
                    return true;
                }
                if self.modal.is_none() && self.composer_pointer_down(mouse.column, mouse.row) {
                    self.selection = None;
                    self.doc_selection = None;
                    return true;
                }
                if self.modal.is_none() && self.click_queued_at(mouse.column, mouse.row) {
                    self.selection = None;
                    self.doc_selection = None;
                    return true;
                }
                self.composer_selection = None;
                self.composer_drag_anchor = None;

                self.pending_toggle = self.entry_at(mouse.column, mouse.row);
                self.pending_digest = self.digest_at(mouse.column, mouse.row);

                if let Some(doc_pos) = self.screen_to_doc_pos_exact(mouse.column, mouse.row) {
                    self.selection = None;
                    if mouse.modifiers.contains(KeyModifiers::SHIFT)
                        && let Some(anchor) = self.last_transcript_anchor
                    {
                        let sel = DocSelection {
                            anchor,
                            head: doc_pos,
                            active: false,
                        };
                        let text = self.extract_doc_selected_text(sel);
                        if !text.is_empty() {
                            self.copy_text_to_clipboard(&text);
                        }
                        self.doc_selection = Some(sel);
                        return true;
                    }
                    let streak = self.register_click(mouse.column, mouse.row);
                    if streak >= 2
                        && let Some((start, end)) = self.word_bounds_at(doc_pos.line, doc_pos.col)
                    {
                        let row = self.transcript_wrapped_lines.get(doc_pos.line);
                        let start_col = row.map_or(start, |row| char_to_display(&row.text, start));
                        let end_col = row.map_or(end, |row| char_to_display(&row.text, end));
                        let sel = DocSelection {
                            anchor: DocPos {
                                line: doc_pos.line,
                                col: start_col,
                            },
                            head: DocPos {
                                line: doc_pos.line,
                                col: end_col,
                            },
                            active: false,
                        };
                        let text = self.extract_doc_selected_text(sel);
                        if !text.is_empty() {
                            self.copy_text_to_clipboard(&text);
                        }
                        self.doc_selection = Some(sel);
                        self.last_transcript_anchor = Some(sel.anchor);
                        return true;
                    }
                    self.last_transcript_anchor = Some(doc_pos);
                    self.doc_selection = Some(DocSelection {
                        anchor: doc_pos,
                        head: doc_pos,
                        active: true,
                    });
                    return true;
                }

                self.doc_selection = None;
                self.selection = Some(TextSelection {
                    anchor: pos,
                    head: pos,
                    active: true,
                });
                return true;
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(anchor) = self.composer_drag_anchor {
                    self.extend_composer_drag(anchor, mouse.column, mouse.row);
                    return true;
                }
                if let Some(mut sel) = self.doc_selection
                    && sel.active
                {
                    let inner = self.transcript_inner_area;
                    if inner.height > 0 {
                        if mouse.row < inner.y {
                            self.scroll_up(1);
                        } else if mouse.row >= inner.y + inner.height {
                            self.scroll_down(1);
                        }
                    }
                    if let Some(head) = self.screen_to_doc_pos_clamped(mouse.column, mouse.row) {
                        if head != sel.anchor {
                            self.pending_toggle = None;
                            self.pending_digest = None;
                        }
                        sel.head = head;
                    }
                    self.doc_selection = Some(sel);
                    return true;
                }
                if let Some(sel) = &mut self.selection {
                    if pos != sel.anchor {
                        self.pending_toggle = None;
                        self.pending_digest = None;
                    }
                    sel.head = pos;
                    sel.active = true;
                    return true;
                }
                return false;
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.composer_drag_anchor = None;
                if let Some((start, end)) = self.composer_selection {
                    if start >= end {
                        self.composer_selection = None;
                    } else {
                        let text = self
                            .tab()
                            .input
                            .get(start..end)
                            .unwrap_or_default()
                            .to_string();
                        if !text.is_empty() {
                            self.copy_text_to_clipboard(&text);
                        }
                        return true;
                    }
                }
                let mut was_click = true;
                if let Some(mut sel) = self.doc_selection.take()
                    && sel.active
                {
                    if let Some(head) = self.screen_to_doc_pos_clamped(mouse.column, mouse.row) {
                        sel.head = head;
                    }
                    sel.active = false;
                    if sel.anchor != sel.head {
                        was_click = false;
                        let text = self.extract_doc_selected_text(sel);
                        if !text.is_empty() {
                            self.copy_text_to_clipboard(&text);
                        }
                    }
                } else if let Some(mut sel) = self.selection.take() {
                    sel.head = pos;
                    sel.active = false;
                    if sel.anchor != sel.head {
                        was_click = false;
                        let text = self.extract_selected_text(sel);
                        if !text.is_empty() {
                            self.copy_text_to_clipboard(&text);
                        }
                    }
                }
                if was_click {
                    if let Some(idx) = self.pending_digest.take() {
                        self.toggle_turn_activity(idx);
                    } else if let Some(idx) = self.pending_toggle.take() {
                        self.toggle_entry_index(idx);
                    }
                }
                return true;
            }
            _ => {}
        }

        match mouse.kind {
            MouseEventKind::ScrollUp if self.sidebar_changes_contains(mouse.column, mouse.row) => {
                self.scroll_file_changes(true);
                true
            }
            MouseEventKind::ScrollDown
                if self.sidebar_changes_contains(mouse.column, mouse.row) =>
            {
                self.scroll_file_changes(false);
                true
            }
            MouseEventKind::ScrollUp => {
                self.scroll_up(3);
                self.refresh_active_doc_selection_head();
                true
            }
            MouseEventKind::ScrollDown => {
                self.scroll_down(3);
                self.refresh_active_doc_selection_head();
                true
            }
            _ => false,
        }
    }
    pub(crate) fn handle_modal_scroll(&mut self, kind: MouseEventKind) -> bool {
        if let Some(ActiveModal::DiffView { diff, scroll, .. }) = &mut self.modal {
            let max_scroll = diff.rows.len().saturating_sub(1);
            return match kind {
                MouseEventKind::ScrollUp => {
                    *scroll = scroll.saturating_sub(3);
                    true
                }
                MouseEventKind::ScrollDown => {
                    *scroll = (*scroll + 3).min(max_scroll);
                    true
                }
                _ => false,
            };
        }
        if let Some(ActiveModal::Terminal { output, scroll, .. }) = &mut self.modal {
            let max = output
                .as_deref()
                .map(|body| body.lines().count().saturating_sub(1))
                .unwrap_or(0);
            return match kind {
                MouseEventKind::ScrollUp => {
                    *scroll = scroll.saturating_sub(3);
                    true
                }
                MouseEventKind::ScrollDown => {
                    *scroll = (*scroll + 3).min(max);
                    true
                }
                _ => false,
            };
        }
        let detail_max = self.run_detail_max_scroll;
        if let Some(ActiveModal::RunDetail { scroll, .. }) = &mut self.modal {
            return match kind {
                MouseEventKind::ScrollUp => {
                    *scroll = scroll.saturating_sub(3);
                    true
                }
                MouseEventKind::ScrollDown => {
                    *scroll = (*scroll + 3).min(detail_max);
                    true
                }
                _ => false,
            };
        }
        if let Some(ActiveModal::Search {
            preview: Some(prev),
            ..
        }) = &mut self.modal
        {
            let max_scroll = prev.lines.len().saturating_sub(6);
            return match kind {
                MouseEventKind::ScrollUp => {
                    prev.scroll = prev.scroll.saturating_sub(3);
                    true
                }
                MouseEventKind::ScrollDown => {
                    prev.scroll = (prev.scroll + 3).min(max_scroll);
                    true
                }
                _ => false,
            };
        }
        false
    }
    pub fn handle_key_event(&mut self, key: KeyEvent) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            self.pending_g = false;
            self.handle_ctrl_c();
            return true;
        }

        self.last_ctrl_c = None;
        self.status_notice = None;

        if self.modal.is_some() {
            return self.handle_modal_key(key);
        }

        if key.code == KeyCode::BackTab {
            self.cycle_approval_mode();
            return true;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('b') => {
                    self.toggle_sidebar();
                    return true;
                }
                KeyCode::Char('f' | 'F') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.handle_slash_command("/grep");
                    return true;
                }
                KeyCode::Char('f') => {
                    self.open_search_modal(SearchMode::Files, "");
                    return true;
                }
                KeyCode::Char('w') => {
                    self.close_active_tab();
                    return true;
                }
                KeyCode::Char('p') | KeyCode::Char('P') if !self.completion.is_active() => {
                    self.open_command_palette();
                    return true;
                }
                KeyCode::Char('n') if !self.completion.is_active() => {
                    self.handle_slash_command("/new");
                    return true;
                }
                KeyCode::Char('k') => {
                    self.handle_slash_command("/compact");
                    return true;
                }
                KeyCode::Char('r' | 'R') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.handle_slash_command("/reset");
                    return true;
                }
                KeyCode::Char('q') => {
                    self.quit();
                    return true;
                }
                KeyCode::Char('s') => {
                    self.open_session_select();
                    return true;
                }
                KeyCode::Char('o') => {
                    self.open_model_input();
                    return true;
                }
                KeyCode::Char('a') => {
                    self.open_provider_select();
                    return true;
                }
                KeyCode::Char('e') => {
                    self.open_first_diff();
                    return true;
                }
                KeyCode::PageUp => {
                    self.prev_tab();
                    return true;
                }
                KeyCode::PageDown => {
                    self.next_tab();
                    return true;
                }
                _ => {}
            }
        }

        if key.code == KeyCode::F(1) {
            self.handle_slash_command("/help");
            return true;
        }

        if key.modifiers.contains(KeyModifiers::ALT) {
            let shift = key.modifiers.contains(KeyModifiers::SHIFT);
            match key.code {
                KeyCode::Left if shift => {
                    if self.active_tab > 0 {
                        let active = self.active_tab;
                        self.move_tab(active, active - 1);
                    }
                    return true;
                }
                KeyCode::Right if shift => {
                    let active = self.active_tab;
                    if active + 1 < self.tabs.len() {
                        self.move_tab(active, active + 1);
                    }
                    return true;
                }
                KeyCode::Left => {
                    self.prev_tab();
                    return true;
                }
                KeyCode::Right => {
                    self.next_tab();
                    return true;
                }
                KeyCode::Char(c @ '1'..='9') => {
                    let target = (c as usize) - ('1' as usize);
                    if target < self.tabs.len() {
                        self.switch_to_tab(target);
                        return true;
                    }
                }
                _ => {}
            }
        }

        match self.mode {
            InputMode::Insert => self.handle_insert_key(key),
            InputMode::Normal => self.handle_normal_key(key),
        }
    }
}
