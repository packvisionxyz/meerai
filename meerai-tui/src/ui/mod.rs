use std::sync::OnceLock;

use meerai_agents::events::ApprovalKind;
use meerai_client::providers::KeyStatus;
use meerai_common::definitions::{AgentDefinition, SkillDefinition};
use meerai_core::EffectClass;
use meerai_core::env::now_millis;
use meerai_core::text::truncate_single_line;
use meerai_policy::{ApprovalMode, approval_mode_badge};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use regex::Regex;

use crate::app::{
    ActiveModal, AppState, DocPos, HoverZone, InputMode, MentionKind, TabHitKind, TabHitZone,
    ThinkingHitZone, ToolCallStatus, TranscriptEntry, TranscriptWrappedLine, UserAction,
    mention_is_known, normalize_selection_in_bounds, parse_mention_token,
};
use crate::completion::{CompletionItemKind, CompletionKind};
use crate::diff::{DiffCell, DiffRowKind};
use crate::search::SearchMode;
use crate::theme;

mod composer;
mod modals;
mod selection;
mod sidebar;
mod statusline;
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
    let composer_lines = app.tab().input.split('\n').count().clamp(1, 6) as u16;
    let composer_height = composer_lines + 2;

    let outer = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(6),
        Constraint::Length(1),
    ])
    .split(area);
    let tab_area = outer[0];
    let main_area = outer[1];
    let status_area = outer[2];

    selection::render_tab_bar(frame, app, tab_area);

    let (transcript_area, composer_area) = if app.is_sidebar_visible() {
        let sidebar_width = (main_area.width / 4).clamp(30, 38);
        let cols = Layout::horizontal([Constraint::Min(50), Constraint::Length(sidebar_width)])
            .split(main_area);
        let chat_chunks =
            Layout::vertical([Constraint::Min(3), Constraint::Length(composer_height)])
                .split(cols[0]);
        let sidebar = cols[1];
        let gap = 1u16.min(sidebar.height);
        let body = sidebar.height.saturating_sub(gap);
        let top = (body as u32 * 55 / 100) as u16;
        let bottom = body.saturating_sub(top);
        let overview_area = Rect::new(sidebar.x, sidebar.y, sidebar.width, top);
        let changes_area = Rect::new(sidebar.x, sidebar.y + top + gap, sidebar.width, bottom);

        transcript::render_transcript(frame, app, chat_chunks[0]);
        composer::render_composer(frame, app, chat_chunks[1]);
        sidebar::render_sidebar_overview(frame, app, overview_area);
        sidebar::render_sidebar_changes(frame, app, changes_area);
        (chat_chunks[0], chat_chunks[1])
    } else {
        app.sidebar_changes_inner_area = Rect::default();
        app.visible_file_change_rows = 0;
        let chat_chunks =
            Layout::vertical([Constraint::Min(3), Constraint::Length(composer_height)])
                .split(main_area);
        transcript::render_transcript(frame, app, chat_chunks[0]);
        composer::render_composer(frame, app, chat_chunks[1]);
        (chat_chunks[0], chat_chunks[1])
    };

    statusline::render_statusline(frame, app, status_area);
    composer::render_completion_popup(frame, app, transcript_area, composer_area);
    modals::render_active_modal(frame, app, area);
    selection::capture_screen_lines(frame, app);
    selection::render_selection_highlight(frame, app);
}

struct TaggedTranscriptLine {
    line: Line<'static>,
    entry_tag: Option<usize>,
    gutter_len: usize,
    band: bool,
    code_band: bool,
    hover_tag: Option<usize>,
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

fn dialog_frame(frame: &mut Frame<'_>, popup: Rect, title: &str) -> Rect {
    dialog_frame_styled(
        frame,
        popup,
        title,
        Style::default()
            .fg(theme::p().text)
            .add_modifier(Modifier::BOLD),
    )
}

fn dialog_frame_styled(
    frame: &mut Frame<'_>,
    popup: Rect,
    title: &str,
    title_style: Style,
) -> Rect {
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().surface)),
        popup,
    );
    if popup.width > 2 {
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
            Paragraph::new(Line::from(Span::styled(title.to_string(), title_style))),
            Rect {
                x: popup.x.saturating_add(1),
                y: popup.y,
                width: popup.width.saturating_sub(2),
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

fn input_band(frame: &mut Frame<'_>, area: Rect) -> Rect {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().surface_high)),
        area,
    );
    Rect {
        x: area.x.saturating_add(1),
        y: area.y,
        width: area.width.saturating_sub(2),
        height: area.height,
    }
}

fn pane_header(frame: &mut Frame<'_>, area: Rect, title: &str) -> Rect {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().surface_high)),
        area,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            title.to_string(),
            Style::default()
                .fg(theme::p().muted)
                .add_modifier(Modifier::BOLD),
        ))),
        Rect {
            x: area.x.saturating_add(1),
            y: area.y,
            width: area.width.saturating_sub(2),
            height: area.height,
        },
    );
    Rect {
        y: area.y.saturating_add(1),
        height: area.height.saturating_sub(1),
        ..area
    }
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
