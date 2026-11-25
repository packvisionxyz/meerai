use std::ops::Deref;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextInput {
    pub text: String,
    pub cursor: usize,
    pub anchor: Option<usize>,
    goal_col: Option<usize>,
}

impl TextInput {
    pub fn new(text: impl Into<String>) -> Self {
        let text = normalize(text.into());
        let cursor = text.len();
        Self {
            text,
            cursor,
            anchor: None,
            goal_col: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn with_cursor(text: impl Into<String>, cursor: usize) -> Self {
        let mut input = Self::new(text);
        input.set_cursor(cursor);
        input
    }

    pub fn cursor(&self) -> usize {
        self.cursor.min(self.text.len())
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = normalize(text.into());
        self.cursor = self.text.len();
        self.anchor = None;
        self.goal_col = None;
    }

    pub fn replace_text(&mut self, text: impl Into<String>) {
        self.set_text(text);
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.anchor = None;
        self.goal_col = None;
    }

    pub fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        self.cursor = 0;
        self.anchor = None;
        self.goal_col = None;
        text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn cursor_col(&self) -> usize {
        let cursor = self.cursor();
        self.text[..cursor].chars().count()
    }

    pub fn cursor_line_col(&self) -> (usize, usize) {
        self.line_col()
    }

    pub fn trim(&self) -> &str {
        self.text.trim()
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        let cursor = self.cursor();
        let anchor = anchor.min(self.text.len());
        (anchor != cursor).then_some(if anchor < cursor {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        })
    }

    pub fn has_selection(&self) -> bool {
        self.selection().is_some()
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|(start, end)| &self.text[start..end])
    }

    pub fn set_cursor(&mut self, cursor: usize) {
        self.cursor = floor_boundary(&self.text, cursor.min(self.text.len()));
        self.anchor = None;
        self.goal_col = None;
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.text.len();
        self.goal_col = None;
    }

    pub fn set_selection(&mut self, anchor: usize, head: usize) {
        let anchor = floor_boundary(&self.text, anchor.min(self.text.len()));
        let head = floor_boundary(&self.text, head.min(self.text.len()));
        self.anchor = (anchor != head).then_some(anchor);
        self.cursor = head;
        self.goal_col = None;
    }

    pub fn insert_char(&mut self, c: char) {
        self.delete_selection();
        let at = self.cursor();
        self.text.insert(at, c);
        self.cursor = at + c.len_utf8();
        self.goal_col = None;
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub fn insert_str(&mut self, text: &str) {
        self.delete_selection();
        let text = normalize(text.to_owned());
        let at = self.cursor();
        self.text.insert_str(at, &text);
        self.cursor = at + text.len();
        self.goal_col = None;
    }

    pub fn insert(&mut self, text: &str) {
        self.insert_str(text);
    }

    pub fn push(&mut self, c: char) {
        self.insert_char(c);
    }

    pub fn push_str(&mut self, text: &str) {
        self.insert_str(text);
    }

    pub fn pop(&mut self) -> Option<char> {
        let cursor = self.cursor();
        let previous = floor_boundary(&self.text, cursor.saturating_sub(1));
        if cursor == 0 {
            return None;
        }
        let removed = self.text[previous..cursor].chars().next();
        self.text.drain(previous..cursor);
        self.cursor = previous;
        self.anchor = None;
        self.goal_col = None;
        removed
    }

    pub fn extend(&mut self, chars: impl IntoIterator<Item = char>) {
        for c in chars {
            self.insert_char(c);
        }
    }

    pub fn backspace(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let cursor = self.cursor();
        if cursor == 0 {
            return false;
        }
        let previous = floor_boundary(&self.text, cursor.saturating_sub(1));
        self.text.drain(previous..cursor);
        self.cursor = previous;
        self.goal_col = None;
        true
    }

    pub fn delete_forward(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let cursor = self.cursor();
        if cursor >= self.text.len() {
            return false;
        }
        let next = ceil_boundary(&self.text, cursor.saturating_add(1).min(self.text.len()));
        self.text.drain(cursor..next);
        self.goal_col = None;
        true
    }

    pub fn delete_word_backward(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let start = self.word_left_from(self.cursor());
        let end = self.cursor();
        if start == end {
            return false;
        }
        self.text.drain(start..end);
        self.cursor = start;
        self.goal_col = None;
        true
    }

    pub fn delete_word_forward(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let start = self.cursor();
        let end = self.word_right_from(start);
        if start == end {
            return false;
        }
        self.text.drain(start..end);
        self.goal_col = None;
        true
    }

    pub fn delete_to_line_start(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let start = self.line_start(self.cursor());
        let end = self.cursor();
        if start == end {
            return false;
        }
        self.text.drain(start..end);
        self.cursor = start;
        self.goal_col = None;
        true
    }

    pub fn delete_to_line_end(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let start = self.cursor();
        let end = self.line_end(start);
        if start == end {
            return false;
        }
        self.text.drain(start..end);
        self.goal_col = None;
        true
    }

    pub fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        self.text.replace_range(start..end, "");
        self.cursor = start;
        self.anchor = None;
        self.goal_col = None;
        true
    }

    pub fn move_left(&mut self) {
        self.anchor = None;
        self.goal_col = None;
        let cursor = self.cursor();
        if cursor > 0 {
            self.cursor = floor_boundary(&self.text, cursor.saturating_sub(1));
        }
    }

    pub fn move_right(&mut self) {
        self.anchor = None;
        self.goal_col = None;
        let cursor = self.cursor();
        if cursor < self.text.len() {
            self.cursor = ceil_boundary(&self.text, cursor.saturating_add(1).min(self.text.len()));
        }
    }

    pub fn move_word_left(&mut self) {
        self.anchor = None;
        self.goal_col = None;
        self.cursor = self.word_left_from(self.cursor());
    }

    pub fn move_word_right(&mut self) {
        self.anchor = None;
        self.goal_col = None;
        self.cursor = self.word_right_from(self.cursor());
    }

    pub fn move_home(&mut self) {
        self.anchor = None;
        self.goal_col = None;
        self.cursor = self.line_start(self.cursor());
    }

    pub fn move_end(&mut self) {
        self.anchor = None;
        self.goal_col = None;
        self.cursor = self.line_end(self.cursor());
    }

    pub fn move_document_start(&mut self) {
        self.anchor = None;
        self.goal_col = None;
        self.cursor = 0;
    }

    pub fn move_document_end(&mut self) {
        self.anchor = None;
        self.goal_col = None;
        self.cursor = self.text.len();
    }

    pub fn move_up(&mut self, width: usize) -> bool {
        self.move_vertical(width, -1)
    }

    pub fn move_down(&mut self, width: usize) -> bool {
        self.move_vertical(width, 1)
    }

    pub fn extend_left(&mut self) {
        let anchor = self.anchor.unwrap_or(self.cursor());
        self.move_left();
        self.finish_selection(anchor);
    }

    pub fn extend_right(&mut self) {
        let anchor = self.anchor.unwrap_or(self.cursor());
        self.move_right();
        self.finish_selection(anchor);
    }

    pub fn extend_word_left(&mut self) {
        let anchor = self.anchor.unwrap_or(self.cursor());
        self.move_word_left();
        self.finish_selection(anchor);
    }

    pub fn extend_word_right(&mut self) {
        let anchor = self.anchor.unwrap_or(self.cursor());
        self.move_word_right();
        self.finish_selection(anchor);
    }

    pub fn extend_home(&mut self) {
        let anchor = self.anchor.unwrap_or(self.cursor());
        self.move_home();
        self.finish_selection(anchor);
    }

    pub fn extend_end(&mut self) {
        let anchor = self.anchor.unwrap_or(self.cursor());
        self.move_end();
        self.finish_selection(anchor);
    }

    pub fn extend_up(&mut self, width: usize) {
        let anchor = self.anchor.unwrap_or(self.cursor());
        self.move_up(width);
        self.finish_selection(anchor);
    }

    pub fn extend_down(&mut self, width: usize) {
        let anchor = self.anchor.unwrap_or(self.cursor());
        self.move_down(width);
        self.finish_selection(anchor);
    }

    pub fn line_bounds(&self) -> (usize, usize) {
        (self.line_start(self.cursor()), self.line_end(self.cursor()))
    }

    pub fn line_col(&self) -> (usize, usize) {
        let start = self.line_start(self.cursor());
        let before = &self.text[start..self.cursor()];
        (
            self.text[..start].matches('\n').count(),
            before.chars().count(),
        )
    }

    pub fn wrapped_lines(&self, width: usize) -> Vec<(usize, usize)> {
        wrap_text(&self.text, width)
    }

    pub fn cursor_row_col(&self, width: usize) -> (usize, usize) {
        let lines = self.wrapped_lines(width);
        cursor_row_col(&lines, &self.text, self.cursor())
    }

    pub fn byte_at_row_col(&self, width: usize, row: usize, col: usize) -> usize {
        let lines = self.wrapped_lines(width);
        let Some((start, end)) = lines.get(row).copied() else {
            return self.text.len();
        };
        let body = &self.text[start..end];
        let offset = body
            .char_indices()
            .nth(col)
            .map_or(body.len(), |(index, _)| index);
        start + offset
    }

    fn finish_selection(&mut self, anchor: usize) {
        self.anchor = (anchor != self.cursor()).then_some(anchor);
    }

    fn move_vertical(&mut self, width: usize, delta: isize) -> bool {
        let lines = wrap_text(&self.text, width);
        let (row, col) = cursor_row_col(&lines, &self.text, self.cursor());
        let Some(target) = row.checked_add_signed(delta) else {
            return false;
        };
        let Some((start, end)) = lines.get(target).copied() else {
            return false;
        };
        let goal = self.goal_col.unwrap_or(col);
        let body = &self.text[start..end];
        let offset = body
            .char_indices()
            .nth(goal)
            .map_or(body.len(), |(index, _)| index);
        self.cursor = start + offset;
        self.goal_col = Some(goal);
        true
    }

    fn line_start(&self, cursor: usize) -> usize {
        let cursor = floor_boundary(&self.text, cursor.min(self.text.len()));
        self.text[..cursor]
            .rfind('\n')
            .map(|index| index + 1)
            .unwrap_or(0)
    }

    fn line_end(&self, cursor: usize) -> usize {
        let cursor = floor_boundary(&self.text, cursor.min(self.text.len()));
        self.text[cursor..]
            .find('\n')
            .map(|index| cursor + index)
            .unwrap_or(self.text.len())
    }

    fn word_left_from(&self, cursor: usize) -> usize {
        let cursor = floor_boundary(&self.text, cursor.min(self.text.len()));
        let before = &self.text[..cursor];
        let mut chars = before.char_indices().rev();
        let mut end = cursor;
        for (index, ch) in chars.by_ref() {
            if is_word(ch) {
                end = index;
                break;
            }
            end = index;
        }
        for (index, ch) in chars {
            if is_word(ch) {
                end = index;
            } else {
                break;
            }
        }
        end
    }

    fn word_right_from(&self, cursor: usize) -> usize {
        let cursor = floor_boundary(&self.text, cursor.min(self.text.len()));
        let mut end = cursor;
        let mut seen_word = false;
        for (index, ch) in self.text[cursor..].char_indices() {
            if is_word(ch) {
                seen_word = true;
                end = cursor + index + ch.len_utf8();
            } else if seen_word {
                break;
            } else {
                end = cursor + index + ch.len_utf8();
            }
        }
        end
    }
}

impl Deref for TextInput {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.text
    }
}

