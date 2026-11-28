use super::statusline::key_status_style;
use super::*;
use crate::app::{ApprovalClickAction, DiffListState, PackTab, ProviderConnectionAction};

use crate::diff::DiffRow;
use meerai_input::TextInput;

mod chat;
mod insight;
mod misc;
mod panels;
mod provider;
use chat::*;
use insight::*;
use misc::*;
use panels::*;
use provider::*;

fn auth_file_display() -> String {
    meerai_common::settings::default_data_dir()
        .map(|dir| dir.join("meerai.db"))
        .map(|path| meerai_core::env::display_path(&path))
        .unwrap_or_else(|| "meerai.db".to_string())
}

pub(super) fn render_active_modal(frame: &mut Frame<'_>, app: &mut AppState, area: Rect) {
    let Some(modal) = app.modal.clone() else {
        return;
    };
    if area.width < 20 || area.height < 8 {
        return;
    }

    match modal {
        ActiveModal::ProviderSelect { .. }
        | ActiveModal::AuthMethodSelect { .. }
        | ActiveModal::ProviderConnection { .. }
        | ActiveModal::ChatGptLogin { .. }
        | ActiveModal::ApiKeyInput { .. }
        | ActiveModal::ModelInput { .. } => render_provider_modal(app, frame, area, modal),
        ActiveModal::UserMessageActions { .. }
        | ActiveModal::CommandPalette { .. }
        | ActiveModal::QueuedEdit { .. }
        | ActiveModal::Terminal { .. }
        | ActiveModal::Approval { .. }
        | ActiveModal::Question { .. } => render_chat_modal(app, frame, area, modal),
        ActiveModal::DiffView { .. }
        | ActiveModal::Search { .. }
        | ActiveModal::ServeDialog { .. }
        | ActiveModal::RewindMenu { .. }
        | ActiveModal::Runs { .. }
        | ActiveModal::McpServers { .. }
        | ActiveModal::RunDetail { .. }
        | ActiveModal::SessionSelect { .. } => render_panels_modal(app, frame, area, modal),
        ActiveModal::Context(_)
        | ActiveModal::Loop { .. }
        | ActiveModal::Memory { .. }
        | ActiveModal::Routing { .. }
        | ActiveModal::Packs { .. }
        | ActiveModal::Sampling { .. } => render_insight_modal(app, frame, area, modal),
        ActiveModal::VaultReset { .. } | ActiveModal::SubagentSpawn(_) => {
            render_misc_modal(app, frame, area, modal)
        }
    }
}

fn place_input_cursor(frame: &mut Frame<'_>, area: Rect, char_offset: usize) {
    if area.width > 0 && area.height > 0 {
        let cursor_x = (area.x + char_offset as u16).min(area.x + area.width.saturating_sub(1));
        frame.set_cursor_position(Position::new(cursor_x, area.y));
    }
}

fn render_text_block(
    frame: &mut Frame<'_>,
    area: Rect,
    input: &TextInput,
    style: Style,
    scroll: &mut usize,
    show_cursor: bool,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let width = area.width as usize;
    let height = area.height as usize;
    let lines = input.wrapped_lines(width);
    let (cursor_row, cursor_col) = input.cursor_row_col(width);
    let max_scroll = lines.len().saturating_sub(height);
    *scroll = (*scroll).min(max_scroll);
    if cursor_row < *scroll {
        *scroll = cursor_row;
    } else if cursor_row >= *scroll + height {
        *scroll = cursor_row + 1 - height;
    }
    for (row, (start, end)) in lines.iter().enumerate().skip(*scroll).take(height) {
        let y = area.y + (row - *scroll) as u16;
        let body = &input.text[*start..*end];
        let line_rect = Rect::new(area.x, y, area.width, 1);
        if show_cursor && row == cursor_row {
            let before: String = body.chars().take(cursor_col).collect();
            let mut after = body.chars().skip(cursor_col);
            let at = after.next().unwrap_or(' ');
            let rest: String = after.collect();
            let cursor_style = Style::default()
                .fg(theme::p().background)
                .bg(theme::p().accent)
                .add_modifier(Modifier::BOLD);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(before, style),
                    Span::styled(at.to_string(), cursor_style),
                    Span::styled(rest, style),
                ])),
                line_rect,
            );
            let cursor_x = area
                .x
                .saturating_add(cursor_col.min(width.saturating_sub(1)) as u16);
            frame.set_cursor_position(Position::new(cursor_x, y));
        } else {
            frame.render_widget(
                Paragraph::new(Span::styled(body.to_string(), style)),
                line_rect,
            );
        }
    }
}

