use super::statusline::{display_model, key_status_style, summarize_error_line};
use super::*;

pub(super) fn render_transcript(frame: &mut Frame<'_>, app: &mut AppState, area: Rect) {
    let inner = Rect {
        x: area.x.saturating_add(2),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(2),
    };
    app.transcript_inner_area = inner;

    let raw_lines = build_transcript_lines(app);
    let mut wrapped = if inner.width > 0 {
        wrap_tagged_lines(raw_lines, inner.width as usize)
    } else {
        WrappedTranscript {
            lines: Vec::new(),
            hit_zones: Vec::new(),
            doc_lines: Vec::new(),
            bands: Vec::new(),
            code_bands: Vec::new(),
            hover_zones: Vec::new(),
        }
    };

    app.thinking_hit_zones = std::mem::take(&mut wrapped.hit_zones);
    app.transcript_wrapped_lines = std::mem::take(&mut wrapped.doc_lines);

    let scroll_y = {
        let tab = app.tab_mut();
        tab.viewport_height = inner.height as usize;
        tab.total_lines = wrapped.lines.len();
        let max_scroll = tab.max_scroll();
        if tab.auto_follow {
            tab.scroll_offset = max_scroll;
        } else {
            tab.scroll_offset = tab.scroll_offset.min(max_scroll);
        }
        tab.scroll_offset.min(u16::MAX as usize) as u16
    };

    let hovered = app.hovered_entry_at_screen(inner, scroll_y as usize);
    app.hovered_entry = hovered;
    let hover_zones = std::mem::take(&mut wrapped.hover_zones);
    let hover_range = hovered.and_then(|idx| {
        hover_zones
            .iter()
            .find(|zone| zone.entry_idx == idx)
            .map(|zone| (zone.start_line, zone.end_line))
    });
    app.hover_zones = hover_zones;

    let paragraph = Paragraph::new(wrapped.lines).scroll((scroll_y, 0));
    frame.render_widget(paragraph, inner);

    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let first_visible = scroll_y as usize;
    let last_visible = first_visible + inner.height as usize;

    let paint = |frame: &mut Frame<'_>, bands: &[(usize, usize)], x_offset: u16, color: Color| {
        let start_x = inner.x.saturating_add(x_offset).min(inner.x + inner.width);
        for &(start, end) in bands {
            if end <= first_visible || start >= last_visible {
                continue;
            }
            let from = start.max(first_visible);
            let to = end.min(last_visible);
            let buf = frame.buffer_mut();
            for row in from..to {
                let y = inner.y + (row - first_visible) as u16;
                for col in start_x..inner.x + inner.width {
                    if let Some(cell) = buf.cell_mut((col, y)) {
                        cell.set_bg(color);
                    }
                }
            }
        }
    };

    paint(frame, &wrapped.code_bands, 2, theme::p().surface_high);
    paint(frame, &wrapped.bands, 0, theme::p().surface);
    if let Some(range) = hover_range {
        paint(frame, &[range], 0, theme::p().surface_high);
    }
}

const LOGO_M_ROWS: [&str; 4] = ["██      ██", "████  ████", "██  ██  ██", "██      ██"];

