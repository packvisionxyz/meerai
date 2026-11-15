use std::sync::OnceLock;

use meerai_agents::events::ApprovalKind;
use meerai_common::definitions::{AgentDefinition, SkillDefinition};
use meerai_core::EffectClass;
use meerai_core::env::now_millis;
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
    ActiveModal, AppState, DocPos, InputMode, MentionKind, TabHitKind, TabHitZone, ThinkingHitZone,
    ToolCallStatus, TranscriptEntry, TranscriptWrappedLine, mention_is_known,
    normalize_selection_in_bounds, parse_mention_token,
};
use crate::completion::{CompletionItemKind, CompletionKind};
use crate::diff::{DiffCell, DiffRowKind};
use crate::search::SearchMode;
use crate::theme;

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

    render_tab_bar(frame, app, tab_area);

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

fn render_tab_bar(frame: &mut Frame<'_>, app: &mut AppState, area: Rect) {
    app.tab_hit_zones.clear();
    if area.width == 0 || area.height == 0 {
        return;
    }

    let width = area.width as usize;
    let labels: Vec<(String, Style)> = app
        .tabs
        .iter()
        .enumerate()
        .map(|(idx, tab)| {
            let is_active = idx == app.active_tab;
            let indicator = if !tab.pending_approvals.is_empty() {
                " [!]".to_string()
            } else if tab.is_running {
                let ch = SPINNER_FRAMES[app.spinner_tick % SPINNER_FRAMES.len()];
                format!(" {ch}")
            } else if tab.has_unread {
                " •".to_string()
            } else {
                String::new()
            };
            let title = truncate_single_line(&tab.display_title(), 18);
            let label = format!(" {}: {}{} ", idx + 1, title, indicator);
            let style = if is_active {
                Style::default()
                    .fg(theme::p().accent)
                    .add_modifier(Modifier::BOLD)
            } else if !tab.pending_approvals.is_empty() {
                Style::default()
                    .fg(theme::p().error)
                    .add_modifier(Modifier::BOLD)
            } else if tab.is_running {
                Style::default().fg(theme::p().warning)
            } else if tab.has_unread {
                Style::default().fg(theme::p().text)
            } else {
                Style::default().fg(theme::p().muted)
            };
            (label, style)
        })
        .collect();

    let new_btn = " + ";
    let mut start = 0;
    let mut end = labels.len();
    let total_tab_width = |s: usize, e: usize| -> usize {
        labels[s..e]
            .iter()
            .map(|(l, _)| l.chars().count() + 1)
            .sum::<usize>()
    };
    let budget = width.saturating_sub(new_btn.len() + 4);
    while total_tab_width(start, end) > budget && end - start > 1 {
        if app.active_tab > start && (end - app.active_tab) <= (app.active_tab - start) {
            start += 1;
        } else {
            end -= 1;
        }
    }

    let mut spans = Vec::new();
    let mut col = area.x;
    if start > 0 {
        let marker = "« ";
        spans.push(Span::styled(marker, Style::default().fg(theme::p().faint)));
        col = col.saturating_add(marker.chars().count() as u16);
    }

    for (idx, (label, style)) in labels.iter().enumerate().take(end).skip(start) {
        let label_len = label.chars().count() as u16;
        let zone_w = label_len.min(area.x + area.width - col);
        if zone_w > 0 {
            app.tab_hit_zones.push(TabHitZone {
                rect: Rect::new(col, area.y, zone_w, 1),
                kind: TabHitKind::Tab(idx),
            });
        }
        spans.push(Span::styled(label.clone(), *style));
        col = col.saturating_add(label_len);
        spans.push(Span::raw(" "));
        col = col.saturating_add(1);
    }

    if end < labels.len() {
        let marker = "» ";
        spans.push(Span::styled(marker, Style::default().fg(theme::p().faint)));
        col = col.saturating_add(marker.chars().count() as u16);
    }

    let new_w = (new_btn.len() as u16).min((area.x + area.width).saturating_sub(col));
    if new_w > 0 {
        app.tab_hit_zones.push(TabHitZone {
            rect: Rect::new(col, area.y, new_w, 1),
            kind: TabHitKind::NewTab,
        });
        spans.push(Span::styled(
            new_btn,
            Style::default()
                .fg(theme::p().muted)
                .add_modifier(Modifier::BOLD),
        ));
        col = col.saturating_add(new_w);
    }

    let used = (col.saturating_sub(area.x)) as usize;
    let hint = "/sessions · /new · Ctrl-w close";
    if width.saturating_sub(used) >= hint.len() + 2 {
        let pad = width - used - hint.len() - 1;
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(hint, Style::default().fg(theme::p().faint)));
    }

    let bar = Paragraph::new(Line::from(spans));
    frame.render_widget(bar, area);
}

fn render_transcript(frame: &mut Frame<'_>, app: &mut AppState, area: Rect) {
    let inner = Rect {
        x: area.x.saturating_add(1),
        y: area.y,
        width: area.width.saturating_sub(2),
        height: area.height,
    };

    let raw_lines = build_transcript_lines(app);
    let wrapped = if inner.width > 0 {
        wrap_tagged_lines(raw_lines, inner.width as usize)
    } else {
        WrappedTranscript {
            lines: Vec::new(),
            hit_zones: Vec::new(),
            doc_lines: Vec::new(),
            bands: Vec::new(),
        }
    };
    let wrapped_lines = wrapped.lines;
    let bands = wrapped.bands;

    app.transcript_inner_area = inner;
    app.thinking_hit_zones = wrapped.hit_zones;
    app.transcript_wrapped_lines = wrapped.doc_lines;

    let scroll_y = {
        let tab = app.tab_mut();
        tab.viewport_height = inner.height as usize;
        tab.total_lines = wrapped_lines.len();
        let max_scroll = tab.max_scroll();
        if tab.auto_follow {
            tab.scroll_offset = max_scroll;
        } else {
            tab.scroll_offset = tab.scroll_offset.min(max_scroll);
        }
        tab.scroll_offset.min(u16::MAX as usize) as u16
    };

    let paragraph = Paragraph::new(wrapped_lines).scroll((scroll_y, 0));
    frame.render_widget(paragraph, inner);

    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let first_visible = scroll_y as usize;
    let last_visible = first_visible + inner.height as usize;
    let band_bg = theme::p().surface;
    for (start, end) in bands {
        if end <= first_visible || start >= last_visible {
            continue;
        }
        let from = start.max(first_visible);
        let to = end.min(last_visible);
        let buf = frame.buffer_mut();
        for row in from..to {
            let y = inner.y + (row - first_visible) as u16;
            for col in inner.x..inner.x + inner.width {
                if let Some(cell) = buf.cell_mut((col, y)) {
                    cell.set_bg(band_bg);
                }
            }
        }
    }
}

