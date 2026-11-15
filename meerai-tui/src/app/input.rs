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
                if let Some(modal) = &mut self.modal {
                    let clean = text.replace(['\r', '\n'], "");
                    match modal {
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
                            selected_recent,
                            error,
                        } => {
                            input.push_str(&clean);
                            *selected_recent = None;
                            *error = None;
                        }
                        ActiveModal::DiffView { .. } => {}
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
                                &self.workspace_root,
                                *mode,
                                workspace_files,
                                query,
                                results,
                                selected,
                                preview,
                            );
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
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.modal.is_none() && self.click_file_change_at(mouse.column, mouse.row) {
                    self.selection = None;
                    return true;
                }
                let pos = ScreenPos {
                    col: mouse.column,
                    row: mouse.row,
                };
                self.selection = Some(TextSelection {
                    anchor: pos,
                    head: pos,
                    active: true,
                });
                if self.modal.is_none() {
                    let _ = self.toggle_thinking_at(mouse.column, mouse.row);
                }
                return true;
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(sel) = &mut self.selection {
                    sel.head = ScreenPos {
                        col: mouse.column,
                        row: mouse.row,
                    };
                    sel.active = true;
                    return true;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(mut sel) = self.selection.take() {
                    sel.head = ScreenPos {
                        col: mouse.column,
                        row: mouse.row,
                    };
                    sel.active = false;
                    if sel.anchor != sel.head {
                        let text = self.extract_selected_text(sel);
                        if !text.is_empty() {
                            self.copy_text_to_clipboard(&text);
                        }
                    }
                    return true;
                }
            }
            _ => {}
        }

        if let Some(ActiveModal::DiffView { diff, scroll, .. }) = &mut self.modal {
            let max_scroll = diff.rows.len().saturating_sub(1);
            return match mouse.kind {
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
        if self.modal.is_some() {
            return false;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_up(3);
                true
            }
            MouseEventKind::ScrollDown => {
                self.scroll_down(3);
                true
            }
            _ => false,
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

    pub fn click_file_change_at(&mut self, column: u16, row: u16) -> bool {
        if !self.is_sidebar_visible() {
            return false;
        }
        let area = self.sidebar_changes_inner_area;
        if area.width == 0
            || area.height == 0
            || column < area.x
            || column >= area.x + area.width
            || row < area.y
            || row >= area.y + area.height
        {
            return false;
        }
        let row_idx = (row - area.y) as usize;
        if row_idx >= self.visible_file_change_rows || row_idx >= self.file_changes.len() {
            return false;
        }
        self.open_diff_modal(row_idx);
        true
    }

    pub fn toggle_thinking_at(&mut self, column: u16, row: u16) -> bool {
        let area = self.transcript_inner_area;
        if area.width == 0
            || area.height == 0
            || column < area.x
            || column >= area.x + area.width
            || row < area.y
            || row >= area.y + area.height
        {
            return false;
        }
        let virtual_line = self
            .effective_scroll()
            .saturating_add((row - area.y) as usize);
        let Some(entry_idx) = self
            .thinking_hit_zones
            .iter()
            .find(|z| virtual_line >= z.start_line && virtual_line < z.end_line)
            .map(|z| z.entry_idx)
        else {
            return false;
        };
        match self.transcript.get_mut(entry_idx) {
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
            _ => false,
        }
    }

    pub fn toggle_latest_thinking(&mut self) -> bool {
        for entry in self.transcript.iter_mut().rev() {
            match entry {
                TranscriptEntry::Assistant {
                    reasoning,
                    reasoning_expanded,
                    ..
                } if !reasoning.is_empty() => {
                    *reasoning_expanded = !*reasoning_expanded;
                    return true;
                }
                TranscriptEntry::Error { expanded, .. } => {
                    *expanded = !*expanded;
                    return true;
                }
                _ => {}
            }
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

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('t') => {
                    return self.toggle_latest_thinking();
                }
                KeyCode::Char('b') => {
                    self.toggle_sidebar();
                    return true;
                }
                KeyCode::Char('f') => {
                    self.open_search_modal(SearchMode::Files, "");
                    return true;
                }
                _ => {}
            }
        }

        match self.mode {
            InputMode::Insert => self.handle_insert_key(key),
            InputMode::Normal => self.handle_normal_key(key),
        }
    }

    pub(super) fn handle_ctrl_c(&mut self) {
        if self.is_running {
            let _ = self.cmd_tx.send(AgentCommand::Cancel);
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
            self.status_notice = Some("Press Ctrl-c again to exit".to_string());
        }
    }

    pub(super) fn handle_insert_key(&mut self, key: KeyEvent) -> bool {
        let has_shift_or_alt = key.modifiers.contains(KeyModifiers::SHIFT)
            || key.modifiers.contains(KeyModifiers::ALT);
        let has_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

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
                    if self.completion.selected_is_noop(&self.input) {
                        self.completion.reset();
                        self.submit_input();
                    } else {
                        self.accept_completion();
                        if is_slash && !self.input.ends_with(' ') {
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
            KeyCode::Esc => {
                self.mode = InputMode::Normal;
            }
            KeyCode::Enter if has_shift_or_alt => {
                self.insert_char('\n');
                self.update_completion();
            }
            KeyCode::Enter if !has_ctrl => {
                self.submit_input();
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
                if self.input == "/search" || self.input == "/grep" {
                    let mode = if self.input == "/grep" {
                        SearchMode::Content
                    } else {
                        SearchMode::Files
                    };
                    self.input.clear();
                    self.cursor = 0;
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

        if key.code != KeyCode::Char('g') || has_ctrl {
            self.pending_g = false;
        }

        match key.code {
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
                if self.pending_g {
                    self.pending_g = false;
                    self.scroll_to_top();
                } else {
                    self.pending_g = true;
                }
            }
            KeyCode::Char('G') => {
                self.scroll_to_bottom();
            }
            KeyCode::Char('t') if !has_ctrl => {
                return self.toggle_latest_thinking();
            }
            KeyCode::Char('y') if !has_ctrl => {
                let text = self
                    .transcript
                    .iter()
                    .rev()
                    .find_map(|entry| match entry {
                        TranscriptEntry::Assistant { text, .. } if !text.is_empty() => {
                            Some(text.clone())
                        }
                        _ => None,
                    })
                    .or_else(|| {
                        self.transcript.iter().rev().find_map(|entry| match entry {
                            TranscriptEntry::Error { text, .. }
                            | TranscriptEntry::System { text }
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
                return false;
            }
            KeyCode::Char('/') if !has_ctrl => {
                self.open_search_modal(SearchMode::Files, "");
            }
            KeyCode::Char('q') if !has_ctrl => {
                self.quit();
            }
            KeyCode::Esc => {
                if self.is_running {
                    let _ = self.cmd_tx.send(AgentCommand::Cancel);
                    self.status_notice = Some("Cancelling turn...".to_string());
                }
            }
            _ => return false,
        }

        true
    }

    pub(super) fn update_completion(&mut self) {
        if self.input.contains('@') {
            self.completion.set_definitions(&self.agents, &self.skills);
        }
        self.completion.update(&self.input, self.cursor);
    }

    pub(super) fn accept_completion(&mut self) {
        if let Some((new_input, new_cursor)) = self.completion.accept(&self.input, self.cursor) {
            self.input = new_input;
            self.cursor = new_cursor;
            self.update_completion();
        }
    }

    pub(super) fn submit_input(&mut self) {
        let trimmed = self.input.trim();
        if trimmed.is_empty() {
            return;
        }

        if trimmed.starts_with('/') {
            let owned = trimmed.to_string();
            self.input.clear();
            self.cursor = 0;
            self.completion.reset();
            self.handle_slash_command(&owned);
            return;
        }

        let plan = parse_mentions(&self.input, &self.agents, &self.skills);
        let remaining = plan
            .text
            .clone()
            .unwrap_or_else(|| self.input.trim_end().to_string());

        if remaining.is_empty() {
            self.apply_mentions(&plan);
            self.acknowledge_mentions(&plan);
            self.input.clear();
            self.cursor = 0;
            self.completion.reset();
            self.auto_follow = true;
            return;
        }

        if !self.is_running && self.model.trim().is_empty() {
            self.status_notice = Some("Set a model first (/model)".to_string());
            self.open_model_input();
            return;
        }

        self.apply_mentions(&plan);

        if self.is_running {
            self.input.clear();
            self.cursor = 0;
            self.completion.reset();
            self.auto_follow = true;
            self.transcript.push(TranscriptEntry::User {
                text: remaining.clone(),
            });
            let _ = self.cmd_tx.send(AgentCommand::Steer(remaining));
            return;
        }

        self.input.clear();
        self.cursor = 0;
        self.completion.reset();
        self.auto_follow = true;
        self.is_running = true;
        self.transcript.push(TranscriptEntry::User {
            text: remaining.clone(),
        });
        let _ = self.cmd_tx.send(AgentCommand::SubmitPrompt(remaining));
    }

    fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub(super) fn insert_str(&mut self, text: &str) {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        self.input.insert_str(self.cursor, &normalized);
        self.cursor += normalized.len();
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = floor_char_boundary(
            &self.input,
            self.cursor.min(self.input.len()).saturating_sub(1),
        );
        self.input.drain(prev..self.cursor);
        self.cursor = prev;
    }

    fn delete_forward(&mut self) {
        if self.cursor >= self.input.len() {
            return;
        }
        let next = ceil_char_boundary(
            &self.input,
            self.cursor.saturating_add(1).min(self.input.len()),
        );
        self.input.drain(self.cursor..next);
    }

    fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = floor_char_boundary(
                &self.input,
                self.cursor.min(self.input.len()).saturating_sub(1),
            );
        }
    }

    fn move_right(&mut self) {
        if self.cursor < self.input.len() {
            self.cursor = ceil_char_boundary(
                &self.input,
                self.cursor.saturating_add(1).min(self.input.len()),
            );
        }
    }

    fn move_home(&mut self) {
        let before = &self.input[..self.cursor];
        self.cursor = before.rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    }

    fn move_end(&mut self) {
        let after = &self.input[self.cursor..];
        if let Some(rel) = after.find('\n') {
            self.cursor += rel;
        } else {
            self.cursor = self.input.len();
        }
    }

    fn move_up(&mut self) {
        let before = &self.input[..self.cursor];
        let Some(cur_line_break) = before.rfind('\n') else {
            return;
        };
        let col_chars = before[cur_line_break + 1..].chars().count();
        let prev_line_start = before[..cur_line_break]
            .rfind('\n')
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let prev_line = &self.input[prev_line_start..cur_line_break];
        self.cursor = prev_line_start + char_col_to_byte_idx(prev_line, col_chars);
    }

    fn move_down(&mut self) {
        let before = &self.input[..self.cursor];
        let cur_line_start = before.rfind('\n').map(|idx| idx + 1).unwrap_or(0);
        let col_chars = before[cur_line_start..].chars().count();
        let Some(rel_next_break) = self.input[self.cursor..].find('\n') else {
            return;
        };
        let next_line_start = self.cursor + rel_next_break + 1;
        let next_line_end = self.input[next_line_start..]
            .find('\n')
            .map(|rel| next_line_start + rel)
            .unwrap_or(self.input.len());
        let next_line = &self.input[next_line_start..next_line_end];
        self.cursor = next_line_start + char_col_to_byte_idx(next_line, col_chars);
    }
}

fn char_col_to_byte_idx(line: &str, target_chars: usize) -> usize {
    line.char_indices()
        .nth(target_chars)
        .map(|(idx, _)| idx)
        .unwrap_or(line.len())
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
