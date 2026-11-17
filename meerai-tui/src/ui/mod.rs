use std::sync::OnceLock;

use meerai_agents::events::ApprovalKind;
use meerai_client::providers::KeyStatus;
use meerai_common::definitions::{AgentDefinition, SkillDefinition};
use meerai_core::EffectClass;
use meerai_core::env::now_millis;
use meerai_core::text::truncate_single_line;
use meerai_policy::approval_mode_badge;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use regex::Regex;

use crate::app::{
    ActiveModal, AppState, CompletionHitZone, DocPos, HoverZone, InputMode, MentionKind,
    ModalHitKind, ModalHitZone, TabHitKind, TabHitZone, ThinkingHitZone, ToolCallStatus,
    TranscriptEntry, TranscriptWrappedLine, UserAction, mention_is_known,
    normalize_selection_in_bounds, parse_mention_token,
};
use crate::completion::{CompletionItemKind, CompletionKind};
use crate::diff::{DiffCell, DiffRowKind};
use crate::search::SearchMode;
use crate::theme;

pub(crate) mod composer;
mod modals;
mod selection;
mod sidebar;
mod statusline;
mod subagent_overlay;
mod transcript;

#[cfg(test)]
mod tests;

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn render(frame: &mut Frame<'_>, app: &mut AppState) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().background)),
        area,
    );
    app.terminal_width = area.width;
    app.modal_hit_zones.clear();
    app.completion_hit_zones.clear();

    let outer = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(6),
        Constraint::Length(1),
    ])
    .split(area);
    let tab_area = outer[0];
    let main_area = outer[1];
    let status_area = outer[2];

    let chat_col = if app.is_sidebar_visible() {
        let sidebar_width = (main_area.width / 4).clamp(30, 38);
        let cols = Layout::horizontal([
            Constraint::Min(50),
            Constraint::Length(1),
            Constraint::Length(sidebar_width),
        ])
        .split(main_area);
        let sidebar = cols[2];
        let gap = 1u16.min(sidebar.height);
        let body = sidebar.height.saturating_sub(gap);
        let top = (body as u32 * 55 / 100) as u16;
        let bottom = body.saturating_sub(top);
        let overview_area = Rect::new(sidebar.x, sidebar.y, sidebar.width, top);
        let changes_area = Rect::new(sidebar.x, sidebar.y + top + gap, sidebar.width, bottom);

        sidebar::render_sidebar_overview(frame, app, overview_area);
        sidebar::render_sidebar_changes(frame, app, changes_area);
        cols[0]
    } else {
        app.sidebar_changes_inner_area = Rect::default();
        app.visible_file_change_rows = 0;
        app.file_change_more_hit = None;
        app.hovered_file_change = None;
        main_area
    };

    let composer_input_width = (chat_col.width.saturating_sub(8) as usize).max(1);
    let composer_lines =
        composer::composer_rows(&app.tab().input, composer_input_width).clamp(1, 8) as u16;
    let composer_bottom_margin: u16 = if main_area.height >= 14 { 1 } else { 0 };
    let composer_height = composer_lines + 4 + composer_bottom_margin;

    selection::render_tab_bar(frame, app, tab_area);

    let chat_chunks =
        Layout::vertical([Constraint::Min(3), Constraint::Length(composer_height)]).split(chat_col);
    let transcript_area = chat_chunks[0];
    let composer_area = composer_box_rect(chat_chunks[1], composer_bottom_margin);
    transcript::render_transcript(frame, app, transcript_area);
    composer::render_composer(frame, app, composer_area);

    statusline::render_statusline(frame, app, status_area);
    if app.modal.is_none() {
        render_toast(frame, app, transcript_area);
    }
    composer::render_completion_popup(frame, app, transcript_area, composer_area);
    subagent_overlay::render_subagent_overlay(frame, app, composer_area);
    modals::render_active_modal(frame, app, area);
    selection::capture_screen_lines(frame, app);
    selection::render_selection_highlight(frame, app);
}

