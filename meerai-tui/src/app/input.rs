use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use meerai_agents::AgentCommand;
use meerai_core::text::{ceil_char_boundary, floor_char_boundary};
use ratatui::layout::Rect;

use super::commands::parse_mentions;
use super::*;
use crate::completion::CompletionKind;
use crate::search::SearchMode;

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
                            filter.push_str(&clean);
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
                                base_url.push_str(&clean);
                            } else {
                                api_key.push_str(&clean);
                            }
                            *error = None;
                        }
                        ActiveModal::ModelInput {
                            input,
                            selected,
                            error,
                        } => {
                            input.push_str(&clean);
                            *selected = None;
                            *error = None;
                        }
                        ActiveModal::ChatGptLogin { input, error, .. } => {
                            input.push_str(&clean);
                            *error = None;
                        }
                        ActiveModal::DiffView { .. }
                        | ActiveModal::AuthMethodSelect { .. }
                        | ActiveModal::ProviderConnection { .. }
                        | ActiveModal::Question { .. }
                        | ActiveModal::Context(_)
                        | ActiveModal::Goal { .. }
                        | ActiveModal::Memory { .. }
                        | ActiveModal::Routing { .. }
                        | ActiveModal::Packs { .. }
                        | ActiveModal::Runs { .. }
                        | ActiveModal::RunDetail { .. }
                        | ActiveModal::RewindMenu { .. } => {}
                        ActiveModal::VaultReset { input, error } => {
                            input.push_str(&clean);
                            *error = None;
                        }
                        ActiveModal::Sampling {
                            temperature, error, ..
                        } => {
                            temperature.push_str(&clean);
                            *error = None;
                        }
                        ActiveModal::SubagentSpawn(panel) => {
                            match panel.field {
                                0 => panel.agent.push_str(&clean),
                                1 => panel.task.push_str(&clean),
                                2 => panel.model.push_str(&clean),
                                3 => panel.steps.push_str(&clean),
                                4 => panel.tokens.push_str(&clean),
                                5 => panel.cost.push_str(&clean),
                                6 => panel.tools.push_str(&clean),
                                _ => {}
                            }
                            panel.error = None;
                        }
                        ActiveModal::QueuedEdit {
                            input,
                            cursor,
                            error,
                            ..
                        } => {
                            let at = (*cursor).min(input.len());
                            input.insert_str(at, &clean);
                            *cursor = at + clean.len();
                            *error = None;
                        }
                        ActiveModal::Approval {
                            reason,
                            editing_reason,
                            ..
                        } => {
                            if *editing_reason {
                                reason.push_str(&clean);
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
                            query.push_str(&clean);
                            Self::refresh_search(
                                workspace.as_ref(),
                                &self.workspace_root,
                                *mode,
                                workspace_files,
                                query,
                                results,
                                selected,
                                preview,
                            );
                        }
                        ActiveModal::SessionSelect {
                            filter, selected, ..
                        } => {
                            filter.push_str(&clean);
                            *selected = 0;
                        }
                        ActiveModal::CommandPalette { filter, selected } => {
                            filter.push_str(&clean);
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
                                    secret_input.push_str(&clean);
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

    fn handle_modal_scroll(&mut self, kind: MouseEventKind) -> bool {
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

    fn refresh_active_doc_selection_head(&mut self) {
        let Some(mut sel) = self.doc_selection else {
            return;
        };
        if !sel.active {
            return;
        }
        if let Some(pos) = self.last_mouse_pos
            && let Some(head) = self.screen_to_doc_pos_clamped(pos.col, pos.row)
        {
            sel.head = head;
            self.doc_selection = Some(sel);
        }
    }

    fn register_click(&mut self, column: u16, row: u16) -> u8 {
        const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(400);
        let now = Instant::now();
        let streak = match self.last_click {
            Some((at, last_col, last_row))
                if now.duration_since(at) <= DOUBLE_CLICK_WINDOW
                    && last_col == column
                    && last_row == row =>
            {
                self.click_streak.saturating_add(1).min(3)
            }
            _ => 1,
        };
        self.last_click = Some((now, column, row));
        self.click_streak = streak;
        streak
    }

    pub(crate) fn word_bounds_at(&self, line: usize, col: usize) -> Option<(usize, usize)> {
        let row = self.transcript_wrapped_lines.get(line)?;
        let chars: Vec<char> = row.text.chars().collect();
        if chars.is_empty() {
            return None;
        }
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let skip = row.gutter_len.max(row.hang_len);
        let click = display_to_char(&row.text, col).min(chars.len() - 1);
        let click = if click < skip {
            let first = chars.get(skip..)?.iter().position(|&c| is_word(c))?;
            skip + first
        } else {
            click
        };
        if !is_word(chars[click]) {
            return None;
        }
        let mut start = click;
        while start > skip && is_word(chars[start - 1]) {
            start -= 1;
        }
        let mut end = click;
        while end + 1 < chars.len() && is_word(chars[end + 1]) {
            end += 1;
        }
        Some((start, end))
    }

    pub fn screen_to_doc_pos_exact(&self, column: u16, row: u16) -> Option<DocPos> {
        let inner = self.transcript_inner_area;
        if self.modal.is_some()
            || self.transcript_wrapped_lines.is_empty()
            || inner.width == 0
            || inner.height == 0
            || column < inner.x
            || column >= inner.x + inner.width
            || row < inner.y
            || row >= inner.y + inner.height
        {
            return None;
        }
        self.screen_to_doc_pos_clamped(column, row)
    }

    fn screen_to_doc_pos_clamped(&self, column: u16, row: u16) -> Option<DocPos> {
        let inner = self.transcript_inner_area;
        if self.transcript_wrapped_lines.is_empty() || inner.width == 0 || inner.height == 0 {
            return None;
        }
        let max_line = self.transcript_wrapped_lines.len() - 1;
        let max_row = inner.y + inner.height - 1;
        let max_col = inner.x + inner.width - 1;
        let doc_line = if row < inner.y {
            self.tab().scroll_offset.min(max_line)
        } else if row > max_row {
            (self.tab().scroll_offset + (inner.height as usize - 1)).min(max_line)
        } else {
            (self.tab().scroll_offset + (row - inner.y) as usize).min(max_line)
        };
        let doc_col = if row < inner.y || column < inner.x {
            0
        } else if row > max_row || column > max_col {
            inner.width as usize - 1
        } else {
            (column - inner.x) as usize
        };
        Some(DocPos {
            line: doc_line,
            col: doc_col,
        })
    }

    pub fn extract_doc_selected_text(&self, sel: DocSelection) -> String {
        selected_text_from_lines(
            &self.transcript_wrapped_lines,
            self.transcript_inner_area.width,
            sel,
        )
    }

    fn extract_subagent_selected_text(&self, sel: DocSelection) -> String {
        selected_text_from_lines(
            &self.subagent_view_cache.doc_lines,
            self.subagent_view_cache.body.width,
            sel,
        )
    }

    fn handle_subagent_selection_mouse(
        &mut self,
        kind: MouseEventKind,
        column: u16,
        row: u16,
        modifiers: KeyModifiers,
    ) -> bool {
        let subagent = matches!(
            self.modal,
            Some(ActiveModal::RunDetail {
                run: RunTarget::Subagent(_),
                ..
            })
        );
        if !subagent {
            return false;
        }
        let body = self.subagent_view_cache.body;
        let inside = rect_contains(body, column, row);
        match kind {
            MouseEventKind::Down(MouseButton::Left) if inside => {
                let Some(pos) = self.subagent_doc_pos(column, row) else {
                    return false;
                };
                if modifiers.contains(KeyModifiers::SHIFT)
                    && let Some(anchor) = self.subagent_view_cache.anchor
                {
                    let sel = DocSelection {
                        anchor,
                        head: pos,
                        active: false,
                    };
                    let text = self.extract_subagent_selected_text(sel);
                    if !text.is_empty() {
                        self.copy_text_to_clipboard(&text);
                    }
                    self.subagent_view_cache.selection = Some(sel);
                    return true;
                }
                self.subagent_view_cache.anchor = Some(pos);
                self.subagent_view_cache.selection = Some(DocSelection {
                    anchor: pos,
                    head: pos,
                    active: true,
                });
                true
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(mut sel) = self.subagent_view_cache.selection else {
                    return false;
                };
                if !sel.active {
                    return false;
                }
                if row < body.y {
                    self.scroll_subagent_view(-1);
                } else if row >= body.y.saturating_add(body.height) {
                    self.scroll_subagent_view(1);
                }
                if let Some(pos) = self.subagent_doc_pos(column, row) {
                    sel.head = pos;
                }
                self.subagent_view_cache.selection = Some(sel);
                true
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let Some(mut sel) = self.subagent_view_cache.selection.take() else {
                    return false;
                };
                if !sel.active {
                    return false;
                }
                sel.active = false;
                if sel.anchor != sel.head {
                    let text = self.extract_subagent_selected_text(sel);
                    if !text.is_empty() {
                        self.copy_text_to_clipboard(&text);
                    }
                }
                self.subagent_view_cache.selection = Some(sel);
                true
            }
            _ => false,
        }
    }

    fn subagent_doc_pos(&self, column: u16, row: u16) -> Option<DocPos> {
        let body = self.subagent_view_cache.body;
        let lines = &self.subagent_view_cache.doc_lines;
        if lines.is_empty() || body.width == 0 || body.height == 0 {
            return None;
        }
        let max_line = lines.len() - 1;
        let scroll = self.subagent_view_cache.scroll;
        let line = if row < body.y {
            scroll.min(max_line)
        } else if row >= body.y.saturating_add(body.height) {
            (scroll + body.height as usize - 1).min(max_line)
        } else {
            (scroll + (row - body.y) as usize).min(max_line)
        };
        let col = if row < body.y || column < body.x {
            0
        } else if column >= body.x.saturating_add(body.width) {
            body.width as usize - 1
        } else {
            (column - body.x) as usize
        };
        Some(DocPos { line, col })
    }

    fn scroll_subagent_view(&mut self, delta: isize) {
        if let Some(ActiveModal::RunDetail { scroll, .. }) = &mut self.modal {
            let max = self.run_detail_max_scroll;
            let next = if delta < 0 {
                scroll.saturating_sub(1)
            } else {
                scroll.saturating_add(1).min(max)
            };
            *scroll = next;
            self.subagent_view_cache.scroll = next;
        }
    }

    pub fn selection_bounds(&self, anchor: ScreenPos) -> Rect {
        let inner = self.transcript_inner_area;
        if self.modal.is_none()
            && inner.width > 0
            && inner.height > 0
            && anchor.col >= inner.x
            && anchor.col < inner.x + inner.width
            && anchor.row >= inner.y
            && anchor.row < inner.y + inner.height
        {
            inner
        } else {
            Rect::new(0, 0, self.terminal_width, self.screen_lines.len() as u16)
        }
    }

    pub fn extract_selected_text(&self, sel: TextSelection) -> String {
        if self.screen_lines.is_empty() {
            return String::new();
        }
        let bounds = self.selection_bounds(sel.anchor);
        let Some((start, end)) = normalize_selection_in_bounds(sel.anchor, sel.head, bounds) else {
            return String::new();
        };
        let max_screen_row = self.screen_lines.len().saturating_sub(1) as u16;
        let row_start = start.row.min(max_screen_row);
        let row_end = end.row.min(max_screen_row);
        if row_start > row_end {
            return String::new();
        }

        let min_col = bounds.x;
        let max_col = bounds.x + bounds.width - 1;
        let in_transcript =
            bounds == self.transcript_inner_area && self.transcript_inner_area.width > 0;
        let mut lines = Vec::new();
        for row in row_start..=row_end {
            let line_cells = &self.screen_lines[row as usize];
            if line_cells.is_empty() {
                lines.push(String::new());
                continue;
            }
            let c_start = (if row == row_start { start.col } else { min_col })
                .max(min_col)
                .min(max_col) as usize;
            let c_end = (if row == row_end { end.col } else { max_col })
                .max(min_col)
                .min(max_col) as usize;
            if c_start >= line_cells.len() || c_start > c_end {
                lines.push(String::new());
                continue;
            }
            let clamped_end = c_end.min(line_cells.len() - 1);
            let mut raw_line: String = line_cells[c_start..=clamped_end]
                .iter()
                .map(String::as_str)
                .collect();
            if in_transcript && c_start == min_col as usize {
                raw_line = raw_line
                    .strip_prefix("┃ ")
                    .map(str::to_string)
                    .unwrap_or(raw_line);
            }
            lines.push(raw_line.trim_end().to_string());
        }

        while lines.first().is_some_and(String::is_empty) {
            lines.remove(0);
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("\n")
    }

    pub fn copy_text_to_clipboard(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        write_system_clipboard(text);
        self.last_copied_text = Some(text.to_string());
        self.status_notice = Some(format!(
            "Copied {} chars to clipboard",
            text.chars().count()
        ));
    }

    fn sidebar_changes_contains(&self, column: u16, row: u16) -> bool {
        let area = self.sidebar_changes_inner_area;
        area.width > 0
            && area.height > 0
            && column >= area.x
            && column < area.x + area.width
            && row >= area.y
            && row < area.y + area.height
    }

    pub(super) fn scroll_file_changes(&mut self, up: bool) -> bool {
        let shown = self.visible_file_change_rows.max(1);
        let max_start = self.file_changes.len().saturating_sub(shown);
        if max_start == 0 {
            return false;
        }
        self.file_change_scroll = if up {
            self.file_change_scroll.saturating_sub(1)
        } else {
            (self.file_change_scroll + 1).min(max_start)
        };
        true
    }

    pub fn sidebar_hit_at(&self, column: u16, row: u16) -> Option<SidebarHitKind> {
        if self.modal.is_some() || !self.is_sidebar_visible() {
            return None;
        }
        let contains = |area: Rect| {
            area.width > 0
                && area.height > 0
                && column >= area.x
                && column < area.x + area.width
                && row >= area.y
                && row < area.y + area.height
        };
        if contains(self.sidebar_diagnose_head_area) {
            return Some(SidebarHitKind::DiagnoseHead);
        }
        if contains(self.sidebar_changes_head_area) {
            return Some(SidebarHitKind::ChangesHead);
        }
        if self.file_change_more_hit.is_some_and(contains) {
            return Some(SidebarHitKind::ChangesMore);
        }
        None
    }

    pub fn status_hit_at(&self, column: u16, row: u16) -> Option<StatusHitKind> {
        if let Some((_, kind)) = self
            .runs_hits
            .iter()
            .find(|(rect, _)| rect_contains(*rect, column, row))
        {
            return Some(StatusHitKind::Run(*kind));
        }
        if self
            .update_hit
            .is_some_and(|hit| rect_contains(hit, column, row))
        {
            return Some(StatusHitKind::Update);
        }
        if self
            .web_server_hit
            .is_some_and(|hit| rect_contains(hit, column, row))
        {
            return Some(StatusHitKind::WebServer);
        }
        None
    }

    pub fn hovered_file_change_at(&self, column: u16, row: u16) -> Option<usize> {
        if !self.is_sidebar_visible()
            || self.modal.is_some()
            || !self.sidebar_changes_contains(column, row)
        {
            return None;
        }
        let area = self.sidebar_changes_inner_area;
        if area.width == 0
            || area.height == 0
            || column < area.x
            || column >= area.x + area.width
            || row < area.y
            || row >= area.y + area.height
        {
            return None;
        }
        let row_idx = (row - area.y) as usize;
        if row_idx >= self.visible_file_change_rows {
            return None;
        }
        let idx = self.file_change_scroll + row_idx;
        if idx >= self.file_changes.len() {
            return None;
        }
        Some(idx)
    }

    pub fn click_file_change_at(&mut self, column: u16, row: u16) -> bool {
        if let Some(hit) = self.file_change_more_hit
            && column >= hit.x
            && column < hit.x + hit.width
            && row >= hit.y
            && row < hit.y + hit.height
        {
            let shown = self.visible_file_change_rows.max(1);
            let max_start = self.file_changes.len().saturating_sub(shown);
            self.file_change_scroll = if self.file_change_scroll >= max_start {
                0
            } else {
                (self.file_change_scroll + shown).min(max_start)
            };
            return true;
        }
        let Some(row_idx) = self.hovered_file_change_at(column, row) else {
            return false;
        };
        self.open_diff_modal(row_idx);
        true
    }

    pub fn path_reference_at(&self, column: u16, row: u16) -> Option<meerai_syntax::PathRef> {
        let area = self.transcript_area;
        if area.width == 0 || area.height == 0 {
            return None;
        }
        if column < area.x
            || column >= area.x + area.width
            || row < area.y
            || row >= area.y + area.height
        {
            return None;
        }
        let scroll = self
            .tabs
            .get(self.active_tab)
            .map_or(0, |tab| tab.scroll_offset);
        let line_index = scroll + (row - area.y) as usize;
        let line = self.transcript_wrapped_lines.get(line_index)?;
        let offset = display_to_char(&line.text, (column - area.x) as usize);
        if offset < line.gutter_len.max(line.hang_len) {
            return None;
        }
        let byte_start = char_to_byte(&line.text, offset);
        let byte_end = char_to_byte(&line.text, offset.saturating_add(1));
        if byte_start >= byte_end || byte_end > line.text.len() {
            return None;
        }
        if !line.text.is_char_boundary(byte_start) || !line.text.is_char_boundary(byte_end) {
            return None;
        }
        let byte = line.text.as_bytes()[byte_start];
        if !is_path_byte(byte) {
            return None;
        }
        let start = line.text[..byte_start]
            .rfind(|ch: char| !is_path_byte(ch as u8))
            .map_or(0, |index| index + 1);
        let end = line.text[byte_start..]
            .find(|ch: char| !is_path_byte(ch as u8))
            .map_or(line.text.len(), |index| byte_start + index);
        let token = &line.text[start..end];
        let reference = meerai_syntax::detect_path_reference(token)?;
        if !self.workspace_root.join(&reference.path).is_file() {
            return None;
        }
        Some(reference)
    }

    pub fn click_path_at(&mut self, column: u16, row: u16) -> bool {
        let Some(reference) = self.path_reference_at(column, row) else {
            return false;
        };
        self.queue_host_action(meerai_host::HostAction::OpenInEditor {
            path: reference.path,
            line: reference.line,
        });
        true
    }

    pub fn tab_hit_at(&self, column: u16, row: u16) -> Option<TabHitKind> {
        self.tab_hit_zones
            .iter()
            .find(|zone| {
                let r = zone.rect;
                r.width > 0
                    && r.height > 0
                    && column >= r.x
                    && column < r.x + r.width
                    && row >= r.y
                    && row < r.y + r.height
            })
            .map(|zone| zone.kind)
    }

    pub fn completion_hit_at(&self, column: u16, row: u16) -> Option<usize> {
        if !self.completion.is_active() || self.modal.is_some() || self.mode != InputMode::Insert {
            return None;
        }
        self.completion_hit_zones
            .iter()
            .find(|zone| {
                let r = zone.rect;
                r.width > 0
                    && r.height > 0
                    && column >= r.x
                    && column < r.x + r.width
                    && row >= r.y
                    && row < r.y + r.height
            })
            .map(|zone| zone.index)
    }

    pub fn click_completion_at(&mut self, column: u16, row: u16) -> bool {
        let Some(index) = self.completion_hit_at(column, row) else {
            return false;
        };
        self.completion.set_selected(index);
        self.accept_completion();
        true
    }

    pub(crate) fn tab_drop_index(&self, column: u16) -> usize {
        let last = self.tabs.len().saturating_sub(1);
        let over_zone = self
            .tab_hit_zones
            .iter()
            .find(|zone| {
                let r = zone.rect;
                r.width > 0 && column >= r.x && column < r.x + r.width
            })
            .map(|zone| zone.kind);
        if let Some(kind) = over_zone {
            return match kind {
                TabHitKind::Tab(index) | TabHitKind::CloseTab(index) => index.min(last),
                TabHitKind::NewTab => last,
            };
        }
        self.tab_hit_zones
            .iter()
            .filter_map(|zone| match zone.kind {
                TabHitKind::Tab(index) if zone.rect.x > column => Some((zone.rect.x, index)),
                _ => None,
            })
            .min_by_key(|(x, _)| *x)
            .map_or(last, |(_, index)| index.min(last))
    }

    pub fn click_tab_at(&mut self, column: u16, row: u16) -> bool {
        let Some(kind) = self.tab_hit_at(column, row) else {
            return false;
        };

        match kind {
            TabHitKind::Tab(index) => {
                let _ = self.switch_to_tab(index);
                if self.modal.is_none() {
                    self.tab_drag = Some(TabDrag {
                        from: index,
                        start_x: column,
                        current_x: column,
                        moved: false,
                    });
                }
                true
            }
            TabHitKind::CloseTab(index) => self.close_tab(index),
            TabHitKind::NewTab => {
                self.request_new_tab();
                true
            }
        }
    }

    pub fn hovered_entry_at(&self, pos: ScreenPos) -> Option<usize> {
        if self.modal.is_some() {
            return None;
        }
        self.hovered_zone_at(self.transcript_inner_area, self.effective_scroll(), pos)
    }

    pub(crate) fn hovered_entry_at_screen(&self, area: Rect, scroll: usize) -> Option<usize> {
        if self.modal.is_some() {
            return None;
        }
        self.hovered_zone_at(area, scroll, self.last_mouse_pos?)
    }

    pub(crate) fn hovered_digest_at_screen(&self, area: Rect, scroll: usize) -> Option<usize> {
        if self.modal.is_some() {
            return None;
        }
        let pos = self.last_mouse_pos?;
        if area.width == 0
            || pos.col < area.x
            || pos.col >= area.x + area.width
            || pos.row < area.y
            || pos.row >= area.y + area.height
        {
            return None;
        }
        let row = scroll.saturating_add((pos.row - area.y) as usize);
        self.digest_hit_zones
            .iter()
            .find(|zone| row >= zone.start_line && row < zone.end_line)
            .map(|zone| zone.entry_idx)
    }

    fn hovered_zone_at(&self, area: Rect, scroll: usize, pos: ScreenPos) -> Option<usize> {
        if area.width == 0
            || pos.col < area.x
            || pos.col >= area.x + area.width
            || pos.row < area.y
            || pos.row >= area.y + area.height
        {
            return None;
        }
        let row = scroll.saturating_add((pos.row - area.y) as usize);
        self.hover_zones
            .iter()
            .find(|zone| row >= zone.start_line && row < zone.end_line)
            .map(|zone| zone.entry_idx)
    }

    fn entry_at(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.transcript_inner_area;
        if area.width == 0
            || area.height == 0
            || column < area.x
            || column >= area.x + area.width
            || row < area.y
            || row >= area.y + area.height
        {
            return None;
        }
        let virtual_line = self
            .effective_scroll()
            .saturating_add((row - area.y) as usize);
        self.thinking_hit_zones
            .iter()
            .find(|z| virtual_line >= z.start_line && virtual_line < z.end_line)
            .map(|z| z.entry_idx)
    }

    pub fn toggle_entry_at(&mut self, column: u16, row: u16) -> bool {
        match self.entry_at(column, row) {
            Some(idx) => self.toggle_entry_index(idx),
            None => false,
        }
    }

    fn open_first_diff(&mut self) -> bool {
        if self.file_changes.is_empty() {
            self.status_notice = Some("No file changes to review".to_string());
        } else {
            self.open_diff_modal(0);
        }
        true
    }

    fn digest_at(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.transcript_inner_area;
        if area.width == 0
            || area.height == 0
            || column < area.x
            || column >= area.x + area.width
            || row < area.y
            || row >= area.y + area.height
        {
            return None;
        }
        let virtual_line = self
            .effective_scroll()
            .saturating_add((row - area.y) as usize);
        self.digest_hit_zones
            .iter()
            .find(|z| virtual_line >= z.start_line && virtual_line < z.end_line)
            .map(|z| z.entry_idx)
    }

    fn toggle_turn_activity(&mut self, group_idx: usize) -> bool {
        if !crate::ui::transcript::is_activity_group_start(&self.tab().transcript, group_idx) {
            return false;
        }
        let tab = self.tab_mut();
        let expanded = tab.turn_activity.get(&group_idx).copied().unwrap_or(false);
        tab.turn_activity.insert(group_idx, !expanded);
        true
    }

    fn toggle_entry_index(&mut self, entry_idx: usize) -> bool {
        let mut load_child: Option<String> = None;
        let toggled = match self.tab_mut().transcript.get_mut(entry_idx) {
            Some(TranscriptEntry::Assistant {
                reasoning,
                reasoning_expanded,
                ..
            }) if !reasoning.is_empty() => {
                *reasoning_expanded = !*reasoning_expanded;
                true
            }
            Some(TranscriptEntry::Error { expanded, .. }) => {
                *expanded = !*expanded;
                true
            }
            Some(TranscriptEntry::ToolCall(tc)) => {
                tc.expanded = !tc.expanded;
                true
            }
            Some(TranscriptEntry::Subagent(entry)) if entry.child_session_id.is_some() => {
                let item_id = entry.id.clone();
                self.subagent_view_cache.selection = None;
                self.subagent_view_cache.anchor = None;
                self.modal = Some(ActiveModal::RunDetail {
                    run: RunTarget::Subagent(entry_idx),
                    selected: 0,
                    scroll: 0,
                });
                load_child = Some(item_id);
                true
            }
            Some(TranscriptEntry::User { .. }) => {
                self.modal = Some(ActiveModal::UserMessageActions {
                    entry_idx,
                    selected: 0,
                });
                true
            }
            _ => false,
        };
        if let Some(item_id) = load_child {
            self.request_child_session_load(&item_id, true);
        }
        toggled
    }

    pub(super) fn run_user_action(&mut self, entry_idx: usize, action: UserAction) -> bool {
        let text = match self.tab().transcript.get(entry_idx) {
            Some(TranscriptEntry::User { text, .. }) => text.clone(),
            _ => return false,
        };
        match action {
            UserAction::Copy => {
                self.copy_text_to_clipboard(&text);
                self.status_notice = Some("Copied message".to_string());
                true
            }
            UserAction::Rewind => self.rewind_to_entry(entry_idx, text),
            UserAction::Fork => self.fork_from_entry(entry_idx, text),
        }
    }

    fn rewind_to_entry(&mut self, entry_idx: usize, message: String) -> bool {
        if self.tab().is_running {
            self.status_notice = Some("Cancel the running turn before rewinding".to_string());
            return true;
        }
        let turn = self.tab().user_turns.get(&entry_idx).cloned();
        let Some(turn_id) = turn else {
            self.status_notice = Some("Rewind is unavailable for this message".to_string());
            return true;
        };
        let _ = self.tab().cmd_tx.send(AgentCommand::Rewind {
            turn_id,
            scope: meerai_agents::RewindScope::Both,
        });
        let tab = self.tab_mut();
        tab.transcript.truncate(entry_idx);
        tab.user_turns.retain(|idx, _| *idx < entry_idx);
        tab.turn_activity.retain(|idx, _| *idx < entry_idx);
        tab.auto_follow = true;
        self.restore_composer_message(message);
        self.status_notice =
            Some("Rewound code and conversation; message restored to the composer".to_string());
        true
    }

    pub(super) fn restore_composer_message(&mut self, message: String) {
        self.mode = InputMode::Insert;
        self.composer_selection = None;
        self.composer_goal_col = None;
        self.composer_drag_anchor = None;
        let tab = self.tab_mut();
        tab.cursor = message.len();
        tab.input = message;
    }

    fn fork_from_entry(&mut self, entry_idx: usize, draft: String) -> bool {
        if self.tab().is_running {
            self.status_notice = Some("Cancel the running turn before forking".to_string());
            return true;
        }
        if self.session_opener.is_none() {
            self.status_notice = Some("Fork needs an open session".to_string());
            return true;
        }
        let session_id = self.tab().session_id.clone();
        let previous_turn = self.tab().transcript[..entry_idx]
            .iter()
            .enumerate()
            .rev()
            .find(|(_, entry)| matches!(entry, TranscriptEntry::User { .. }))
            .and_then(|(idx, _)| self.tab().user_turns.get(&idx).cloned());
        self.pending_session_actions
            .push(PendingSessionAction::ForkSession {
                session_id,
                turn: previous_turn,
                draft,
            });
        self.status_notice = Some("Forking session...".to_string());
        true
    }

    fn queued_hit_at(&self, column: u16, row: u16) -> Option<QueuedHitKind> {
        self.queued_hit_zones
            .iter()
            .find(|zone| {
                column >= zone.rect.x
                    && column < zone.rect.x + zone.rect.width
                    && row >= zone.rect.y
                    && row < zone.rect.y + zone.rect.height
            })
            .map(|zone| zone.kind)
    }

    fn click_queued_at(&mut self, column: u16, row: u16) -> bool {
        let Some(kind) = self.queued_hit_at(column, row) else {
            return false;
        };
        match kind {
            QueuedHitKind::Remove(idx) => {
                let removed = {
                    let tab = self.tab_mut();
                    if idx < tab.queued.len() {
                        Some(tab.queued.remove(idx))
                    } else {
                        None
                    }
                };
                if let Some(text) = removed {
                    let preview = meerai_core::text::truncate_single_line(&text, 40);
                    self.status_notice = Some(format!("Removed queued: {preview}"));
                }
            }
            QueuedHitKind::Edit(idx) => {
                if let Some(text) = self.tab().queued.get(idx).cloned() {
                    self.queued_editor_goal_col = None;
                    self.modal = Some(ActiveModal::QueuedEdit {
                        index: idx,
                        cursor: text.len(),
                        input: text,
                        error: None,
                    });
                }
            }
        }
        true
    }

    fn composer_pointer_down(&mut self, column: u16, row: u16) -> bool {
        let Some(cursor) = self.composer_cursor_at(column, row) else {
            return false;
        };
        let cursor = cursor.min(self.tab().input.len());
        self.composer_selection = None;
        self.composer_goal_col = None;
        self.composer_drag_anchor = Some(cursor);
        self.tab_mut().cursor = cursor;
        self.mode = InputMode::Insert;
        if self.register_click(column, row) >= 2
            && let Some((start, end)) = self.composer_word_bounds_at(cursor)
        {
            self.composer_selection = Some((start, end));
            self.composer_drag_anchor = Some(start);
            self.tab_mut().cursor = end;
        }
        self.update_completion();
        true
    }

    fn composer_cursor_at(&self, column: u16, row: u16) -> Option<usize> {
        let block = self.composer_block_area;
        if block.width == 0
            || block.height == 0
            || column < block.x
            || column >= block.x + block.width
            || row < block.y
            || row >= block.y + block.height
        {
            return None;
        }
        self.composer_cursor_in_block(column, row)
    }

    fn composer_cursor_clamped(&self, column: u16, row: u16) -> Option<usize> {
        let block = self.composer_block_area;
        if block.width == 0 || block.height == 0 {
            return None;
        }
        self.composer_cursor_in_block(column, row)
    }

    fn composer_cursor_in_block(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.composer_input_area;
        if area.width == 0 || area.height == 0 {
            return Some(self.tab().cursor);
        }
        let input = self.tab().input.clone();
        let visual_row = if row < area.y {
            self.composer_scroll
        } else {
            let relative = (row - area.y) as usize;
            self.composer_scroll + relative.min(area.height.saturating_sub(1) as usize)
        };
        let display_col = column.saturating_sub(area.x) as usize;
        let row = crate::ui::composer::composer_row(
            &input,
            self.tab().cursor,
            area.width as usize,
            visual_row,
            &self.agents,
            &self.skills,
        );
        Some(crate::ui::composer::composer_byte_at(
            &input,
            row.logical_line,
            row.start + display_col.min(row.width),
            &self.agents,
            &self.skills,
        ))
    }

    fn composer_word_bounds_at(&self, cursor: usize) -> Option<(usize, usize)> {
        let input = self.tab().input.as_str();
        if input.is_empty() {
            return None;
        }
        let cursor = cursor.min(input.len());
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let mut start = cursor;
        while let Some((idx, c)) = input[..start].char_indices().next_back() {
            if !is_word(c) {
                break;
            }
            start = idx;
        }
        let mut end = cursor;
        for (idx, c) in input[cursor..].char_indices() {
            if !is_word(c) {
                break;
            }
            end = cursor + idx + c.len_utf8();
        }
        (start < end).then_some((start, end))
    }

    fn extend_composer_drag(&mut self, anchor: usize, column: u16, row: u16) {
        let head = self
            .composer_cursor_at(column, row)
            .or_else(|| self.composer_cursor_clamped(column, row))
            .unwrap_or(anchor);
        let (start, end) = if anchor <= head {
            (anchor, head)
        } else {
            (head, anchor)
        };
        self.composer_selection = (start < end).then_some((start, end));
        self.tab_mut().cursor = head;
    }

    fn delete_composer_selection(&mut self) -> bool {
        let Some((start, end)) = self.composer_selection.take() else {
            return false;
        };
        if start >= end {
            return false;
        }
        let tab = self.tab_mut();
        let start = start.min(tab.input.len());
        let end = end.min(tab.input.len());
        tab.input.replace_range(start..end, "");
        tab.cursor = start;
        self.update_completion();
        true
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

    pub(super) fn cycle_approval_mode(&mut self) {
        let next = match self.tab().approval_mode {
            ApprovalMode::Plan => ApprovalMode::Ask,
            ApprovalMode::Ask => ApprovalMode::Auto,
            ApprovalMode::Auto => ApprovalMode::Plan,
        };
        let session_id = {
            let tab = self.tab_mut();
            tab.approval_mode = next;
            let _ = tab.cmd_tx.send(AgentCommand::SetPermissionMode(next));
            tab.session_id.clone()
        };
        if let Some(reg) = &self.registry {
            reg.note_tui_mode(&session_id, next);
        }
        self.status_notice = Some(format!("Approval mode: {}", approval_mode_label(next)));
    }

    pub(super) fn handle_ctrl_c(&mut self) {
        if self.tab().is_running {
            let _ = self.tab().cmd_tx.send(AgentCommand::Cancel);
            self.status_notice = Some("Cancelling turn...".to_string());
            self.last_ctrl_c = None;
            return;
        }

        if self
            .last_ctrl_c
            .is_some_and(|t| t.elapsed() < DOUBLE_CTRL_C_WINDOW)
        {
            self.quit();
        } else {
            self.last_ctrl_c = Some(Instant::now());
            self.status_notice = Some("Press ctrl+c again to exit".to_string());
        }
    }

    pub(super) fn handle_insert_key(&mut self, key: KeyEvent) -> bool {
        let has_shift_or_alt = key.modifiers.contains(KeyModifiers::SHIFT)
            || key.modifiers.contains(KeyModifiers::ALT);
        let has_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        if !matches!(key.code, KeyCode::Up | KeyCode::Down) {
            self.composer_goal_col = None;
        }

        if self.completion.is_active() {
            match key.code {
                KeyCode::Up => {
                    self.completion.select_prev();
                    return true;
                }
                KeyCode::Down => {
                    self.completion.select_next();
                    return true;
                }
                KeyCode::Char('p') if has_ctrl => {
                    self.completion.select_prev();
                    return true;
                }
                KeyCode::Char('n') if has_ctrl => {
                    self.completion.select_next();
                    return true;
                }
                KeyCode::Tab => {
                    self.accept_completion();
                    return true;
                }
                KeyCode::Enter if !has_shift_or_alt && !has_ctrl => {
                    let is_slash = self.completion.kind() == Some(CompletionKind::SlashCommand);
                    let noop = self.completion.selected_is_noop(&self.tab().input);
                    if noop {
                        self.completion.reset();
                        self.submit_input();
                    } else {
                        self.accept_completion();
                        if is_slash && !self.tab().input.ends_with(' ') {
                            self.submit_input();
                        }
                    }
                    return true;
                }
                KeyCode::Esc => {
                    self.completion.dismiss();
                    return true;
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Char('g') if has_ctrl => {
                self.send_now();
            }
            KeyCode::Esc => {
                self.mode = InputMode::Normal;
            }
            KeyCode::Enter if has_shift_or_alt || has_ctrl => {
                self.insert_char('\n');
                self.update_completion();
            }
            KeyCode::Char('j') if has_ctrl => {
                self.insert_char('\n');
                self.update_completion();
            }
            KeyCode::Enter => {
                if self.consume_line_continuation_backslash() {
                    self.insert_char('\n');
                    self.update_completion();
                } else {
                    self.submit_input();
                }
            }
            KeyCode::Backspace => {
                self.backspace();
                self.update_completion();
            }
            KeyCode::Delete => {
                self.delete_forward();
                self.update_completion();
            }
            KeyCode::Left => {
                self.move_left();
                self.update_completion();
            }
            KeyCode::Right => {
                self.move_right();
                self.update_completion();
            }
            KeyCode::Home => {
                self.move_home();
                self.update_completion();
            }
            KeyCode::End => {
                self.move_end();
                self.update_completion();
            }
            KeyCode::Up => {
                self.move_up();
                self.update_completion();
            }
            KeyCode::Down => {
                self.move_down();
                self.update_completion();
            }
            KeyCode::Char(' ')
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let quick_search = match self.tab().input.as_str() {
                    "/search" => Some(SearchMode::Files),
                    "/grep" => Some(SearchMode::Content),
                    _ => None,
                };
                if let Some(mode) = quick_search {
                    let tab = self.tab_mut();
                    tab.input.clear();
                    tab.cursor = 0;
                    self.completion.reset();
                    self.open_search_modal(mode, "");
                    return true;
                }
                self.insert_char(' ');
                self.update_completion();
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert_char(c);
                self.update_completion();
            }
            _ => return false,
        }

        true
    }

    pub(super) fn handle_normal_key(&mut self, key: KeyEvent) -> bool {
        let has_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let had_pending_g = self.pending_g;

        if key.code != KeyCode::Char('g') || has_ctrl {
            self.pending_g = false;
        }

        match key.code {
            KeyCode::Char('t') if had_pending_g && !has_ctrl => {
                self.next_tab();
                return true;
            }
            KeyCode::Char('T') if had_pending_g && !has_ctrl => {
                self.prev_tab();
                return true;
            }
            KeyCode::Char('[') if !has_ctrl => {
                self.prev_tab();
                return true;
            }
            KeyCode::Char(']') if !has_ctrl => {
                self.next_tab();
                return true;
            }
            KeyCode::Char('i') | KeyCode::Char('a') if !has_ctrl => {
                self.mode = InputMode::Insert;
            }
            KeyCode::Char('j') if !has_ctrl => {
                self.scroll_down(1);
            }
            KeyCode::Down => {
                self.scroll_down(1);
            }
            KeyCode::Char('k') if !has_ctrl => {
                self.scroll_up(1);
            }
            KeyCode::Up => {
                self.scroll_up(1);
            }
            KeyCode::Char('d') if has_ctrl => {
                self.scroll_half_page_down();
            }
            KeyCode::Char('u') if has_ctrl => {
                self.scroll_half_page_up();
            }
            KeyCode::Char('g') if !has_ctrl => {
                if had_pending_g {
                    self.pending_g = false;
                    self.scroll_to_top();
                } else {
                    self.pending_g = true;
                }
            }
            KeyCode::Char('G') => {
                self.scroll_to_bottom();
            }
            KeyCode::Char('y') if !has_ctrl => {
                return self.copy_latest_reply();
            }
            KeyCode::Char('/') if !has_ctrl => {
                self.open_search_modal(SearchMode::Files, "");
            }
            KeyCode::Char('q') if !has_ctrl => {
                self.quit();
            }
            KeyCode::Esc => {
                if self.tab().is_running {
                    let _ = self.tab().cmd_tx.send(AgentCommand::Cancel);
                    self.status_notice = Some("Cancelling turn...".to_string());
                } else {
                    self.doc_selection = None;
                }
            }
            _ => return false,
        }

        true
    }

    pub(super) fn copy_latest_reply(&mut self) -> bool {
        let text = self
            .tab()
            .transcript
            .iter()
            .rev()
            .find_map(|entry| match entry {
                TranscriptEntry::Assistant { text, .. } if !text.is_empty() => Some(text.clone()),
                _ => None,
            })
            .or_else(|| {
                self.tab()
                    .transcript
                    .iter()
                    .rev()
                    .find_map(|entry| match entry {
                        TranscriptEntry::Error { text, .. } | TranscriptEntry::System { text }
                            if !text.is_empty() =>
                        {
                            Some(text.clone())
                        }
                        _ => None,
                    })
            });
        if let Some(content) = text {
            self.copy_text_to_clipboard(&content);
            return true;
        }
        false
    }

    pub(super) fn update_completion(&mut self) {
        let tab = &self.tabs[self.active_tab];
        if tab.input.contains('@') {
            self.completion.set_definitions(&self.agents, &self.skills);
        }
        self.completion.update(&tab.input, tab.cursor);
    }

    pub(super) fn accept_completion(&mut self) {
        let accepted = {
            let tab = &self.tabs[self.active_tab];
            self.completion.accept(&tab.input, tab.cursor)
        };
        if let Some((new_input, new_cursor)) = accepted {
            let tab = self.tab_mut();
            tab.input = new_input;
            tab.cursor = new_cursor;
            self.update_completion();
        }
    }

    pub(super) fn submit_input(&mut self) {
        let (trimmed, raw_input) = {
            let tab = self.tab();
            (tab.input.trim().to_string(), tab.input.clone())
        };
        if trimmed.is_empty() {
            return;
        }

        if trimmed.starts_with('/') {
            let tab = self.tab_mut();
            tab.input.clear();
            tab.cursor = 0;
            self.completion.reset();
            self.handle_slash_command(&trimmed);
            return;
        }

        if let Some(command) = trimmed.strip_prefix('!') {
            let command = command.trim().to_string();
            if command.is_empty() {
                return;
            }
            {
                let tab = self.tab_mut();
                tab.input.clear();
                tab.cursor = 0;
            }
            if self.session_opener.is_none() {
                self.status_notice = Some("Shell commands are not available".to_string());
                return;
            }
            if self.tab().approval_mode == ApprovalMode::Plan {
                self.status_notice = Some("Shell commands are disabled in plan mode".to_string());
                self.tab_mut().transcript.push(TranscriptEntry::Error {
                    text: "Shell commands are disabled in plan mode".to_string(),
                    expanded: false,
                });
                self.completion.reset();
                return;
            }
            let (tab_id, run_id) = self.start_shell_run(command.clone());
            let tab = self.tab_mut();
            tab.auto_follow = true;
            tab.transcript_version = tab.transcript_version.wrapping_add(1);
            self.modal = Some(ActiveModal::Terminal {
                run_id,
                command: command.clone(),
                output: None,
                running: true,
                scroll: 0,
            });
            self.pending_session_actions
                .push(PendingSessionAction::RunUserShell {
                    tab_id,
                    run_id,
                    command,
                });
            self.completion.reset();
            return;
        }

        let plan = parse_mentions(&raw_input, &self.agents, &self.skills);
        let display = raw_input.trim_end().to_string();
        let remaining = plan.text.clone().unwrap_or_else(|| display.clone());

        if remaining.is_empty() {
            self.apply_mentions(&plan);
            self.acknowledge_mentions(&plan);
            let tab = self.tab_mut();
            tab.input.clear();
            tab.cursor = 0;
            self.completion.reset();
            self.tab_mut().auto_follow = true;
            return;
        }

        if !self.tab().is_running && self.tab().model.trim().is_empty() {
            self.status_notice = Some("Set a model first (/model)".to_string());
            self.open_model_input();
            return;
        }

        self.apply_mentions(&plan);

        if self.tab().is_running {
            let tab = self.tab_mut();
            tab.input.clear();
            tab.cursor = 0;
            tab.auto_follow = true;
            tab.queued.push(remaining);
            let depth = tab.queued.len();
            self.status_notice = Some(format!("Queued {depth} message(s) · ctrl+g sends now"));
            self.completion.reset();
            return;
        }

        let session_id = {
            let tab = self.tab_mut();
            if tab.title.is_none() {
                tab.title = normalize_tab_title(remaining.clone());
            }
            tab.input.clear();
            tab.cursor = 0;
            tab.auto_follow = true;
            tab.is_running = true;
            tab.transcript.push(TranscriptEntry::User {
                text: display.clone(),
            });
            let _ = tab
                .cmd_tx
                .send(AgentCommand::SubmitPrompt(remaining.clone()));
            tab.session_id.clone()
        };
        if let Some(reg) = &self.registry {
            self.tui_prompt_echoes
                .push_back((session_id.clone(), display.clone()));
            reg.note_tui_user_prompt(&session_id, &remaining, Some(&display), false);
        }
        self.completion.reset();
    }

    pub(super) fn send_now(&mut self) {
        if !self.tab().is_running {
            self.submit_input();
            return;
        }
        let display = {
            let tab = self.tab_mut();
            if let Some(next) = tab.queued.first().cloned() {
                tab.queued.remove(0);
                next
            } else {
                let typed = tab.input.trim().to_string();
                if typed.is_empty() {
                    return;
                }
                tab.input.clear();
                tab.cursor = 0;
                typed
            }
        };
        let plan = parse_mentions(&display, &self.agents, &self.skills);
        self.apply_mentions(&plan);
        let text = plan.text.unwrap_or_else(|| display.clone());
        let session_id = {
            let tab = self.tab_mut();
            tab.transcript.push(TranscriptEntry::User {
                text: display.clone(),
            });
            let _ = tab.cmd_tx.send(AgentCommand::Steer(text.clone()));
            tab.session_id.clone()
        };
        if let Some(reg) = &self.registry {
            self.tui_prompt_echoes
                .push_back((session_id.clone(), display.clone()));
            reg.note_tui_user_prompt(&session_id, &text, Some(&display), true);
        }
        self.status_notice = Some("Sent now".to_string());
        self.completion.reset();
    }

    fn insert_char(&mut self, c: char) {
        self.delete_composer_selection();
        let tab = self.tab_mut();
        tab.input.insert(tab.cursor, c);
        tab.cursor += c.len_utf8();
    }

    fn consume_line_continuation_backslash(&mut self) -> bool {
        let tab = self.tab_mut();
        let before = &tab.input[..tab.cursor.min(tab.input.len())];
        if before.ends_with('\\') && !before.ends_with("\\\\") {
            let new_cursor = tab.cursor - 1;
            tab.input.drain(new_cursor..tab.cursor);
            tab.cursor = new_cursor;
            true
        } else {
            false
        }
    }

    pub(super) fn insert_str(&mut self, text: &str) {
        self.delete_composer_selection();
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let tab = self.tab_mut();
        tab.input.insert_str(tab.cursor, &normalized);
        tab.cursor += normalized.len();
    }

    fn backspace(&mut self) {
        if self.delete_composer_selection() {
            return;
        }
        let tab = self.tab_mut();
        if tab.cursor == 0 {
            return;
        }
        let prev = floor_char_boundary(
            &tab.input,
            tab.cursor.min(tab.input.len()).saturating_sub(1),
        );
        tab.input.drain(prev..tab.cursor);
        tab.cursor = prev;
    }

    fn delete_forward(&mut self) {
        if self.delete_composer_selection() {
            return;
        }
        let tab = self.tab_mut();
        if tab.cursor >= tab.input.len() {
            return;
        }
        let next = ceil_char_boundary(
            &tab.input,
            tab.cursor.saturating_add(1).min(tab.input.len()),
        );
        tab.input.drain(tab.cursor..next);
    }

    fn move_left(&mut self) {
        self.composer_selection = None;
        let tab = self.tab_mut();
        if tab.cursor > 0 {
            tab.cursor = floor_char_boundary(
                &tab.input,
                tab.cursor.min(tab.input.len()).saturating_sub(1),
            );
        }
    }

    fn move_right(&mut self) {
        self.composer_selection = None;
        let tab = self.tab_mut();
        if tab.cursor < tab.input.len() {
            tab.cursor = ceil_char_boundary(
                &tab.input,
                tab.cursor.saturating_add(1).min(tab.input.len()),
            );
        }
    }

    fn move_home(&mut self) {
        self.composer_selection = None;
        let tab = self.tab_mut();
        let before = &tab.input[..tab.cursor];
        tab.cursor = before.rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    }

    fn move_end(&mut self) {
        self.composer_selection = None;
        let tab = self.tab_mut();
        let after = &tab.input[tab.cursor..];
        if let Some(rel) = after.find('\n') {
            tab.cursor += rel;
        } else {
            tab.cursor = tab.input.len();
        }
    }

    fn move_up(&mut self) {
        self.composer_selection = None;
        let (input, cursor) = {
            let tab = self.tab();
            (tab.input.clone(), tab.cursor)
        };
        if let Some((next, goal)) = crate::ui::composer::composer_vertical_move(
            &input,
            cursor,
            self.composer_width(),
            -1,
            self.composer_goal_col,
            &self.agents,
            &self.skills,
        ) {
            self.tab_mut().cursor = next;
            self.composer_goal_col = Some(goal);
        }
    }

    fn move_down(&mut self) {
        self.composer_selection = None;
        let (input, cursor) = {
            let tab = self.tab();
            (tab.input.clone(), tab.cursor)
        };
        if let Some((next, goal)) = crate::ui::composer::composer_vertical_move(
            &input,
            cursor,
            self.composer_width(),
            1,
            self.composer_goal_col,
            &self.agents,
            &self.skills,
        ) {
            self.tab_mut().cursor = next;
            self.composer_goal_col = Some(goal);
        }
    }

    fn composer_width(&self) -> usize {
        let width = self.composer_input_area.width;
        if width > 0 {
            width as usize
        } else {
            self.terminal_width.saturating_sub(4).max(1) as usize
        }
    }
}

pub fn clamp_pos_to_bounds(pos: ScreenPos, bounds: Rect) -> Option<ScreenPos> {
    if bounds.width == 0 || bounds.height == 0 {
        return None;
    }
    let min_col = bounds.x;
    let max_col = bounds.x + bounds.width - 1;
    let min_row = bounds.y;
    let max_row = bounds.y + bounds.height - 1;
    if pos.row < min_row {
        Some(ScreenPos {
            col: min_col,
            row: min_row,
        })
    } else if pos.row > max_row {
        Some(ScreenPos {
            col: max_col,
            row: max_row,
        })
    } else {
        Some(ScreenPos {
            col: pos.col.clamp(min_col, max_col),
            row: pos.row,
        })
    }
}

pub fn normalize_selection_in_bounds(
    anchor: ScreenPos,
    head: ScreenPos,
    bounds: Rect,
) -> Option<(ScreenPos, ScreenPos)> {
    let a = clamp_pos_to_bounds(anchor, bounds)?;
    let b = clamp_pos_to_bounds(head, bounds)?;
    if (a.row, a.col) <= (b.row, b.col) {
        Some((a, b))
    } else {
        Some((b, a))
    }
}

pub(super) fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(not(test))]
pub(super) fn write_system_clipboard(text: &str) {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let b64 = encode_base64(text.as_bytes());
    let mut stdout = std::io::stdout();
    let _ = write!(stdout, "\x1b]52;c;{b64}\x07");
    let _ = stdout.flush();

    let payload = text.as_bytes().to_vec();
    std::thread::spawn(move || {
        #[cfg(target_os = "macos")]
        let candidates: &[(&str, &[&str])] = &[("pbcopy", &[])];
        #[cfg(target_os = "windows")]
        let candidates: &[(&str, &[&str])] = &[("clip", &[])];
        #[cfg(all(unix, not(target_os = "macos")))]
        let candidates: &[(&str, &[&str])] = &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ];

        for (bin, args) in candidates {
            if let Ok(mut child) = Command::new(bin)
                .args(*args)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(&payload);
                }
                let _ = child.wait();
                break;
            }
        }
    });
}

#[cfg(test)]
pub(super) fn write_system_clipboard(_text: &str) {}

fn rect_contains(rect: Rect, column: u16, row: u16) -> bool {
    rect.width > 0
        && rect.height > 0
        && column >= rect.x
        && column < rect.x + rect.width
        && row >= rect.y
        && row < rect.y + rect.height
}

fn is_path_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-' | b':' | b'+')
}

