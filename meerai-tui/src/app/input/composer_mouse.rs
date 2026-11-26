use super::*;

impl AppState {
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
    fn composer_width(&self) -> usize {
        let width = self.composer_input_area.width;
        if width > 0 {
            width as usize
        } else {
            self.terminal_width.saturating_sub(4).max(1) as usize
        }
    }
}
