pub fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

pub fn ceil_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

pub fn truncate_middle(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }

    let head_budget = max_bytes / 2;
    let tail_budget = max_bytes - head_budget;
    let head_end = floor_char_boundary(text, head_budget);
    let tail_start = ceil_char_boundary(text, text.len() - tail_budget);
    let omitted = tail_start.saturating_sub(head_end);

    format!(
        "{}\n... [{omitted} bytes omitted] ...\n{}",
        &text[..head_end],
        &text[tail_start..]
    )
}

pub fn truncate_single_line(text: &str, max_chars: usize) -> String {
    let single: String = text
        .chars()
        .map(|ch| if ch == '\n' || ch == '\r' { ' ' } else { ch })
        .take(max_chars + 1)
        .collect();
    if single.chars().count() > max_chars {
        let prefix: String = single.chars().take(max_chars.saturating_sub(3)).collect();
        format!("{prefix}...")
    } else {
        single
    }
}