fn build_header_lines(app: &AppState) -> Vec<TaggedTranscriptLine> {
    let key_status = app.active_key_status();
    let (badge, badge_color) = key_status_style(&key_status);
    let logo_style = Style::default()
        .fg(theme::p().accent)
        .add_modifier(Modifier::BOLD);

    let mut lines = vec![
        tagged_line(
            Line::from(vec![
                Span::styled(LOGO_M_ROWS[0], logo_style),
                Span::raw("  "),
                Span::styled(
                    "meerai",
                    Style::default()
                        .fg(theme::p().accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("{} / {}", app.tab().active_provider, display_model(app)),
                    Style::default().fg(theme::p().text),
                ),
                Span::raw(" "),
                Span::styled(format!("[{badge}]"), Style::default().fg(badge_color)),
            ]),
            None,
            0,
        ),
        tagged_line(
            Line::from(vec![
                Span::styled(LOGO_M_ROWS[1], logo_style),
                Span::styled(
                    "  Commands: /sessions, /new, /connect, /models, /help, @ (files).",
                    Style::default().fg(theme::p().muted),
                ),
            ]),
            None,
            0,
        ),
        tagged_line(
            Line::from(vec![
                Span::styled(LOGO_M_ROWS[2], logo_style),
                Span::styled(
                    "  Shortcuts: Ctrl-j newline, Ctrl-t think, Drag/Shift+Click copy.",
                    Style::default().fg(theme::p().muted),
                ),
            ]),
            None,
            0,
        ),
        tagged_line(
            Line::from(vec![
                Span::styled(LOGO_M_ROWS[3], logo_style),
                Span::styled(
                    format!("  Workspace: {}", app.workspace_root.display()),
                    Style::default().fg(theme::p().faint),
                ),
            ]),
            None,
            0,
        ),
    ];

    if app.tab().transcript.is_empty() {
        let tip = |key: &str, desc: &str| {
            banded_line(
                Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(
                        format!("{key:<14}"),
                        Style::default()
                            .fg(theme::p().text)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(desc.to_string(), Style::default().fg(theme::p().muted)),
                ]),
                None,
                0,
            )
        };
        lines.push(tagged_line(Line::default(), None, 0));
        lines.push(banded_line(Line::default(), None, 0));
        lines.push(banded_line(
            Line::from(Span::styled(
                "  Quick start",
                Style::default()
                    .fg(theme::p().muted)
                    .add_modifier(Modifier::BOLD),
            )),
            None,
            0,
        ));
        lines.push(banded_line(Line::default(), None, 0));
        lines.push(tip("Ctrl-p", "Command palette"));
        lines.push(tip("Ctrl-f  /", "Search workspace files & content"));
        lines.push(tip("@", "Mention agents, skills, or files"));
        lines.push(tip("/connect", "Configure provider or API key"));
        lines.push(tip("/help", "List all commands & shortcuts"));
        lines.push(banded_line(Line::default(), None, 0));
    }

    lines
}

