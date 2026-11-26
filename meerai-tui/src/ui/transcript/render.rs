use super::digest::*;
use super::tools::*;
use super::wrap::*;
use super::*;

pub(super) fn push_user_mention_spans(
    spans: &mut Vec<Span<'static>>,
    line: &str,
    agents: &[AgentDefinition],
    skills: &[SkillDefinition],
) {
    let plain_style = Style::default().fg(theme::p().text);
    let mut plain_start = 0;
    let mut index = 0;
    while index < line.len() {
        let Some(ch) = line[index..].chars().next() else {
            break;
        };
        if ch == '@'
            && let Some((kind, id, end)) = parse_mention_token(line, index)
            && mention_is_known(kind, id, agents, skills)
        {
            if plain_start < index {
                spans.push(Span::styled(
                    line[plain_start..index].to_string(),
                    plain_style,
                ));
            }
            spans.push(Span::styled(id.to_string(), user_mention_chip_style(kind)));
            index = end;
            plain_start = end;
            continue;
        }
        index += ch.len_utf8();
    }
    if plain_start < line.len() {
        spans.push(Span::styled(line[plain_start..].to_string(), plain_style));
    }
}

pub(super) fn user_mention_chip_style(kind: MentionKind) -> Style {
    let color = match kind {
        MentionKind::Agent => theme::p().accent,
        MentionKind::Skill => theme::p().warning,
    };
    Style::default()
        .fg(color)
        .bg(theme::p().surface_high)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn render_entry_lines(
    app: &AppState,
    memo: &mut TranscriptBuildMemo,
    lines: &mut Vec<TaggedTranscriptLine>,
    entry: &TranscriptEntry,
    idx: usize,
    width: usize,
) {
    match entry {
        TranscriptEntry::User { text } => {
            for line in text.split('\n') {
                let mut spans = vec![Span::styled("▌ ", Style::default().fg(theme::p().accent))];
                push_user_mention_spans(&mut spans, line, &app.agents, &app.skills);
                lines.push(with_hang(
                    banded_line(Line::from(spans), Some(idx), 2),
                    "  ".to_string(),
                ));
            }
        }
        TranscriptEntry::Assistant { text, .. } => {
            if !text.is_empty() {
                let mut in_code_block = false;
                let rendered = memo.markdown(idx, width, text);
                for line in rendered.iter() {
                    if line.code && !in_code_block {
                        lines.push(code_line(tagged_line(Line::default(), None, 4)));
                        in_code_block = true;
                    } else if !line.code && in_code_block {
                        lines.push(code_line(tagged_line(Line::default(), None, 4)));
                        in_code_block = false;
                    }
                    let prefix = if line.code { "    " } else { "  " };
                    let gutter = if line.code { 4 } else { 2 };
                    let hang = format!("{prefix}{}", line.hang);
                    let mut spans = vec![Span::styled(prefix, Style::default())];
                    spans.extend(line.spans.iter().cloned());
                    let tagged = with_hang(tagged_line(Line::from(spans), None, gutter), hang);
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
            let diff = memo.edit_diff(idx, tc, || edit_diff(tc));
            let diff = diff.as_ref().as_ref().map(Vec::as_slice);
            let (added, removed) = diff.map(crate::diff::diff_counts).unwrap_or((0, 0));
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
                && let Some(diff) = diff
            {
                let language = tool_path(tc)
                    .as_deref()
                    .and_then(crate::syntax::language_for_path);
                render_edit_diff(lines, diff, language, idx, memo);
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
            let mut header = vec![
                Span::styled(lead, lead_style),
                Span::styled(sub.display_name().to_string(), name_style),
            ];
            if sub.named().is_some() {
                header.push(Span::styled(
                    format!(" ({})", sub.agent),
                    Style::default().fg(theme::p().faint),
                ));
            }
            header.push(Span::styled(" ", Style::default()));
            header.push(Span::styled(format!("[{badge}]"), badge_style));
            lines.push(with_hover(
                banded_line(Line::from(header), Some(idx), 2),
                idx,
            ));
            if sub.expanded {
                match &sub.nested {
                    Some(nested) => render_nested_conversation(lines, nested, idx, 12),
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
        TranscriptEntry::Question {
            questions,
            answers,
            resolved,
            ..
        } => {
            for (q_idx, prompt) in questions.iter().enumerate() {
                let header = if questions.len() > 1 {
                    format!("? {}/{} {}", q_idx + 1, questions.len(), prompt.header)
                } else {
                    format!("? {}", prompt.header)
                };
                lines.push(banded_line(
                    Line::from(Span::styled(
                        header,
                        Style::default()
                            .fg(theme::p().accent)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Some(idx),
                    2,
                ));
                lines.push(tagged_line(
                    Line::from(vec![
                        Span::styled("  ", Style::default()),
                        Span::styled(
                            prompt.question.clone(),
                            Style::default().fg(theme::p().text),
                        ),
                    ]),
                    None,
                    2,
                ));
                let selected = answers
                    .as_ref()
                    .and_then(|answers| answers.get(q_idx))
                    .map(|answer| answer.selected.as_slice())
                    .unwrap_or_default();
                if selected.is_empty() {
                    let label = if *resolved {
                        "(cancelled)"
                    } else {
                        "(waiting for answer)"
                    };
                    lines.push(tagged_line(
                        Line::from(Span::styled(
                            format!("  {label}"),
                            Style::default().fg(theme::p().muted),
                        )),
                        None,
                        2,
                    ));
                } else {
                    for label in selected {
                        lines.push(tagged_line(
                            Line::from(vec![
                                Span::styled("  ✓ ", Style::default().fg(theme::p().success)),
                                Span::styled(label.clone(), Style::default().fg(theme::p().text)),
                            ]),
                            None,
                            2,
                        ));
                    }
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

pub(super) fn render_reasoning_lines(
    lines: &mut Vec<TaggedTranscriptLine>,
    hover_text_lines: &mut Vec<(usize, usize)>,
    reasoning: &str,
    streaming: bool,
    expanded: bool,
    idx: usize,
) {
    let line_count = reasoning.lines().count().max(1);
    let label = if streaming { "Thinking" } else { "Thought" };
    let count = if line_count == 1 {
        "1 line".to_string()
    } else {
        format!("{line_count} lines")
    };
    let header_style = Style::default()
        .fg(theme::p().muted)
        .add_modifier(Modifier::ITALIC);
    if expanded {
        hover_text_lines.push((lines.len(), idx));
        lines.push(with_hover(
            tagged_line(
                Line::from(Span::styled(format!("- {label} ({count})"), header_style)),
                Some(idx),
                2,
            ),
            idx,
        ));
        for line in reasoning.split('\n') {
            lines.push(with_hover(
                with_hang(
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
                    "  │ ".to_string(),
                ),
                idx,
            ));
        }
    } else {
        hover_text_lines.push((lines.len(), idx));
        lines.push(with_hover(
            tagged_line(
                Line::from(Span::styled(format!("+ {label} ({count})"), header_style)),
                Some(idx),
                2,
            ),
            idx,
        ));
    }
}

pub(crate) fn subagent_view_lines(
    app: &mut AppState,
    entry_idx: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let key = (
        app.tab().tab_id,
        entry_idx,
        app.tab().transcript_version,
        width,
    );
    if app.subagent_view_cache.key == Some(key) {
        return app.subagent_view_cache.lines.clone();
    }
    let built = {
        let tab = app.tab();
        let Some(TranscriptEntry::Subagent(subagent)) = tab.transcript.get(entry_idx) else {
            return Vec::new();
        };
        let nested = subagent.nested.as_deref().unwrap_or(&[]);
        let mut memo = TranscriptBuildMemo::default();
        memo.begin(u64::MAX, theme::p(), nested.len());
        let (badge, badge_color) = tool_status_badge(&subagent.status);
        let name = match subagent.named() {
            Some(name) => format!("{name} ({})", subagent.agent),
            None => subagent.agent.clone(),
        };
        let usage = &subagent.usage;
        let mut lines = vec![
            tagged_line(
                Line::from(vec![
                    Span::styled(
                        name,
                        Style::default()
                            .fg(theme::p().text)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" ", Style::default()),
                    Span::styled(format!("[{badge}]"), Style::default().fg(badge_color)),
                    Span::styled(
                        format!(
                            " · {} · {}",
                            subagent.model.as_deref().unwrap_or("parent model"),
                            subagent.approval_mode
                        ),
                        Style::default().fg(theme::p().muted),
                    ),
                ]),
                None,
                0,
            ),
            tagged_line(
                Line::from(Span::styled(
                    format!(
                        "{}/{} steps · ↑{} ↓{} · {}",
                        subagent.steps_used,
                        subagent.max_steps,
                        usage.prompt_tokens.unwrap_or(0),
                        usage.completion_tokens.unwrap_or(0),
                        subagent.active_tool.as_deref().unwrap_or("idle")
                    ),
                    Style::default().fg(theme::p().faint),
                )),
                None,
                0,
            ),
            tagged_line(Line::default(), None, 0),
        ];
        let mut hover_text_lines = Vec::new();
        for (idx, entry) in nested.iter().enumerate() {
            if idx > 0 {
                lines.push(tagged_line(Line::default(), None, 0));
            }
            if let TranscriptEntry::Assistant {
                reasoning,
                streaming,
                ..
            } = entry
                && !reasoning.is_empty()
            {
                render_reasoning_lines(
                    &mut lines,
                    &mut hover_text_lines,
                    reasoning,
                    *streaming,
                    true,
                    idx,
                );
            }
            render_entry_lines(app, &mut memo, &mut lines, entry, idx, width as usize);
        }
        if nested.is_empty() {
            let label = if subagent.nested_loading {
                "loading conversation…"
            } else {
                "This agent has no conversation yet."
            };
            lines.push(tagged_line(
                Line::from(Span::styled(label, Style::default().fg(theme::p().faint))),
                None,
                0,
            ));
        }
        wrap_tagged_lines(
            BuiltTranscript {
                lines,
                hover_text_lines,
            },
            width as usize,
        )
    };
    app.subagent_view_cache = SubagentViewCache {
        key: Some(key),
        lines: built.lines,
        doc_lines: built.doc_lines,
        bands: built.bands,
        code_bands: built.code_bands,
        body: Rect::default(),
        scroll: 0,
        selection: None,
        anchor: None,
    };
    app.subagent_view_cache.lines.clone()
}

pub(super) fn nested_line(parent_idx: usize, text: String, faint: bool) -> TaggedTranscriptLine {
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

pub(super) fn render_nested_conversation(
    lines: &mut Vec<TaggedTranscriptLine>,
    nested: &[TranscriptEntry],
    parent_idx: usize,
    max_text_lines: usize,
) {
    const MAX_ENTRIES: usize = 60;
    let max_text_lines = max_text_lines.max(1);
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
                    if index >= max_text_lines {
                        lines.push(nested_line(
                            parent_idx,
                            format!("… {} more lines", total - max_text_lines),
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
            TranscriptEntry::Question {
                questions, answers, ..
            } => {
                for (q_idx, prompt) in questions.iter().enumerate() {
                    lines.push(nested_line(
                        parent_idx,
                        format!("? {}", prompt.header),
                        true,
                    ));
                    lines.push(nested_line(parent_idx, prompt.question.clone(), false));
                    if let Some(answer) = answers.as_ref().and_then(|answers| answers.get(q_idx)) {
                        for label in &answer.selected {
                            lines.push(nested_line(parent_idx, format!("✓ {label}"), false));
                        }
                    }
                }
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