fn composer_box_rect(slot: Rect, bottom_margin: u16) -> Rect {
    let h_margin = if slot.width >= 16 { 2 } else { 0 };
    let b_margin = if slot.height >= 3 + bottom_margin {
        bottom_margin
    } else {
        0
    };
    Rect {
        x: slot.x.saturating_add(h_margin),
        y: slot.y,
        width: slot.width.saturating_sub(h_margin * 2),
        height: slot.height.saturating_sub(b_margin),
    }
}

struct TaggedTranscriptLine {
    line: Line<'static>,
    entry_tag: Option<usize>,
    gutter_len: usize,
    band: bool,
    code_band: bool,
    hover_tag: Option<usize>,
    digest_tag: Option<usize>,
}

fn tagged_line(
    line: Line<'static>,
    entry_tag: Option<usize>,
    gutter_len: usize,
) -> TaggedTranscriptLine {
    TaggedTranscriptLine {
        line,
        entry_tag,
        gutter_len,
        band: false,
        code_band: false,
        hover_tag: None,
        digest_tag: None,
    }
}

fn banded_line(
    line: Line<'static>,
    entry_tag: Option<usize>,
    gutter_len: usize,
) -> TaggedTranscriptLine {
    TaggedTranscriptLine {
        line,
        entry_tag,
        gutter_len,
        band: true,
        code_band: false,
        hover_tag: None,
        digest_tag: None,
    }
}

fn code_line(mut tagged: TaggedTranscriptLine) -> TaggedTranscriptLine {
    tagged.code_band = true;
    tagged
}

fn with_hover(mut tagged: TaggedTranscriptLine, entry_idx: usize) -> TaggedTranscriptLine {
    tagged.hover_tag = Some(entry_idx);
    tagged
}

fn with_digest(mut tagged: TaggedTranscriptLine, user_idx: usize) -> TaggedTranscriptLine {
    tagged.digest_tag = Some(user_idx);
    tagged
}

fn centered_rect(area: Rect, percent_x: u16, percent_y: u16, min_w: u16, min_h: u16) -> Rect {
    let width = ((area.width as u32 * percent_x as u32) / 100)
        .max(min_w as u32)
        .min(area.width.saturating_sub(2) as u32) as u16;
    let height = ((area.height as u32 * percent_y as u32) / 100)
        .max(min_h as u32)
        .min(area.height.saturating_sub(2) as u32) as u16;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width, height)
}

fn cap_width(rect: Rect, max: u16) -> Rect {
    if rect.width <= max {
        return rect;
    }
    Rect {
        x: rect.x + (rect.width - max) / 2,
        width: max,
        ..rect
    }
}

fn centered_rect_fixed(area: Rect, max_w: u16, max_h: u16) -> Rect {
    let width = max_w.min(area.width.saturating_sub(2));
    let height = max_h.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width, height)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NoticeSeverity {
    Success,
    Warning,
    Error,
    Info,
}

pub(super) fn notice_severity(msg: &str) -> NoticeSeverity {
    let lower = msg.to_ascii_lowercase();
    if lower.contains("failed")
        || lower.contains("error")
        || lower.contains("unavailable")
        || lower.contains("cannot")
        || lower.contains("disconnected")
        || lower.contains("set a model first")
        || lower.contains("cancel the running turn")
    {
        NoticeSeverity::Error
    } else if lower.contains("cancelling")
        || lower.contains("queued")
        || lower.contains("press ctrl-c")
        || lower.contains("already gone")
        || lower.contains("nothing to")
        || lower.contains("no closed turn")
        || lower.contains("clean")
    {
        NoticeSeverity::Warning
    } else if lower.contains("copied")
        || lower.contains("saved")
        || lower.contains("connected")
        || lower.contains("reloaded")
        || lower.contains("deleted")
        || lower.contains("sent now")
        || lower.contains("rewound")
    {
        NoticeSeverity::Success
    } else {
        NoticeSeverity::Info
    }
}

