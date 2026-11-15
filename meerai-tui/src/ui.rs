use std::sync::OnceLock;

use meerai_agents::events::ApprovalKind;
use meerai_common::definitions::{AgentDefinition, SkillDefinition};
use meerai_core::EffectClass;
use meerai_core::text::truncate_single_line;
use meerai_policy::{ApprovalMode, approval_mode_badge};
use meerai_providers::providers::KeyStatus;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use regex::Regex;

use crate::app::{
    ActiveModal, AppState, InputMode, MentionKind, ThinkingHitZone, ToolCallStatus,
    TranscriptEntry, mention_is_known, normalize_selection_in_bounds, parse_mention_token,
};
use crate::completion::{CompletionItemKind, CompletionKind};
use crate::diff::{DiffCell, DiffRowKind};
use crate::search::SearchMode;

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn render(frame: &mut Frame<'_>, app: &mut AppState) {
    let area = frame.area();
    app.terminal_width = area.width;
    let composer_lines = app.input.split('\n').count().clamp(1, 6) as u16;
    let composer_height = composer_lines + 2;

    let outer = Layout::vertical([Constraint::Min(6), Constraint::Length(1)]).split(area);
    let main_area = outer[0];
    let status_area = outer[1];

    let (transcript_area, composer_area) = if app.is_sidebar_visible() {
        let sidebar_width = (main_area.width / 4).clamp(30, 38);
        let cols = Layout::horizontal([Constraint::Min(50), Constraint::Length(sidebar_width)])
            .split(main_area);
        let chat_chunks =
            Layout::vertical([Constraint::Min(3), Constraint::Length(composer_height)])
                .split(cols[0]);
        let sidebar_chunks =
            Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)])
                .split(cols[1]);

        render_transcript(frame, app, chat_chunks[0]);
        render_composer(frame, app, chat_chunks[1]);
        render_sidebar_overview(frame, app, sidebar_chunks[0]);
        render_sidebar_changes(frame, app, sidebar_chunks[1]);
        (chat_chunks[0], chat_chunks[1])
    } else {
        app.sidebar_changes_inner_area = Rect::default();
        app.visible_file_change_rows = 0;
        let chat_chunks =
            Layout::vertical([Constraint::Min(3), Constraint::Length(composer_height)])
                .split(main_area);
        render_transcript(frame, app, chat_chunks[0]);
        render_composer(frame, app, chat_chunks[1]);
        (chat_chunks[0], chat_chunks[1])
    };

    render_statusline(frame, app, status_area);
    render_completion_popup(frame, app, transcript_area, composer_area);
    render_active_modal(frame, app, area);
    capture_screen_lines(frame, app);
    render_selection_highlight(frame, app);
}

fn render_transcript(frame: &mut Frame<'_>, app: &mut AppState, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" Chat ");
    let inner = block.inner(area);

    let raw_lines = build_transcript_lines(app);
    let (wrapped_lines, hit_zones) = if inner.width > 0 {
        wrap_tagged_lines(raw_lines, inner.width as usize)
    } else {
        (Vec::new(), Vec::new())
    };

    app.transcript_inner_area = inner;
    app.thinking_hit_zones = hit_zones;
    app.viewport_height = inner.height as usize;
    app.total_lines = wrapped_lines.len();

    let max_scroll = app.max_scroll();
    if app.auto_follow {
        app.scroll_offset = max_scroll;
    } else {
        app.scroll_offset = app.scroll_offset.min(max_scroll);
    }

    let scroll_y = app.scroll_offset.min(u16::MAX as usize) as u16;
    let paragraph = Paragraph::new(wrapped_lines)
        .block(block)
        .scroll((scroll_y, 0));
    frame.render_widget(paragraph, area);
}

