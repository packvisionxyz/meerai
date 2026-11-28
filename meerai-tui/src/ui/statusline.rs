use super::*;
use crate::Backend;

pub(super) fn display_model(app: &AppState) -> &str {
    let trimmed = app.tab().model.trim();
    if trimmed.is_empty() {
        "(not set)"
    } else {
        trimmed
    }
}

pub(super) fn render_statusline(frame: &mut Frame<'_>, app: &mut AppState, area: Rect) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().surface)),
        area,
    );
    app.runs_hits.clear();
    app.web_server_hit = None;
    app.mcp_hit = None;
    let mut runs_hits: Vec<(Rect, RunKind)> = Vec::new();
    let mut web_server_hit: Option<Rect> = None;
    let mut mcp_hit: Option<Rect> = None;
    let mut triggers: Vec<(usize, StatusHitKind)> = Vec::new();
    let tab = app.tab();

    let mut spans = vec![Span::styled(" ", Style::default().bg(theme::p().surface))];
    spans.push(Span::styled(
        format!(" {}", display_workspace(&app.workspace_root)),
        Style::default().fg(theme::p().muted),
    ));
    if let Some(branch) = &app.git_branch {
        spans.push(Span::styled(
            format!(" · {branch}"),
            Style::default().fg(theme::p().muted),
        ));
    }

    if !app.sandbox_active {
        spans.push(Span::styled(
            " ⚠ host sandbox ",
            Style::default()
                .fg(theme::p().warning)
                .bg(theme::p().surface_high)
                .add_modifier(Modifier::BOLD),
        ));
    }

    if let Backend::Acp { preset, .. } = &tab.backend {
        spans.push(Span::styled(
            format!(" ACP:{preset} "),
            Style::default()
                .fg(theme::p().info)
                .bg(theme::p().surface_high)
                .add_modifier(Modifier::BOLD),
        ));
    }

    for (kind, color) in [
        (RunKind::Agents, theme::p().accent),
        (RunKind::Shells, theme::p().info),
    ] {
        let count = app.run_count(kind);
        if count == 0 {
            continue;
        }
        let noun = match kind {
            RunKind::Agents if count == 1 => "agent",
            RunKind::Agents => "agents",
            RunKind::Shells if count == 1 => "shell",
            RunKind::Shells => "shells",
        };
        let label = format!(" · {count} {noun}");
        let offset: usize = spans.iter().map(|span| span.content.chars().count()).sum();
        let width = label.chars().count() as u16;
        let x = area.x.saturating_add(offset as u16);
        if x.saturating_add(width) <= area.x.saturating_add(area.width) {
            runs_hits.push((Rect::new(x, area.y, width, 1), kind));
            triggers.push((spans.len(), StatusHitKind::Run(kind)));
            spans.push(Span::styled(label, Style::default().fg(color)));
        }
    }

    if let Some(server) = app.web_server.clone() {
        let label = format!(" · web :{}", server.port);
        let offset: usize = spans.iter().map(|span| span.content.chars().count()).sum();
        let width = label.chars().count() as u16;
        let x = area.x.saturating_add(offset as u16);
        if x.saturating_add(width) <= area.x.saturating_add(area.width) {
            web_server_hit = Some(Rect::new(x, area.y, width, 1));
            triggers.push((spans.len(), StatusHitKind::WebServer));
            spans.push(Span::styled(label, Style::default().fg(theme::p().accent)));
        }
    }

    if !app.mcp_servers.is_empty() {
        let connected = app.mcp_servers.iter().filter(|s| s.is_connected()).count();
        let failed = app.mcp_servers.iter().filter(|s| s.is_failed()).count();
        let untrusted = app.mcp_servers.iter().filter(|s| s.needs_trust()).count();
        let total = app.mcp_servers.len();
        let label = if failed > 0 {
            format!(" · mcp {connected}/{total} · {failed} failed")
        } else if untrusted > 0 {
            format!(" · mcp {connected}/{total} · {untrusted} needs trust")
        } else {
            format!(" · mcp {connected}/{total}")
        };
        let color = if failed > 0 || untrusted > 0 {
            theme::p().warning
        } else if connected > 0 {
            theme::p().success
        } else {
            theme::p().faint
        };
        let offset: usize = spans.iter().map(|span| span.content.chars().count()).sum();
        let width = label.chars().count() as u16;
        let x = area.x.saturating_add(offset as u16);
        if x.saturating_add(width) <= area.x.saturating_add(area.width) {
            mcp_hit = Some(Rect::new(x, area.y, width, 1));
            triggers.push((spans.len(), StatusHitKind::McpServers));
            spans.push(Span::styled(label, Style::default().fg(color)));
        }
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
        let color = notice_color(notice_severity(notice));
        spans.push(Span::styled(
            format!(" · {notice}"),
            Style::default().fg(color),
        ));
    }

    app.runs_hits = runs_hits;
    app.web_server_hit = web_server_hit;
    app.mcp_hit = mcp_hit;
    let hovered_status = app.hovered_status;
    let spans: Vec<Span<'static>> = spans
        .into_iter()
        .enumerate()
        .map(|(idx, span)| {
            let hovered = triggers
                .iter()
                .any(|(trigger_idx, kind)| *trigger_idx == idx && Some(*kind) == hovered_status);
            let style = if hovered {
                hover_row_style()
            } else {
                Style::default().fg(theme::p().muted)
            };
            Span::styled(span.content, style)
        })
        .collect();
    let bar = Paragraph::new(Line::from(spans));
    frame.render_widget(bar, area);
}

pub(super) fn format_compact_tokens(count: u64) -> String {
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

pub(crate) fn format_compact_ms(ms: u64) -> String {
    if ms >= 60_000 {
        let total_secs = ms / 1000;
        format!("{}m{}s", total_secs / 60, total_secs % 60)
    } else if ms >= 1_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{ms}ms")
    }
}

pub(super) fn key_status_style(status: &KeyStatus) -> (String, Color) {
    let badge = status.badge();
    let color = match status {
        KeyStatus::Stored | KeyStatus::Config => theme::p().success,
        KeyStatus::OAuth { needs_relogin, .. } => {
            if *needs_relogin {
                theme::p().error
            } else {
                theme::p().success
            }
        }
        KeyStatus::Env(_) => theme::p().muted,
        KeyStatus::NotRequired => theme::p().muted,
        KeyStatus::Missing => theme::p().error,
    };
    (badge, color)
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

fn display_workspace(path: &std::path::Path) -> String {
    let raw = path.display().to_string();
    if let Some(home) = meerai_core::env::home_dir() {
        let home = home.display().to_string();
        if let Some(rest) = raw.strip_prefix(&home) {
            return format!("~{rest}");
        }
    }
    raw
}