fn render_sidebar_overview(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().surface)),
        area,
    );
    let area = Rect {
        x: area.x.saturating_add(1),
        width: area.width.saturating_sub(1),
        ..area
    };
    let width = area.width as usize;
    let tab = app.tab();

    let key_status = app.active_key_status();
    let (key_badge, key_color) = key_status_style(&key_status);
    let state_span = if tab.is_running {
        let ch = SPINNER_FRAMES[app.spinner_tick % SPINNER_FRAMES.len()];
        let label = match tab.metrics.elapsed_ms() {
            Some(ms) => format!("{ch} running ({})", format_compact_ms(ms)),
            None => format!("{ch} running"),
        };
        Span::styled(label, Style::default().fg(theme::p().warning))
    } else {
        let label = match tab.metrics.last_turn_ms {
            Some(ms) => format!("idle ({})", format_compact_ms(ms)),
            None => "idle".to_string(),
        };
        Span::styled(label, Style::default().fg(theme::p().success))
    };

    let mut token_summary = format_usage_compact(app);
    if tab.metrics.turn_count > 1 && tab.metrics.session_total_tokens > 0 {
        token_summary.push_str(&format!(
            " · s:Σ{}",
            format_compact_tokens(tab.metrics.session_total_tokens)
        ));
    }
    let speed_summary = match (tab.metrics.last_tokens_per_sec, tab.metrics.last_ttft_ms) {
        (Some(tps), Some(ttft)) => format!("{tps:.1} tok/s · {}", format_compact_ms(ttft)),
        (Some(tps), None) => format!("{tps:.1} tok/s"),
        (None, Some(ttft)) => format!("ttft {}", format_compact_ms(ttft)),
        (None, None) => "-".to_string(),
    };

    let field =
        |name: &str| Span::styled(format!("{name:<9}"), Style::default().fg(theme::p().muted));

    let mut lines = vec![
        Line::from(Span::styled(" SESSION", theme::label())),
        Line::from(vec![
            field("session"),
            Span::styled(
                truncate_single_line(tab.session_id.as_str(), width.saturating_sub(10)),
                theme::text(),
            ),
        ]),
        Line::from(vec![
            field("provider"),
            Span::styled(
                truncate_single_line(&tab.active_provider, width.saturating_sub(10)),
                theme::text(),
            ),
        ]),
        Line::from(vec![
            field("model"),
            Span::styled(
                truncate_single_line(display_model(app), width.saturating_sub(10)),
                theme::text(),
            ),
        ]),
        Line::from(vec![
            field("auth"),
            Span::styled(format!("[{key_badge}]"), Style::default().fg(key_color)),
        ]),
        Line::from(vec![field("state"), state_span]),
        Line::from(vec![
            field("tokens"),
            Span::styled(
                truncate_single_line(&token_summary, width.saturating_sub(10)),
                theme::text(),
            ),
        ]),
        Line::from(vec![
            field("speed"),
            Span::styled(
                truncate_single_line(&speed_summary, width.saturating_sub(10)),
                Style::default().fg(theme::p().muted),
            ),
        ]),
        Line::default(),
    ];

    if app.mcp_servers.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(" MCP ", theme::label()),
            Span::styled("none", Style::default().fg(theme::p().faint)),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled(" MCP ", theme::label()),
            Span::styled(
                format!("({})", app.mcp_servers.len()),
                Style::default().fg(theme::p().faint),
            ),
        ]));
        for srv in &app.mcp_servers {
            lines.push(Line::from(Span::styled(
                format!("  • {}", truncate_single_line(srv, width.saturating_sub(4))),
                theme::muted(),
            )));
        }
    }

    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::styled(" AGENTS ", theme::label()),
        Span::raw("main "),
        if app.tab().is_running {
            Span::styled("[active]", Style::default().fg(theme::p().warning))
        } else {
            Span::styled("[ready]", Style::default().fg(theme::p().faint))
        },
    ]));

    let recent_tools: Vec<_> = app
        .tab()
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
            Span::styled(" TOOLS ", theme::label()),
            Span::styled("(0 calls) idle", Style::default().fg(theme::p().faint)),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled(" TOOLS ", theme::label()),
            Span::styled(
                if recent_tools.len() == 1 {
                    "(1 call)".to_string()
                } else {
                    format!("({} calls)", recent_tools.len())
                },
                Style::default().fg(theme::p().faint),
            ),
        ]));
        for tool_name in app.registered_tools.iter().take(4) {
            lines.push(Line::from(Span::styled(
                format!(
                    "  • {}",
                    truncate_single_line(tool_name, width.saturating_sub(4))
                ),
                theme::muted(),
            )));
        }
        for tc in recent_tools.into_iter().rev() {
            let (badge, color) = match tc.status {
                ToolCallStatus::Proposed | ToolCallStatus::Running => ("run", theme::p().warning),
                ToolCallStatus::Succeeded => ("ok", theme::p().success),
                ToolCallStatus::Failed => ("fail", theme::p().error),
                ToolCallStatus::Stopped => ("stop", theme::p().muted),
                ToolCallStatus::Cancelled => ("cancel", theme::p().muted),
            };
            let max_name = width.saturating_sub(10);
            lines.push(Line::from(vec![
                Span::raw(format!("  {} ", truncate_single_line(&tc.name, max_name))),
                Span::styled(format!("[{badge}]"), Style::default().fg(color)),
            ]));
        }
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn render_sidebar_changes(frame: &mut Frame<'_>, app: &mut AppState, area: Rect) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().surface)),
        area,
    );
    let area = Rect {
        x: area.x.saturating_add(1),
        width: area.width.saturating_sub(1),
        ..area
    };
    let title = format!(" File Changes ({})", app.file_changes.len());
    let width = area.width as usize;

    app.sidebar_changes_inner_area = area;

    let mut lines = vec![Line::from(Span::styled(title, theme::label()))];
    let mut visible_rows = 0usize;
    if app.file_changes.is_empty() {
        lines.push(Line::from(Span::styled(
            " Clean working tree",
            Style::default().fg(theme::p().faint),
        )));
    } else {
        let max_rows = area.height.max(1) as usize;
        for entry in app.file_changes.iter().take(max_rows) {
            let status_color = match entry.status.as_str() {
                "??" | "A" => theme::p().success,
                "D" => theme::p().error,
                _ => theme::p().warning,
            };
            let badge = format!("{:>2} ", entry.status);
            let max_path = width.saturating_sub(badge.len() + 1);
            lines.push(Line::from(vec![
                Span::styled(badge, Style::default().fg(status_color)),
                Span::styled(
                    truncate_single_line(&entry.path, max_path),
                    Style::default().fg(theme::p().muted),
                ),
            ]));
            visible_rows += 1;
        }
        if app.file_changes.len() > max_rows && visible_rows > 1 {
            let remaining = app.file_changes.len() - max_rows + 1;
            lines.pop();
            visible_rows = visible_rows.saturating_sub(1);
            lines.push(Line::from(Span::styled(
                format!("... and {remaining} more"),
                Style::default().fg(theme::p().faint),
            )));
        }
    }
    app.visible_file_change_rows = visible_rows;

    frame.render_widget(Paragraph::new(lines), area);
}

struct TaggedTranscriptLine {
    line: Line<'static>,
    entry_tag: Option<usize>,
    gutter_len: usize,
    band: bool,
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
    }
}