impl From<TextInput> for String {
    fn from(input: TextInput) -> Self {
        input.text
    }
}

impl From<String> for TextInput {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

impl From<&str> for TextInput {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

impl std::fmt::Display for TextInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl From<TextInput> for std::borrow::Cow<'_, str> {
    fn from(input: TextInput) -> Self {
        std::borrow::Cow::Owned(input.text)
    }
}

impl PartialEq<str> for TextInput {
    fn eq(&self, other: &str) -> bool {
        self.text == other
    }
}

impl PartialEq<&str> for TextInput {
    fn eq(&self, other: &&str) -> bool {
        self.text == *other
    }
}

impl PartialEq<TextInput> for str {
    fn eq(&self, other: &TextInput) -> bool {
        self == other.text
    }
}

impl PartialEq<TextInput> for &str {
    fn eq(&self, other: &TextInput) -> bool {
        *self == other.text
    }
}

impl PartialEq<TextInput> for String {
    fn eq(&self, other: &TextInput) -> bool {
        self == &other.text
    }
}

impl PartialEq<String> for TextInput {
    fn eq(&self, other: &String) -> bool {
        &self.text == other
    }
}

pub fn wrap_text(text: &str, width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line_start = 0usize;
    for segment in text.split('\n') {
        let seg_end = line_start + segment.len();
        let mut start = line_start;
        if start == seg_end {
            lines.push((start, start));
        }
        while start < seg_end {
            let mut end = start;
            let mut last_space = None;
            for (used, (offset, ch)) in text[start..seg_end].char_indices().enumerate() {
                if used == width {
                    break;
                }
                if ch == ' ' && offset > 0 {
                    last_space = Some(start + offset);
                }
                end = start + offset + ch.len_utf8();
            }
            if end < seg_end
                && !text[end..].starts_with(' ')
                && let Some(space) = last_space
            {
                end = space;
            }
            lines.push((start, end));
            start = end;
            while start < seg_end && text[start..].starts_with(' ') {
                start += 1;
            }
        }
        line_start = seg_end + 1;
    }
    if lines.is_empty() {
        lines.push((0, 0));
    }
    lines
}

pub fn cursor_row_col(lines: &[(usize, usize)], text: &str, cursor: usize) -> (usize, usize) {
    let cursor = cursor.min(text.len());
    let row = lines
        .iter()
        .rposition(|(start, _)| *start <= cursor)
        .unwrap_or(0);
    let (start, end) = lines[row];
    let cursor = cursor.min(end);
    (row, text[start..cursor].chars().count())
}

fn normalize(text: String) -> String {
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text
    }
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn floor_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::TextInput;

