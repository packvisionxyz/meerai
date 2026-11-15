use super::statusline::{display_model, key_status_style};
use super::*;

pub(super) fn render_sidebar_overview(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().surface)),
        area,
    );
    let inner = pad_rect(area, 2, 1);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let width = inner.width as usize;
    let tab = app.tab();

    let key_status = app.active_key_status();
    let (key_badge, key_color) = key_status_style(&key_status);
    let field =
        |name: &str| Span::styled(format!("{name:<10}"), Style::default().fg(theme::p().muted));
    let workspace = display_workspace(&app.workspace_root);
    let section_style = Style::default()
        .fg(theme::p().text)
        .add_modifier(Modifier::BOLD);

    let mut lines = vec![
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
    ];

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
        app.visible_file_change_rows = 0;
        return;
    }

    let title = format!("File Changes ({})", app.file_changes.len());
    let width = inner.width as usize;

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            truncate_single_line(&title, width),
            Style::default()
                .fg(theme::p().text)
                .add_modifier(Modifier::BOLD),
        ))),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

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
    app.visible_file_change_rows = shown_count;

    for (idx, entry) in app.file_changes.iter().take(shown_count).enumerate() {
        let row_y = rows_area.y + idx as u16;
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
        let remaining = total - shown_count;
        let footer_y = rows_area.y + shown_count as u16;
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("… and {remaining} more"),
                Style::default().fg(theme::p().faint),
            ))),
            Rect::new(rows_area.x, footer_y, rows_area.width, 1),
        );
    }
}
