use super::statusline::{display_model, key_status_style, summarize_error_line};
use super::*;
use crate::app::{
    SubagentViewCache, ToolCallEntry, TranscriptAnchor, TranscriptBuildMemo, TranscriptDocument,
    TranscriptRenderKey, char_width, display_width,
};

pub(super) fn render_transcript(frame: &mut Frame<'_>, app: &mut AppState, area: Rect) {
    let inner = Rect {
        x: area.x.saturating_add(2),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(2),
    };
    app.transcript_inner_area = inner;

    if inner.width == 0 {
        app.transcript_cache.clear();
        app.thinking_hit_zones.clear();
        app.digest_hit_zones.clear();
        app.transcript_anchors.clear();
        app.anchor_rail_hover = None;
        app.transcript_wrapped_lines.clear();
        app.hover_zones.clear();
    } else if !app.transcript_cache.matches(app, inner.width) {
        rebuild_transcript_document(app, inner.width);
    }

    let line_count = app.transcript_cache.document.line_count;
    let scroll_y = {
        let tab = app.tab_mut();
        tab.viewport_height = inner.height as usize;
        tab.total_lines = line_count;
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
    let hover_range = hovered.and_then(|idx| {
        app.hover_zones
            .iter()
            .find(|zone| zone.entry_idx == idx)
            .map(|zone| (zone.start_line, zone.end_line))
    });

    let scrolled =
        std::mem::take(&mut app.transcript_cache.document.paragraph).scroll((scroll_y, 0));
    frame.render_widget(&scrolled, inner);
    app.transcript_cache.document.paragraph = scrolled;

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

    let rows = VisibleRows {
        inner,
        first: first_visible,
        last: last_visible,
    };

    let anchor_hover_range = anchor_rail_hover_range(app);

    let document = &app.transcript_cache.document;
    paint(frame, &document.code_bands, 2, theme::p().surface_high);
    paint(frame, &document.bands, 0, theme::p().surface);
    if let Some(range) = anchor_hover_range {
        paint(frame, &[range], 0, theme::p().surface_high);
    }
    if let Some(range) = hover_range
        && paint_hover
    {
        paint(frame, &[range], 0, theme::p().surface_high);
    }

    if let Some(idx) = app.hovered_digest
        && let Some(zone) = app
            .digest_hit_zones
            .iter()
            .find(|zone| zone.entry_idx == idx)
    {
        paint_text(
            frame,
            rows,
            zone.start_line,
            zone.end_line,
            2,
            theme::p().text,
        );
    }
    if let Some(idx) = app.hovered_entry
        && let Some(zone) = document
            .hover_text_zones
            .iter()
            .find(|zone| zone.entry_idx == idx)
    {
        paint_text(
            frame,
            rows,
            zone.start_line,
            zone.end_line,
            0,
            theme::p().text,
        );
    }
}

fn anchor_rail_hover_range(app: &AppState) -> Option<(usize, usize)> {
    let row = app.anchor_rail_hover?;
    let mark = app
        .anchor_rail_layout(app.anchor_rail_area)
        .marks
        .into_iter()
        .find(|mark| mark.row == row)?;
    let zone = app
        .thinking_hit_zones
        .iter()
        .find(|zone| zone.entry_idx == mark.entry_idx)?;
    Some((zone.start_line, zone.end_line))
}

fn rebuild_transcript_document(app: &mut AppState, width: u16) {
    let key = TranscriptRenderKey::capture(app, width);
    let mut memo = app.transcript_cache.take_memo();
    let built = build_transcript_lines(app, &mut memo, width as usize);
    let wrapped = wrap_tagged_lines(built, width as usize);
    let line_count = wrapped.lines.len();
    let mut anchors: Vec<TranscriptAnchor> = {
        let transcript = &app.tab().transcript;
        wrapped
            .hit_zones
            .iter()
            .filter(|zone| {
                matches!(
                    transcript.get(zone.entry_idx),
                    Some(TranscriptEntry::User { .. })
                )
            })
            .map(|zone| TranscriptAnchor {
                entry_idx: zone.entry_idx,
                line: zone.start_line,
                number: 0,
            })
            .collect()
    };
    for (index, anchor) in anchors.iter_mut().enumerate() {
        anchor.number = index + 1;
    }
    app.transcript_anchors = anchors;
    app.thinking_hit_zones = wrapped.hit_zones;
    app.digest_hit_zones = wrapped.digest_hit_zones;
    app.transcript_wrapped_lines = wrapped.doc_lines;
    app.hover_zones = wrapped.hover_zones;
    app.transcript_cache.store(
        key,
        memo,
        TranscriptDocument {
            paragraph: Paragraph::new(wrapped.lines),
            line_count,
            bands: wrapped.bands,
            code_bands: wrapped.code_bands,
            hover_text_zones: wrapped.hover_text_zones,
        },
    );
}

#[derive(Clone, Copy)]
struct VisibleRows {
    inner: Rect,
    first: usize,
    last: usize,
}

fn paint_text(
    frame: &mut Frame<'_>,
    rows: VisibleRows,
    start_line: usize,
    end_line: usize,
    first_row_offset: u16,
    color: Color,
) {
    for row in start_line.max(rows.first)..end_line.min(rows.last) {
        let offset = if row == start_line {
            first_row_offset
        } else {
            0
        };
        let y = rows.inner.y + (row - rows.first) as u16;
        for col in rows.inner.x.saturating_add(offset)..rows.inner.x + rows.inner.width {
            if let Some(cell) = frame.buffer_mut().cell_mut((col, y)) {
                cell.set_fg(color);
            }
        }
    }
}

fn build_header_lines(app: &AppState) -> Vec<TaggedTranscriptLine> {
    let mut identity = crate::logo::m_spans(0);
    identity.extend([
        Span::raw("  "),
        Span::styled(
            "meerai",
            Style::default()
                .fg(theme::p().accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
    ]);
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

    let logo_line = |row: usize, suffix: Span<'static>| {
        let mut spans = crate::logo::m_spans(row);
        spans.push(suffix);
        tagged_line(Line::from(spans), None, 0)
    };

    let lines = vec![
        tagged_line(Line::from(identity), None, 0),
        logo_line(
            1,
            Span::styled(
                "  Commands: /sessions, /new, /connect, /models, /help, @ (files).",
                Style::default().fg(theme::p().muted),
            ),
        ),
        logo_line(
            2,
            Span::styled(
                format!(
                    "  Shortcuts: {} newline, {} commands, drag/shift+click copy.",
                    crate::app::ctrl_label('j'),
                    crate::app::ctrl_label('p')
                ),
                Style::default().fg(theme::p().muted),
            ),
        ),
        logo_line(
            3,
            Span::styled(
                format!("  Workspace: {}", app.workspace_root.display()),
                Style::default().fg(theme::p().faint),
            ),
        ),
    ];

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
        if name == meerai_agents::QUESTION_TOOL {
            return;
        }
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

#[derive(Clone, Copy)]
enum DigestItem {
    Reasoning(usize),
    ToolCall(usize),
    Subagent(usize),
}

struct ActivityGroup {
    start_idx: usize,
    digest: TurnDigest,
    items: Vec<DigestItem>,
}

enum TranscriptSegment {
    Digest(ActivityGroup),
    Entry(usize),
}

fn segment_transcript(transcript: &[TranscriptEntry]) -> Vec<TranscriptSegment> {
    let mut segments = Vec::new();
    let mut current: Option<ActivityGroup> = None;

    let flush = |segments: &mut Vec<TranscriptSegment>, current: &mut Option<ActivityGroup>| {
        if let Some(group) = current.take()
            && !group.digest.is_empty()
        {
            segments.push(TranscriptSegment::Digest(group));
        }
    };

    for (idx, entry) in transcript.iter().enumerate() {
        match entry {
            TranscriptEntry::ToolCall(tc) if tc.name == meerai_agents::QUESTION_TOOL => continue,
            TranscriptEntry::Assistant {
                text, reasoning, ..
            } if text.is_empty() && reasoning.is_empty() => continue,
            TranscriptEntry::Assistant {
                text,
                reasoning: _,
                streaming,
                ..
            } if text.is_empty() => {
                let group = current.get_or_insert_with(|| ActivityGroup {
                    start_idx: idx,
                    digest: TurnDigest::default(),
                    items: Vec::new(),
                });
                group.digest.thoughts += 1;
                group.digest.thoughts_active |= *streaming;
                group.items.push(DigestItem::Reasoning(idx));
            }
            TranscriptEntry::ToolCall(tc) => {
                let group = current.get_or_insert_with(|| ActivityGroup {
                    start_idx: idx,
                    digest: TurnDigest::default(),
                    items: Vec::new(),
                });
                group.digest.record_tool(&tc.name, &tc.status);
                group.items.push(DigestItem::ToolCall(idx));
            }
            TranscriptEntry::Subagent(sub) => {
                let group = current.get_or_insert_with(|| ActivityGroup {
                    start_idx: idx,
                    digest: TurnDigest::default(),
                    items: Vec::new(),
                });
                group.digest.agents += 1;
                group.digest.agents_active |= matches!(
                    sub.status,
                    ToolCallStatus::Proposed | ToolCallStatus::Running
                );
                group.items.push(DigestItem::Subagent(idx));
            }
            TranscriptEntry::Assistant { reasoning, .. } => {
                if !reasoning.is_empty() {
                    let group = current.get_or_insert_with(|| ActivityGroup {
                        start_idx: idx,
                        digest: TurnDigest::default(),
                        items: Vec::new(),
                    });
                    group.digest.thoughts += 1;
                    group.items.push(DigestItem::Reasoning(idx));
                }
                flush(&mut segments, &mut current);
                segments.push(TranscriptSegment::Entry(idx));
            }
            TranscriptEntry::User { .. }
            | TranscriptEntry::Question { .. }
            | TranscriptEntry::System { .. }
            | TranscriptEntry::Error { .. } => {
                flush(&mut segments, &mut current);
                segments.push(TranscriptSegment::Entry(idx));
            }
        }
    }

    flush(&mut segments, &mut current);
    segments
}

pub(crate) fn is_activity_group_start(transcript: &[TranscriptEntry], idx: usize) -> bool {
    segment_transcript(transcript)
        .iter()
        .any(|seg| matches!(seg, TranscriptSegment::Digest(group) if group.start_idx == idx))
}

fn digest_line(digest: &TurnDigest, expanded: bool) -> Option<Line<'static>> {
    if digest.is_empty() {
        return None;
    }
    Some(Line::from(vec![
        Span::styled(
            if expanded { "- " } else { "+ " },
            Style::default().fg(theme::p().faint),
        ),
        Span::styled(digest.summary(), Style::default().fg(theme::p().faint)),
    ]))
}

fn push_user_mention_spans(
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

fn user_mention_chip_style(kind: MentionKind) -> Style {
    let color = match kind {
        MentionKind::Agent => theme::p().accent,
        MentionKind::Skill => theme::p().warning,
    };
    Style::default()
        .fg(color)
        .bg(theme::p().surface_high)
        .add_modifier(Modifier::BOLD)
}

struct BuiltTranscript {
    lines: Vec<TaggedTranscriptLine>,
    hover_text_lines: Vec<(usize, usize)>,
}

fn build_transcript_lines(
    app: &AppState,
    memo: &mut TranscriptBuildMemo,
    width: usize,
) -> BuiltTranscript {
    let mut lines = build_header_lines(app);
    let mut hover_text_lines: Vec<(usize, usize)> = Vec::new();
    let tab = app.tab();
    let transcript = &tab.transcript;
    memo.begin(tab.tab_id, theme::p(), transcript.len());
    if transcript.is_empty() {
        return BuiltTranscript {
            lines,
            hover_text_lines,
        };
    }

    enum RenderOp {
        Blank,
        DigestHeader {
            start_idx: usize,
            digest: TurnDigest,
            expanded: bool,
        },
        Reasoning(usize),
        Entry(usize),
    }

    let segments = segment_transcript(transcript);
    let mut ops = Vec::new();
    let mut emitted_any = false;
    let mut prev_was_digest = false;
    for seg in segments {
        match seg {
            TranscriptSegment::Digest(group) => {
                if emitted_any {
                    ops.push(RenderOp::Blank);
                }
                let expanded = tab
                    .turn_activity
                    .get(&group.start_idx)
                    .copied()
                    .unwrap_or(false);
                ops.push(RenderOp::DigestHeader {
                    start_idx: group.start_idx,
                    digest: group.digest,
                    expanded,
                });
                if expanded {
                    for item in group.items {
                        ops.push(RenderOp::Blank);
                        match item {
                            DigestItem::Reasoning(idx) => ops.push(RenderOp::Reasoning(idx)),
                            DigestItem::ToolCall(idx) | DigestItem::Subagent(idx) => {
                                ops.push(RenderOp::Entry(idx))
                            }
                        }
                    }
                }
                emitted_any = true;
                prev_was_digest = true;
            }
            TranscriptSegment::Entry(idx) => {
                if emitted_any {
                    ops.push(RenderOp::Blank);
                    if !prev_was_digest || matches!(transcript[idx], TranscriptEntry::User { .. }) {
                        ops.push(RenderOp::Blank);
                    }
                }
                ops.push(RenderOp::Entry(idx));
                emitted_any = true;
                prev_was_digest = false;
            }
        }
    }

    lines.push(tagged_line(Line::default(), None, 0));
    for op in ops {
        let idx = match op {
            RenderOp::Blank => {
                lines.push(tagged_line(Line::default(), None, 0));
                continue;
            }
            RenderOp::DigestHeader {
                start_idx,
                digest,
                expanded,
            } => {
                if let Some(line) = digest_line(&digest, expanded) {
                    lines.push(with_digest(tagged_line(line, None, 2), start_idx));
                }
                continue;
            }
            RenderOp::Reasoning(idx) => {
                if let Some(TranscriptEntry::Assistant {
                    reasoning,
                    streaming,
                    reasoning_expanded,
                    ..
                }) = transcript.get(idx)
                    && !reasoning.is_empty()
                {
                    render_reasoning_lines(
                        &mut lines,
                        &mut hover_text_lines,
                        reasoning,
                        *streaming,
                        *reasoning_expanded,
                        idx,
                    );
                }
                continue;
            }
            RenderOp::Entry(idx) => idx,
        };
        render_entry_lines(app, memo, &mut lines, &transcript[idx], idx, width);
    }

    BuiltTranscript {
        lines,
        hover_text_lines,
    }
}

fn render_entry_lines(
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

fn render_reasoning_lines(
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

pub(super) fn subagent_view_lines(
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
    meerai_host::diff::tool_edit_diff(&tc.name, &tc.args, tc.output.as_deref())
}

fn render_edit_diff(
    lines: &mut Vec<TaggedTranscriptLine>,
    diff: &[crate::diff::UnifiedLine],
    language: Option<crate::syntax::Language>,
    idx: usize,
    memo: &mut TranscriptBuildMemo,
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
    let tokens = memo.tokens(idx, language, &source, || {
        crate::syntax::token_lines(language, &source)
    });

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

struct WrappedTranscript {
    lines: Vec<Line<'static>>,
    hit_zones: Vec<ThinkingHitZone>,
    digest_hit_zones: Vec<ThinkingHitZone>,
    doc_lines: Vec<TranscriptWrappedLine>,
    bands: Vec<(usize, usize)>,
    code_bands: Vec<(usize, usize)>,
    hover_zones: Vec<HoverZone>,
    hover_text_zones: Vec<HoverZone>,
}

fn wrap_tagged_lines(built: BuiltTranscript, width: usize) -> WrappedTranscript {
    let BuiltTranscript {
        lines,
        hover_text_lines,
    } = built;
    if width == 0 {
        return WrappedTranscript {
            lines: Vec::new(),
            hit_zones: Vec::new(),
            digest_hit_zones: Vec::new(),
            doc_lines: Vec::new(),
            bands: Vec::new(),
            code_bands: Vec::new(),
            hover_zones: Vec::new(),
            hover_text_zones: Vec::new(),
        };
    }

    let mut out = Vec::with_capacity(lines.len());
    let mut doc_lines = Vec::with_capacity(lines.len());
    let mut hit_zones: Vec<ThinkingHitZone> = Vec::new();
    let mut digest_hit_zones: Vec<ThinkingHitZone> = Vec::new();
    let mut bands: Vec<(usize, usize)> = Vec::new();
    let mut code_bands: Vec<(usize, usize)> = Vec::new();
    let mut hover_zones: Vec<HoverZone> = Vec::new();
    let mut hover_text_zones: Vec<HoverZone> = Vec::new();
    let mut hover_active: Option<(usize, usize)> = None;
    let mut active_zone: Option<(usize, usize)> = None;
    let mut active_digest: Option<(usize, usize)> = None;
    let mut hover_text_marker = 0;

    for (tag_idx, tagged) in lines.into_iter().enumerate() {
        let TaggedTranscriptLine {
            line,
            entry_tag,
            gutter_len,
            hang,
            band,
            code_band,
            hover_tag,
            digest_tag,
        } = tagged;
        let line_style = line.style;
        let line_start = out.len();
        if line.spans.is_empty() {
            out.push(Line::default().style(line_style));
            doc_lines.push(TranscriptWrappedLine {
                text: String::new(),
                gutter_len: 0,
                hang_len: 0,
                soft_wrapped_next: false,
            });
        } else {
            for row in wrap_line(line.spans, width, &hang, gutter_len) {
                out.push(Line::from(row.spans).style(line_style));
                doc_lines.push(TranscriptWrappedLine {
                    text: row.text,
                    gutter_len: row.gutter_len,
                    hang_len: row.hang_len,
                    soft_wrapped_next: row.soft_wrapped_next,
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
        if let Some((marker_idx, entry_idx)) = hover_text_lines.get(hover_text_marker)
            && *marker_idx == tag_idx
        {
            hover_text_zones.push(HoverZone {
                entry_idx: *entry_idx,
                start_line: line_start,
                end_line: out.len(),
            });
            hover_text_marker += 1;
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
        hover_text_zones,
    }
}

struct WrappedRow {
    spans: Vec<Span<'static>>,
    text: String,
    gutter_len: usize,
    hang_len: usize,
    soft_wrapped_next: bool,
}

pub(super) fn markdown_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for line in crate::markdown::render(text, width) {
        let hang = line.hang.clone();
        for row in wrap_line(line.spans, width, &hang, 0) {
            out.push(Line::from(row.spans));
        }
    }
    out
}

fn wrap_line(
    spans: Vec<Span<'static>>,
    width: usize,
    hang: &str,
    gutter_len: usize,
) -> Vec<WrappedRow> {
    let chars: Vec<(char, Style)> = spans
        .iter()
        .flat_map(|span| span.content.chars().map(|ch| (ch, span.style)))
        .collect();
    if chars.is_empty() {
        return vec![WrappedRow {
            spans: Vec::new(),
            text: String::new(),
            gutter_len,
            hang_len: 0,
            soft_wrapped_next: false,
        }];
    }
    let width = width.max(1);
    let hang_width = display_width(hang);
    let hang_len = hang.chars().count();
    let mut rows = Vec::new();
    let mut start = 0;
    let mut first = true;
    while start < chars.len() {
        let prefix = if first { "" } else { hang };
        let prefix_width = if first { 0 } else { hang_width };
        let available = width.saturating_sub(prefix_width).max(1);
        let mut column = 0;
        let mut end = start;
        while end < chars.len() {
            let next = char_width(chars[end].0);
            if end > start && column + next > available {
                break;
            }
            column += next;
            end += 1;
        }
        if end < chars.len()
            && let Some(position) = (start..end).rev().find(|&idx| chars[idx].0 == ' ')
        {
            end = position + 1;
        }
        if end == start {
            end += 1;
        }
        let mut row_spans: Vec<Span<'static>> = Vec::new();
        if !prefix.is_empty() {
            row_spans.push(Span::styled(
                prefix.to_string(),
                Style::default().fg(theme::p().faint),
            ));
        }
        row_spans.extend(spans_from_chars(&chars[start..end]));
        let mut text = String::with_capacity(prefix.len() + end - start);
        text.push_str(prefix);
        text.extend(chars[start..end].iter().map(|(ch, _)| *ch));
        rows.push(WrappedRow {
            spans: row_spans,
            text,
            gutter_len,
            hang_len: if first { 0 } else { hang_len },
            soft_wrapped_next: end < chars.len(),
        });
        start = end;
        first = false;
    }
    rows
}

fn spans_from_chars(chars: &[(char, Style)]) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (ch, style) in chars {
        if let Some(last) = spans.last_mut()
            && last.style == *style
        {
            last.content.to_mut().push(*ch);
        } else {
            spans.push(Span::styled(ch.to_string(), *style));
        }
    }
    spans
}
