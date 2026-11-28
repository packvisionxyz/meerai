use super::*;

impl AppState {
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
    pub(crate) fn scroll_file_changes(&mut self, up: bool) -> bool {
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
    pub(crate) fn run_user_action(&mut self, entry_idx: usize, action: UserAction) -> bool {
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
    pub(crate) fn restore_composer_message(&mut self, message: String) {
        self.mode = InputMode::Insert;
        self.composer_selection = None;
        self.composer_goal_col = None;
        self.composer_drag_anchor = None;
        let tab = self.tab_mut();
        tab.cursor = message.len();
        tab.input = message;
    }
    pub(crate) fn cycle_approval_mode(&mut self) {
        if self.tab().ephemeral {
            self.status_notice = Some("btw is read-only".to_string());
            return;
        }
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
    pub(crate) fn handle_ctrl_c(&mut self) {
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
    pub(crate) fn handle_insert_key(&mut self, key: KeyEvent) -> bool {
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
    pub(crate) fn handle_normal_key(&mut self, key: KeyEvent) -> bool {
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
            KeyCode::Char('t') if !has_ctrl => {
                self.toggle_reasoning_at_viewport();
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
    pub(crate) fn copy_latest_reply(&mut self) -> bool {
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
    pub(crate) fn update_completion(&mut self) {
        let tab = &self.tabs[self.active_tab];
        if tab.input.contains('@') {
            self.completion.set_definitions(&self.agents, &self.skills);
        }
        self.completion.update(&tab.input, tab.cursor);
    }
    pub(crate) fn accept_completion(&mut self) {
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
    pub(crate) fn submit_input(&mut self) {
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
    pub(crate) fn send_now(&mut self) {
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
    pub(crate) fn insert_str(&mut self, text: &str) {
        self.delete_composer_selection();
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let tab = self.tab_mut();
        tab.input.insert_str(tab.cursor, &normalized);
        tab.cursor += normalized.len();
    }
}