fn render_sidebar_overview(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" Session ");
    let inner = block.inner(area);
    let width = inner.width as usize;

    let key_status = app.active_key_status();
    let (key_badge, key_color) = key_status_style(&key_status);
    let state_span = if app.is_running {
        let ch = SPINNER_FRAMES[app.spinner_tick % SPINNER_FRAMES.len()];
        let label = match app.metrics.elapsed_ms() {
            Some(ms) => format!("{ch} running ({})", format_compact_ms(ms)),
            None => format!("{ch} running"),
        };
        Span::styled(
            label,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        let label = match app.metrics.last_turn_ms {
            Some(ms) => format!("idle ({})", format_compact_ms(ms)),
            None => "idle".to_string(),
        };
        Span::styled(label, Style::default().fg(Color::Green))
    };

    let mut token_summary = format_usage_compact(app);
    if app.metrics.turn_count > 1 && app.metrics.session_total_tokens > 0 {
        token_summary.push_str(&format!(
            " · s:Σ{}",
            format_compact_tokens(app.metrics.session_total_tokens)
        ));
    }
    let speed_summary = match (app.metrics.last_tokens_per_sec, app.metrics.last_ttft_ms) {
        (Some(tps), Some(ttft)) => format!("{tps:.1} tok/s · {}", format_compact_ms(ttft)),
        (Some(tps), None) => format!("{tps:.1} tok/s"),
        (None, Some(ttft)) => format!("ttft {}", format_compact_ms(ttft)),
        (None, None) => "-".to_string(),
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled("provider ", Style::default().fg(Color::DarkGray)),
            Span::raw(truncate_single_line(
                &app.active_provider,
                width.saturating_sub(9),
            )),
        ]),
        Line::from(vec![
            Span::styled("model    ", Style::default().fg(Color::DarkGray)),
            Span::raw(truncate_single_line(
                display_model(app),
                width.saturating_sub(9),
            )),
        ]),
        Line::from(vec![
            Span::styled("auth     ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("[{key_badge}]"), Style::default().fg(key_color)),
        ]),
        Line::from(vec![
            Span::styled("state    ", Style::default().fg(Color::DarkGray)),
            state_span,
        ]),
        Line::from(vec![
            Span::styled("tokens   ", Style::default().fg(Color::DarkGray)),
            Span::raw(truncate_single_line(
                &token_summary,
                width.saturating_sub(9),
            )),
        ]),
        Line::from(vec![
            Span::styled("speed    ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                truncate_single_line(&speed_summary, width.saturating_sub(9)),
                Style::default().fg(Color::Rgb(133, 191, 181)),
            ),
        ]),
        Line::default(),
    ];

    if app.mcp_servers.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(
                "MCP ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("(0) ", Style::default().fg(Color::DarkGray)),
            Span::styled("none", Style::default().fg(Color::DarkGray)),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled(
                "MCP ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("({})", app.mcp_servers.len()),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
        for srv in &app.mcp_servers {
            lines.push(Line::from(format!(
                "  • {}",
                truncate_single_line(srv, width.saturating_sub(4))
            )));
        }
    }

    lines.push(Line::from(vec![
        Span::styled(
            "Agents ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("(1) ", Style::default().fg(Color::DarkGray)),
        Span::raw("main "),
        if app.is_running {
            Span::styled("[active]", Style::default().fg(Color::Yellow))
        } else {
            Span::styled("[ready]", Style::default().fg(Color::Green))
        },
    ]));

    let recent_tools: Vec<_> = app
        .transcript
        .iter()
        .filter_map(|e| match e {
            TranscriptEntry::ToolCall(tc) => Some(tc),
            _ => None,
        })
        .rev()
        .take(5)
        .collect();

    if recent_tools.is_empty() && app.registered_tools.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(
                "Tools ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("(0 calls) ", Style::default().fg(Color::DarkGray)),
            Span::styled("idle", Style::default().fg(Color::DarkGray)),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled(
                "Tools ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("({} calls)", recent_tools.len()),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
        for tool_name in app.registered_tools.iter().take(4) {
            lines.push(Line::from(Span::styled(
                format!(
                    "  • {}",
                    truncate_single_line(tool_name, width.saturating_sub(4))
                ),
                Style::default().fg(Color::Gray),
            )));
        }
        for tc in recent_tools.into_iter().rev() {
            let (badge, color) = match tc.status {
                ToolCallStatus::Proposed | ToolCallStatus::Running => ("run", Color::Yellow),
                ToolCallStatus::Succeeded => ("ok", Color::Green),
                ToolCallStatus::Failed => ("fail", Color::Red),
                ToolCallStatus::Stopped => ("stop", Color::Magenta),
                ToolCallStatus::Cancelled => ("cancel", Color::Magenta),
            };
            let max_name = width.saturating_sub(10);
            lines.push(Line::from(vec![
                Span::raw(format!("  {} ", truncate_single_line(&tc.name, max_name))),
                Span::styled(format!("[{badge}]"), Style::default().fg(color)),
            ]));
        }
    }

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_sidebar_changes(frame: &mut Frame<'_>, app: &mut AppState, area: Rect) {
    let title = format!(" File Changes ({}) ", app.file_changes.len());
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    let width = inner.width as usize;

    app.sidebar_changes_inner_area = inner;

    let mut lines = Vec::new();
    let mut visible_rows = 0usize;
    if app.file_changes.is_empty() {
        lines.push(Line::from(Span::styled(
            "Clean working tree",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        let max_rows = inner.height.max(1) as usize;
        for entry in app.file_changes.iter().take(max_rows) {
            let status_color = match entry.status.as_str() {
                "??" | "A" => Color::Green,
                "D" => Color::Red,
                _ => Color::Yellow,
            };
            let badge = format!("{:>2} ", entry.status);
            let max_path = width.saturating_sub(badge.len());
            lines.push(Line::from(vec![
                Span::styled(badge, Style::default().fg(status_color)),
                Span::raw(truncate_single_line(&entry.path, max_path)),
            ]));
            visible_rows += 1;
        }
        if app.file_changes.len() > max_rows && !lines.is_empty() {
            let remaining = app.file_changes.len() - max_rows + 1;
            lines.pop();
            visible_rows = visible_rows.saturating_sub(1);
            lines.push(Line::from(Span::styled(
                format!("... and {remaining} more"),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
    app.visible_file_change_rows = visible_rows;

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

const MINT_TEAL: Color = Color::Rgb(133, 191, 181);
const USER_ROW_BG: Color = Color::Rgb(34, 38, 52);
const TOOL_ROW_BG: Color = Color::Rgb(34, 42, 34);
const LOGO_M_ROWS: [&str; 4] = ["██      ██", "████  ████", "██  ██  ██", "██      ██"];

fn build_header_lines(app: &AppState) -> Vec<(Line<'static>, Option<usize>)> {
    let key_status = app.active_key_status();
    let (badge, badge_color) = key_status_style(&key_status);
    let logo_style = Style::default().fg(MINT_TEAL).add_modifier(Modifier::BOLD);

    vec![
        (
            Line::from(vec![
                Span::styled(LOGO_M_ROWS[0], logo_style),
                Span::raw("  "),
                Span::styled(
                    "meerai",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("{} / {}", app.active_provider, display_model(app)),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(" "),
                Span::styled(format!("[{badge}]"), Style::default().fg(badge_color)),
            ]),
            None,
        ),
        (
            Line::from(vec![
                Span::styled(LOGO_M_ROWS[1], logo_style),
                Span::styled(
                    "  Commands: /connect, /models, /sidebar, /help, @ (files).",
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
            None,
        ),
        (
            Line::from(vec![
                Span::styled(LOGO_M_ROWS[2], logo_style),
                Span::styled(
                    "  Shortcuts: Ctrl-t think, Ctrl-b sidebar, click thinking.",
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
            None,
        ),
        (
            Line::from(vec![
                Span::styled(LOGO_M_ROWS[3], logo_style),
                Span::styled(
                    format!("  Workspace: {}", app.workspace_root.display()),
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
            None,
        ),
    ]
}

fn tool_status_badge(status: &ToolCallStatus) -> (&'static str, Color) {
    match status {
        ToolCallStatus::Proposed => ("proposed", Color::Yellow),
        ToolCallStatus::Running => ("running", Color::Yellow),
        ToolCallStatus::Succeeded => ("ok", Color::Green),
        ToolCallStatus::Failed => ("fail", Color::Red),
        ToolCallStatus::Stopped => ("stop", Color::Magenta),
        ToolCallStatus::Cancelled => ("cancelled", Color::Magenta),
    }
}

fn build_transcript_lines(app: &AppState) -> Vec<(Line<'static>, Option<usize>)> {
    let mut lines = build_header_lines(app);
    if app.transcript.is_empty() {
        return lines;
    }

    lines.push((Line::default(), None));
    for (idx, entry) in app.transcript.iter().enumerate() {
        if idx > 0 {
            lines.push((Line::default(), None));
        }
        match entry {
            TranscriptEntry::User { text } => {
                for line in text.split('\n') {
                    lines.push((
                        Line::from(vec![
                            Span::styled(
                                "┃ ",
                                Style::default()
                                    .fg(MINT_TEAL)
                                    .bg(USER_ROW_BG)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                line.to_string(),
                                Style::default().fg(Color::White).bg(USER_ROW_BG),
                            ),
                        ])
                        .style(Style::default().bg(USER_ROW_BG)),
                        None,
                    ));
                }
            }
            TranscriptEntry::Assistant {
                text,
                reasoning,
                streaming,
                reasoning_expanded,
            } => {
                if !reasoning.is_empty() {
                    let line_count = reasoning.lines().count().max(1);
                    if *reasoning_expanded {
                        lines.push((
                            Line::from(Span::styled(
                                format!("  ▾ Thinking ({line_count} lines)  [click to hide]"),
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD),
                            )),
                            Some(idx),
                        ));
                        for line in reasoning.split('\n') {
                            lines.push((
                                Line::from(vec![
                                    Span::styled("  │ ", Style::default().fg(Color::DarkGray)),
                                    Span::styled(
                                        line.to_string(),
                                        Style::default()
                                            .fg(Color::DarkGray)
                                            .add_modifier(Modifier::ITALIC),
                                    ),
                                ]),
                                Some(idx),
                            ));
                        }
                    } else {
                        lines.push((
                            Line::from(Span::styled(
                                format!("  ▸ Thinking ({line_count} lines)  [click or Ctrl-t]"),
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::ITALIC),
                            )),
                            Some(idx),
                        ));
                    }
                }
                if !text.is_empty() {
                    for line in text.split('\n') {
                        lines.push((Line::from(format!("  {line}")), None));
                    }
                } else if *streaming && reasoning.is_empty() {
                    let ch = SPINNER_FRAMES[app.spinner_tick % SPINNER_FRAMES.len()];
                    lines.push((
                        Line::from(Span::styled(
                            format!("  {ch} ..."),
                            Style::default().fg(Color::DarkGray),
                        )),
                        None,
                    ));
                }
            }
            TranscriptEntry::ToolCall(tc) => {
                let (badge, color) = tool_status_badge(&tc.status);
                let args_preview = truncate_single_line(&tc.args, 60);
                lines.push((
                    Line::from(vec![
                        Span::styled(
                            "▸ [tool] ",
                            Style::default().fg(Color::Blue).bg(TOOL_ROW_BG),
                        ),
                        Span::styled(
                            tc.name.clone(),
                            Style::default()
                                .bg(TOOL_ROW_BG)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(" ", Style::default().bg(TOOL_ROW_BG)),
                        Span::styled(
                            format!("[{badge}]"),
                            Style::default().fg(color).bg(TOOL_ROW_BG),
                        ),
                        if args_preview.is_empty() {
                            Span::raw("")
                        } else {
                            Span::styled(
                                format!(" {args_preview}"),
                                Style::default().fg(Color::Gray).bg(TOOL_ROW_BG),
                            )
                        },
                    ])
                    .style(Style::default().bg(TOOL_ROW_BG)),
                    None,
                ));
                if let Some(output) = &tc.output {
                    let preview = truncate_single_line(output, 120);
                    if !preview.is_empty() {
                        lines.push((
                            Line::from(Span::styled(
                                format!("  {preview}"),
                                Style::default().fg(Color::Gray).bg(TOOL_ROW_BG),
                            ))
                            .style(Style::default().bg(TOOL_ROW_BG)),
                            None,
                        ));
                    }
                }
            }
            TranscriptEntry::Subagent(sub) => {
                let (badge, color) = tool_status_badge(&sub.status);
                lines.push((
                    Line::from(vec![
                        Span::styled(
                            "▸ [subagent] ",
                            Style::default().fg(Color::Cyan).bg(TOOL_ROW_BG),
                        ),
                        Span::styled(
                            sub.agent.clone(),
                            Style::default()
                                .bg(TOOL_ROW_BG)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(" ", Style::default().bg(TOOL_ROW_BG)),
                        Span::styled(
                            format!("[{badge}]"),
                            Style::default().fg(color).bg(TOOL_ROW_BG),
                        ),
                    ])
                    .style(Style::default().bg(TOOL_ROW_BG)),
                    None,
                ));
                if let Some(report) = &sub.report {
                    let preview = truncate_single_line(report, 120);
                    if !preview.is_empty() {
                        lines.push((
                            Line::from(Span::styled(
                                format!("  {preview}"),
                                Style::default().fg(Color::Gray).bg(TOOL_ROW_BG),
                            ))
                            .style(Style::default().bg(TOOL_ROW_BG)),
                            None,
                        ));
                    }
                }
            }
            TranscriptEntry::System { text } => {
                for line in text.split('\n') {
                    lines.push((
                        Line::from(Span::styled(
                            format!("[system] {line}"),
                            Style::default().fg(Color::Yellow),
                        )),
                        None,
                    ));
                }
            }
            TranscriptEntry::Error { text, expanded } => {
                if *expanded {
                    let line_count = text.lines().count().max(1);
                    lines.push((
                        Line::from(Span::styled(
                            format!("  ▾ [error] ({line_count} lines)  [click to hide]"),
                            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                        )),
                        Some(idx),
                    ));
                    for line in text.split('\n') {
                        lines.push((
                            Line::from(vec![
                                Span::styled("  │ ", Style::default().fg(Color::Red)),
                                Span::styled(
                                    line.to_string(),
                                    Style::default().fg(Color::LightRed),
                                ),
                            ]),
                            Some(idx),
                        ));
                    }
                } else {
                    let avail_width = if app.transcript_inner_area.width > 0 {
                        app.transcript_inner_area.width as usize
                    } else {
                        (app.terminal_width as usize).saturating_sub(4).max(40)
                    };
                    let max_summary = avail_width.saturating_sub(32).max(12);
                    let summary = summarize_error_line(text, max_summary);
                    lines.push((
                        Line::from(vec![
                            Span::styled(
                                format!("  ▸ [error] {summary}"),
                                Style::default().fg(Color::Red),
                            ),
                            Span::styled(
                                "  [click to expand]",
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::ITALIC),
                            ),
                        ]),
                        Some(idx),
                    ));
                }
            }
        }
    }

    lines
}

fn render_composer(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let (title, border_style) = match app.mode {
        InputMode::Insert => (
            " Prompt (Insert) — / commands, @ mentions, Ctrl-f search, Esc Normal ",
            Style::default().fg(Color::Green),
        ),
        InputMode::Normal => (
            " Prompt (Normal) — i/a edit, / search, j/k scroll, q quit ",
            Style::default().fg(Color::Blue),
        ),
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);
    let inner = block.inner(area);

    let (cursor_row, cursor_col) = cursor_row_col(&app.input, app.cursor);
    let visible_rows = inner.height.max(1) as usize;
    let scroll_y = cursor_row.saturating_sub(visible_rows.saturating_sub(1));

    let mut cursor_display_col = cursor_col;
    let content = if app.input.is_empty() {
        Paragraph::new(Line::from(Span::styled(
            "Ask meerai anything...",
            Style::default().fg(Color::DarkGray),
        )))
    } else {
        let lines: Vec<Line<'_>> = app
            .input
            .split('\n')
            .enumerate()
            .map(|(index, line)| {
                let cursor = (index == cursor_row).then_some(cursor_col);
                let (spans, display_col) =
                    render_composer_line(line, cursor, &app.agents, &app.skills);
                if index == cursor_row {
                    cursor_display_col = display_col;
                }
                Line::from(spans)
            })
            .collect();
        Paragraph::new(lines).scroll((scroll_y.min(u16::MAX as usize) as u16, 0))
    };

    frame.render_widget(content.block(block), area);

    if app.modal.is_none() && app.mode == InputMode::Insert && inner.width > 0 && inner.height > 0 {
        let max_x = inner.x + inner.width.saturating_sub(1);
        let rel_row = cursor_row.saturating_sub(scroll_y);
        let max_y = inner.y + inner.height.saturating_sub(1);
        let x = (inner.x + cursor_display_col as u16).min(max_x);
        let y = (inner.y + rel_row as u16).min(max_y);
        frame.set_cursor_position(Position::new(x, y));
    }
}

fn render_composer_line(
    line: &str,
    cursor_char: Option<usize>,
    agents: &[AgentDefinition],
    skills: &[SkillDefinition],
) -> (Vec<Span<'static>>, usize) {
    let cursor_byte = cursor_char.map(|col| {
        line.char_indices()
            .nth(col)
            .map(|(idx, _)| idx)
            .unwrap_or(line.len())
    });

    let mut spans = Vec::new();
    let mut display_col = 0usize;
    let mut cursor_col = None;
    let mut plain = String::new();
    let mut plain_start = 0usize;
    let mut index = 0usize;

    while index < line.len() {
        let Some(ch) = line[index..].chars().next() else {
            break;
        };
        if ch == '@'
            && let Some((kind, id, end)) = parse_mention_token(line, index)
            && mention_is_known(kind, id, agents, skills)
        {
            if !plain.is_empty() {
                push_composer_text(
                    &mut spans,
                    &mut plain,
                    plain_start,
                    index,
                    cursor_byte,
                    &mut display_col,
                    &mut cursor_col,
                    line,
                );
            }
            if cursor_col.is_none()
                && let Some(cursor) = cursor_byte
            {
                if cursor <= index {
                    cursor_col = Some(display_col);
                } else if cursor <= end {
                    cursor_col = Some(display_col + id.chars().count());
                }
            }
            display_col += id.chars().count();
            spans.push(Span::styled(id.to_string(), mention_chip_style(kind)));
            index = end;
            plain_start = end;
            continue;
        }
        if plain.is_empty() {
            plain_start = index;
        }
        plain.push(ch);
        index += ch.len_utf8();
    }

    if !plain.is_empty() {
        push_composer_text(
            &mut spans,
            &mut plain,
            plain_start,
            line.len(),
            cursor_byte,
            &mut display_col,
            &mut cursor_col,
            line,
        );
    }

    (spans, cursor_col.unwrap_or(display_col))
}

#[allow(clippy::too_many_arguments)]
fn push_composer_text(
    spans: &mut Vec<Span<'static>>,
    plain: &mut String,
    start: usize,
    end: usize,
    cursor_byte: Option<usize>,
    display_col: &mut usize,
    cursor_col: &mut Option<usize>,
    line: &str,
) {
    let text = std::mem::take(plain);
    if cursor_col.is_none()
        && let Some(cursor) = cursor_byte
    {
        if cursor <= start {
            *cursor_col = Some(*display_col);
        } else if cursor < end {
            *cursor_col = Some(*display_col + line[start..cursor].chars().count());
        }
    }
    *display_col += text.chars().count();
    spans.push(Span::raw(text));
}

fn mention_chip_style(kind: MentionKind) -> Style {
    let color = match kind {
        MentionKind::Agent => Color::Cyan,
        MentionKind::Skill => Color::Magenta,
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

fn render_completion_popup(
    frame: &mut Frame<'_>,
    app: &AppState,
    transcript_area: Rect,
    composer_area: Rect,
) {
    if app.modal.is_some() || app.mode != InputMode::Insert || !app.completion.is_active() {
        return;
    }

    let items = app.completion.items();
    if items.is_empty() || transcript_area.height < 3 || composer_area.width < 12 {
        return;
    }

    let max_visible = (transcript_area.height.saturating_sub(2) as usize).clamp(1, 8);
    let height = (items.len().min(max_visible) as u16) + 2;
    let width = composer_area.width.saturating_sub(2).min(78);

    let x = composer_area.x + 1;
    let y = composer_area.y.saturating_sub(height);
    let popup_area = Rect::new(x, y, width, height);

    let title = match app.completion.kind() {
        Some(CompletionKind::SlashCommand) => {
            format!(
                " Commands ({}/{}) ",
                app.completion.selected() + 1,
                items.len()
            )
        }
        Some(CompletionKind::Model) => {
            format!(
                " Models ({}/{}) ",
                app.completion.selected() + 1,
                items.len()
            )
        }
        Some(CompletionKind::File) => {
            format!(
                " Mentions ({}/{}) ",
                app.completion.selected() + 1,
                items.len()
            )
        }
        None => String::new(),
    };

    let list_items: Vec<ListItem<'_>> = items
        .iter()
        .map(|item| {
            let mut spans = vec![Span::styled(
                item.label.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            )];
            if let Some(badge) = item.item_kind.badge() {
                let color = match item.item_kind {
                    CompletionItemKind::Agent => Color::Cyan,
                    CompletionItemKind::Skill => Color::Magenta,
                    CompletionItemKind::Plain | CompletionItemKind::File => Color::Gray,
                };
                spans.push(Span::styled(
                    format!("|{badge}"),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ));
            }
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                item.description.clone(),
                Style::default().fg(Color::DarkGray),
            ));
            ListItem::new(Line::from(spans))
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(app.completion.selected()));

    let list = List::new(list_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(title),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );

    frame.render_widget(Clear, popup_area);
    frame.render_stateful_widget(list, popup_area, &mut list_state);
}

fn render_active_modal(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let Some(modal) = &app.modal else {
        return;
    };
    if area.width < 20 || area.height < 8 {
        return;
    }

    match modal {
        ActiveModal::ProviderSelect { filter, selected } => {
            let popup = centered_rect(area, 72, 68, 44, 10);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Connect Provider (/connect) ");
            let inner = block.inner(popup);
            frame.render_widget(Clear, popup);
            frame.render_widget(block, popup);

            let rows = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(1),
            ])
            .split(inner);

            let filter_line = Line::from(vec![
                Span::styled("Filter: ", Style::default().fg(Color::Yellow)),
                Span::raw(if filter.is_empty() {
                    "(type to filter)"
                } else {
                    filter.as_str()
                }),
            ]);
            frame.render_widget(Paragraph::new(filter_line), rows[0]);

            let providers = app.filtered_providers(filter);
            let list_items: Vec<ListItem<'_>> = providers
                .iter()
                .map(|(p, status)| {
                    let (badge, color) = key_status_style(status);
                    let active_mark = if p.id == app.active_provider {
                        "*"
                    } else {
                        " "
                    };
                    ListItem::new(Line::from(vec![
                        Span::raw(format!("{active_mark} ")),
                        Span::styled(
                            p.display_name.clone(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(" ({}) ", p.id),
                            Style::default().fg(Color::DarkGray),
                        ),
                        Span::styled(format!("[{badge}]"), Style::default().fg(color)),
                    ]))
                })
                .collect();

            let mut state = ListState::default();
            if !providers.is_empty() {
                state.select(Some((*selected).min(providers.len() - 1)));
            }
            let list = List::new(list_items).highlight_style(
                Style::default()
                    .bg(Color::Blue)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            );
            frame.render_stateful_widget(list, rows[1], &mut state);

            let footer = Paragraph::new(Line::from(Span::styled(
                "Enter:configure  Up/Down:move  Type:filter  Esc:close",
                Style::default().fg(Color::DarkGray),
            )));
            frame.render_widget(footer, rows[2]);
        }
        ActiveModal::ApiKeyInput {
            provider_id,
            provider_name,
            api_key,
            base_url,
            editing_base_url,
            existing_key_status,
            error,
        } => {
            let popup = centered_rect_fixed(area, 72, 13);
            let title = format!(" Connect {provider_name} ({provider_id}) ");
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(title);
            let inner = block.inner(popup);
            frame.render_widget(Clear, popup);
            frame.render_widget(block, popup);

            let (badge, badge_color) = key_status_style(existing_key_status);
            let masked_key = if api_key.is_empty() {
                if existing_key_status.is_configured() {
                    "(press Enter to keep existing key, or type new key)".to_string()
                } else {
                    "(paste or type API key)".to_string()
                }
            } else {
                "•".repeat(api_key.chars().count())
            };

            let default_url = app
                .active_provider_default_base_url(provider_id)
                .unwrap_or_else(|| "provider default".to_string());
            let url_display = if base_url.is_empty() {
                format!("({default_url})")
            } else {
                base_url.clone()
            };

            let key_label_style = if !editing_base_url {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let url_label_style = if *editing_base_url {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let mut lines = vec![
                Line::from(vec![
                    Span::raw("Status: "),
                    Span::styled(format!("[{badge}]"), Style::default().fg(badge_color)),
                ]),
                Line::default(),
                Line::from(vec![
                    Span::styled("API Key : ", key_label_style),
                    Span::raw(masked_key),
                ]),
                Line::from(vec![
                    Span::styled("Base URL: ", url_label_style),
                    Span::raw(url_display),
                ]),
                Line::default(),
            ];

            if let Some(err) = error {
                lines.push(Line::from(Span::styled(
                    format!("Error: {err}"),
                    Style::default().fg(Color::Red),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    "Saved in ~/.local/share/meerai/auth.json (0600).",
                    Style::default().fg(Color::DarkGray),
                )));
            }
            lines.push(Line::from(Span::styled(
                "Enter:save  Tab:switch field  Ctrl-d:delete key  Esc:back",
                Style::default().fg(Color::DarkGray),
            )));

            frame.render_widget(Paragraph::new(lines), inner);
        }
        ActiveModal::ModelInput {
            input,
            selected_recent,
            error,
        } => {
            let recent = app.recent_models_for_active_provider();
            let height = if recent.is_empty() {
                9
            } else {
                (8 + recent.len().min(8)) as u16
            };
            let popup = centered_rect_fixed(area, 72, height);
            let title = format!(" Set Model [{}] (/model) ", app.active_provider);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(title);
            let inner = block.inner(popup);
            frame.render_widget(Clear, popup);
            frame.render_widget(block, popup);

            let rows = Layout::vertical([
                Constraint::Length(2),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(inner);

            let placeholder = app
                .current_or_default_model(&app.active_provider)
                .map(|m| format!("(Enter keeps '{m}', or type model name)"))
                .unwrap_or_else(|| "(type model name, e.g. gpt-4o)".to_string());
            let input_display = if input.is_empty() {
                Span::styled(placeholder, Style::default().fg(Color::DarkGray))
            } else {
                Span::raw(input.as_str())
            };
            let input_lines = vec![
                Line::from(vec![
                    Span::styled(
                        "Model: ",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    input_display,
                ]),
                Line::default(),
            ];
            frame.render_widget(Paragraph::new(input_lines), rows[0]);

            if recent.is_empty() {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "No recent models saved for this provider.",
                        Style::default().fg(Color::DarkGray),
                    ))),
                    rows[1],
                );
            } else {
                let list_items: Vec<ListItem<'_>> = recent
                    .iter()
                    .map(|m| {
                        let active_mark = if *m == app.model { "*" } else { " " };
                        ListItem::new(Line::from(vec![
                            Span::raw(format!("{active_mark} ")),
                            Span::styled(m.clone(), Style::default().add_modifier(Modifier::BOLD)),
                        ]))
                    })
                    .collect();
                let mut state = ListState::default();
                if let Some(idx) = selected_recent {
                    state.select(Some((*idx).min(recent.len() - 1)));
                }
                let list = List::new(list_items).highlight_style(
                    Style::default()
                        .bg(Color::Blue)
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                );
                frame.render_stateful_widget(list, rows[1], &mut state);
            }

            let status_line = if let Some(err) = error {
                Line::from(Span::styled(
                    format!("Error: {err}"),
                    Style::default().fg(Color::Red),
                ))
            } else {
                Line::from(Span::styled(
                    "Any model name is accepted; validation runs on request.",
                    Style::default().fg(Color::DarkGray),
                ))
            };
            let footer_lines = vec![
                status_line,
                Line::from(Span::styled(
                    "Enter:apply  Up/Down:recent  Ctrl-u:clear  Esc:close",
                    Style::default().fg(Color::DarkGray),
                )),
            ];
            frame.render_widget(Paragraph::new(footer_lines), rows[2]);
        }
        ActiveModal::DiffView {
            file_index,
            diff,
            scroll,
        } => {
            let popup = centered_rect(area, 92, 84, 60, 12);
            let file_pos = if app.file_changes.is_empty() {
                String::new()
            } else {
                format!(" [{}/{}]", file_index + 1, app.file_changes.len())
            };
            let title = format!(
                " Diff: {} ({}) +{} -{}{file_pos} ",
                diff.path, diff.status, diff.additions, diff.deletions
            );
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(title);
            let inner = block.inner(popup);
            frame.render_widget(Clear, popup);
            frame.render_widget(block, popup);

            let rows = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).split(inner);
            let panes =
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(rows[0]);

            let left_block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(format!(" Original (a/{}) ", diff.path));
            let right_block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(format!(" Working Tree (b/{}) ", diff.path));

            let left_inner_w = left_block.inner(panes[0]).width as usize;
            let right_inner_w = right_block.inner(panes[1]).width as usize;
            let view_h = left_block.inner(panes[0]).height.max(1) as usize;
            let max_scroll = diff.rows.len().saturating_sub(view_h);
            let effective_scroll = (*scroll).min(max_scroll);

            let mut left_lines = Vec::with_capacity(view_h);
            let mut right_lines = Vec::with_capacity(view_h);
            for row in diff.rows.iter().skip(effective_scroll).take(view_h) {
                left_lines.push(format_diff_pane_line(
                    row.left.as_ref(),
                    &row.kind,
                    true,
                    left_inner_w,
                ));
                right_lines.push(format_diff_pane_line(
                    row.right.as_ref(),
                    &row.kind,
                    false,
                    right_inner_w,
                ));
            }

            frame.render_widget(Paragraph::new(left_lines).block(left_block), panes[0]);
            frame.render_widget(Paragraph::new(right_lines).block(right_block), panes[1]);
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "j/k:scroll  Ctrl-d/u:page  g/G:top/bot  Tab/[:]:next/prev file  Esc/q:close",
                    Style::default().fg(Color::DarkGray),
                ))),
                rows[1],
            );
        }
        ActiveModal::Search {
            query,
            mode,
            results,
            selected,
            preview,
            ..
        } => {
            let popup = centered_rect(area, 90, 82, 60, 14);
            let title = format!(
                " Search [{}] ({} matches) (/search) ",
                mode.label(),
                results.len()
            );
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(title);
            let inner = block.inner(popup);
            frame.render_widget(Clear, popup);
            frame.render_widget(block, popup);

            let layout = Layout::vertical([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(1),
            ])
            .split(inner);

            let mode_color = match mode {
                SearchMode::Files => Color::Green,
                SearchMode::Content => Color::Yellow,
            };
            let placeholder = match mode {
                SearchMode::Files => "(type to fuzzy-match file paths)",
                SearchMode::Content => "(type query to search content with ripgrep)",
            };
            let query_block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(" Query ");
            let query_inner = query_block.inner(layout[0]);
            let input_box = Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("[{}] ", mode.label()),
                    Style::default().fg(mode_color).add_modifier(Modifier::BOLD),
                ),
                if query.is_empty() {
                    Span::styled(placeholder, Style::default().fg(Color::DarkGray))
                } else {
                    Span::raw(query.as_str())
                },
            ]))
            .block(query_block);
            frame.render_widget(input_box, layout[0]);
            if query_inner.width > 0 && query_inner.height > 0 {
                let prefix_chars = mode.label().chars().count() + 3;
                let query_chars = query.chars().count();
                let cursor_x = (query_inner.x + (prefix_chars + query_chars) as u16)
                    .min(query_inner.x + query_inner.width.saturating_sub(1));
                frame.set_cursor_position(Position::new(cursor_x, query_inner.y));
            }

            let panes =
                Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
                    .split(layout[1]);

            let results_block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(" Results ");
            let results_inner_w = results_block.inner(panes[0]).width as usize;

            let list_items: Vec<ListItem<'_>> = results
                .iter()
                .map(|m| {
                    let text = match m.line {
                        Some(ln) => {
                            let col = m.column.unwrap_or(1);
                            format!("{}:{ln}:{col}  {}", m.path, m.snippet)
                        }
                        None => m.path.clone(),
                    };
                    ListItem::new(Line::from(truncate_single_line(&text, results_inner_w)))
                })
                .collect();

            let mut list_state = ListState::default();
            if !results.is_empty() {
                list_state.select(Some((*selected).min(results.len() - 1)));
            }
            let list = List::new(list_items).block(results_block).highlight_style(
                Style::default()
                    .bg(Color::Blue)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            );
            frame.render_stateful_widget(list, panes[0], &mut list_state);

            let preview_title = preview
                .as_ref()
                .map(|p| format!(" Preview: {} ", p.title))
                .unwrap_or_else(|| " Preview ".to_string());
            let preview_block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(preview_title);
            let preview_inner = preview_block.inner(panes[1]);
            let preview_w = preview_inner.width as usize;
            let preview_h = preview_inner.height.max(1) as usize;

            let preview_lines: Vec<Line<'_>> = match preview {
                Some(prev) => {
                    let max_scroll = prev.lines.len().saturating_sub(preview_h);
                    let scroll = prev.scroll.min(max_scroll);
                    prev.lines
                        .iter()
                        .skip(scroll)
                        .take(preview_h)
                        .map(|(ln, text, highlighted)| {
                            let gutter = format!("{ln:>4} │ ");
                            let body = truncate_single_line(
                                text,
                                preview_w.saturating_sub(gutter.chars().count()),
                            );
                            if *highlighted {
                                Line::from(vec![
                                    Span::styled(
                                        gutter,
                                        Style::default()
                                            .fg(Color::Yellow)
                                            .bg(USER_ROW_BG)
                                            .add_modifier(Modifier::BOLD),
                                    ),
                                    Span::styled(
                                        body,
                                        Style::default()
                                            .fg(Color::White)
                                            .bg(USER_ROW_BG)
                                            .add_modifier(Modifier::BOLD),
                                    ),
                                ])
                                .style(Style::default().bg(USER_ROW_BG))
                            } else {
                                Line::from(vec![
                                    Span::styled(gutter, Style::default().fg(Color::DarkGray)),
                                    Span::raw(body),
                                ])
                            }
                        })
                        .collect()
                }
                None => vec![Line::from(Span::styled(
                    "No matching results.",
                    Style::default().fg(Color::DarkGray),
                ))],
            };
            frame.render_widget(Paragraph::new(preview_lines).block(preview_block), panes[1]);

            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "Tab:Files/Content  Up/Down:select  PgUp/PgDn:scroll preview  Enter:insert @file  Esc:close",
                    Style::default().fg(Color::DarkGray),
                ))),
                layout[2],
            );
        }
        ActiveModal::Approval {
            queue,
            reason,
            editing_reason,
        } => {
            let Some(request) = queue.first() else {
                return;
            };
            let popup = centered_rect(area, 88, 48, 50, 12);
            let title = if queue.len() > 1 {
                format!(" Approval required ({}) ", queue.len())
            } else {
                " Approval required ".to_string()
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::LightRed))
                .title(title);
            let inner = block.inner(popup);
            frame.render_widget(Clear, popup);
            frame.render_widget(block, popup);

            let rows = Layout::vertical([
                Constraint::Length(2),
                Constraint::Min(3),
                Constraint::Length(3),
            ])
            .split(inner);

            let kind = match request.kind {
                ApprovalKind::ToolCall => "tool call",
                ApprovalKind::Spend => "spend",
                ApprovalKind::SandboxEscape => "sandbox escape",
            };
            let effect = match request.effect {
                Some(EffectClass::Read) => " · effect: read",
                Some(EffectClass::Write) => " · effect: write",
                Some(EffectClass::Execute) => " · effect: execute",
                Some(EffectClass::External) => " · effect: external",
                None => "",
            };
            let header = vec![
                Line::from(Span::styled(
                    request.preview.title.clone(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    format!("kind: {kind}{effect}"),
                    Style::default().fg(Color::DarkGray),
                )),
            ];
            frame.render_widget(Paragraph::new(header), rows[0]);

            let detail = if request.preview.detail.is_empty() {
                vec![Line::from(Span::styled(
                    "(no details)",
                    Style::default().fg(Color::DarkGray),
                ))]
            } else {
                request
                    .preview
                    .detail
                    .lines()
                    .map(|line| Line::from(line.to_string()))
                    .collect()
            };
            frame.render_widget(Paragraph::new(detail).wrap(Wrap { trim: false }), rows[1]);

            let footer = if *editing_reason {
                vec![
                    Line::from(vec![
                        Span::styled("Reason: ", Style::default().fg(Color::Yellow)),
                        Span::raw(reason.as_str()),
                        Span::styled("█", Style::default().fg(Color::Yellow)),
                    ]),
                    Line::from(Span::styled(
                        "Enter:send decline reason  Esc:back",
                        Style::default().fg(Color::DarkGray),
                    )),
                ]
            } else {
                vec![
                    Line::from(Span::styled(
                        "y accept · t accept-for-turn · a accept-for-session · n decline · r decline-with-reason · esc cancel",
                        Style::default().fg(Color::DarkGray),
                    )),
                    Line::default(),
                ]
            };
            frame.render_widget(Paragraph::new(footer), rows[2]);
        }
    }
}

fn format_diff_pane_line(
    cell: Option<&DiffCell>,
    kind: &DiffRowKind,
    is_left: bool,
    pane_width: usize,
) -> Line<'static> {
    if let DiffRowKind::HunkHeader(header) = kind {
        return Line::from(Span::styled(
            truncate_single_line(header, pane_width),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let Some(cell) = cell else {
        return Line::from(Span::styled("~", Style::default().fg(Color::DarkGray)));
    };

    let prefix = match (kind, is_left) {
        (DiffRowKind::Removed, true) | (DiffRowKind::Modified, true) => "-",
        (DiffRowKind::Added, false) | (DiffRowKind::Modified, false) => "+",
        _ => " ",
    };
    let line_num = match cell.line_no {
        Some(n) => format!("{n:>4} {prefix} "),
        None => format!("     {prefix} "),
    };
    let body = truncate_single_line(
        &cell.text,
        pane_width.saturating_sub(line_num.chars().count()),
    );

    let style = match (kind, is_left) {
        (DiffRowKind::Removed, true) | (DiffRowKind::Modified, true) => {
            Style::default().fg(Color::Red)
        }
        (DiffRowKind::Added, false) | (DiffRowKind::Modified, false) => {
            Style::default().fg(Color::Green)
        }
        _ => Style::default().fg(Color::Gray),
    };

    Line::from(vec![
        Span::styled(line_num, Style::default().fg(Color::DarkGray)),
        Span::styled(body, style),
    ])
}

fn display_model(app: &AppState) -> &str {
    let trimmed = app.model.trim();
    if trimmed.is_empty() {
        "(not set)"
    } else {
        trimmed
    }
}

fn render_statusline(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let (badge_text, badge_bg) = match app.mode {
        InputMode::Insert => (" INS ", Color::Green),
        InputMode::Normal => (" NOR ", Color::Blue),
    };

    let mut spans = vec![Span::styled(
        badge_text,
        Style::default()
            .fg(Color::Black)
            .bg(badge_bg)
            .add_modifier(Modifier::BOLD),
    )];

    let mode_bg = match app.approval_mode {
        ApprovalMode::Plan => Color::Magenta,
        ApprovalMode::Ask => Color::Yellow,
        ApprovalMode::AutoEdit => Color::Cyan,
        ApprovalMode::Yolo => Color::LightRed,
    };
    spans.push(Span::styled(
        format!("{} ", approval_mode_badge(app.approval_mode)),
        Style::default()
            .fg(Color::Black)
            .bg(mode_bg)
            .add_modifier(Modifier::BOLD),
    ));

    let key_status = app.active_key_status();
    let (key_badge, key_color) = key_status_style(&key_status);

    spans.push(Span::raw(format!(
        "  {} / {} ",
        app.active_provider,
        display_model(app)
    )));
    spans.push(Span::styled(
        format!("[{key_badge}]"),
        Style::default().fg(key_color),
    ));

    let usage_str = format_usage_compact(app);
    spans.push(Span::styled(
        format!(" | {usage_str}"),
        Style::default().fg(Color::Gray),
    ));

    if let Some(session_usage) = format_session_usage(app) {
        spans.push(Span::styled(
            format!(" · {session_usage}"),
            Style::default().fg(Color::DarkGray),
        ));
    }

    if let Some(speed) = app.metrics.last_tokens_per_sec {
        spans.push(Span::styled(
            format!(" {speed:.1} tok/s"),
            Style::default().fg(MINT_TEAL),
        ));
    }

    if app.is_running {
        let frame_char = SPINNER_FRAMES[app.spinner_tick % SPINNER_FRAMES.len()];
        let elapsed = app
            .metrics
            .elapsed_ms()
            .map(|ms| format!(" ({})", format_compact_ms(ms)))
            .unwrap_or_default();
        let activity = match &app.active_tool {
            Some(tool) => format!(" | {frame_char} tool:{tool}{elapsed}"),
            None => format!(" | {frame_char} running{elapsed}"),
        };
        spans.push(Span::styled(
            activity,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    } else if let Some(turn_ms) = app.metrics.last_turn_ms {
        spans.push(Span::styled(
            format!(" {}", format_compact_ms(turn_ms)),
            Style::default().fg(Color::DarkGray),
        ));
    }

    if let Some(notice) = &app.status_notice {
        spans.push(Span::styled(
            format!(" | {notice}"),
            Style::default().fg(Color::LightRed),
        ));
    } else {
        let hint = match app.mode {
            InputMode::Insert => {
                if app.completion.is_active() {
                    " | Tab/Enter:accept Esc:dismiss"
                } else {
                    " | Ctrl-f:search Ctrl-t:think Ctrl-b:side /connect /models Esc:Normal"
                }
            }
            InputMode::Normal => " | i:Insert /:search t:think Ctrl-b:side j/k:scroll q:quit",
        };
        spans.push(Span::styled(hint, Style::default().fg(Color::DarkGray)));
    }

    let bar = Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::Black));
    frame.render_widget(bar, area);
}

fn format_compact_tokens(count: u64) -> String {
    if count >= 1_000_000 {
        let formatted = format!("{:.1}M", count as f64 / 1_000_000.0);
        formatted
            .strip_suffix(".0M")
            .map_or(formatted.clone(), |s| format!("{s}M"))
    } else if count >= 1_000 {
        let formatted = format!("{:.1}k", count as f64 / 1_000.0);
        formatted
            .strip_suffix(".0k")
            .map_or(formatted.clone(), |s| format!("{s}k"))
    } else {
        count.to_string()
    }
}

fn format_compact_ms(ms: u64) -> String {
    if ms >= 60_000 {
        let total_secs = ms / 1000;
        format!("{}m{}s", total_secs / 60, total_secs % 60)
    } else if ms >= 1_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{ms}ms")
    }
}

fn format_usage_compact(app: &AppState) -> String {
    let p = app.usage.prompt_tokens.map(|v| v.max(0) as u64);
    let c = app.usage.completion_tokens.map(|v| v.max(0) as u64);
    let t = app
        .usage
        .total_tokens
        .map(|v| v.max(0) as u64)
        .or_else(|| match (p, c) {
            (Some(pv), Some(cv)) => Some(pv + cv),
            _ => None,
        });

    match (p, c, t) {
        (Some(pv), Some(cv), Some(tv)) => format!(
            "↑{} ↓{} Σ{}",
            format_compact_tokens(pv),
            format_compact_tokens(cv),
            format_compact_tokens(tv)
        ),
        (_, _, Some(tv)) => format!("Σ{}", format_compact_tokens(tv)),
        (Some(pv), Some(cv), None) => format!(
            "↑{} ↓{}",
            format_compact_tokens(pv),
            format_compact_tokens(cv)
        ),
        _ => "-".to_string(),
    }
}

fn format_session_usage(app: &AppState) -> Option<String> {
    let total = app.metrics.session_total_tokens;
    if total == 0 {
        return None;
    }
    Some(format!(
        "session ↑{} ↓{} Σ{}",
        format_compact_tokens(app.metrics.session_prompt_tokens),
        format_compact_tokens(app.metrics.session_completion_tokens),
        format_compact_tokens(total)
    ))
}

fn key_status_style(status: &KeyStatus) -> (String, Color) {
    let badge = status.badge();
    let color = match status {
        KeyStatus::Stored | KeyStatus::Config => Color::Green,
        KeyStatus::Env(_) => Color::Cyan,
        KeyStatus::NotRequired => Color::Blue,
        KeyStatus::Missing => Color::Red,
    };
    (badge, color)
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

fn centered_rect_fixed(area: Rect, max_w: u16, max_h: u16) -> Rect {
    let width = max_w.min(area.width.saturating_sub(2));
    let height = max_h.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width, height)
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

pub fn summarize_error_line(text: &str, max_chars: usize) -> String {
    static MESSAGE_RE: OnceLock<Regex> = OnceLock::new();
    static CODE_RE: OnceLock<Regex> = OnceLock::new();
    static SPACE_RE: OnceLock<Regex> = OnceLock::new();

    let mut cleaned = text.trim();
    for prefix in ["LLM stream request failed: ", "LLM stream error: "] {
        if let Some(rest) = cleaned.strip_prefix(prefix) {
            cleaned = rest.trim_start();
        }
    }

    let unescaped = cleaned.replace("\\\"", "\"");
    let msg_re = MESSAGE_RE
        .get_or_init(|| Regex::new(r#""message"\s*:\s*"([^"]+)""#).expect("valid msg regex"));
    let code_re =
        CODE_RE.get_or_init(|| Regex::new(r#""code"\s*:\s*"([^"]+)""#).expect("valid code regex"));
    let space_re = SPACE_RE.get_or_init(|| Regex::new(r"\s+").expect("valid space regex"));

    let extracted = if let Some(msg) = msg_re
        .captures(&unescaped)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim())
        .filter(|s| !s.is_empty())
    {
        match code_re
            .captures(&unescaped)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().trim())
            .filter(|s| !s.is_empty())
        {
            Some(code) => format!("{code}: {msg}"),
            None => msg.to_string(),
        }
    } else {
        space_re.replace_all(cleaned, " ").trim().to_string()
    };

    let single = space_re.replace_all(&extracted, " ");
    truncate_single_line(single.trim(), max_chars)
}

fn capture_screen_lines(frame: &mut Frame<'_>, app: &mut AppState) {
    let area = frame.area();
    let buf = frame.buffer_mut();
    let mut rows = Vec::with_capacity(area.height as usize);
    for row in 0..area.height {
        let y = area.y + row;
        let mut cells = Vec::with_capacity(area.width as usize);
        for col in 0..area.width {
            let x = area.x + col;
            let sym = buf.cell((x, y)).map_or(" ", |c| c.symbol());
            cells.push(sym.to_string());
        }
        rows.push(cells);
    }
    app.screen_lines = rows;
}

fn render_selection_highlight(frame: &mut Frame<'_>, app: &AppState) {
    let Some(sel) = app.selection else {
        return;
    };
    if !sel.active || sel.anchor == sel.head {
        return;
    }
    let bounds = app.selection_bounds(sel.anchor);
    let Some((start, end)) = normalize_selection_in_bounds(sel.anchor, sel.head, bounds) else {
        return;
    };
    let min_col = bounds.x;
    let max_col = bounds.x + bounds.width - 1;
    let buf = frame.buffer_mut();
    let highlight = Style::default()
        .bg(Color::Rgb(58, 74, 107))
        .fg(Color::White);
    for row in start.row..=end.row {
        let c_start = if row == start.row { start.col } else { min_col }.max(min_col);
        let c_end = if row == end.row { end.col } else { max_col }.min(max_col);
        for col in c_start..=c_end {
            if let Some(cell) = buf.cell_mut((col, row)) {
                cell.set_style(highlight);
            }
        }
    }
}

fn wrap_tagged_lines(
    lines: Vec<(Line<'static>, Option<usize>)>,
    width: usize,
) -> (Vec<Line<'static>>, Vec<ThinkingHitZone>) {
    if width == 0 {
        return (Vec::new(), Vec::new());
    }

    let mut out = Vec::with_capacity(lines.len());
    let mut hit_zones: Vec<ThinkingHitZone> = Vec::new();
    let mut active_zone: Option<(usize, usize)> = None;

    for (line, entry_tag) in lines {
        let line_start = out.len();
        let total_chars: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
        if total_chars == 0 {
            out.push(Line::default());
        } else {
            let mut current_spans: Vec<Span<'static>> = Vec::new();
            let mut current_col = 0;

            for span in line.spans {
                let style = span.style;
                let mut buf = String::new();
                for ch in span.content.chars() {
                    if current_col == width {
                        if !buf.is_empty() {
                            current_spans.push(Span::styled(std::mem::take(&mut buf), style));
                        }
                        out.push(Line::from(std::mem::take(&mut current_spans)));
                        current_col = 0;
                    }
                    buf.push(ch);
                    current_col += 1;
                }
                if !buf.is_empty() {
                    current_spans.push(Span::styled(buf, style));
                }
            }

            if !current_spans.is_empty() {
                out.push(Line::from(current_spans));
            }
        }
        match (active_zone, entry_tag) {
            (Some((cur_idx, _start)), Some(idx)) if cur_idx == idx => {}
            (Some((cur_idx, start)), Some(idx)) => {
                hit_zones.push(ThinkingHitZone {
                    entry_idx: cur_idx,
                    start_line: start,
                    end_line: line_start,
                });
                active_zone = Some((idx, line_start));
            }
            (Some((cur_idx, start)), None) => {
                hit_zones.push(ThinkingHitZone {
                    entry_idx: cur_idx,
                    start_line: start,
                    end_line: line_start,
                });
                active_zone = None;
            }
            (None, Some(idx)) => {
                active_zone = Some((idx, line_start));
            }
            (None, None) => {}
        }
    }

    if let Some((cur_idx, start)) = active_zone {
        hit_zones.push(ThinkingHitZone {
            entry_idx: cur_idx,
            start_line: start,
            end_line: out.len(),
        });
    }

    (out, hit_zones)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, RwLock};

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use meerai_agents::{
        AgentEvent, ErrorCode, ItemDelta, ItemId, SessionItem, TurnEndReason, TurnId, Usage,
    };
    use meerai_common::settings::AppSettings;
    use meerai_providers::providers::{ProviderCatalog, RuntimeProviderState};
    use meerai_providers::secrets::{MemorySecretStore, SecretStore};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tokio::sync::mpsc;

    use super::*;

    fn test_app() -> AppState {
        let (tx, _rx) = mpsc::unbounded_channel();
        let secrets = Arc::new(MemorySecretStore::new());
        secrets.set_key("gemini", "test-key").unwrap();
        let settings = AppSettings::default();
        let runtime = Arc::new(RwLock::new(RuntimeProviderState::from_settings(
            &settings,
            ProviderCatalog::new(),
        )));
        AppState::new(
            "gemini",
            "gemini-3.8-flash",
            PathBuf::from("."),
            tx,
            runtime,
            secrets,
            settings,
            None,
        )
    }

    fn render_to_string_with_size(app: &mut AppState, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn render_to_string(app: &mut AppState) -> String {
        render_to_string_with_size(app, 120, 28)
    }

    fn agent_definition(id: &str) -> AgentDefinition {
        AgentDefinition {
            id: id.to_string(),
            source: meerai_common::definitions::DefinitionSource::Agents,
            description: Some("Reviews changes".to_string()),
            model: None,
            steps: None,
            mode: None,
            prompt: String::new(),
            path: PathBuf::from(format!(".agents/agents/{id}.md")),
        }
    }

    fn skill_definition(id: &str) -> SkillDefinition {
        SkillDefinition {
            id: id.to_string(),
            source: meerai_common::definitions::DefinitionSource::Claude,
            description: Some("Delivers work".to_string()),
            body: String::new(),
            path: PathBuf::from(format!(".claude/skills/{id}/SKILL.md")),
        }
    }

    fn span_text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|span| span.content.to_string()).collect()
    }

    #[test]
    fn hides_sidebar_when_terminal_narrow_and_clears_hit_area() {
        let mut app = test_app();
        app.file_changes.push(crate::app::FileChangeEntry {
            status: "M".to_string(),
            path: "meerai-tui/src/ui.rs".to_string(),
        });

        let wide = render_to_string_with_size(&mut app, 120, 28);
        assert!(wide.contains("Session"));
        assert!(wide.contains("File Changes (1)"));
        assert!(app.visible_file_change_rows == 1);
        assert!(app.sidebar_changes_inner_area.width > 0);

        let narrow = render_to_string_with_size(&mut app, 90, 28);
        assert!(!narrow.contains("File Changes (1)"));
        assert_eq!(app.visible_file_change_rows, 0);
        assert_eq!(app.sidebar_changes_inner_area, Rect::default());
    }

    #[test]
    fn renders_transcript_completion_and_statusline() {
        let mut app = test_app();
        app.transcript.push(TranscriptEntry::User {
            text: "Explain Cargo.toml".to_string(),
        });
        let turn_id = TurnId::from_raw("turn-ui-render");
        app.handle_agent_event(AgentEvent::TurnStarted {
            turn_id: turn_id.clone(),
            model: "gemini::gemini-3.8-flash".to_string(),
        });
        let message_id = ItemId::from_raw("item-ui-render");
        app.handle_agent_event(AgentEvent::ItemStarted {
            item: SessionItem::AgentMessage {
                id: message_id.clone(),
                text: String::new(),
            },
        });
        app.handle_agent_event(AgentEvent::ItemDelta {
            item_id: message_id.clone(),
            delta: ItemDelta::AgentText("Sure thing.".to_string()),
        });
        app.handle_agent_event(AgentEvent::UsageUpdated {
            usage: Usage {
                prompt_tokens: Some(100),
                completion_tokens: Some(25),
                total_tokens: Some(125),
                cached_tokens: None,
            },
            cumulative: Usage {
                prompt_tokens: Some(100),
                completion_tokens: Some(25),
                total_tokens: Some(125),
                cached_tokens: None,
            },
            est_cost_usd: None,
        });
        app.handle_agent_event(AgentEvent::ItemCompleted {
            item: SessionItem::AgentMessage {
                id: message_id,
                text: "Sure thing.".to_string(),
            },
        });
        app.handle_agent_event(AgentEvent::TurnCompleted {
            turn_id,
            reason: TurnEndReason::Completed,
        });

        app.metrics.last_tokens_per_sec = Some(42.5);
        app.metrics.last_ttft_ms = Some(310);
        app.metrics.last_turn_ms = Some(1400);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert!(app.completion.is_active());

        let rendered = render_to_string(&mut app);
        assert!(rendered.contains("INS"));
        assert!(rendered.contains("gemini / gemini-3.8-flash"));
        assert!(rendered.contains("[saved]"));
        assert!(rendered.contains("↑100 ↓25 Σ125"));
        assert!(rendered.contains("session ↑100 ↓25 Σ125"));
        assert!(rendered.contains("42.5 tok/s · 310ms"));
        assert!(rendered.contains("idle (1.4s)"));
        assert!(rendered.contains("██      ██"));
        assert!(rendered.contains("meerai"));
        assert!(rendered.contains("Commands"));
        assert!(!rendered.contains("> You"));
        assert!(!rendered.contains("◆ Assistant"));
        assert!(rendered.contains("Session"));
        assert!(rendered.contains("MCP (0)"));
        assert!(rendered.contains("Agents (1)"));
        assert!(rendered.contains("Tools (0 calls)"));
        assert!(rendered.contains("File Changes"));
    }

    #[test]
    fn renders_agent_and_skill_mention_badges() {
        use meerai_common::definitions::{AgentDefinition, DefinitionSource, SkillDefinition};

        let mut app = test_app();
        app.agents = vec![AgentDefinition {
            id: "reviewer".to_string(),
            source: DefinitionSource::Agents,
            description: Some("Reviews changes".to_string()),
            model: None,
            steps: None,
            mode: None,
            prompt: String::new(),
            path: PathBuf::from(".agents/agents/reviewer.md"),
        }];
        app.skills = vec![SkillDefinition {
            id: "reviewer".to_string(),
            source: DefinitionSource::Claude,
            description: Some("Review skill".to_string()),
            body: String::new(),
            path: PathBuf::from(".claude/skills/reviewer/SKILL.md"),
        }];

        for ch in "@reviewer".chars() {
            app.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        assert!(app.completion.is_active());

        let rendered = render_to_string(&mut app);
        assert!(rendered.contains("Mentions"));
        assert!(rendered.contains("reviewer|agent"));
        assert!(rendered.contains("reviewer|skill"));
        assert!(rendered.contains(".agents — Reviews changes"));
        assert!(rendered.contains(".claude — Review skill"));
        assert!(!rendered.contains("@agent/reviewer"));
        assert!(!rendered.contains("@skill/reviewer"));
    }

    #[test]
    fn composer_title_names_mentions() {
        let mut app = test_app();
        let rendered = render_to_string(&mut app);
        assert!(rendered.contains("@ mentions"));
    }

    #[test]
    fn composer_renders_known_mention_chips() {
        let agents = vec![agent_definition("reviewer")];
        let skills = vec![skill_definition("deliver")];

        let (spans, cursor) = render_composer_line("@skill/deliver", Some(14), &agents, &skills);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "deliver");
        assert_eq!(spans[0].style.fg, Some(Color::Magenta));
        assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(cursor, 7);

        let (spans, cursor) = render_composer_line("@agent/reviewer", Some(15), &agents, &skills);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "reviewer");
        assert_eq!(spans[0].style.fg, Some(Color::Cyan));
        assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(cursor, 8);
    }

    #[test]
    fn composer_keeps_unknown_and_partial_mentions_raw() {
        let agents = vec![agent_definition("reviewer")];
        let skills = vec![skill_definition("deliver")];

        let (spans, cursor) =
            render_composer_line("@skill/doesnotexist", Some(19), &agents, &skills);
        assert_eq!(span_text(&spans), "@skill/doesnotexist");
        assert_eq!(cursor, 19);

        let (spans, cursor) = render_composer_line("@skill/del", Some(10), &agents, &skills);
        assert_eq!(span_text(&spans), "@skill/del");
        assert_eq!(cursor, 10);

        let (spans, _) = render_composer_line("@agent/nope @skill/deliver", None, &agents, &skills);
        assert_eq!(span_text(&spans), "@agent/nope deliver");
    }

    #[test]
    fn composer_renders_multiple_chips_and_surrounding_text() {
        let agents = vec![agent_definition("reviewer")];
        let skills = vec![skill_definition("deliver")];

        let line = "run @skill/deliver then @agent/reviewer now";
        let (spans, _) = render_composer_line(line, None, &agents, &skills);
        let texts: Vec<String> = spans.iter().map(|span| span.content.to_string()).collect();
        assert_eq!(texts, vec!["run ", "deliver", " then ", "reviewer", " now"]);
        assert_eq!(span_text(&spans), "run deliver then reviewer now");
        assert_eq!(spans[1].style.fg, Some(Color::Magenta));
        assert_eq!(spans[3].style.fg, Some(Color::Cyan));
    }

    #[test]
    fn composer_cursor_column_uses_chip_display_width() {
        let agents = vec![agent_definition("reviewer")];
        let skills = vec![skill_definition("deliver")];
        let line = "x @skill/deliver";

        let (_, before) = render_composer_line(line, Some(2), &agents, &skills);
        assert_eq!(before, 2);

        let (_, inside) = render_composer_line(line, Some(6), &agents, &skills);
        assert_eq!(inside, 9);

        let (_, after) = render_composer_line(line, Some(line.chars().count()), &agents, &skills);
        assert_eq!(after, 9);

        let (_, middle) = render_composer_line(line, Some(4), &agents, &skills);
        assert_eq!(middle, 9);
    }

    #[test]
    fn composer_cursor_keeps_multiline_row_and_chip_columns() {
        let agents = vec![agent_definition("reviewer")];
        let skills = vec![skill_definition("deliver")];
        let input = "first line\nrun @skill/deliver";
        let cursor = input.len();

        let (row, raw_col) = cursor_row_col(input, cursor);
        assert_eq!(row, 1);

        let line = input.split('\n').nth(row).unwrap();
        let (spans, display_col) = render_composer_line(line, Some(raw_col), &agents, &skills);
        assert_eq!(span_text(&spans), "run deliver");
        assert_eq!(display_col, "run deliver".chars().count());
    }

    #[test]
    fn statusline_renders_approval_mode_badge() {
        let mut app = test_app();
        app.approval_mode = ApprovalMode::AutoEdit;
        let edit = render_to_string(&mut app);
        assert!(edit.contains("EDIT"));
        assert!(edit.contains("INS"));

        app.approval_mode = ApprovalMode::Yolo;
        assert!(render_to_string(&mut app).contains("YOLO"));

        app.approval_mode = ApprovalMode::Plan;
        assert!(render_to_string(&mut app).contains("PLAN"));
    }

    #[test]
    fn renders_approval_overlay_with_preview_and_key_hints() {
        use meerai_agents::events::{ApprovalKind, ApprovalPreview, ApprovalRequest, RequestId};
        use meerai_core::EffectClass;

        let mut app = test_app();
        let request = ApprovalRequest {
            request_id: RequestId::from_raw("req-ui"),
            turn_id: TurnId::from_raw("turn-ui-approval"),
            kind: ApprovalKind::ToolCall,
            preview: ApprovalPreview {
                title: "edit: src/main.rs".to_string(),
                detail: "tool: code-write_file\neffect: write\nresource: src/main.rs".to_string(),
            },
            effect: Some(EffectClass::Write),
        };
        app.handle_agent_event(AgentEvent::ApprovalRequested { request });

        let rendered = render_to_string(&mut app);
        assert!(rendered.contains("Approval required"));
        assert!(rendered.contains("edit: src/main.rs"));
        assert!(rendered.contains("kind: tool call · effect: write"));
        assert!(rendered.contains("resource: src/main.rs"));
        assert!(rendered.contains("y accept"));
        assert!(rendered.contains("t accept-for-turn"));
        assert!(rendered.contains("accept-for-session"));
        assert!(rendered.contains("esc cancel"));

        app.handle_key_event(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        let editing = render_to_string(&mut app);
        assert!(editing.contains("Reason:"));
        assert!(editing.contains("Enter:send decline reason"));

        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        let second = ApprovalRequest {
            request_id: RequestId::from_raw("req-ui-2"),
            turn_id: TurnId::from_raw("turn-ui-approval"),
            kind: ApprovalKind::ToolCall,
            preview: ApprovalPreview {
                title: "shell: cargo test".to_string(),
                detail: "tool: code-shell\neffect: execute".to_string(),
            },
            effect: Some(EffectClass::Execute),
        };
        app.handle_agent_event(AgentEvent::ApprovalRequested { request: second });
        let queued = render_to_string(&mut app);
        assert!(queued.contains("Approval required (2)"));
        assert!(queued.contains("edit: src/main.rs"));
        assert!(!queued.contains("shell: cargo test"));
    }

    #[test]
    fn formats_compact_tokens_and_ms() {
        assert_eq!(format_compact_tokens(0), "0");
        assert_eq!(format_compact_tokens(999), "999");
        assert_eq!(format_compact_tokens(1_000), "1k");
        assert_eq!(format_compact_tokens(1_100), "1.1k");
        assert_eq!(format_compact_tokens(1_000_000), "1M");
        assert_eq!(format_compact_tokens(2_500_000), "2.5M");

        assert_eq!(format_compact_ms(45), "45ms");
        assert_eq!(format_compact_ms(1_420), "1.4s");
        assert_eq!(format_compact_ms(65_000), "1m5s");
    }

    #[test]
    fn renders_collapsible_thinking_and_records_click_hit_zone() {
        let mut app = test_app();
        app.file_changes.push(crate::app::FileChangeEntry {
            status: "M".to_string(),
            path: "meerai-tui/src/ui.rs".to_string(),
        });
        app.transcript.push(TranscriptEntry::User {
            text: "hi".to_string(),
        });
        app.handle_agent_event(AgentEvent::TurnStarted {
            turn_id: TurnId::from_raw("turn-ui-thinking"),
            model: "gemini::gemini-4-hydrogen".to_string(),
        });
        let reasoning_id = ItemId::from_raw("item-ui-thinking");
        app.handle_agent_event(AgentEvent::ItemStarted {
            item: SessionItem::Reasoning {
                id: reasoning_id.clone(),
                summary: String::new(),
            },
        });
        app.handle_agent_event(AgentEvent::ItemDelta {
            item_id: reasoning_id,
            delta: ItemDelta::ReasoningSummary("secret thought step 1\nstep 2".to_string()),
        });
        app.handle_agent_event(AgentEvent::ItemDelta {
            item_id: ItemId::from_raw("item-ui-thinking-text"),
            delta: ItemDelta::AgentText("Hello!".to_string()),
        });

        let collapsed = render_to_string(&mut app);
        assert!(collapsed.contains("▸ Thinking (2 lines)"));
        assert!(!collapsed.contains("secret thought step 1"));
        assert!(collapsed.contains("File Changes (1)"));
        assert!(collapsed.contains("meerai-tui/src/ui.rs"));
        assert!(!app.thinking_hit_zones.is_empty());

        let zone = app.thinking_hit_zones[0];
        let click_row = app.transcript_inner_area.y + zone.start_line as u16;
        let click_col = app.transcript_inner_area.x + 2;
        assert!(app.toggle_thinking_at(click_col, click_row));

        let expanded = render_to_string(&mut app);
        assert!(expanded.contains("▾ Thinking (2 lines)"));
        assert!(expanded.contains("secret thought step 1"));
    }

    #[test]
    fn renders_provider_apikey_masked_and_model_modals() {
        let mut app = test_app();

        app.modal = Some(ActiveModal::ProviderSelect {
            filter: String::new(),
            selected: 0,
        });
        let provider_screen = render_to_string(&mut app);
        assert!(provider_screen.contains("Connect Provider (/connect)"));

        app.modal = Some(ActiveModal::ApiKeyInput {
            provider_id: "opencode-go".to_string(),
            provider_name: "OpenCode Go".to_string(),
            api_key: "sk-super-secret".to_string(),
            base_url: String::new(),
            editing_base_url: false,
            existing_key_status: KeyStatus::Missing,
            error: None,
        });
        let key_screen = render_to_string(&mut app);
        assert!(key_screen.contains("Connect OpenCode Go (opencode-go)"));
        assert!(key_screen.contains("•••••••••••••••"));
        assert!(!key_screen.contains("sk-super-secret"));

        app.modal = Some(ActiveModal::ModelInput {
            input: String::new(),
            selected_recent: None,
            error: None,
        });
        let model_screen = render_to_string(&mut app);
        assert!(model_screen.contains("Set Model [gemini] (/model)"));

        app.modal = Some(ActiveModal::DiffView {
            file_index: 0,
            diff: crate::diff::FileDiffView {
                path: "src/lib.rs".to_string(),
                status: "M".to_string(),
                additions: 1,
                deletions: 1,
                rows: vec![crate::diff::DiffRow {
                    kind: DiffRowKind::Modified,
                    left: Some(DiffCell {
                        line_no: Some(10),
                        text: "old_val".to_string(),
                    }),
                    right: Some(DiffCell {
                        line_no: Some(10),
                        text: "new_val".to_string(),
                    }),
                }],
            },
            scroll: 0,
        });
        let diff_screen = render_to_string(&mut app);
        assert!(diff_screen.contains("Original (a/src/lib.rs)"));
        assert!(diff_screen.contains("Working Tree (b/src/lib.rs)"));
        assert!(diff_screen.contains("old_val"));
        assert!(diff_screen.contains("new_val"));

        app.modal = Some(ActiveModal::Search {
            query: "lib".to_string(),
            mode: SearchMode::Files,
            workspace_files: vec!["src/lib.rs".to_string()],
            results: vec![crate::search::SearchMatch {
                path: "src/lib.rs".to_string(),
                line: Some(10),
                column: Some(1),
                snippet: "pub fn lib()".to_string(),
            }],
            selected: 0,
            preview: Some(crate::search::SearchPreview {
                title: "src/lib.rs:10".to_string(),
                lines: vec![(10, "pub fn lib() {}".to_string(), true)],
                scroll: 0,
            }),
        });
        let search_screen = render_to_string(&mut app);
        assert!(search_screen.contains("Search [Files] (1 matches) (/search)"));
        assert!(search_screen.contains("Preview: src/lib.rs:10"));
    }
    #[test]
    fn renders_collapsible_single_line_error_and_expands_on_click() {
        let mut app = test_app();
        app.handle_agent_event(AgentEvent::Error {
            code: ErrorCode::Provider,
            message: "LLM stream request failed: WebModelCall { body: \"{\\\"error\\\":{\\\"code\\\":\\\"MissingSessionID\\\",\\\"message\\\":\\\"Request is missing x-opencode-session\\\"}}\", headers: { foo: bar }\nline 2 details }".to_string(),
        });

        let collapsed = render_to_string(&mut app);
        assert!(
            collapsed.contains("▸ [error] MissingSessionID: Request is missing x-opencode-session")
        );
        assert!(collapsed.contains("[click to expand]"));
        assert!(!collapsed.contains("line 2 details"));
        assert!(!collapsed.contains("▾ [error]"));
        assert_eq!(app.thinking_hit_zones.len(), 1);

        let zone = app.thinking_hit_zones[0];
        let click_row = app.transcript_inner_area.y + zone.start_line as u16;
        let click_col = app.transcript_inner_area.x + 2;
        assert!(app.toggle_thinking_at(click_col, click_row));

        let expanded = render_to_string(&mut app);
        assert!(expanded.contains("▾ [error] (2 lines)"));
        assert!(expanded.contains("line 2 details"));

        let zone2 = app.thinking_hit_zones[0];
        let click_row2 = app.transcript_inner_area.y + zone2.start_line as u16;
        assert!(app.toggle_thinking_at(click_col, click_row2));
        let recollapsed = render_to_string(&mut app);
        assert!(recollapsed.contains("▸ [error]"));
        assert!(!recollapsed.contains("line 2 details"));
    }
}
