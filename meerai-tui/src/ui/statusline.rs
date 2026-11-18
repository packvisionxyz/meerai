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
    app.update_hit = None;
    app.subagent_hit = None;
    app.web_server_hit = None;
    let mut update_hit: Option<Rect> = None;
    let mut subagent_hit: Option<Rect> = None;
    let mut web_server_hit: Option<Rect> = None;
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

    if let Some(command) = &app.user_shell_running {
        let command = truncate_single_line(command, 32);
        spans.push(Span::styled(
            format!(" · shell {command}"),
            Style::default().fg(theme::p().muted),
        ));
    }
    let usage_str = format_usage_compact(app);
    if usage_str != "-" {
        spans.push(Span::styled(
            format!(" · {usage_str}"),
            Style::default().fg(theme::p().muted),
        ));
    }

    if let Some(latest) = app.update_available.clone() {
        let label = format!(" · update v{latest} (/update)");
        let offset: usize = spans.iter().map(|span| span.content.chars().count()).sum();
        let width = label.chars().count() as u16;
        let x = area.x.saturating_add(offset as u16);
        if x.saturating_add(width) <= area.x.saturating_add(area.width) {
            update_hit = Some(Rect::new(x, area.y, width, 1));
            spans.push(Span::styled(label, Style::default().fg(theme::p().warning)));
        }
    }

    if let Some(server) = app.web_server.clone() {
        let label = format!(" · web :{}", server.port);
        let offset: usize = spans.iter().map(|span| span.content.chars().count()).sum();
        let width = label.chars().count() as u16;
        let x = area.x.saturating_add(offset as u16);
        if x.saturating_add(width) <= area.x.saturating_add(area.width) {
            web_server_hit = Some(Rect::new(x, area.y, width, 1));
            spans.push(Span::styled(label, Style::default().fg(theme::p().accent)));
        }
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
        let mut subagent_activity = false;
        let activity = match &tab.active_tool {
            Some(tool) if tool == "subagent-run" => {
                let agents: Vec<&str> = tab
                    .transcript
                    .iter()
                    .filter_map(|entry| match entry {
                        TranscriptEntry::Subagent(sub)
                            if matches!(
                                sub.status,
                                ToolCallStatus::Running | ToolCallStatus::Proposed
                            ) =>
                        {
                            Some(sub.agent.as_str())
                        }
                        _ => None,
                    })
                    .collect();
                let label = if agents.is_empty() {
                    "subagents".to_string()
                } else if agents.len() <= 2 {
                    agents.join(", ")
                } else {
                    format!("{}, +{}", agents[..2].join(", "), agents.len() - 2)
                };
                subagent_activity = true;
                format!(" · {frame_char} {label}{elapsed}")
            }
            Some(tool) => format!(" · {frame_char} {tool}{elapsed}"),
            None => format!(" · {frame_char} running{elapsed}"),
        };
        if subagent_activity {
            let offset: usize = spans.iter().map(|span| span.content.chars().count()).sum();
            let width = activity.chars().count() as u16;
            let x = area.x.saturating_add(offset as u16);
            if x.saturating_add(width) <= area.x.saturating_add(area.width) {
                subagent_hit = Some(Rect::new(x, area.y, width, 1));
            }
        }
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
    } else {
        let hint = if tab.is_running {
            let cancel = crate::app::ctrl_label('c');
            let steer = crate::app::ctrl_label('g');
            let palette = crate::app::ctrl_label('p');
            if tab.queued.is_empty() {
                format!(" · {cancel} cancel · {steer} send now · {palette} commands")
            } else {
                format!(" · {cancel} cancel · {steer} send queued · {palette} commands")
            }
        } else {
            match app.mode {
                InputMode::Insert => {
                    if app.completion.is_active() {
                        " · Tab/Enter accept · Esc dismiss".to_string()
                    } else {
                        format!(" · {} commands · Esc normal", crate::app::ctrl_label('p'))
                    }
                }
                InputMode::Normal => format!(
                    " · {} commands · i insert · / search · q quit",
                    crate::app::ctrl_label('p')
                ),
            }
        };
        spans.push(Span::styled(hint, Style::default().fg(theme::p().faint)));
    }

    app.update_hit = update_hit;
    app.subagent_hit = subagent_hit;
    app.web_server_hit = web_server_hit;
    let spans: Vec<Span<'static>> = spans
        .into_iter()
        .map(|span| {
            let style = Style::default().fg(theme::p().muted);
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

pub(super) fn format_compact_ms(ms: u64) -> String {
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
    let cached = if metrics.session_cached_tokens > 0 {
        format!(" ↻{}", format_compact_tokens(metrics.session_cached_tokens))
    } else {
        String::new()
    };
    Some(format!(
        "session ↑{} ↓{} Σ{}{}",
        format_compact_tokens(metrics.session_prompt_tokens),
        format_compact_tokens(metrics.session_completion_tokens),
        format_compact_tokens(total),
        cached
    ))
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
