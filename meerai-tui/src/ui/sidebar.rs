use super::statusline::{display_model, format_compact_tokens, key_status_style};
use super::*;

pub(super) fn render_sidebar_overview(frame: &mut Frame<'_>, app: &mut AppState, area: Rect) {
    app.update_hit = None;
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().surface)),
        area,
    );
    let inner = pad_rect(area, 2, 1);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let width = inner.width as usize;

    let key_status = app.active_key_status();
    let (key_badge, key_color) = key_status_style(&key_status);
    let field =
        |name: &str| Span::styled(format!("{name:<10}"), Style::default().fg(theme::p().muted));
    let workspace = display_workspace(&app.workspace_root);
    let section_style = Style::default()
        .fg(theme::p().text)
        .add_modifier(Modifier::BOLD);

    let mut lines = Vec::new();
    if let Some(latest) = app.update_available.clone() {
        lines.push(Line::from(Span::styled(
            format!("UPDATE  v{latest} · /update"),
            Style::default()
                .fg(theme::p().warning)
                .add_modifier(Modifier::BOLD),
        )));
        app.update_hit = Some(Rect::new(inner.x, inner.y, inner.width, 1));
        lines.push(Line::default());
    }
    let tab = app.tab();
    lines.extend([
        Line::from(Span::styled("SESSION", section_style)),
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
        Line::from(vec![
            field("dir"),
            Span::styled(
                truncate_single_line(&workspace, width.saturating_sub(10)),
                Style::default().fg(theme::p().muted),
            ),
        ]),
        Line::default(),
    ]);

    if app.mcp_servers.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(format!("{:<10}", "MCP"), section_style),
            Span::styled("none", Style::default().fg(theme::p().faint)),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("MCP ", section_style),
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
        Span::styled(format!("{:<10}", "AGENTS"), section_style),
        Span::styled("main ", theme::text()),
        if app.tab().is_running {
            Span::styled("[active]", Style::default().fg(theme::p().warning))
        } else {
            Span::styled("[ready]", Style::default().fg(theme::p().faint))
        },
    ]));

    let mut subagents: Vec<_> = app
        .tab()
        .transcript
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::Subagent(subagent) => Some((
                match subagent.named() {
                    Some(name) => format!("{name} ({})", subagent.agent),
                    None => subagent.agent.clone(),
                },
                subagent.status,
                subagent.steps_used,
                subagent.max_steps,
            )),
            _ => None,
        })
        .collect();
    subagents.sort_by_key(|(_, status, _, _)| {
        !matches!(status, ToolCallStatus::Proposed | ToolCallStatus::Running)
    });
    const MAX_SUBAGENT_ROWS: usize = 4;
    for (agent, status, steps_used, max_steps) in subagents.iter().take(MAX_SUBAGENT_ROWS) {
        let (badge, color) = status_label(*status);
        let steps = if *max_steps > 0 {
            format!(" {steps_used}/{max_steps}")
        } else {
            String::new()
        };
        let name_width = width.saturating_sub(4 + badge.len() + 2 + steps.len());
        lines.push(Line::from(vec![
            Span::styled(
                format!("  • {} ", truncate_single_line(agent, name_width)),
                theme::muted(),
            ),
            Span::styled(format!("[{badge}]"), Style::default().fg(color)),
            Span::styled(steps, Style::default().fg(theme::p().faint)),
        ]));
    }
    if subagents.len() > MAX_SUBAGENT_ROWS {
        lines.push(Line::from(Span::styled(
            format!("  … {} more", subagents.len() - MAX_SUBAGENT_ROWS),
            Style::default().fg(theme::p().faint),
        )));
    }

    let recent_tools: Vec<_> = app
        .tab()
        .transcript
        .iter()
        .filter_map(|e| match e {
            TranscriptEntry::ToolCall(tc) if !is_delegation_tool(&tc.name) => Some(tc),
            _ => None,
        })
        .rev()
        .take(5)
        .collect();

    if recent_tools.is_empty() && app.registered_tools.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(format!("{:<10}", "TOOLS"), section_style),
            Span::styled("(0 calls) idle", Style::default().fg(theme::p().faint)),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("TOOLS ", section_style),
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
                Span::styled(
                    format!("  {} ", truncate_single_line(&tc.name, max_name)),
                    theme::muted(),
                ),
                Span::styled(format!("[{badge}]"), Style::default().fg(color)),
            ]));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

