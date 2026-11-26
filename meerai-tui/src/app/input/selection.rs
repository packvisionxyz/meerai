use super::*;

impl AppState {
    pub(crate) fn refresh_active_doc_selection_head(&mut self) {
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
    pub(crate) fn register_click(&mut self, column: u16, row: u16) -> u8 {
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
    pub(crate) fn screen_to_doc_pos_clamped(&self, column: u16, row: u16) -> Option<DocPos> {
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
    pub(crate) fn extract_subagent_selected_text(&self, sel: DocSelection) -> String {
        selected_text_from_lines(
            &self.subagent_view_cache.doc_lines,
            self.subagent_view_cache.body.width,
            sel,
        )
    }
    pub(crate) fn handle_subagent_selection_mouse(
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
    pub(crate) fn subagent_doc_pos(&self, column: u16, row: u16) -> Option<DocPos> {
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
    pub(crate) fn scroll_subagent_view(&mut self, delta: isize) {
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
}
