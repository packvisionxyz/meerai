use super::statusline::{display_model, key_status_style, summarize_error_line};
use super::*;
use crate::app::ToolCallEntry;

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
            digest_hit_zones: Vec::new(),
            doc_lines: Vec::new(),
            bands: Vec::new(),
            code_bands: Vec::new(),
            hover_zones: Vec::new(),
        }
    };

    app.thinking_hit_zones = std::mem::take(&mut wrapped.hit_zones);
    app.digest_hit_zones = std::mem::take(&mut wrapped.digest_hit_zones);
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
    app.hovered_digest = app.hovered_digest_at_screen(inner, scroll_y as usize);
    let paint_hover = hovered.is_some_and(|idx| {
        matches!(
            app.tab().transcript.get(idx),
            Some(TranscriptEntry::ToolCall(_))
                | Some(TranscriptEntry::Subagent(_))
                | Some(TranscriptEntry::Error { .. })
        )
    });
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
    if let Some(range) = hover_range
        && paint_hover
    {
        paint(frame, &[range], 0, theme::p().surface_high);
    }
}

const LOGO_M_ROWS: [&str; 4] = ["██      ██", "████  ████", "██  ██  ██", "██      ██"];

fn build_header_lines(app: &AppState) -> Vec<TaggedTranscriptLine> {
    let logo_style = Style::default()
        .fg(theme::p().accent)
        .add_modifier(Modifier::BOLD);

    let mut identity = vec![
        Span::styled(LOGO_M_ROWS[0], logo_style),
        Span::raw("  "),
        Span::styled(
            "meerai",
            Style::default()
                .fg(theme::p().accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
    ];
    match app.tab().acp_preset() {
        Some(preset) => {
            identity.push(Span::styled(
                format!("{preset} (acp)"),
                Style::default().fg(theme::p().text),
            ));
            identity.push(Span::raw("  "));
            identity.push(Span::styled(
                "external agent · policy not applied",
                Style::default().fg(theme::p().warning),
            ));
        }
        None => {
            let key_status = app.active_key_status();
            let (badge, badge_color) = key_status_style(&key_status);
            identity.push(Span::styled(
                format!("{} / {}", app.tab().active_provider, display_model(app)),
                Style::default().fg(theme::p().text),
            ));
            identity.push(Span::raw(" "));
            identity.push(Span::styled(
                format!("[{badge}]"),
                Style::default().fg(badge_color),
            ));
        }
    }

    let mut lines = vec![
        tagged_line(Line::from(identity), None, 0),
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
                    format!(
                        "  Shortcuts: {} newline, {} commands, drag/shift+click copy.",
                        crate::app::ctrl_label('j'),
                        crate::app::ctrl_label('p')
                    ),
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
        lines.push(tip("ctrl+p", "Command palette"));
        lines.push(tip("shift+tab", "Cycle plan / ask / auto"));
        lines.push(tip("ctrl+f  /", "Search workspace files & content"));
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
    let language = value
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("script")
        .to_string();
    if let Some(script) = value.get("script").and_then(|v| v.as_str()) {
        return Some((language, script.to_string()));
    }
    let path = value.get("path").and_then(|v| v.as_str())?;
    Some((language, format!("(file) {path}")))
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
    commands_active: bool,
    tools: usize,
    tools_active: bool,
    reads: usize,
    reads_active: bool,
    edits: usize,
    edits_active: bool,
    agents: usize,
    agents_active: bool,
    thoughts: usize,
    thoughts_active: bool,
}

impl TurnDigest {
    fn record_tool(&mut self, name: &str, status: &ToolCallStatus) {
        let active = matches!(status, ToolCallStatus::Proposed | ToolCallStatus::Running);
        match name {
            "code-shell" | "sac-run_script" => {
                self.commands += 1;
                self.commands_active |= active;
            }
            "code-read_file" => {
                self.reads += 1;
                self.reads_active |= active;
            }
            "code-write_file" | "code-edit_file" => {
                self.edits += 1;
                self.edits_active |= active;
            }
            "subagent-run" | "acp-run" => {
                self.agents += 1;
                self.agents_active |= active;
            }
            _ => {
                self.tools += 1;
                self.tools_active |= active;
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.commands + self.tools + self.reads + self.edits + self.agents + self.thoughts == 0
    }

    fn summary(&self) -> String {
        fn part(
            count: usize,
            active: bool,
            gerund: &str,
            singular: &str,
            plural: &str,
        ) -> Option<String> {
            match count {
                0 => None,
                n if active => Some(format!("{gerund} {n}")),
                1 => Some(format!("1 {singular}")),
                n => Some(format!("{n} {plural}")),
            }
        }
        [
            part(
                self.commands,
                self.commands_active,
                "Running",
                "command",
                "commands",
            ),
            part(self.tools, self.tools_active, "Calling", "tool", "tools"),
            part(self.reads, self.reads_active, "Reading", "read", "reads"),
            part(self.edits, self.edits_active, "Editing", "edit", "edits"),
            part(
                self.agents,
                self.agents_active,
                "Delegating",
                "agent",
                "agents",
            ),
            part(
                self.thoughts,
                self.thoughts_active,
                "Thinking",
                "thought",
                "thoughts",
            ),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
    }
}

fn turn_digests(transcript: &[TranscriptEntry]) -> Vec<Option<TurnDigest>> {
    let mut digests: Vec<Option<TurnDigest>> = (0..transcript.len()).map(|_| None).collect();
    let mut current: Option<usize> = None;
    let mut digest = TurnDigest::default();

    for (idx, entry) in transcript.iter().enumerate() {
        if matches!(entry, TranscriptEntry::User { .. }) {
            if let Some(start) = current {
                digests[start] = Some(std::mem::take(&mut digest));
            }
            current = Some(idx);
            digest = TurnDigest::default();
        }
        match entry {
            TranscriptEntry::Assistant {
                reasoning,
                streaming,
                ..
            } if !reasoning.is_empty() => {
                digest.thoughts += 1;
                digest.thoughts_active |= *streaming;
            }
            TranscriptEntry::ToolCall(call) => digest.record_tool(&call.name, &call.status),
            TranscriptEntry::Subagent(sub) => {
                digest.agents += 1;
                digest.agents_active |= matches!(
                    sub.status,
                    ToolCallStatus::Proposed | ToolCallStatus::Running
                );
            }
            _ => {}
        }
    }

    if let Some(start) = current {
        digests[start] = Some(digest);
    }
    digests
}

fn digest_line(digest: &TurnDigest, expanded: bool, hovered: bool) -> Option<Line<'static>> {
    if digest.is_empty() {
        return None;
    }
    let text_style = if hovered {
        Style::default().fg(theme::p().text)
    } else {
        Style::default().fg(theme::p().faint)
    };
    Some(Line::from(vec![
        Span::styled(
            if expanded { "- " } else { "+ " },
            Style::default().fg(theme::p().faint),
        ),
        Span::styled(digest.summary(), text_style),
    ]))
}

fn build_transcript_lines(app: &AppState) -> Vec<TaggedTranscriptLine> {
    let mut lines = build_header_lines(app);
    let tab = app.tab();
    let transcript = &tab.transcript;
    if transcript.is_empty() {
        return lines;
    }

    let digests = turn_digests(transcript);

    lines.push(tagged_line(Line::default(), None, 0));
    let mut current_turn: Option<usize> = None;
    for (idx, entry) in transcript.iter().enumerate() {
        if matches!(entry, TranscriptEntry::User { .. }) {
            current_turn = Some(idx);
        }
        let collapsed = current_turn
            .map(|turn| !tab.turn_activity.get(&turn).copied().unwrap_or(false))
            .unwrap_or(false);
        let hidden = collapsed
            && match entry {
                TranscriptEntry::ToolCall(_) | TranscriptEntry::Subagent(_) => true,
                TranscriptEntry::Assistant {
                    text, reasoning, ..
                } => text.is_empty() && !reasoning.is_empty(),
                _ => false,
            };
        if hidden {
            continue;
        }
        if idx > 0 {
            lines.push(tagged_line(Line::default(), None, 0));
        }
        match entry {
            TranscriptEntry::User { text } => {
                for line in text.split('\n') {
                    lines.push(banded_line(
                        Line::from(vec![
                            Span::styled("▌ ", Style::default().fg(theme::p().accent)),
                            Span::styled(line.to_string(), Style::default().fg(theme::p().text)),
                        ]),
                        Some(idx),
                        2,
                    ));
                }
                if let Some(digest) = digests.get(idx).and_then(|d| d.as_ref()) {
                    let expanded = tab.turn_activity.get(&idx).copied().unwrap_or(false);
                    let hovered = app.hovered_digest == Some(idx);
                    if let Some(line) = digest_line(digest, expanded, hovered) {
                        lines.push(with_digest(tagged_line(line, None, 2), idx));
                    }
                }
            }
            TranscriptEntry::Assistant {
                text,
                reasoning,
                streaming,
                reasoning_expanded,
            } => {
                if !reasoning.is_empty() && !collapsed {
                    let line_count = reasoning.lines().count().max(1);
                    let label = if *streaming { "Thinking" } else { "Thought" };
                    let count = if line_count == 1 {
                        "1 line".to_string()
                    } else {
                        format!("{line_count} lines")
                    };
                    let header_style = if app.hovered_entry == Some(idx) {
                        Style::default()
                            .fg(theme::p().text)
                            .add_modifier(Modifier::ITALIC)
                    } else {
                        Style::default()
                            .fg(theme::p().muted)
                            .add_modifier(Modifier::ITALIC)
                    };
                    if *reasoning_expanded {
                        lines.push(with_hover(
                            tagged_line(
                                Line::from(Span::styled(
                                    format!("- {label} ({count})"),
                                    header_style,
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
                                    format!("+ {label} ({count})"),
                                    header_style,
                                )),
                                Some(idx),
                                2,
                            ),
                            idx,
                        ));
                    }
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
                let active = matches!(
                    tc.status,
                    ToolCallStatus::Proposed | ToolCallStatus::Running
                );
                let diff = edit_diff(tc);
                let (added, removed) = diff
                    .as_deref()
                    .map(crate::diff::diff_counts)
                    .unwrap_or((0, 0));
                let (gerund, past) = activity_verbs(&tc.name);
                let verb = if active { gerund } else { past };
                let target = tool_target(&tc.name, &tc.args, &script);
                let marker = if active {
                    let ch = SPINNER_FRAMES[app.spinner_tick % SPINNER_FRAMES.len()];
                    format!("{ch} ")
                } else if tc.expanded {
                    "- ".to_string()
                } else {
                    "+ ".to_string()
                };
                let (marker_style, name_style, badge_style, args_style) = if active {
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

                let mut header = vec![
                    Span::styled(marker, marker_style),
                    Span::styled(verb.to_string(), name_style),
                ];
                if !target.is_empty() {
                    header.push(Span::styled(" ", Style::default()));
                    header.push(Span::styled(target, args_style));
                }
                header.push(Span::styled(" ", Style::default()));
                header.push(Span::styled(format!("[{badge}]"), badge_style));
                if diff.is_some() {
                    header.push(Span::styled(
                        format!(" +{added} -{removed}"),
                        Style::default().fg(theme::p().faint),
                    ));
                }
                lines.push(with_hover(
                    banded_line(Line::from(header), Some(idx), 2),
                    idx,
                ));

                if tc.expanded
                    && let Some(diff) = &diff
                {
                    let language = tool_path(tc)
                        .as_deref()
                        .and_then(crate::syntax::language_for_path);
                    render_edit_diff(&mut lines, diff, language, idx);
                }

                if tc.expanded && diff.is_none() && !full_args.is_empty() {
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
                    && diff.is_none()
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
                if diff.is_none() && !output_lines.is_empty() {
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
                let lead = if sub.expanded {
                    "- [subagent] "
                } else {
                    "+ [subagent] "
                };
                let header = vec![
                    Span::styled(lead, lead_style),
                    Span::styled(sub.agent.clone(), name_style),
                    Span::styled(" ", Style::default()),
                    Span::styled(format!("[{badge}]"), badge_style),
                ];
                lines.push(with_hover(
                    banded_line(Line::from(header), Some(idx), 2),
                    idx,
                ));
                if sub.expanded {
                    match &sub.nested {
                        Some(nested) => render_nested_conversation(&mut lines, nested, idx),
                        None => {
                            let label = if sub.nested_loading {
                                "loading conversation…"
                            } else {
                                "conversation unavailable"
                            };
                            lines.push(nested_line(idx, label.to_string(), true));
                        }
                    }
                } else if let Some(report) = &sub.report {
                    let preview = truncate_single_line(report, 120);
                    if !preview.is_empty() {
                        lines.push(with_hover(
                            banded_line(
                                Line::from(vec![
                                    Span::styled("  ↳ ", Style::default().fg(theme::p().faint)),
                                    Span::styled(preview, Style::default().fg(theme::p().faint)),
                                ]),
                                Some(idx),
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
                                format!("- [error] ({line_count} lines)"),
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
                    lines.push(with_hover(
                        banded_line(
                            Line::from(Span::styled(
                                format!("+ [error] {summary}"),
                                Style::default().fg(theme::p().error),
                            )),
                            Some(idx),
                            2,
                        ),
                        idx,
                    ));
                }
            }
        }
    }

    lines
}

fn activity_verbs(name: &str) -> (&'static str, &'static str) {
    match name {
        "code-read_file" => ("Reading", "Read"),
        "code-write_file" | "code-edit_file" => ("Editing", "Edited"),
        "code-shell" => ("Running", "Ran"),
        "sac-run_script" => ("Running script", "Ran script"),
        "code-grep" => ("Searching", "Searched"),
        "code-glob" => ("Matching", "Matched"),
        "code-list_dir" => ("Listing", "Listed"),
        "retrieval-repo_map" => ("Mapping", "Mapped"),
        "question-ask" => ("Asking", "Asked"),
        name if name.starts_with("lsp-") => ("Inspecting", "Inspected"),
        _ => ("Using", "Used"),
    }
}

fn tool_target(name: &str, args: &str, script: &Option<(String, String)>) -> String {
    if let Some((language, code)) = script {
        let first = code.lines().next().unwrap_or("").trim();
        return truncate_single_line(&format!("{language} · {first}"), 60);
    }
    let value = serde_json::from_str::<serde_json::Value>(args).ok();
    let field = |keys: &[&str]| -> Option<String> {
        let value = value.as_ref()?;
        keys.iter().find_map(|key| {
            value
                .get(*key)
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
    };
    let target = match name {
        "code-read_file" | "code-write_file" | "code-edit_file" => {
            field(&["path", "file", "file_path"])
        }
        "code-list_dir" => field(&["path", "dir"]),
        "code-grep" => field(&["query", "pattern", "regex"]),
        "code-glob" => field(&["query", "pattern", "glob"]),
        "code-shell" => field(&["command", "cmd"]),
        _ => None,
    };
    match target {
        Some(target) if !target.trim().is_empty() => truncate_single_line(target.trim(), 60),
        _ => truncate_single_line(args.trim(), 60),
    }
}

fn tool_path(tc: &ToolCallEntry) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(&tc.args)
        .ok()
        .and_then(|value| {
            ["path", "file", "file_path"].iter().find_map(|key| {
                value
                    .get(*key)
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            })
        })
        .or_else(|| {
            tc.output.as_deref().and_then(|output| {
                output.lines().find_map(|line| {
                    line.strip_prefix("+++ b/")
                        .or_else(|| line.strip_prefix("+++ "))
                        .map(str::to_string)
                })
            })
        })
}

fn edit_diff(tc: &ToolCallEntry) -> Option<Vec<crate::diff::UnifiedLine>> {
    if !matches!(tc.name.as_str(), "code-write_file" | "code-edit_file") {
        return None;
    }
    if let Some(output) = tc.output.as_deref() {
        let parsed = crate::diff::parse_unified_diff(output);
        if !parsed.is_empty() {
            return Some(parsed);
        }
    }
    let value = serde_json::from_str::<serde_json::Value>(&tc.args).ok()?;
    let path = value.get("path").and_then(|v| v.as_str())?;
    let old = value
        .get("old_string")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let new = value
        .get("new_string")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("content").and_then(|v| v.as_str()))?;
    let lines = crate::diff::build_edit_diff(path, old, new);
    (!lines.is_empty()).then_some(lines)
}

fn render_edit_diff(
    lines: &mut Vec<TaggedTranscriptLine>,
    diff: &[crate::diff::UnifiedLine],
    language: Option<crate::syntax::Language>,
    idx: usize,
) {
    use crate::diff::UnifiedLineKind;
    const MAX_DIFF_LINES: usize = 200;
    let body: Vec<&crate::diff::UnifiedLine> = diff
        .iter()
        .filter(|line| line.kind != UnifiedLineKind::FileHeader)
        .collect();
    let visible = body.len().min(MAX_DIFF_LINES);
    let source = body
        .iter()
        .take(visible)
        .map(|line| {
            if line.kind == UnifiedLineKind::HunkHeader {
                ""
            } else {
                line.text.as_str()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let tokens = crate::syntax::token_lines(language, &source);

    for (position, line) in body.iter().take(visible).enumerate() {
        let line_tokens = tokens.get(position).map(Vec::as_slice).unwrap_or(&[]);
        let tagged = match line.kind {
            UnifiedLineKind::HunkHeader => with_hover(
                banded_line(
                    Line::from(vec![
                        Span::styled("  ", Style::default()),
                        Span::styled(line.text.clone(), Style::default().fg(theme::p().faint)),
                    ]),
                    Some(idx),
                    4,
                ),
                idx,
            ),
            UnifiedLineKind::Context => {
                code_diff_line(idx, " ", theme::p().diff_context, line_tokens, &line.text)
            }
            UnifiedLineKind::Added => {
                code_diff_line(idx, "+", theme::p().diff_added, line_tokens, &line.text)
            }
            UnifiedLineKind::Removed => {
                code_diff_line(idx, "-", theme::p().diff_removed, line_tokens, &line.text)
            }
            UnifiedLineKind::FileHeader => continue,
        };
        lines.push(tagged);
    }

    if body.len() > visible {
        lines.push(nested_line(idx, "…".to_string(), true));
    }
}

fn code_diff_line(
    idx: usize,
    sign: &str,
    color: Color,
    tokens: &[crate::syntax::Token],
    text: &str,
) -> TaggedTranscriptLine {
    let base = Style::default().fg(color);
    let mut spans = vec![
        Span::styled("  ", Style::default()),
        Span::styled(format!("{sign} "), Style::default().fg(color)),
        Span::styled("  ", Style::default()),
    ];
    if tokens.is_empty() {
        spans.push(Span::styled(text.to_string(), base));
    } else {
        spans.extend(crate::syntax::line_spans(tokens, base));
    }
    with_hover(banded_line(Line::from(spans), Some(idx), 4), idx)
}

fn nested_line(parent_idx: usize, text: String, faint: bool) -> TaggedTranscriptLine {
    let color = if faint {
        theme::p().faint
    } else {
        theme::p().muted
    };
    with_hover(
        banded_line(
            Line::from(vec![
                Span::styled("  │ ", Style::default().fg(theme::p().faint)),
                Span::styled(text, Style::default().fg(color)),
            ]),
            Some(parent_idx),
            4,
        ),
        parent_idx,
    )
}

fn render_nested_conversation(
    lines: &mut Vec<TaggedTranscriptLine>,
    nested: &[TranscriptEntry],
    parent_idx: usize,
) {
    const MAX_ENTRIES: usize = 60;
    const MAX_TEXT_LINES: usize = 12;
    let start = nested.len().saturating_sub(MAX_ENTRIES);
    if start > 0 {
        lines.push(nested_line(
            parent_idx,
            format!("… {start} earlier entries"),
            true,
        ));
    }
    for entry in &nested[start..] {
        match entry {
            TranscriptEntry::User { text } => {
                let first = text.lines().next().unwrap_or("");
                lines.push(nested_line(
                    parent_idx,
                    format!("you: {}", truncate_single_line(first, 100)),
                    false,
                ));
            }
            TranscriptEntry::Assistant {
                text, reasoning, ..
            } => {
                if !reasoning.is_empty() {
                    let count = reasoning.lines().count().max(1);
                    let label = if count == 1 {
                        "1 line".to_string()
                    } else {
                        format!("{count} lines")
                    };
                    lines.push(nested_line(parent_idx, format!("thought ({label})"), true));
                }
                let total = text.lines().count();
                for (index, line) in text.lines().enumerate() {
                    if index >= MAX_TEXT_LINES {
                        lines.push(nested_line(
                            parent_idx,
                            format!("… {} more lines", total - MAX_TEXT_LINES),
                            true,
                        ));
                        break;
                    }
                    lines.push(nested_line(parent_idx, line.to_string(), false));
                }
            }
            TranscriptEntry::ToolCall(tc) => {
                let (badge, _) = tool_status_badge(&tc.status);
                let preview = truncate_single_line(&tc.args, 60);
                lines.push(nested_line(
                    parent_idx,
                    format!("[tool] {} [{badge}] {preview}", tc.name),
                    false,
                ));
                if let Some(output) = &tc.output {
                    let output_lines: Vec<&str> = output.lines().collect();
                    let tail = &output_lines[output_lines.len().saturating_sub(2)..];
                    for line in tail {
                        lines.push(nested_line(
                            parent_idx,
                            format!("  ↳ {}", truncate_single_line(line, 100)),
                            true,
                        ));
                    }
                }
            }
            TranscriptEntry::Subagent(sub) => {
                let (badge, _) = tool_status_badge(&sub.status);
                lines.push(nested_line(
                    parent_idx,
                    format!("[subagent] {} [{badge}]", sub.agent),
                    false,
                ));
            }
            TranscriptEntry::System { text } => {
                lines.push(nested_line(
                    parent_idx,
                    format!("[system] {}", truncate_single_line(text, 100)),
                    true,
                ));
            }
            TranscriptEntry::Error { text, .. } => {
                lines.push(nested_line(
                    parent_idx,
                    format!("[error] {}", summarize_error_line(text, 100)),
                    false,
                ));
            }
        }
    }
}

struct WrappedTranscript {
    lines: Vec<Line<'static>>,
    hit_zones: Vec<ThinkingHitZone>,
    digest_hit_zones: Vec<ThinkingHitZone>,
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
            digest_hit_zones: Vec::new(),
            doc_lines: Vec::new(),
            bands: Vec::new(),
            code_bands: Vec::new(),
            hover_zones: Vec::new(),
        };
    }

    let mut out = Vec::with_capacity(lines.len());
    let mut doc_lines = Vec::with_capacity(lines.len());
    let mut hit_zones: Vec<ThinkingHitZone> = Vec::new();
    let mut digest_hit_zones: Vec<ThinkingHitZone> = Vec::new();
    let mut bands: Vec<(usize, usize)> = Vec::new();
    let mut code_bands: Vec<(usize, usize)> = Vec::new();
    let mut hover_zones: Vec<HoverZone> = Vec::new();
    let mut hover_active: Option<(usize, usize)> = None;
    let mut active_zone: Option<(usize, usize)> = None;
    let mut active_digest: Option<(usize, usize)> = None;

    for tagged in lines {
        let TaggedTranscriptLine {
            line,
            entry_tag,
            gutter_len,
            band,
            code_band,
            hover_tag,
            digest_tag,
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
        match (active_digest, digest_tag) {
            (Some((cur_idx, _start)), Some(idx)) if cur_idx == idx => {}
            (Some((cur_idx, start)), Some(idx)) => {
                digest_hit_zones.push(ThinkingHitZone {
                    entry_idx: cur_idx,
                    start_line: start,
                    end_line: line_start,
                });
                active_digest = Some((idx, line_start));
            }
            (Some((cur_idx, start)), None) => {
                digest_hit_zones.push(ThinkingHitZone {
                    entry_idx: cur_idx,
                    start_line: start,
                    end_line: line_start,
                });
                active_digest = None;
            }
            (None, Some(idx)) => {
                active_digest = Some((idx, line_start));
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

    if let Some((cur_idx, start)) = active_digest {
        digest_hit_zones.push(ThinkingHitZone {
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
        digest_hit_zones,
        doc_lines,
        bands,
        code_bands,
        hover_zones,
    }
}