pub(super) fn render_sidebar_diagnose(frame: &mut Frame<'_>, app: &mut AppState, area: Rect) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().surface)),
        area,
    );
    let inner = pad_rect(area, 2, 1);
    if inner.width == 0 || inner.height == 0 {
        app.sidebar_diagnose_head_area = Rect::default();
        return;
    }
    let width = inner.width as usize;
    let header_area = Rect::new(inner.x, inner.y, inner.width, 1);
    app.sidebar_diagnose_head_area = header_area;
    let head_hovered = app.hovered_sidebar == Some(SidebarHitKind::DiagnoseHead);
    if head_hovered {
        frame.render_widget(
            Block::default().style(Style::default().bg(theme::p().surface_high)),
            Rect::new(area.x, header_area.y, area.width, 1),
        );
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "DIAGNOSE ",
                Style::default()
                    .fg(theme::p().text)
                    .bg(if head_hovered {
                        theme::p().surface_high
                    } else {
                        theme::p().surface
                    })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if app.sidebar_diagnose_open {
                    "[hide]"
                } else {
                    "[show]"
                },
                Style::default()
                    .fg(if head_hovered {
                        theme::p().text
                    } else {
                        theme::p().faint
                    })
                    .bg(if head_hovered {
                        theme::p().surface_high
                    } else {
                        theme::p().surface
                    }),
            ),
        ])),
        header_area,
    );
    if !app.sidebar_diagnose_open || inner.height < 2 {
        return;
    }
    let rows_area = Rect::new(
        inner.x,
        inner.y.saturating_add(1),
        inner.width,
        inner.height.saturating_sub(1),
    );
    frame.render_widget(Paragraph::new(diagnose_lines(app, width)), rows_area);
}

fn diagnose_lines(app: &AppState, width: usize) -> Vec<Line<'static>> {
    let tab = app.tab();
    let field =
        |name: &str| Span::styled(format!("{name:<10}"), Style::default().fg(theme::p().muted));
    let value = |text: String| {
        Span::styled(
            truncate_single_line(&text, width.saturating_sub(10)),
            theme::text(),
        )
    };
    let ratio = |part: u64, whole: u64| {
        if whole == 0 {
            return "n/a".to_string();
        }
        let percent = format!("{:.1}%", part as f64 * 100.0 / whole as f64);
        let raw = format!(
            " {}/{}",
            format_compact_tokens(part),
            format_compact_tokens(whole)
        );
        if percent.chars().count() + raw.chars().count() <= width.saturating_sub(10) {
            format!("{percent}{raw}")
        } else {
            percent
        }
    };

    let prompt = tab.usage.prompt_tokens.unwrap_or(0).max(0) as u64;
    let completion = tab.usage.completion_tokens.unwrap_or(0).max(0) as u64;
    let hit = match tab.usage.cached_tokens {
        Some(cached) => ratio(cached.max(0) as u64, prompt),
        None => "n/a".to_string(),
    };
    let write = match tab.usage.cache_creation_tokens {
        Some(written) => format_compact_tokens(written.max(0) as u64),
        None => "n/a".to_string(),
    };
    let (context, memory, tools) = match tab.context.as_ref() {
        Some(context) => (
            ratio(
                context.estimated_total_tokens,
                context.context_window_tokens,
            ),
            format!("{} tokens", format_compact_tokens(context.memory_tokens)),
            format!(
                "{} tokens",
                format_compact_tokens(context.tool_schemas_tokens)
            ),
        ),
        None => ("n/a".to_string(), "n/a".to_string(), "n/a".to_string()),
    };

    vec![
        Line::from(vec![
            field("tokens"),
            value(format!(
                "↑{} ↓{}",
                format_compact_tokens(prompt),
                format_compact_tokens(completion)
            )),
        ]),
        Line::from(vec![field("hit"), value(hit)]),
        Line::from(vec![field("write"), value(write)]),
        Line::from(vec![field("context"), value(context)]),
        Line::from(vec![field("memory"), value(memory)]),
        Line::from(vec![field("tools"), value(tools)]),
    ]
}

fn display_workspace(path: &std::path::Path) -> String {
    let raw = path.display().to_string();
    if let Some(home) = meerai_core::env::home_dir()
        && let Some(rest) = raw.strip_prefix(&home.display().to_string())
    {
        return format!("~{rest}");
    }
    raw
}