fn char_to_byte(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(index, _)| index)
}

fn selected_text_from_lines(
    lines: &[TranscriptWrappedLine],
    width: u16,
    sel: DocSelection,
) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let max_line = lines.len() - 1;
    let a = DocPos {
        line: sel.anchor.line.min(max_line),
        col: sel.anchor.col,
    };
    let b = DocPos {
        line: sel.head.line.min(max_line),
        col: sel.head.col,
    };
    let (start, end) = if (a.line, a.col) <= (b.line, b.col) {
        (a, b)
    } else {
        (b, a)
    };

    let mut out = String::new();
    let max_width_col = width.saturating_sub(1) as usize;
    let mut prev_joined_soft = false;

    for (line_idx, row) in lines.iter().enumerate().take(end.line + 1).skip(start.line) {
        let chars: Vec<char> = row.text.chars().collect();
        let start_display = if line_idx == start.line { start.col } else { 0 };
        let end_display = if line_idx == end.line {
            end.col
        } else {
            max_width_col
        };
        let c_start = display_to_char(&row.text, start_display);
        let c_end = display_to_char(&row.text, end_display);
        let skip = row.gutter_len.max(row.hang_len);
        let effective_start = c_start.max(skip);
        let slice_str = if effective_start < chars.len() && effective_start <= c_end {
            let clamped_end = c_end.min(chars.len() - 1);
            let raw: String = chars[effective_start..=clamped_end].iter().collect();
            if row.soft_wrapped_next && line_idx < end.line && clamped_end == chars.len() - 1 {
                raw
            } else {
                raw.trim_end().to_string()
            }
        } else {
            String::new()
        };

        if line_idx > start.line && !prev_joined_soft {
            out.push('\n');
        }
        out.push_str(&slice_str);
        prev_joined_soft = row.soft_wrapped_next
            && line_idx < end.line
            && (chars.is_empty() || c_end >= chars.len().saturating_sub(1));
    }

    out.trim_matches('\n').to_string()
}
