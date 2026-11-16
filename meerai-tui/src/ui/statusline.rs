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
    let mut update_hit: Option<Rect> = None;
    let tab = app.tab();
    let (badge_text, badge_color) = match app.mode {
        InputMode::Insert => (" INS ", theme::p().accent),
        InputMode::Normal => (" NOR ", theme::p().info),
    };

    let mut spans = vec![
        Span::styled(" ", Style::default().bg(theme::p().surface)),
        Span::styled(
            badge_text,
            Style::default()
                .fg(badge_color)
                .bg(theme::p().surface_high)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", Style::default().bg(theme::p().surface)),
    ];

    let mode_color = match tab.approval_mode {
        ApprovalMode::Plan => theme::p().accent,
        ApprovalMode::Ask => theme::p().warning,
        ApprovalMode::AutoEdit => theme::p().info,
        ApprovalMode::AutoApprove => theme::p().success,
        ApprovalMode::Yolo => theme::p().error,
    };
    spans.push(Span::styled(
        format!(" {} ", approval_mode_badge(tab.approval_mode)),
        Style::default()
            .fg(mode_color)
            .bg(theme::p().surface_high)
            .add_modifier(Modifier::BOLD),
    ));

    if let Backend::Acp { preset, .. } = &tab.backend {
        spans.push(Span::styled(
            format!(" ACP:{preset} "),
            Style::default()
                .fg(theme::p().info)
                .bg(theme::p().surface_high)
                .add_modifier(Modifier::BOLD),
        ));
    }

    if !tab.is_acp() {
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
                format!(" · {frame_char} {label}{elapsed}")
            }
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
    Some(format!(
        "session ↑{} ↓{} Σ{}",
        format_compact_tokens(metrics.session_prompt_tokens),
        format_compact_tokens(metrics.session_completion_tokens),
        format_compact_tokens(total)
    ))
}

pub(super) fn key_status_style(status: &KeyStatus) -> (String, Color) {
    let badge = status.badge();
    let color = match status {
        KeyStatus::Stored | KeyStatus::Config => theme::p().success,
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