fn register_list_hit_zones(app: &mut AppState, area: Rect, offset: usize, len: usize) {
    for row in 0..area.height as usize {
        let idx = offset + row;
        if idx >= len {
            break;
        }
        app.modal_hit_zones.push(ModalHitZone {
            rect: Rect::new(area.x, area.y + row as u16, area.width, 1),
            kind: ModalHitKind::ListItem(idx),
        });
    }
}

fn wrap_count(text: &str, width: usize) -> usize {
    if width == 0 {
        return 1;
    }
    text.split('\n')
        .map(|line| {
            let chars = line.chars().count();
            chars.div_ceil(width).max(1)
        })
        .sum::<usize>()
        .max(1)
}

fn format_relative_age(now_ms: u64, updated_ms: u64) -> String {
    let diff_secs = now_ms.saturating_sub(updated_ms) / 1000;
    if diff_secs < 60 {
        "just now".to_string()
    } else if diff_secs < 3600 {
        format!("{}m ago", diff_secs / 60)
    } else if diff_secs < 86_400 {
        format!("{}h ago", diff_secs / 3600)
    } else {
        format!("{}d ago", diff_secs / 86_400)
    }
}

fn diff_new_path(path: &str) -> &str {
    path.split_once(" -> ").map(|(_, new)| new).unwrap_or(path)
}

enum DiffPaneRow {
    Plain(Line<'static>),
    Cell {
        gutter: String,
        body: String,
        style: Style,
    },
}

fn diff_pane_lines(
    rows: &[&DiffRow],
    is_left: bool,
    pane_width: usize,
    language: Option<crate::syntax::Language>,
) -> Vec<Line<'static>> {
    let mut pane = Vec::with_capacity(rows.len());
    let mut source = String::new();
    for (idx, row) in rows.iter().enumerate() {
        if idx > 0 {
            source.push('\n');
        }
        let entry = if let DiffRowKind::HunkHeader(header) = &row.kind {
            DiffPaneRow::Plain(Line::from(Span::styled(
                truncate_single_line(header, pane_width),
                Style::default()
                    .fg(theme::p().accent)
                    .add_modifier(Modifier::BOLD),
            )))
        } else {
            match diff_cell(row, is_left) {
                None => DiffPaneRow::Plain(Line::from(Span::styled(
                    "     · ",
                    Style::default().fg(theme::p().faint),
                ))),
                Some(cell) => {
                    let gutter = diff_gutter(cell.line_no, &row.kind, is_left);
                    let body = truncate_single_line(
                        &cell.text,
                        pane_width.saturating_sub(gutter.chars().count()),
                    );
                    source.push_str(&body);
                    DiffPaneRow::Cell {
                        gutter,
                        body,
                        style: diff_base_style(&row.kind, is_left),
                    }
                }
            }
        };
        pane.push(entry);
    }

    let Some(language) = language else {
        return pane
            .into_iter()
            .map(|row| match row {
                DiffPaneRow::Plain(line) => line,
                DiffPaneRow::Cell {
                    gutter,
                    body,
                    style,
                } => pane_line(gutter, vec![Span::styled(body, style)]),
            })
            .collect();
    };

    let tokens = crate::syntax::token_lines(Some(language), &source);
    pane.into_iter()
        .enumerate()
        .map(|(idx, row)| match row {
            DiffPaneRow::Plain(line) => line,
            DiffPaneRow::Cell {
                gutter,
                body,
                style,
            } => {
                let line_tokens = tokens.get(idx).map(Vec::as_slice).unwrap_or(&[]);
                let body_spans = if line_tokens.is_empty() {
                    vec![Span::styled(body, style)]
                } else {
                    crate::syntax::line_spans(line_tokens, style)
                };
                pane_line(gutter, body_spans)
            }
        })
        .collect()
}

fn diff_cell(row: &DiffRow, is_left: bool) -> Option<&DiffCell> {
    if is_left {
        row.left.as_ref()
    } else {
        row.right.as_ref()
    }
}

fn pane_line(gutter: String, body: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = Vec::with_capacity(body.len() + 1);
    spans.push(Span::styled(gutter, Style::default().fg(theme::p().faint)));
    spans.extend(body);
    Line::from(spans)
}

fn diff_gutter(line_no: Option<usize>, kind: &DiffRowKind, is_left: bool) -> String {
    let prefix = match (kind, is_left) {
        (DiffRowKind::Removed, true) | (DiffRowKind::Modified, true) => "-",
        (DiffRowKind::Added, false) | (DiffRowKind::Modified, false) => "+",
        _ => " ",
    };
    match line_no {
        Some(n) => format!("{n:>4} {prefix} "),
        None => format!("     {prefix} "),
    }
}

