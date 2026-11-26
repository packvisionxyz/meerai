use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use meerai_agents::AgentCommand;
use meerai_core::text::{ceil_char_boundary, floor_char_boundary};
use ratatui::layout::Rect;

use super::commands::parse_mentions;
use super::modals::{SearchOutput, SearchSources};
use super::*;
use crate::completion::CompletionKind;
use crate::search::SearchMode;

mod composer_edit;
mod composer_mouse;
mod dispatch;
mod events;
mod hit_completion;
mod hit_transcript;
mod selection;

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
