use super::*;

impl AppState {
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
}
