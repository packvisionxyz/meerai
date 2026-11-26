use super::*;

#[derive(Clone, Copy)]
pub(super) struct VisibleRows {
    pub(super) inner: Rect,
    pub(super) first: usize,
    pub(super) last: usize,
}

pub(super) struct WrappedTranscript {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) hit_zones: Vec<ThinkingHitZone>,
    pub(super) digest_hit_zones: Vec<ThinkingHitZone>,
    pub(super) doc_lines: Vec<TranscriptWrappedLine>,
    pub(super) bands: Vec<(usize, usize)>,
    pub(super) code_bands: Vec<(usize, usize)>,
    pub(super) hover_zones: Vec<HoverZone>,
    pub(super) hover_text_zones: Vec<HoverZone>,
}

pub(super) fn wrap_tagged_lines(built: BuiltTranscript, width: usize) -> WrappedTranscript {
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

pub(super) struct WrappedRow {
    pub(super) spans: Vec<Span<'static>>,
    pub(super) text: String,
    pub(super) gutter_len: usize,
    pub(super) hang_len: usize,
    pub(super) soft_wrapped_next: bool,
}

pub(crate) fn markdown_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for line in crate::markdown::render(text, width) {
        let hang = line.hang.clone();
        for row in wrap_line(line.spans, width, &hang, 0) {
            out.push(Line::from(row.spans));
        }
    }
    out
}

pub(super) fn wrap_line(
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

pub(super) fn spans_from_chars(chars: &[(char, Style)]) -> Vec<Span<'static>> {
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