pub(super) fn notice_color(severity: NoticeSeverity) -> Color {
    match severity {
        NoticeSeverity::Success => theme::p().success,
        NoticeSeverity::Warning => theme::p().warning,
        NoticeSeverity::Error => theme::p().error,
        NoticeSeverity::Info => theme::p().info,
    }
}

fn render_toast(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let Some(notice) = app.status_notice.as_deref() else {
        return;
    };
    if area.width < 32 || area.height < 3 {
        return;
    }
    let color = notice_color(notice_severity(notice));
    let max_text = (area.width as usize).saturating_sub(10).clamp(12, 56);
    let text = truncate_single_line(notice, max_text);
    let toast_w = (text.chars().count() + 4).min(area.width.saturating_sub(4) as usize) as u16;
    let x = area.x + area.width.saturating_sub(toast_w + 2);
    let y = area.y + 1;
    let toast_area = Rect::new(x, y, toast_w, 1);

    frame.render_widget(Clear, toast_area);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().surface_high)),
        toast_area,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("▌ ", Style::default().fg(color)),
            Span::styled(text, Style::default().fg(theme::p().text)),
            Span::raw(" "),
        ])),
        toast_area,
    );
}

fn pad_rect(rect: Rect, horiz: u16, vert: u16) -> Rect {
    Rect {
        x: rect.x.saturating_add(horiz),
        y: rect.y.saturating_add(vert),
        width: rect.width.saturating_sub(horiz * 2),
        height: rect.height.saturating_sub(vert * 2),
    }
}

fn dim_backdrop(frame: &mut Frame<'_>, full: Rect, popup: Rect) {
    let buf = frame.buffer_mut();
    let faint = theme::p().faint;
    let bg = theme::p().background;
    for y in full.y..full.y + full.height {
        for x in full.x..full.x + full.width {
            if x >= popup.x
                && x < popup.x + popup.width
                && y >= popup.y
                && y < popup.y + popup.height
            {
                continue;
            }
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_fg(faint);
                cell.set_bg(bg);
            }
        }
    }
}

fn dialog_frame(
    frame: &mut Frame<'_>,
    app: &mut AppState,
    popup: Rect,
    title: &str,
    dismissible: bool,
) -> Rect {
    dialog_frame_styled(
        frame,
        app,
        popup,
        title,
        Style::default()
            .fg(theme::p().text)
            .add_modifier(Modifier::BOLD),
        dismissible,
    )
}

fn dialog_frame_styled(
    frame: &mut Frame<'_>,
    app: &mut AppState,
    popup: Rect,
    title: &str,
    title_style: Style,
    dismissible: bool,
) -> Rect {
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().surface)),
        popup,
    );
    if popup.width > 4 {
        let band = Rect {
            x: popup.x,
            y: popup.y,
            width: popup.width,
            height: 1,
        };
        frame.render_widget(
            Block::default().style(Style::default().bg(theme::p().surface_high)),
            band,
        );
        let close_label = "esc ×";
        let close_reserve = if dismissible && popup.width > 24 {
            close_label.chars().count() as u16 + 2
        } else {
            0
        };
        let title_w = popup.width.saturating_sub(4 + close_reserve);
        let title_text = truncate_single_line(title, title_w as usize);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(title_text, title_style))),
            Rect {
                x: popup.x.saturating_add(2),
                y: popup.y,
                width: title_w,
                height: 1,
            },
        );
        if close_reserve > 0 {
            let close_w = close_label.chars().count() as u16;
            let close_x = popup.x + popup.width.saturating_sub(close_w + 2);
            let close_rect = Rect::new(close_x, popup.y, close_w, 1);
            let hovered = app.hovered_modal_hit == Some(ModalHitKind::Close);
            let close_style = if hovered {
                Style::default()
                    .fg(theme::p().text)
                    .bg(theme::p().surface_high)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::p().muted)
            };
            frame.render_widget(
                Paragraph::new(Span::styled(close_label, close_style)),
                close_rect,
            );
            app.modal_hit_zones.push(ModalHitZone {
                rect: close_rect,
                kind: ModalHitKind::Close,
            });
        }
    }
    let horiz = if popup.width >= 10 { 2 } else { 1 };
    let top_pad = if popup.height >= 7 { 2 } else { 1 };
    let bottom_pad = if popup.height >= 7 { 1 } else { 0 };
    Rect {
        x: popup.x.saturating_add(horiz),
        y: popup.y.saturating_add(top_pad),
        width: popup.width.saturating_sub(horiz * 2),
        height: popup.height.saturating_sub(top_pad + bottom_pad),
    }
}