pub(super) fn render_sidebar_changes(frame: &mut Frame<'_>, app: &mut AppState, area: Rect) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().surface)),
        area,
    );
    let inner = pad_rect(area, 2, 1);
    if inner.width == 0 || inner.height == 0 {
        app.sidebar_changes_inner_area = Rect::default();
        app.sidebar_changes_head_area = Rect::default();
        app.visible_file_change_rows = 0;
        return;
    }

    let width = inner.width as usize;
    let action = if app.sidebar_changes_open {
        "[hide]"
    } else {
        "[show]"
    };
    let title = format!("File Changes ({})", app.file_changes.len());
    let title = truncate_single_line(&title, width.saturating_sub(action.chars().count() + 1));
    let header_area = Rect::new(inner.x, inner.y, inner.width, 1);
    app.sidebar_changes_head_area = header_area;
    let head_hovered = app.hovered_sidebar == Some(SidebarHitKind::ChangesHead);
    let head_bg = if head_hovered {
        theme::p().surface_high
    } else {
        theme::p().surface
    };
    if head_hovered {
        frame.render_widget(
            Block::default().style(Style::default().bg(head_bg)),
            Rect::new(area.x, header_area.y, area.width, 1),
        );
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{title} "),
                Style::default()
                    .fg(theme::p().text)
                    .bg(head_bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                action,
                Style::default()
                    .fg(if head_hovered {
                        theme::p().text
                    } else {
                        theme::p().faint
                    })
                    .bg(head_bg),
            ),
        ])),
        header_area,
    );
    if !app.sidebar_changes_open || inner.height < 2 {
        app.sidebar_changes_inner_area = Rect::default();
        app.visible_file_change_rows = 0;
        app.file_change_more_hit = None;
        app.hovered_file_change = None;
        return;
    }

    let top_gap = if inner.height >= 4 { 2 } else { 1 };
    let rows_area = Rect {
        x: inner.x,
        y: inner.y.saturating_add(top_gap),
        width: inner.width,
        height: inner.height.saturating_sub(top_gap),
    };
    app.sidebar_changes_inner_area = rows_area;

    if rows_area.height == 0 {
        app.visible_file_change_rows = 0;
        return;
    }

    if app.file_changes.is_empty() {
        app.visible_file_change_rows = 0;
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "Clean working tree",
                Style::default().fg(theme::p().faint),
            ))),
            rows_area,
        );
        return;
    }

    let max_rows = rows_area.height as usize;
    let total = app.file_changes.len();
    let overflow = total > max_rows && max_rows > 1;
    let shown_count = if overflow {
        max_rows - 1
    } else {
        total.min(max_rows)
    };
    let max_start = total.saturating_sub(shown_count);
    app.file_change_scroll = app.file_change_scroll.min(max_start);
    let start = app.file_change_scroll;
    app.visible_file_change_rows = shown_count;
    app.file_change_more_hit = None;

    for (offset, entry) in app
        .file_changes
        .iter()
        .skip(start)
        .take(shown_count)
        .enumerate()
    {
        let idx = start + offset;
        let row_y = rows_area.y + offset as u16;
        let hovered = app.hovered_file_change == Some(idx);
        let row_bg = if hovered {
            theme::p().surface_high
        } else {
            theme::p().surface
        };
        if hovered {
            frame.render_widget(
                Block::default().style(Style::default().bg(row_bg)),
                Rect::new(area.x, row_y, area.width, 1),
            );
        }
        let status_color = match entry.status.as_str() {
            "??" | "A" => theme::p().success,
            "D" => theme::p().error,
            _ => theme::p().warning,
        };
        let badge = format!("{:<2} ", entry.status);
        let max_path = width.saturating_sub(badge.chars().count());
        let path_style = if hovered {
            Style::default()
                .fg(theme::p().text)
                .bg(row_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::p().muted).bg(row_bg)
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(badge, Style::default().fg(status_color).bg(row_bg)),
                Span::styled(truncate_single_line(&entry.path, max_path), path_style),
            ])),
            Rect::new(rows_area.x, row_y, rows_area.width, 1),
        );
    }

    if overflow {
        let remaining = total.saturating_sub(start + shown_count);
        let label = if remaining > 0 {
            format!("… and {remaining} more")
        } else {
            "… back to top".to_string()
        };
        let footer_y = rows_area.y + shown_count as u16;
        let footer_rect = Rect::new(rows_area.x, footer_y, rows_area.width, 1);
        app.file_change_more_hit = Some(footer_rect);
        let footer_hovered = app.hovered_sidebar == Some(SidebarHitKind::ChangesMore);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                label,
                Style::default()
                    .fg(if footer_hovered {
                        theme::p().text
                    } else {
                        theme::p().faint
                    })
                    .bg(if footer_hovered {
                        theme::p().surface_high
                    } else {
                        theme::p().surface
                    }),
            ))),
            footer_rect,
        );
    }
}
