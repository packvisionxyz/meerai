use super::statusline::{display_model, key_status_style};
use super::*;

pub(super) fn render_sidebar_overview(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
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
    let field =
        |name: &str| Span::styled(format!("{name:<9}"), Style::default().fg(theme::p().muted));
    let workspace = display_workspace(&app.workspace_root);

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
        Block::default().style(Style::default().bg(theme::p().surface_high)),
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