fn build_header_lines(app: &AppState) -> Vec<TaggedTranscriptLine> {
    let key_status = app.active_key_status();
    let (badge, badge_color) = key_status_style(&key_status);

    vec![
        tagged_line(
            Line::from(vec![
                Span::styled(
                    "meerai",
                    Style::default()
                        .fg(theme::p().accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("{} / {}", app.tab().active_provider, display_model(app)),
                    Style::default().fg(theme::p().muted),
                ),
                Span::raw(" "),
                Span::styled(format!("[{badge}]"), Style::default().fg(badge_color)),
            ]),
            None,
            0,
        ),
        tagged_line(
            Line::from(Span::styled(
                format!("{}", app.workspace_root.display()),
                Style::default().fg(theme::p().faint),
            )),
            None,
            0,
        ),
    ]
}

fn tool_status_badge(status: &ToolCallStatus) -> (&'static str, Color) {
    match status {
        ToolCallStatus::Proposed => ("proposed", theme::p().warning),
        ToolCallStatus::Running => ("running", theme::p().warning),
        ToolCallStatus::Succeeded => ("ok", theme::p().success),
        ToolCallStatus::Failed => ("fail", theme::p().error),
        ToolCallStatus::Stopped => ("stop", theme::p().muted),
        ToolCallStatus::Cancelled => ("cancelled", theme::p().muted),
    }
}

fn assistant_line_style(line: &str, in_code_block: &mut bool) -> Style {
    let trimmed = line.trim_start();
    if trimmed.starts_with("```") {
        *in_code_block = !*in_code_block;
        return Style::default().fg(theme::p().faint).bg(theme::p().surface);
    }
    if *in_code_block {
        return Style::default().fg(theme::p().text).bg(theme::p().surface);
    }
    if trimmed.starts_with("# ")
        || trimmed.starts_with("## ")
        || trimmed.starts_with("### ")
        || trimmed.starts_with("#### ")
    {
        return Style::default()
            .fg(theme::p().text)
            .add_modifier(Modifier::BOLD);
    }
    if trimmed.starts_with("> ") {
        return Style::default()
            .fg(theme::p().muted)
            .add_modifier(Modifier::ITALIC);
    }
    Style::default().fg(theme::p().text)
}

fn build_transcript_lines(app: &AppState) -> Vec<TaggedTranscriptLine> {
    let mut lines = build_header_lines(app);
    let transcript = &app.tab().transcript;
    if transcript.is_empty() {
        return lines;
    }

    lines.push(tagged_line(Line::default(), None, 0));
    for (idx, entry) in transcript.iter().enumerate() {
        if idx > 0 {
            lines.push(tagged_line(Line::default(), None, 0));
        }
        match entry {
            TranscriptEntry::User { text } => {
                lines.push(banded_line(
                    Line::from(Span::styled(
                        "▌ you",
                        Style::default()
                            .fg(theme::p().accent)
                            .add_modifier(Modifier::BOLD),
                    )),
                    None,
                    2,
                ));
                for line in text.split('\n') {
                    lines.push(banded_line(
                        Line::from(vec![
                            Span::styled("▌ ", Style::default().fg(theme::p().accent)),
                            Span::styled(line.to_string(), Style::default().fg(theme::p().text)),
                        ]),
                        None,
                        2,
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
                        lines.push(tagged_line(
                            Line::from(Span::styled(
                                format!(
                                    "  ▾ Thinking ({line_count} lines)  [click or Ctrl-t to hide]"
                                ),
                                Style::default()
                                    .fg(theme::p().muted)
                                    .add_modifier(Modifier::ITALIC),
                            )),
                            Some(idx),
                            2,
                        ));
                        for line in reasoning.split('\n') {
                            lines.push(tagged_line(
                                Line::from(vec![
                                    Span::styled("  │ ", Style::default().fg(theme::p().faint)),
                                    Span::styled(
                                        line.to_string(),
                                        Style::default()
                                            .fg(theme::p().muted)
                                            .add_modifier(Modifier::ITALIC),
                                    ),
                                ]),
                                Some(idx),
                                4,
                            ));
                        }
                    } else {
                        lines.push(tagged_line(
                            Line::from(Span::styled(
                                format!("  ▸ Thinking ({line_count} lines)  [click or Ctrl-t]"),
                                Style::default()
                                    .fg(theme::p().muted)
                                    .add_modifier(Modifier::ITALIC),
                            )),
                            Some(idx),
                            2,
                        ));
                    }
                }
                if !text.is_empty() || *streaming {
                    let mut header_spans = vec![Span::styled(
                        "  ● Response",
                        Style::default()
                            .fg(theme::p().muted)
                            .add_modifier(Modifier::BOLD),
                    )];
                    if *streaming {
                        let ch = SPINNER_FRAMES[app.spinner_tick % SPINNER_FRAMES.len()];
                        header_spans.push(Span::styled(
                            format!(" {ch}"),
                            Style::default().fg(theme::p().muted),
                        ));
                    }
                    lines.push(tagged_line(Line::from(header_spans), None, 2));
                }
                if !text.is_empty() {
                    let mut in_code_block = false;
                    for line in text.split('\n') {
                        let line_style = assistant_line_style(line, &mut in_code_block);
                        lines.push(tagged_line(
                            Line::from(vec![
                                Span::styled("  ", Style::default()),
                                Span::styled(line.to_string(), line_style),
                            ]),
                            None,
                            2,
                        ));
                    }
                }
            }
            TranscriptEntry::ToolCall(tc) => {
                let (badge, color) = tool_status_badge(&tc.status);
                let args_preview = truncate_single_line(&tc.args, 60);
                let lead = if tc.status == ToolCallStatus::Running {
                    let ch = SPINNER_FRAMES[app.spinner_tick % SPINNER_FRAMES.len()];
                    format!("{ch} [tool] ")
                } else {
                    "▸ [tool] ".to_string()
                };
                lines.push(banded_line(
                    Line::from(vec![
                        Span::styled(lead, Style::default().fg(theme::p().interactive)),
                        Span::styled(
                            tc.name.clone(),
                            Style::default()
                                .fg(theme::p().text)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(" ", Style::default()),
                        Span::styled(format!("[{badge}]"), Style::default().fg(color)),
                        if args_preview.is_empty() {
                            Span::raw("")
                        } else {
                            Span::styled(
                                format!(" {args_preview}"),
                                Style::default().fg(theme::p().muted),
                            )
                        },
                    ]),
                    None,
                    2,
                ));
                if let Some(output) = &tc.output {
                    let preview = truncate_single_line(output, 120);
                    if !preview.is_empty() {
                        lines.push(tagged_line(
                            Line::from(vec![
                                Span::styled("  ↳ ", Style::default().fg(theme::p().faint)),
                                Span::styled(preview, Style::default().fg(theme::p().muted)),
                            ]),
                            None,
                            4,
                        ));
                    }
                }
            }
            TranscriptEntry::Subagent(sub) => {
                let (badge, color) = tool_status_badge(&sub.status);
                lines.push(tagged_line(
                    Line::from(vec![
                        Span::styled("▸ [subagent] ", Style::default().fg(theme::p().accent)),
                        Span::styled(
                            sub.agent.clone(),
                            Style::default()
                                .fg(theme::p().text)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(" ", Style::default()),
                        Span::styled(format!("[{badge}]"), Style::default().fg(color)),
                    ]),
                    None,
                    2,
                ));
                if let Some(report) = &sub.report {
                    let preview = truncate_single_line(report, 120);
                    if !preview.is_empty() {
                        lines.push(tagged_line(
                            Line::from(vec![
                                Span::styled("  ↳ ", Style::default().fg(theme::p().faint)),
                                Span::styled(preview, Style::default().fg(theme::p().muted)),
                            ]),
                            None,
                            4,
                        ));
                    }
                }
            }
            TranscriptEntry::System { text } => {
                for line in text.split('\n') {
                    lines.push(tagged_line(
                        Line::from(Span::styled(
                            format!("[system] {line}"),
                            Style::default().fg(theme::p().muted),
                        )),
                        None,
                        0,
                    ));
                }
            }
            TranscriptEntry::Error { text, expanded } => {
                if *expanded {
                    let line_count = text.lines().count().max(1);
                    lines.push(tagged_line(
                        Line::from(Span::styled(
                            format!("  ▾ [error] ({line_count} lines)  [click to hide]"),
                            Style::default()
                                .fg(theme::p().error)
                                .add_modifier(Modifier::BOLD),
                        )),
                        Some(idx),
                        2,
                    ));
                    for line in text.split('\n') {
                        lines.push(tagged_line(
                            Line::from(vec![
                                Span::styled("  │ ", Style::default().fg(theme::p().faint)),
                                Span::styled(
                                    line.to_string(),
                                    Style::default().fg(theme::p().error),
                                ),
                            ]),
                            Some(idx),
                            4,
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
                    lines.push(tagged_line(
                        Line::from(vec![
                            Span::styled(
                                format!("  ▸ [error] {summary}"),
                                Style::default().fg(theme::p().error),
                            ),
                            Span::styled(
                                "  [click to expand]",
                                Style::default()
                                    .fg(theme::p().faint)
                                    .add_modifier(Modifier::ITALIC),
                            ),
                        ]),
                        Some(idx),
                        2,
                    ));
                }
            }
        }
    }

    lines
}

fn render_composer(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().surface)),
        area,
    );
    let (mode_label, mode_color) = match app.mode {
        InputMode::Insert => ("insert", theme::p().accent),
        InputMode::Normal => ("normal", theme::p().info),
    };
    let queued = app.tab().queued.len();
    let mode_hint = if queued == 0 {
        mode_label.to_string()
    } else {
        format!("{mode_label} · {queued} queued · Ctrl-g send now")
    };

    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    let rule_style = Style::default().fg(theme::p().border);
    let hint_width = mode_hint.chars().count() + 2;
    let rule_width = (area.width as usize).saturating_sub(hint_width);
    let rule = Line::from(vec![
        Span::styled("─".repeat(rule_width), rule_style),
        Span::styled(format!(" {mode_hint} "), Style::default().fg(mode_color)),
    ]);
    frame.render_widget(Paragraph::new(rule), rows[0]);

    let body = rows[1];
    let inner = Rect {
        x: body.x.saturating_add(2),
        y: body.y,
        width: body.width.saturating_sub(2),
        height: body.height,
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "❯",
            Style::default().fg(theme::p().interactive),
        ))),
        Rect::new(body.x, body.y, 2, 1),
    );

    let tab = app.tab();
    let (cursor_row, cursor_col) = cursor_row_col(&tab.input, tab.cursor);
    let visible_rows = inner.height.max(1) as usize;
    let scroll_y = cursor_row.saturating_sub(visible_rows.saturating_sub(1));

    let mut cursor_display_col = cursor_col;
    let content = if tab.input.is_empty() {
        Paragraph::new(Line::from(Span::styled(
            "Ask meerai anything...",
            Style::default().fg(theme::p().faint),
        )))
    } else {
        let lines: Vec<Line<'_>> = tab
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

    frame.render_widget(content, inner);

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
        MentionKind::Agent => theme::p().accent,
        MentionKind::Skill => theme::p().interactive,
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
                    CompletionItemKind::Agent => theme::p().accent,
                    CompletionItemKind::Skill => theme::p().interactive,
                    CompletionItemKind::Plain | CompletionItemKind::File => theme::p().muted,
                };
                spans.push(Span::styled(
                    format!("|{badge}"),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ));
            }
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                item.description.clone(),
                Style::default().fg(theme::p().faint),
            ));
            ListItem::new(Line::from(spans))
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(app.completion.selected()));

    let list = List::new(list_items)
        .block(theme::dialog(&title))
        .highlight_style(
            Style::default()
                .bg(theme::p().surface_high)
                .fg(theme::p().text)
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
                .border_style(Style::default().fg(theme::p().border))
                .style(Style::default().bg(theme::p().surface))
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
                Span::styled("Filter: ", Style::default().fg(theme::p().warning)),
                Span::raw(if filter.is_empty() {
                    "(type to filter)"
                } else {
                    filter.as_str()
                }),
            ]);
            frame.render_widget(Paragraph::new(filter_line), rows[0]);

            let providers = app.filtered_providers(filter);
            let active_provider = &app.tab().active_provider;
            let list_items: Vec<ListItem<'_>> = providers
                .iter()
                .map(|(p, status)| {
                    let (badge, color) = key_status_style(status);
                    let active_mark = if p.id == *active_provider { "*" } else { " " };
                    ListItem::new(Line::from(vec![
                        Span::raw(format!("{active_mark} ")),
                        Span::styled(
                            p.display_name.clone(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(" ({}) ", p.id),
                            Style::default().fg(theme::p().faint),
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
                    .bg(theme::p().surface_high)
                    .fg(theme::p().text)
                    .add_modifier(Modifier::BOLD),
            );
            frame.render_stateful_widget(list, rows[1], &mut state);

            let footer = Paragraph::new(Line::from(Span::styled(
                "Enter:configure  Up/Down:move  Type:filter  Esc:close",
                Style::default().fg(theme::p().faint),
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
                .border_style(Style::default().fg(theme::p().border))
                .style(Style::default().bg(theme::p().surface))
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
                    .fg(theme::p().success)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::p().faint)
            };
            let url_label_style = if *editing_base_url {
                Style::default()
                    .fg(theme::p().success)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::p().faint)
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
                    Style::default().fg(theme::p().error),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    "Saved in ~/.local/share/meerai/auth.json (0600).",
                    Style::default().fg(theme::p().faint),
                )));
            }
            lines.push(Line::from(Span::styled(
                "Enter:save  Tab:switch field  Ctrl-d:delete key  Esc:back",
                Style::default().fg(theme::p().faint),
            )));

            frame.render_widget(Paragraph::new(lines), inner);
        }
        ActiveModal::ModelInput {
            input,
            selected_recent,
            error,
        } => {
            let active_provider = &app.tab().active_provider;
            let active_model = &app.tab().model;
            let recent = app.recent_models_for_active_provider();
            let height = if recent.is_empty() {
                9
            } else {
                (8 + recent.len().min(8)) as u16
            };
            let popup = centered_rect_fixed(area, 72, height);
            let title = format!(" Set Model [{active_provider}] (/model) ");
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme::p().border))
                .style(Style::default().bg(theme::p().surface))
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
                .current_or_default_model(active_provider)
                .map(|m| format!("(Enter keeps '{m}', or type model name)"))
                .unwrap_or_else(|| "(type model name, e.g. gpt-4o)".to_string());
            let input_display = if input.is_empty() {
                Span::styled(placeholder, Style::default().fg(theme::p().faint))
            } else {
                Span::raw(input.as_str())
            };
            let input_lines = vec![
                Line::from(vec![
                    Span::styled(
                        "Model: ",
                        Style::default()
                            .fg(theme::p().success)
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
                        Style::default().fg(theme::p().faint),
                    ))),
                    rows[1],
                );
            } else {
                let list_items: Vec<ListItem<'_>> = recent
                    .iter()
                    .map(|m| {
                        let active_mark = if m == active_model { "*" } else { " " };
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
                        .bg(theme::p().surface_high)
                        .fg(theme::p().text)
                        .add_modifier(Modifier::BOLD),
                );
                frame.render_stateful_widget(list, rows[1], &mut state);
            }

            let status_line = if let Some(err) = error {
                Line::from(Span::styled(
                    format!("Error: {err}"),
                    Style::default().fg(theme::p().error),
                ))
            } else {
                Line::from(Span::styled(
                    "Any model name is accepted; validation runs on request.",
                    Style::default().fg(theme::p().faint),
                ))
            };
            let footer_lines = vec![
                status_line,
                Line::from(Span::styled(
                    "Enter:apply  Up/Down:recent  Ctrl-u:clear  Esc:close",
                    Style::default().fg(theme::p().faint),
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
                .border_style(Style::default().fg(theme::p().border))
                .style(Style::default().bg(theme::p().surface))
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
                .border_style(Style::default().fg(theme::p().border))
                .style(Style::default().bg(theme::p().surface))
                .title(format!(" Original (a/{}) ", diff.path));
            let right_block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme::p().border))
                .style(Style::default().bg(theme::p().surface))
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
                    Style::default().fg(theme::p().faint),
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
                .border_style(Style::default().fg(theme::p().border))
                .style(Style::default().bg(theme::p().surface))
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
                SearchMode::Files => theme::p().muted,
                SearchMode::Content => theme::p().muted,
            };
            let placeholder = match mode {
                SearchMode::Files => "(type to fuzzy-match file paths)",
                SearchMode::Content => "(type query to search content with ripgrep)",
            };
            let query_block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme::p().border))
                .style(Style::default().bg(theme::p().surface))
                .title(" Query ");
            let query_inner = query_block.inner(layout[0]);
            let input_box = Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("[{}] ", mode.label()),
                    Style::default().fg(mode_color).add_modifier(Modifier::BOLD),
                ),
                if query.is_empty() {
                    Span::styled(placeholder, Style::default().fg(theme::p().faint))
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
                .border_style(Style::default().fg(theme::p().border))
                .style(Style::default().bg(theme::p().surface))
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
                    .bg(theme::p().surface_high)
                    .fg(theme::p().text)
                    .add_modifier(Modifier::BOLD),
            );
            frame.render_stateful_widget(list, panes[0], &mut list_state);

            let preview_title = preview
                .as_ref()
                .map(|p| format!(" Preview: {} ", p.title))
                .unwrap_or_else(|| " Preview ".to_string());
            let preview_block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme::p().border))
                .style(Style::default().bg(theme::p().surface))
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
                                            .fg(theme::p().warning)
                                            .bg(theme::p().surface_high)
                                            .add_modifier(Modifier::BOLD),
                                    ),
                                    Span::styled(
                                        body,
                                        Style::default()
                                            .fg(theme::p().text)
                                            .bg(theme::p().surface_high)
                                            .add_modifier(Modifier::BOLD),
                                    ),
                                ])
                                .style(Style::default().bg(theme::p().surface_high))
                            } else {
                                Line::from(vec![
                                    Span::styled(gutter, Style::default().fg(theme::p().faint)),
                                    Span::raw(body),
                                ])
                            }
                        })
                        .collect()
                }
                None => vec![Line::from(Span::styled(
                    "No matching results.",
                    Style::default().fg(theme::p().faint),
                ))],
            };
            frame.render_widget(Paragraph::new(preview_lines).block(preview_block), panes[1]);

            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "Tab:Files/Content  Up/Down:select  PgUp/PgDn:scroll preview  Enter:insert @file  Esc:close",
                    Style::default().fg(theme::p().faint),
                ))),
                layout[2],
            );
        }
        ActiveModal::CommandPalette { filter, selected } => {
            let popup = centered_rect(area, 64, 62, 54, 10);
            let block = theme::dialog(" Commands (Ctrl-p) ");
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
                Span::styled("> ", Style::default().fg(theme::p().interactive)),
                if filter.is_empty() {
                    Span::styled(
                        "type to filter commands",
                        Style::default().fg(theme::p().faint),
                    )
                } else {
                    Span::styled(filter.clone(), Style::default().fg(theme::p().text))
                },
            ]);
            frame.render_widget(Paragraph::new(filter_line), rows[0]);

            let commands = crate::app::filtered_commands(filter);
            let width = rows[1].width as usize;
            let list_items: Vec<ListItem<'_>> = commands
                .iter()
                .map(|command| {
                    let label_len = command.label.chars().count();
                    let hint_len = command.hint.chars().count();
                    let gap = width.saturating_sub(label_len + hint_len + 1);
                    ListItem::new(Line::from(vec![
                        Span::styled(command.label, Style::default().fg(theme::p().text)),
                        Span::raw(" ".repeat(gap)),
                        Span::styled(command.hint, Style::default().fg(theme::p().muted)),
                    ]))
                })
                .collect();

            let mut state = ListState::default();
            if !commands.is_empty() {
                state.select(Some((*selected).min(commands.len() - 1)));
            }
            let list = List::new(list_items).highlight_style(
                Style::default()
                    .bg(theme::p().surface_high)
                    .fg(theme::p().text)
                    .add_modifier(Modifier::BOLD),
            );
            frame.render_stateful_widget(list, rows[1], &mut state);

            let footer = if commands.is_empty() {
                "No matching commands · Esc close"
            } else {
                "Enter run · Up/Down move · type filter · Esc close"
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    footer,
                    Style::default().fg(theme::p().faint),
                ))),
                rows[2],
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
                .border_style(Style::default().fg(theme::p().error))
                .style(Style::default().bg(theme::p().surface))
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
                        .fg(theme::p().text)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    format!("kind: {kind}{effect}"),
                    Style::default().fg(theme::p().faint),
                )),
            ];
            frame.render_widget(Paragraph::new(header), rows[0]);

            let detail = if request.preview.detail.is_empty() {
                vec![Line::from(Span::styled(
                    "(no details)",
                    Style::default().fg(theme::p().faint),
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
                        Span::styled("Reason: ", Style::default().fg(theme::p().warning)),
                        Span::raw(reason.as_str()),
                        Span::styled("█", Style::default().fg(theme::p().warning)),
                    ]),
                    Line::from(Span::styled(
                        "Enter:send decline reason  Esc:back",
                        Style::default().fg(theme::p().faint),
                    )),
                ]
            } else {
                vec![
                    Line::from(Span::styled(
                        "y accept · t accept-for-turn · a accept-for-session · n decline · r decline-with-reason · esc cancel",
                        Style::default().fg(theme::p().faint),
                    )),
                    Line::default(),
                ]
            };
            frame.render_widget(Paragraph::new(footer), rows[2]);
        }
        ActiveModal::SessionSelect {
            filter,
            selected,
            sessions,
            loading,
            error,
        } => {
            let popup = centered_rect(area, 78, 72, 48, 12);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme::p().border))
                .style(Style::default().bg(theme::p().surface))
                .title(" Switch Session (/sessions) ");
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
                Span::styled("Filter: ", Style::default().fg(theme::p().warning)),
                Span::raw(if filter.is_empty() {
                    "(type to filter by id, title, or model)"
                } else {
                    filter.as_str()
                }),
            ]);
            frame.render_widget(Paragraph::new(filter_line), rows[0]);

            let filtered = app.filtered_sessions(sessions, filter);
            if *loading && filtered.is_empty() {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "Loading sessions...",
                        Style::default().fg(theme::p().faint),
                    ))),
                    rows[1],
                );
            } else if let Some(err) = error {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        format!("Failed to list sessions: {err}"),
                        Style::default().fg(theme::p().error),
                    ))),
                    rows[1],
                );
            } else if filtered.is_empty() {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "No matching sessions.",
                        Style::default().fg(theme::p().faint),
                    ))),
                    rows[1],
                );
            } else {
                let now = now_millis();
                let max_title = (inner.width as usize).saturating_sub(36).max(16);
                let list_items: Vec<ListItem<'_>> = filtered
                    .iter()
                    .map(|summary| {
                        let open_idx = app.tab_index_for_session(&summary.id);
                        let (tab_badge, badge_style) = match open_idx {
                            Some(idx) if idx == app.active_tab => (
                                format!("[tab {}*] ", idx + 1),
                                Style::default()
                                    .fg(theme::p().success)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Some(idx) => (
                                format!("[tab {}] ", idx + 1),
                                Style::default().fg(theme::p().accent),
                            ),
                            None => ("       ".to_string(), Style::default().fg(theme::p().faint)),
                        };
                        let raw_title = summary
                            .title
                            .as_deref()
                            .filter(|t| !t.trim().is_empty())
                            .unwrap_or("(untitled)");
                        let title_text = truncate_single_line(raw_title, max_title);
                        let title_style = if summary
                            .title
                            .as_deref()
                            .is_some_and(|t| !t.trim().is_empty())
                        {
                            Style::default().add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(theme::p().faint)
                        };
                        let mut item_spans = vec![
                            Span::styled(tab_badge, badge_style),
                            Span::styled(title_text, title_style),
                            Span::styled(
                                format!("  {}", summary.id),
                                Style::default().fg(theme::p().faint),
                            ),
                        ];
                        if let Some(model) =
                            summary.model.as_deref().filter(|m| !m.trim().is_empty())
                        {
                            item_spans.push(Span::styled(
                                format!(" · {}", truncate_single_line(model, 22)),
                                Style::default().fg(theme::p().muted),
                            ));
                        }
                        if summary.updated_at_ms > 0 {
                            item_spans.push(Span::styled(
                                format!(" · {}", format_relative_age(now, summary.updated_at_ms)),
                                Style::default().fg(theme::p().faint),
                            ));
                        }
                        ListItem::new(Line::from(item_spans))
                    })
                    .collect();

                let mut state = ListState::default();
                state.select(Some((*selected).min(filtered.len() - 1)));
                let list = List::new(list_items).highlight_style(
                    Style::default()
                        .bg(theme::p().surface_high)
                        .fg(theme::p().text)
                        .add_modifier(Modifier::BOLD),
                );
                frame.render_stateful_widget(list, rows[1], &mut state);
            }

            let footer_text = if *loading {
                "Refreshing...  Enter:switch  Up/Down:move  Esc:close"
            } else {
                "Enter:switch  Up/Down:move  Type:filter  Esc:close"
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    footer_text,
                    Style::default().fg(theme::p().faint),
                ))),
                rows[2],
            );
        }
    }
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
                .fg(theme::p().accent)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let Some(cell) = cell else {
        return Line::from(Span::styled("~", Style::default().fg(theme::p().faint)));
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
            Style::default().fg(theme::p().diff_removed)
        }
        (DiffRowKind::Added, false) | (DiffRowKind::Modified, false) => {
            Style::default().fg(theme::p().diff_added)
        }
        _ => Style::default().fg(theme::p().diff_context),
    };

    Line::from(vec![
        Span::styled(line_num, Style::default().fg(theme::p().faint)),
        Span::styled(body, style),
    ])
}