    #[test]
    fn insert_and_navigate_multiline() {
        let mut input = TextInput::new("first\nthird");
        input.move_up(80);
        input.move_end();
        input.insert_newline();
        input.insert_str("second");
        assert_eq!(input.text, "first\nsecond\nthird");
        assert_eq!(input.cursor, "first\nsecond".len());
        input.move_up(80);
        assert_eq!(input.cursor, "first".len());
    }

    #[test]
    fn edits_use_the_cursor_not_the_end() {
        let mut input = TextInput::new("abcd");
        input.move_home();
        input.insert_str("X");
        assert_eq!(input.text, "Xabcd");
        input.delete_forward();
        assert_eq!(input.text, "Xbcd");
        input.move_end();
        input.backspace();
        assert_eq!(input.text, "Xbc");
    }

    #[test]
    fn selection_replaces_text() {
        let mut input = TextInput::new("hello world");
        input.move_home();
        input.extend_word_right();
        assert_eq!(input.selected_text(), Some("hello"));
        input.insert_char('X');
        assert_eq!(input.text, "X world");
        assert!(!input.has_selection());
    }

    #[test]
    fn paste_is_multiline_and_replaces_selection() {
        let mut input = TextInput::new("line");
        input.select_all();
        input.insert_str("a\r\nb\rc");
        assert_eq!(input.text, "a\nb\nc");
        assert_eq!(input.cursor, input.text.len());
    }

    #[test]
    fn word_and_line_deletes() {
        let mut input = TextInput::new("alpha beta");
        input.move_end();
        assert!(input.delete_word_backward());
        assert_eq!(input.text, "alpha ");
        assert!(input.delete_word_backward());
        assert_eq!(input.text, "");
        input.replace_text("one\ntwo");
        input.move_end();
        assert!(input.delete_to_line_start());
        assert_eq!(input.text, "one\n");
    }

    #[test]
    fn vertical_movements_keep_the_goal_column() {
        let mut input = TextInput::new("abcdef\nxy\nabcdef");
        input.move_end();
        input.move_up(80);
        assert_eq!(input.cursor, "abcdef\nxy".len());
        input.move_up(80);
        assert_eq!(input.cursor, "abcdef".len());
    }
}
