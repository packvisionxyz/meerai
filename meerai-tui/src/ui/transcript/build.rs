use super::digest::*;
use super::render::*;
use super::wrap::*;
use super::*;

pub(super) fn anchor_rail_hover_range(app: &AppState) -> Option<(usize, usize)> {
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

pub(super) fn rebuild_transcript_document(app: &mut AppState, width: u16) {
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

pub(super) fn paint_text(
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

pub(super) fn build_header_lines(app: &AppState) -> Vec<TaggedTranscriptLine> {
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

pub(super) struct BuiltTranscript {
    pub(super) lines: Vec<TaggedTranscriptLine>,
    pub(super) hover_text_lines: Vec<(usize, usize)>,
}

pub(super) fn build_transcript_lines(
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
