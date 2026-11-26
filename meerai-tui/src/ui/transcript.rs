use super::statusline::{display_model, key_status_style, summarize_error_line};
use super::*;
use crate::app::{
    SubagentViewCache, ToolCallEntry, TranscriptAnchor, TranscriptBuildMemo, TranscriptDocument,
    TranscriptRenderKey, char_width, display_width,
};

mod build;
use build::*;
mod digest;
mod render;
mod tools;
mod wrap;
pub(crate) use digest::*;
pub(crate) use render::*;
pub(crate) use wrap::*;

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