fn diff_base_style(kind: &DiffRowKind, is_left: bool) -> Style {
    match (kind, is_left) {
        (DiffRowKind::Removed, true) | (DiffRowKind::Modified, true) => {
            Style::default().fg(theme::p().diff_removed)
        }
        (DiffRowKind::Added, false) | (DiffRowKind::Modified, false) => {
            Style::default().fg(theme::p().diff_added)
        }
        _ => Style::default().fg(theme::p().diff_context),
    }
}

fn render_diff_file_list(
    frame: &mut Frame<'_>,
    app: &mut AppState,
    area: Rect,
    selected: usize,
    file_index: usize,
) {
    let inner = pane_header(frame, area, &format!("Files ({})", app.file_changes.len()));
    if app.file_changes.is_empty() || inner.height == 0 {
        return;
    }
    let width = inner.width as usize;
    let selected_idx = selected.min(app.file_changes.len() - 1);
    let items: Vec<ListItem<'_>> = app
        .file_changes
        .iter()
        .enumerate()
        .map(|(idx, entry)| {
            let is_current = idx == file_index;
            let is_selected = idx == selected_idx;
            let is_hovered = app.hovered_modal_hit == Some(ModalHitKind::ListItem(idx));
            let marker = if is_current { "● " } else { "  " };
            let action = entry.action_label();
            let action_width = action.chars().count();
            let prefix_width =
                1 + marker.chars().count() + entry.status.chars().count() + action_width + 2;
            let path_style = if is_selected {
                Style::default()
                    .fg(theme::p().accent)
                    .add_modifier(Modifier::BOLD)
            } else if is_current {
                Style::default()
                    .fg(theme::p().text)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::p().text)
            };
            let row = ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {marker}"),
                    Style::default().fg(if is_current {
                        theme::p().accent
                    } else {
                        theme::p().faint
                    }),
                ),
                Span::styled(
                    format!("{} ", entry.status),
                    Style::default().fg(theme::p().muted),
                ),
                Span::styled(
                    truncate_single_line(&entry.path, width.saturating_sub(prefix_width)),
                    path_style,
                ),
                Span::styled(format!(" {action}"), Style::default().fg(theme::p().faint)),
            ]));
            if is_hovered && !is_selected {
                row.style(hover_row_style())
            } else {
                row
            }
        })
        .collect();

    let mut state = ListState::default();
    state.select(Some(selected_idx));
    let list = List::new(items).highlight_style(Style::default().bg(theme::p().surface_high));
    frame.render_stateful_widget(list, inner, &mut state);
    register_list_hit_zones(app, inner, state.offset(), app.file_changes.len());
}

fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

fn supporting_suffix(files: &[String]) -> String {
    if files.is_empty() {
        String::new()
    } else {
        format!(" · files: {}", files.join(", "))
    }
}

fn invocation_suffix(model_invocable: Option<bool>) -> &'static str {
    match model_invocable {
        Some(true) => " · model",
        Some(false) => " · manual",
        None => "",
    }
}

fn mode_suffix(mode: Option<&str>) -> String {
    match mode {
        Some(mode) if mode != "all" => format!(" · {mode}"),
        _ => String::new(),
    }
}

fn blank_as<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}

pub(crate) fn wrap_queued_editor(text: &str, width: usize) -> Vec<(usize, usize)> {
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

pub(crate) fn queued_editor_cursor(
    lines: &[(usize, usize)],
    text: &str,
    cursor: usize,
) -> (usize, usize) {
    let cursor = cursor.min(text.len());
    let row = lines
        .iter()
        .rposition(|(start, _)| *start <= cursor)
        .unwrap_or(0);
    let (start, end) = lines[row];
    let cursor = cursor.min(end);
    (row, text[start..cursor].chars().count())
}

#[cfg(test)]
mod tests {
    use super::{invocation_suffix, mode_suffix};

    #[test]
    fn invocation_suffix_marks_model_and_manual_skills() {
        assert_eq!(invocation_suffix(Some(true)), " · model");
        assert_eq!(invocation_suffix(Some(false)), " · manual");
        assert_eq!(invocation_suffix(None), "");
    }

    #[test]
    fn mode_suffix_marks_non_default_agent_modes() {
        assert_eq!(mode_suffix(Some("subagent")), " · subagent");
        assert_eq!(mode_suffix(Some("primary")), " · primary");
        assert_eq!(mode_suffix(Some("all")), "");
        assert_eq!(mode_suffix(None), "");
    }
}