fn display_model(app: &AppState) -> &str {
    let trimmed = app.tab().model.trim();
    if trimmed.is_empty() {
        "(not set)"
    } else {
        trimmed
    }
}

fn render_statusline(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let tab = app.tab();
    let (badge_text, badge_color) = match app.mode {
        InputMode::Insert => (" INS ", theme::p().accent),
        InputMode::Normal => (" NOR ", theme::p().info),
    };

    let mut spans = vec![Span::styled(
        badge_text,
        Style::default()
            .fg(badge_color)
            .add_modifier(Modifier::BOLD),
    )];

    let mode_color = match tab.approval_mode {
        ApprovalMode::Plan => theme::p().accent,
        ApprovalMode::Ask => theme::p().warning,
        ApprovalMode::AutoEdit => theme::p().info,
        ApprovalMode::Yolo => theme::p().error,
    };
    spans.push(Span::styled(
        format!(" {} ", approval_mode_badge(tab.approval_mode)),
        Style::default().fg(mode_color).add_modifier(Modifier::BOLD),
    ));

    let key_status = app.active_key_status();
    let (key_badge, key_color) = key_status_style(&key_status);

    spans.push(Span::styled(
        format!(" {} / {} ", tab.active_provider, display_model(app)),
        Style::default().fg(theme::p().muted),
    ));
    spans.push(Span::styled(
        format!("[{key_badge}]"),
        Style::default().fg(key_color),
    ));

    let usage_str = format_usage_compact(app);
    if usage_str != "-" {
        spans.push(Span::styled(
            format!(" · {usage_str}"),
            Style::default().fg(theme::p().muted),
        ));
    }

    if let Some(session_usage) = format_session_usage(app) {
        spans.push(Span::styled(
            format!(" · {session_usage}"),
            Style::default().fg(theme::p().faint),
        ));
    }

    if let Some(speed) = tab.metrics.last_tokens_per_sec {
        spans.push(Span::styled(
            format!(" {speed:.1} tok/s"),
            Style::default().fg(theme::p().faint),
        ));
    }

    if tab.is_running {
        let frame_char = SPINNER_FRAMES[app.spinner_tick % SPINNER_FRAMES.len()];
        let elapsed = tab
            .metrics
            .elapsed_ms()
            .map(|ms| format!(" ({})", format_compact_ms(ms)))
            .unwrap_or_default();
        let activity = match &tab.active_tool {
            Some(tool) => format!(" · {frame_char} {tool}{elapsed}"),
            None => format!(" · {frame_char} running{elapsed}"),
        };
        spans.push(Span::styled(
            activity,
            Style::default().fg(theme::p().warning),
        ));
    } else if let Some(turn_ms) = tab.metrics.last_turn_ms {
        spans.push(Span::styled(
            format!(" {}", format_compact_ms(turn_ms)),
            Style::default().fg(theme::p().faint),
        ));
    }

    if let Some(notice) = &app.status_notice {
        spans.push(Span::styled(
            format!(" · {notice}"),
            Style::default().fg(theme::p().error),
        ));
    } else {
        let hint = if tab.is_running {
            if tab.queued.is_empty() {
                " · Ctrl-c cancel · Ctrl-g send now · Ctrl-p commands"
            } else {
                " · Ctrl-c cancel · Ctrl-g send queued · Ctrl-p commands"
            }
        } else {
            match app.mode {
                InputMode::Insert => {
                    if app.completion.is_active() {
                        " · Tab/Enter accept · Esc dismiss"
                    } else {
                        " · Ctrl-p commands · Ctrl-b side · Ctrl-t think · Esc normal"
                    }
                }
                InputMode::Normal => " · Ctrl-p commands · i insert · / search · q quit",
            }
        };
        spans.push(Span::styled(hint, Style::default().fg(theme::p().faint)));
    }

    let bar = Paragraph::new(Line::from(spans));
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
    let usage = &app.tab().usage;
    let p = usage.prompt_tokens.map(|v| v.max(0) as u64);
    let c = usage.completion_tokens.map(|v| v.max(0) as u64);
    let t = usage
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
    let metrics = &app.tab().metrics;
    let total = metrics.session_total_tokens;
    if total == 0 {
        return None;
    }
    Some(format!(
        "session ↑{} ↓{} Σ{}",
        format_compact_tokens(metrics.session_prompt_tokens),
        format_compact_tokens(metrics.session_completion_tokens),
        format_compact_tokens(total)
    ))
}