fn script_from_args(name: &str, args: &str) -> Option<(String, String)> {
    if !name.ends_with("run_script") {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(args).ok()?;
    let script = value.get("script")?.as_str()?.to_string();
    let language = value
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("script")
        .to_string();
    Some((language, script))
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

#[derive(Default)]
struct TurnDigest {
    commands: usize,
    edits: usize,
    reads: usize,
    agents: usize,
    thoughts: usize,
}

impl TurnDigest {
    fn record_tool(&mut self, name: &str) {
        match name {
            "code-shell" | "sac-run_script" => self.commands += 1,
            "code-write_file" | "code-edit_file" => self.edits += 1,
            "subagent-run" => self.agents += 1,
            _ => self.reads += 1,
        }
    }

    fn is_empty(&self) -> bool {
        self.commands + self.edits + self.reads + self.agents + self.thoughts == 0
    }

    fn summary(&self) -> String {
        fn part(count: usize, singular: &str, plural: &str) -> Option<String> {
            match count {
                0 => None,
                1 => Some(format!("1 {singular}")),
                n => Some(format!("{n} {plural}")),
            }
        }
        [
            part(self.commands, "command", "commands"),
            part(self.edits, "edit", "edits"),
            part(self.reads, "read", "reads"),
            part(self.agents, "agent", "agents"),
            part(self.thoughts, "thought", "thoughts"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
    }
}

fn turn_digests(transcript: &[TranscriptEntry]) -> (Vec<Option<TurnDigest>>, Option<usize>) {
    let mut digests: Vec<Option<TurnDigest>> = (0..transcript.len()).map(|_| None).collect();
    let mut last_user = None;
    let mut current: Option<usize> = None;
    let mut digest = TurnDigest::default();

    for (idx, entry) in transcript.iter().enumerate() {
        if matches!(entry, TranscriptEntry::User { .. }) {
            if let Some(start) = current {
                digests[start] = Some(std::mem::take(&mut digest));
            }
            current = Some(idx);
            last_user = Some(idx);
            digest = TurnDigest::default();
        }
        match entry {
            TranscriptEntry::Assistant { reasoning, .. } if !reasoning.is_empty() => {
                digest.thoughts += 1;
            }
            TranscriptEntry::ToolCall(call) => digest.record_tool(&call.name),
            TranscriptEntry::Subagent(_) => digest.agents += 1,
            _ => {}
        }
    }

    if let Some(start) = current {
        digests[start] = Some(digest);
    }
    (digests, last_user)
}

fn digest_line(digest: &TurnDigest, running: bool, spinner: &str) -> Option<Line<'static>> {
    if digest.is_empty() {
        return None;
    }
    let marker = if running {
        format!("  {spinner} ")
    } else {
        "  · ".to_string()
    };
    let marker_style = if running {
        Style::default().fg(theme::p().warning)
    } else {
        Style::default().fg(theme::p().faint)
    };
    Some(Line::from(vec![
        Span::styled(marker, marker_style),
        Span::styled(digest.summary(), Style::default().fg(theme::p().faint)),
    ]))
}

fn build_transcript_lines(app: &AppState) -> Vec<TaggedTranscriptLine> {
    let mut lines = build_header_lines(app);
    let transcript = &app.tab().transcript;
    if transcript.is_empty() {
        return lines;
    }

    let (digests, last_user) = turn_digests(transcript);
    let spinner = SPINNER_FRAMES[app.spinner_tick % SPINNER_FRAMES.len()];

    lines.push(tagged_line(Line::default(), None, 0));
    for (idx, entry) in transcript.iter().enumerate() {
        if idx > 0 {
            lines.push(tagged_line(Line::default(), None, 0));
        }
        match entry {
            TranscriptEntry::User { text } => {
                let mut header = vec![Span::styled(
                    "▌ you",
                    Style::default()
                        .fg(theme::p().accent)
                        .add_modifier(Modifier::BOLD),
                )];
                if app.hovered_entry == Some(idx) {
                    header.push(Span::styled(
                        "  [click for actions]",
                        Style::default()
                            .fg(theme::p().faint)
                            .add_modifier(Modifier::ITALIC),
                    ));
                }
                lines.push(with_hover(
                    banded_line(Line::from(header), Some(idx), 2),
                    idx,
                ));
                for line in text.split('\n') {
                    lines.push(with_hover(
                        banded_line(
                            Line::from(vec![
                                Span::styled("▌ ", Style::default().fg(theme::p().accent)),
                                Span::styled(
                                    line.to_string(),
                                    Style::default().fg(theme::p().text),
                                ),
                            ]),
                            Some(idx),
                            2,
                        ),
                        idx,
                    ));
                }
                if let Some(digest) = digests.get(idx).and_then(|d| d.as_ref()) {
                    let running = app.tab().is_running && last_user == Some(idx);
                    if let Some(line) = digest_line(digest, running, spinner) {
                        lines.push(tagged_line(line, None, 2));
                    }
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
                    let label = if *streaming { "Thinking" } else { "Thought" };
                    let count = if line_count == 1 {
                        "1 line".to_string()
                    } else {
                        format!("{line_count} lines")
                    };
                    if *reasoning_expanded {
                        lines.push(with_hover(
                            tagged_line(
                                Line::from(Span::styled(
                                    format!("  ▾ {label} ({count})  [click to hide]"),
                                    Style::default()
                                        .fg(theme::p().muted)
                                        .add_modifier(Modifier::ITALIC),
                                )),
                                Some(idx),
                                2,
                            ),
                            idx,
                        ));
                        for line in reasoning.split('\n') {
                            lines.push(with_hover(
                                tagged_line(
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
                                ),
                                idx,
                            ));
                        }
                    } else {
                        lines.push(with_hover(
                            tagged_line(
                                Line::from(Span::styled(
                                    format!("  ▸ {label} ({count})  [click to expand]"),
                                    Style::default()
                                        .fg(theme::p().muted)
                                        .add_modifier(Modifier::ITALIC),
                                )),
                                Some(idx),
                                2,
                            ),
                            idx,
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
                    for line in crate::markdown::render(text) {
                        if line.code && !in_code_block {
                            lines.push(code_line(tagged_line(Line::default(), None, 4)));
                            in_code_block = true;
                        } else if !line.code && in_code_block {
                            lines.push(code_line(tagged_line(Line::default(), None, 4)));
                            in_code_block = false;
                        }
                        let prefix = if line.code { "    " } else { "  " };
                        let gutter = if line.code { 4 } else { 2 };
                        let mut spans = vec![Span::styled(prefix, Style::default())];
                        spans.extend(line.spans);
                        let tagged = tagged_line(Line::from(spans), None, gutter);
                        lines.push(if line.code { code_line(tagged) } else { tagged });
                    }
                    if in_code_block {
                        lines.push(code_line(tagged_line(Line::default(), None, 4)));
                    }
                }
            }
            TranscriptEntry::ToolCall(tc) => {
                let (badge, badge_color) = tool_status_badge(&tc.status);
                let script = script_from_args(&tc.name, &tc.args);
                let args_preview = match &script {
                    Some((language, code)) => {
                        let first = code.lines().next().unwrap_or("").trim();
                        truncate_single_line(&format!("{language} · {first}"), 60)
                    }
                    None => truncate_single_line(&tc.args, 60),
                };
                let active = matches!(
                    tc.status,
                    ToolCallStatus::Proposed | ToolCallStatus::Running
                );
                let lead = if tc.status == ToolCallStatus::Running {
                    let ch = SPINNER_FRAMES[app.spinner_tick % SPINNER_FRAMES.len()];
                    format!("{ch} [tool] ")
                } else {
                    "▸ [tool] ".to_string()
                };
                let (lead_style, name_style, badge_style, args_style) = if active {
                    (
                        Style::default().fg(theme::p().interactive),
                        Style::default()
                            .fg(theme::p().text)
                            .add_modifier(Modifier::BOLD),
                        Style::default().fg(badge_color),
                        Style::default().fg(theme::p().muted),
                    )
                } else {
                    let badge_color = if tc.status == ToolCallStatus::Failed {
                        theme::p().error
                    } else {
                        theme::p().faint
                    };
                    (
                        Style::default().fg(theme::p().faint),
                        Style::default().fg(theme::p().muted),
                        Style::default().fg(badge_color),
                        Style::default().fg(theme::p().faint),
                    )
                };

                let output = tc.output.as_deref().unwrap_or("").trim_end();
                let output_lines: Vec<&str> = if output.is_empty() {
                    Vec::new()
                } else {
                    output.lines().collect()
                };
                const PREVIEW_LINES: usize = 8;
                let full_args = tc.args.trim().to_string();
                let args_collapsible = full_args.chars().count() > 60;
                let collapsible = script.is_some()
                    || args_collapsible
                    || !output_lines.is_empty()
                    || output_lines
                        .first()
                        .is_some_and(|line| line.chars().count() > 120);

                let mut header = vec![
                    Span::styled(lead, lead_style),
                    Span::styled(tc.name.clone(), name_style),
                    Span::styled(" ", Style::default()),
                    Span::styled(format!("[{badge}]"), badge_style),
                ];
                if args_preview.is_empty() {
                    header.push(Span::raw(""));
                } else {
                    header.push(Span::styled(format!(" {args_preview}"), args_style));
                }
                if collapsible {
                    header.push(Span::styled(
                        if tc.expanded {
                            "  [click to hide]"
                        } else {
                            "  [click to expand]"
                        },
                        Style::default()
                            .fg(theme::p().faint)
                            .add_modifier(Modifier::ITALIC),
                    ));
                }
                lines.push(with_hover(
                    banded_line(Line::from(header), Some(idx), 2),
                    idx,
                ));

                if tc.expanded && !full_args.is_empty() {
                    let pretty = serde_json::from_str::<serde_json::Value>(&full_args)
                        .ok()
                        .and_then(|value| serde_json::to_string_pretty(&value).ok())
                        .unwrap_or_else(|| full_args.clone());
                    for line in pretty.lines().take(40) {
                        lines.push(with_hover(
                            banded_line(
                                Line::from(vec![
                                    Span::styled("  · ", Style::default().fg(theme::p().faint)),
                                    Span::styled(
                                        line.to_string(),
                                        Style::default().fg(theme::p().faint),
                                    ),
                                ]),
                                Some(idx),
                                4,
                            ),
                            idx,
                        ));
                    }
                }
                if tc.expanded
                    && let Some((_, code)) = &script
                {
                    const MAX_SCRIPT_LINES: usize = 100;
                    for line in code.lines().take(MAX_SCRIPT_LINES) {
                        lines.push(with_hover(
                            banded_line(
                                Line::from(vec![
                                    Span::styled("  › ", Style::default().fg(theme::p().faint)),
                                    Span::styled(
                                        line.to_string(),
                                        Style::default().fg(theme::p().text),
                                    ),
                                ]),
                                Some(idx),
                                4,
                            ),
                            idx,
                        ));
                    }
                    if code.lines().count() > MAX_SCRIPT_LINES {
                        lines.push(with_hover(
                            banded_line(
                                Line::from(Span::styled(
                                    format!(
                                        "  … {} more script lines",
                                        code.lines().count() - MAX_SCRIPT_LINES
                                    ),
                                    Style::default().fg(theme::p().faint),
                                )),
                                Some(idx),
                                4,
                            ),
                            idx,
                        ));
                    }
                }
                if !output_lines.is_empty() {
                    if tc.expanded {
                        const MAX_EXPANDED_LINES: usize = 200;
                        for line in output_lines.iter().take(MAX_EXPANDED_LINES) {
                            lines.push(with_hover(
                                banded_line(
                                    Line::from(vec![
                                        Span::styled("  │ ", Style::default().fg(theme::p().faint)),
                                        Span::styled(
                                            (*line).to_string(),
                                            Style::default().fg(theme::p().muted),
                                        ),
                                    ]),
                                    Some(idx),
                                    4,
                                ),
                                idx,
                            ));
                        }
                        if output_lines.len() > MAX_EXPANDED_LINES {
                            lines.push(with_hover(
                                banded_line(
                                    Line::from(Span::styled(
                                        format!(
                                            "  … {} more lines",
                                            output_lines.len() - MAX_EXPANDED_LINES
                                        ),
                                        Style::default().fg(theme::p().faint),
                                    )),
                                    Some(idx),
                                    4,
                                ),
                                idx,
                            ));
                        }
                    } else {
                        let start = output_lines.len().saturating_sub(PREVIEW_LINES);
                        for (offset, line) in output_lines[start..].iter().enumerate() {
                            let gutter = if offset == 0 { "  ↳ " } else { "    " };
                            lines.push(with_hover(
                                banded_line(
                                    Line::from(vec![
                                        Span::styled(gutter, Style::default().fg(theme::p().faint)),
                                        Span::styled(
                                            truncate_single_line(line, 200),
                                            Style::default().fg(theme::p().faint),
                                        ),
                                    ]),
                                    Some(idx),
                                    4,
                                ),
                                idx,
                            ));
                        }
                    }
                }
            }
            TranscriptEntry::Subagent(sub) => {
                let (badge, badge_color) = tool_status_badge(&sub.status);
                let active = matches!(
                    sub.status,
                    ToolCallStatus::Proposed | ToolCallStatus::Running
                );
                let (lead_style, name_style, badge_style) = if active {
                    (
                        Style::default().fg(theme::p().accent),
                        Style::default()
                            .fg(theme::p().text)
                            .add_modifier(Modifier::BOLD),
                        Style::default().fg(badge_color),
                    )
                } else {
                    let badge_color = if sub.status == ToolCallStatus::Failed {
                        theme::p().error
                    } else {
                        theme::p().faint
                    };
                    (
                        Style::default().fg(theme::p().faint),
                        Style::default().fg(theme::p().muted),
                        Style::default().fg(badge_color),
                    )
                };
                let line = Line::from(vec![
                    Span::styled("▸ [subagent] ", lead_style),
                    Span::styled(sub.agent.clone(), name_style),
                    Span::styled(" ", Style::default()),
                    Span::styled(format!("[{badge}]"), badge_style),
                ]);
                lines.push(with_hover(banded_line(line, None, 2), idx));
                if let Some(report) = &sub.report {
                    let preview = truncate_single_line(report, 120);
                    if !preview.is_empty() {
                        lines.push(with_hover(
                            banded_line(
                                Line::from(vec![
                                    Span::styled("  ↳ ", Style::default().fg(theme::p().faint)),
                                    Span::styled(preview, Style::default().fg(theme::p().faint)),
                                ]),
                                None,
                                4,
                            ),
                            idx,
                        ));
                    }
                }
            }
            TranscriptEntry::System { text } => {
                for (line_idx, line) in text.split('\n').enumerate() {
                    let prefix = if line_idx == 0 {
                        Span::styled("  [system] ", Style::default().fg(theme::p().info))
                    } else {
                        Span::styled("           ", Style::default())
                    };
                    lines.push(tagged_line(
                        Line::from(vec![
                            prefix,
                            Span::styled(line.to_string(), Style::default().fg(theme::p().muted)),
                        ]),
                        None,
                        11,
                    ));
                }
            }
            TranscriptEntry::Error { text, expanded } => {
                if *expanded {
                    let line_count = text.lines().count().max(1);
                    lines.push(with_hover(
                        banded_line(
                            Line::from(Span::styled(
                                format!("  ▾ [error] ({line_count} lines)  [click to hide]"),
                                Style::default()
                                    .fg(theme::p().error)
                                    .add_modifier(Modifier::BOLD),
                            )),
                            Some(idx),
                            2,
                        ),
                        idx,
                    ));
                    for line in text.split('\n') {
                        lines.push(with_hover(
                            banded_line(
                                Line::from(vec![
                                    Span::styled("  │ ", Style::default().fg(theme::p().faint)),
                                    Span::styled(
                                        line.to_string(),
                                        Style::default().fg(theme::p().error),
                                    ),
                                ]),
                                Some(idx),
                                4,
                            ),
                            idx,
                        ));
                    }
                } else {
                    let avail_width = if app.transcript_inner_area.width > 0 {
                        app.transcript_inner_area.width as usize
                    } else {
                        (app.terminal_width as usize).saturating_sub(4).max(40)
                    };
                    let max_summary = avail_width.saturating_sub(14).max(12);
                    let summary = summarize_error_line(text, max_summary);
                    let prefix_len = 12;
                    let hint_len = 19;
                    if prefix_len + summary.chars().count() + hint_len <= avail_width {
                        lines.push(with_hover(
                            banded_line(
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
                            ),
                            idx,
                        ));
                    } else {
                        lines.push(with_hover(
                            banded_line(
                                Line::from(Span::styled(
                                    format!("  ▸ [error] {summary}"),
                                    Style::default().fg(theme::p().error),
                                )),
                                Some(idx),
                                2,
                            ),
                            idx,
                        ));
                        lines.push(with_hover(
                            banded_line(
                                Line::from(Span::styled(
                                    "    [click to expand]",
                                    Style::default()
                                        .fg(theme::p().faint)
                                        .add_modifier(Modifier::ITALIC),
                                )),
                                Some(idx),
                                4,
                            ),
                            idx,
                        ));
                    }
                }
            }
        }
    }

    lines
}

struct WrappedTranscript {
    lines: Vec<Line<'static>>,
    hit_zones: Vec<ThinkingHitZone>,
    doc_lines: Vec<TranscriptWrappedLine>,
    bands: Vec<(usize, usize)>,
    code_bands: Vec<(usize, usize)>,
    hover_zones: Vec<HoverZone>,
}

fn wrap_tagged_lines(lines: Vec<TaggedTranscriptLine>, width: usize) -> WrappedTranscript {
    if width == 0 {
        return WrappedTranscript {
            lines: Vec::new(),
            hit_zones: Vec::new(),
            doc_lines: Vec::new(),
            bands: Vec::new(),
            code_bands: Vec::new(),
            hover_zones: Vec::new(),
        };
    }

    let mut out = Vec::with_capacity(lines.len());
    let mut doc_lines = Vec::with_capacity(lines.len());
    let mut hit_zones: Vec<ThinkingHitZone> = Vec::new();
    let mut bands: Vec<(usize, usize)> = Vec::new();
    let mut code_bands: Vec<(usize, usize)> = Vec::new();
    let mut hover_zones: Vec<HoverZone> = Vec::new();
    let mut hover_active: Option<(usize, usize)> = None;
    let mut active_zone: Option<(usize, usize)> = None;

    for tagged in lines {
        let TaggedTranscriptLine {
            line,
            entry_tag,
            gutter_len,
            band,
            code_band,
            hover_tag,
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
        if code_band {
            match code_bands.last_mut() {
                Some((_, end)) if *end == line_start => *end = out.len(),
                _ => code_bands.push((line_start, out.len())),
            }
        }
        match (hover_active, hover_tag) {
            (Some((idx, start)), Some(tag)) if idx == tag => {
                hover_active = Some((idx, start));
            }
            (Some((idx, start)), _) => {
                hover_zones.push(HoverZone {
                    entry_idx: idx,
                    start_line: start,
                    end_line: line_start,
                });
                hover_active = hover_tag.map(|tag| (tag, line_start));
            }
            (None, Some(tag)) => hover_active = Some((tag, line_start)),
            (None, None) => {}
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

    if let Some((idx, start)) = hover_active {
        hover_zones.push(HoverZone {
            entry_idx: idx,
            start_line: start,
            end_line: out.len(),
        });
    }

    WrappedTranscript {
        lines: out,
        hit_zones,
        doc_lines,
        bands,
        code_bands,
        hover_zones,
    }
}