fn dialog_frame_compact(frame: &mut Frame<'_>, popup: Rect, title: &str) -> Rect {
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().surface)),
        popup,
    );
    if popup.width > 4 {
        let band = Rect {
            x: popup.x,
            y: popup.y,
            width: popup.width,
            height: 1,
        };
        frame.render_widget(
            Block::default().style(Style::default().bg(theme::p().surface_high)),
            band,
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                truncate_single_line(title, popup.width.saturating_sub(4) as usize),
                Style::default()
                    .fg(theme::p().muted)
                    .add_modifier(Modifier::BOLD),
            ))),
            Rect {
                x: popup.x.saturating_add(2),
                y: popup.y,
                width: popup.width.saturating_sub(4),
                height: 1,
            },
        );
    }
    Rect {
        x: popup.x.saturating_add(1),
        y: popup.y.saturating_add(1),
        width: popup.width.saturating_sub(2),
        height: popup.height.saturating_sub(2),
    }
}

fn selection_style() -> Style {
    Style::default()
        .bg(theme::p().surface_high)
        .fg(theme::p().text)
        .add_modifier(Modifier::BOLD)
}

fn hover_row_style() -> Style {
    Style::default()
        .bg(theme::p().surface_high)
        .fg(theme::p().text)
}

fn input_band(frame: &mut Frame<'_>, area: Rect, focused: bool) -> Rect {
    let bg = if focused {
        theme::p().surface_high
    } else {
        theme::p().background
    };
    frame.render_widget(Block::default().style(Style::default().bg(bg)), area);
    if focused && area.width > 2 {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "▌",
                Style::default().fg(theme::p().interactive).bg(bg),
            )),
            Rect::new(area.x, area.y, 1, 1),
        );
    }
    Rect {
        x: area.x.saturating_add(2),
        y: area.y,
        width: area.width.saturating_sub(3),
        height: area.height,
    }
}

fn pane_header(frame: &mut Frame<'_>, area: Rect, title: &str) -> Rect {
    let band = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().surface_high)),
        band,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            truncate_single_line(title, area.width.saturating_sub(4) as usize),
            Style::default()
                .fg(theme::p().muted)
                .add_modifier(Modifier::BOLD),
        ))),
        Rect {
            x: area.x.saturating_add(2),
            y: area.y,
            width: area.width.saturating_sub(4),
            height: 1,
        },
    );
    Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(1),
    }
}

fn key_hints_line(hints: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::with_capacity(hints.len() * 3);
    for (idx, (key, desc)) in hints.iter().enumerate() {
        if idx > 0 {
            spans.push(Span::styled("  ·  ", Style::default().fg(theme::p().faint)));
        }
        spans.push(Span::styled(
            (*key).to_string(),
            Style::default()
                .fg(theme::p().muted)
                .add_modifier(Modifier::BOLD),
        ));
        if !desc.is_empty() {
            spans.push(Span::styled(
                format!(" {desc}"),
                Style::default().fg(theme::p().faint),
            ));
        }
    }
    Line::from(spans)
}

fn cursor_row_col(input: &str, cursor: usize) -> (usize, usize) {
    let clamped = cursor.min(input.len());
    let before = &input[..clamped];
    let row = before.matches('\n').count();
    let col = before
        .rsplit('\n')
        .next()
        .map(|line| line.chars().count())
        .unwrap_or(0);
    (row, col)
}