fn key_status_style(status: &KeyStatus) -> (String, Color) {
    let badge = status.badge();
    let color = match status {
        KeyStatus::Stored | KeyStatus::Config => theme::p().success,
        KeyStatus::Env(_) => theme::p().muted,
        KeyStatus::NotRequired => theme::p().muted,
        KeyStatus::Missing => theme::p().error,
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
    let highlight = Style::default().bg(theme::p().border).fg(theme::p().text);

    if let Some(sel) = app.doc_selection
        && sel.anchor != sel.head
        && app.modal.is_none()
    {
        let inner = app.transcript_inner_area;
        if inner.width > 0 && inner.height > 0 && !app.transcript_wrapped_lines.is_empty() {
            let max_line = app.transcript_wrapped_lines.len() - 1;
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
            let vis_start = app.tab().scroll_offset;
            let vis_end = vis_start + inner.height as usize;
            let draw_start = start.line.max(vis_start);
            let draw_end = end.line.min(vis_end.saturating_sub(1));
            if draw_start <= draw_end {
                let min_col = inner.x;
                let max_col = inner.x + inner.width - 1;
                let buf = frame.buffer_mut();
                for doc_line in draw_start..=draw_end {
                    let row = inner.y + (doc_line - vis_start) as u16;
                    let c_start = if doc_line == start.line {
                        inner.x.saturating_add(start.col as u16)
                    } else {
                        min_col
                    }
                    .clamp(min_col, max_col);
                    let c_end = if doc_line == end.line {
                        inner.x.saturating_add(end.col as u16)
                    } else {
                        max_col
                    }
                    .clamp(min_col, max_col);
                    for col in c_start..=c_end {
                        if let Some(cell) = buf.cell_mut((col, row)) {
                            cell.set_style(highlight);
                        }
                    }
                }
            }
            return;
        }
    }

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

struct WrappedTranscript {
    lines: Vec<Line<'static>>,
    hit_zones: Vec<ThinkingHitZone>,
    doc_lines: Vec<TranscriptWrappedLine>,
    bands: Vec<(usize, usize)>,
}

fn wrap_tagged_lines(lines: Vec<TaggedTranscriptLine>, width: usize) -> WrappedTranscript {
    if width == 0 {
        return WrappedTranscript {
            lines: Vec::new(),
            hit_zones: Vec::new(),
            doc_lines: Vec::new(),
            bands: Vec::new(),
        };
    }

    let mut out = Vec::with_capacity(lines.len());
    let mut doc_lines = Vec::with_capacity(lines.len());
    let mut hit_zones: Vec<ThinkingHitZone> = Vec::new();
    let mut bands: Vec<(usize, usize)> = Vec::new();
    let mut active_zone: Option<(usize, usize)> = None;

    for tagged in lines {
        let TaggedTranscriptLine {
            line,
            entry_tag,
            gutter_len,
            band,
        } = tagged;
        let line_style = line.style;
        let line_start = out.len();
        let total_chars: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
        if total_chars == 0 {
            out.push(Line::default().style(line_style));
            doc_lines.push(TranscriptWrappedLine {
                text: String::new(),
                gutter_len: 0,
                soft_wrapped_next: false,
            });
        } else {
            let mut current_spans: Vec<Span<'static>> = Vec::new();
            let mut current_text = String::new();
            let mut current_col = 0;
            let mut is_first_row = true;

            for span in line.spans {
                let style = span.style;
                let mut buf = String::new();
                for ch in span.content.chars() {
                    if current_col == width {
                        if !buf.is_empty() {
                            current_spans.push(Span::styled(std::mem::take(&mut buf), style));
                        }
                        out.push(Line::from(std::mem::take(&mut current_spans)).style(line_style));
                        doc_lines.push(TranscriptWrappedLine {
                            text: std::mem::take(&mut current_text),
                            gutter_len: if is_first_row { gutter_len } else { 0 },
                            soft_wrapped_next: true,
                        });
                        is_first_row = false;
                        current_col = 0;
                    }
                    buf.push(ch);
                    current_text.push(ch);
                    current_col += 1;
                }
                if !buf.is_empty() {
                    current_spans.push(Span::styled(buf, style));
                }
            }

            if !current_spans.is_empty() {
                out.push(Line::from(current_spans).style(line_style));
                doc_lines.push(TranscriptWrappedLine {
                    text: current_text,
                    gutter_len: if is_first_row { gutter_len } else { 0 },
                    soft_wrapped_next: false,
                });
            }
        }
        if band {
            match bands.last_mut() {
                Some((_, end)) if *end == line_start => *end = out.len(),
                _ => bands.push((line_start, out.len())),
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

    WrappedTranscript {
        lines: out,
        hit_zones,
        doc_lines,
        bands,
    }
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
        assert!(wide.contains("SESSION"));
        assert!(wide.contains("File Changes (1)"));
        assert!(app.visible_file_change_rows == 1);
        assert!(app.sidebar_changes_inner_area.width > 0);

        let narrow = render_to_string_with_size(&mut app, 90, 28);
        assert!(!narrow.contains("File Changes (1)"));
        assert_eq!(app.visible_file_change_rows, 0);
        assert_eq!(app.sidebar_changes_inner_area, Rect::default());
    }

    #[test]
    fn renders_command_palette_with_shortcuts() {
        let mut app = test_app();
        app.modal = Some(ActiveModal::CommandPalette {
            filter: String::new(),
            selected: 0,
        });
        let rendered = render_to_string(&mut app);
        assert!(rendered.contains("Commands (Ctrl-p)"));
        assert!(rendered.contains("New session tab"));
        assert!(rendered.contains("/new"));
        assert!(rendered.contains("type to filter commands"));
        assert!(rendered.contains("Enter run"));
    }

    #[test]
    fn paints_sidebar_panel_and_user_band_without_main_borders() {
        let mut app = test_app();
        app.tab_mut().transcript.push(TranscriptEntry::User {
            text: "highlight me".to_string(),
        });
        app.file_changes.push(crate::app::FileChangeEntry {
            status: "M".to_string(),
            path: "src/main.rs".to_string(),
        });

        let backend = TestBackend::new(120, 28);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer().clone();

        let surface = crate::theme::Palette::default().surface;
        assert_eq!(buffer[(110, 4)].bg, surface, "sidebar panel background");

        let band_row = (0..28)
            .find(|y| {
                (0..100).any(|x| {
                    let mut symbol = String::new();
                    symbol.push_str(buffer[(x, *y)].symbol());
                    symbol == "▌"
                })
            })
            .expect("user band row");
        assert_eq!(buffer[(80, band_row)].bg, surface, "user band background");

        let rendered: String = (0..28)
            .flat_map(|y| (0..120).map(move |x| (x, y)))
            .map(|(x, y)| buffer[(x, y)].symbol().to_string())
            .collect();
        assert!(!rendered.contains('┌'), "no box border in the main layout");
    }

    #[test]
    fn renders_transcript_completion_and_statusline() {
        let mut app = test_app();
        app.tab_mut().transcript.push(TranscriptEntry::User {
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

        app.tab_mut().metrics.last_tokens_per_sec = Some(42.5);
        app.tab_mut().metrics.last_ttft_ms = Some(310);
        app.tab_mut().metrics.last_turn_ms = Some(1400);

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
        assert!(rendered.contains("meerai"));
        assert!(!rendered.contains("> You"));
        assert!(!rendered.contains("◆ Assistant"));
        assert!(rendered.contains("SESSION"));
        assert!(rendered.contains("MCP"));
        assert!(rendered.contains("AGENTS"));
        assert!(rendered.contains("TOOLS"));
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
    fn composer_renders_prompt_and_mode_rule() {
        let mut app = test_app();
        let rendered = render_to_string(&mut app);
        assert!(rendered.contains("Ask meerai anything"));
        assert!(rendered.contains("insert"));
    }

    #[test]
    fn composer_renders_known_mention_chips() {
        let agents = vec![agent_definition("reviewer")];
        let skills = vec![skill_definition("deliver")];

        let (spans, cursor) = render_composer_line("@skill/deliver", Some(14), &agents, &skills);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "deliver");
        assert_eq!(spans[0].style.fg, Some(theme::p().interactive));
        assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(cursor, 7);

        let (spans, cursor) = render_composer_line("@agent/reviewer", Some(15), &agents, &skills);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "reviewer");
        assert_eq!(spans[0].style.fg, Some(theme::p().accent));
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
        assert_eq!(spans[1].style.fg, Some(theme::p().interactive));
        assert_eq!(spans[3].style.fg, Some(theme::p().accent));
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
        app.tab_mut().approval_mode = ApprovalMode::AutoEdit;
        let edit = render_to_string(&mut app);
        assert!(edit.contains("EDIT"));
        assert!(edit.contains("INS"));

        app.tab_mut().approval_mode = ApprovalMode::Yolo;
        assert!(render_to_string(&mut app).contains("YOLO"));

        app.tab_mut().approval_mode = ApprovalMode::Plan;
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
        app.tab_mut().transcript.push(TranscriptEntry::User {
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

    #[test]
    fn renders_session_tabs_badges_and_session_select_modal() {
        let mut app = test_app();
        app.tab_mut().title = Some("First Tab".to_string());
        app.tab_mut().session_id = meerai_session::SessionId::from_raw("sess-first");

        let (tx2, _rx2) = mpsc::unbounded_channel();
        let (_etx2, erx2) = mpsc::unbounded_channel();
        app.apply_opened_session(
            crate::OpenedSession {
                session_id: meerai_session::SessionId::from_raw("sess-second"),
                title: Some("Second Tab".to_string()),
                provider: "gemini".to_string(),
                model: "gemini-3.8-flash".to_string(),
                approval_mode: ApprovalMode::Ask,
                transcript: Vec::new(),
                events: Vec::new(),
                cmd_tx: tx2,
                event_rx: erx2,
            },
            false,
        );
        app.switch_to_tab(0);
        app.tabs[1].has_unread = true;

        let tab_bar_screen = render_to_string(&mut app);
        assert!(tab_bar_screen.contains("1: First Tab"));
        assert!(tab_bar_screen.contains("2: Second Tab •"));
        assert!(tab_bar_screen.contains(" + "));
        assert!(app.tab_hit_zones.len() >= 3);

        app.modal = Some(ActiveModal::SessionSelect {
            filter: String::new(),
            selected: 0,
            sessions: vec![
                meerai_session::SessionSummary {
                    id: meerai_session::SessionId::from_raw("sess-first"),
                    dir: PathBuf::from("/tmp/sess-first"),
                    title: Some("First Tab".to_string()),
                    model: Some("gemini-3.8-flash".to_string()),
                    created_at_ms: 100,
                    updated_at_ms: 200,
                    bytes: 64,
                    parent_session_id: None,
                },
                meerai_session::SessionSummary {
                    id: meerai_session::SessionId::from_raw("sess-second"),
                    dir: PathBuf::from("/tmp/sess-second"),
                    title: Some("Second Tab".to_string()),
                    model: Some("gemini-3.8-flash".to_string()),
                    created_at_ms: 80,
                    updated_at_ms: 150,
                    bytes: 64,
                    parent_session_id: None,
                },
                meerai_session::SessionSummary {
                    id: meerai_session::SessionId::from_raw("sess-other"),
                    dir: PathBuf::from("/tmp/sess-other"),
                    title: Some("Other Task".to_string()),
                    model: Some("gemini-3.8-flash".to_string()),
                    created_at_ms: 50,
                    updated_at_ms: 100,
                    bytes: 64,
                    parent_session_id: None,
                },
            ],
            loading: false,
            error: None,
        });

        let modal_screen = render_to_string(&mut app);
        assert!(modal_screen.contains("Switch Session (/sessions)"));
        assert!(modal_screen.contains("[tab 1*]"));
        assert!(modal_screen.contains("[tab 2]"));
        assert!(modal_screen.contains("Other Task"));
    }

    #[test]
    fn renders_distinct_user_thinking_tool_and_response_roles() {
        let mut app = test_app();
        app.tab_mut().transcript.push(TranscriptEntry::User {
            text: "Inspect Cargo.toml".to_string(),
        });
        app.tab_mut()
            .transcript
            .push(TranscriptEntry::ToolCall(crate::ToolCallEntry {
                call_id: "tc-1".to_string(),
                name: "code-read_file".to_string(),
                args: "{\"path\":\"Cargo.toml\"}".to_string(),
                status: ToolCallStatus::Succeeded,
                output: Some("[workspace]\nmembers = []".to_string()),
            }));
        app.tab_mut().transcript.push(TranscriptEntry::Assistant {
            text: "# Summary\n```toml\nname = \"meerai\"\n```".to_string(),
            reasoning: "Check workspace layout.".to_string(),
            streaming: false,
            reasoning_expanded: true,
        });

        let rendered = render_to_string(&mut app);
        assert!(rendered.contains("▌ you"));
        assert!(rendered.contains("▌ Inspect Cargo.toml"));
        assert!(rendered.contains("▸ [tool] code-read_file [ok]"));
        assert!(rendered.contains("↳ [workspace] members = []"));
        assert!(rendered.contains("▾ Thinking (1 lines)"));
        assert!(rendered.contains("● Response"));
        assert!(rendered.contains("# Summary"));
        assert!(rendered.contains("name = \"meerai\""));
    }
}
