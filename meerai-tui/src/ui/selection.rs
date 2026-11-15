use super::*;

pub(super) fn render_tab_bar(frame: &mut Frame<'_>, app: &mut AppState, area: Rect) {
    app.tab_hit_zones.clear();
    if area.width == 0 || area.height == 0 {
        return;
    }

    let width = area.width as usize;
    let labels: Vec<(String, Style)> = app
        .tabs
        .iter()
        .enumerate()
        .map(|(idx, tab)| {
            let is_active = idx == app.active_tab;
            let indicator = if !tab.pending_approvals.is_empty() {
                " [!]".to_string()
            } else if tab.is_running {
                let ch = SPINNER_FRAMES[app.spinner_tick % SPINNER_FRAMES.len()];
                format!(" {ch}")
            } else if tab.has_unread {
                " •".to_string()
            } else {
                String::new()
            };
            let title = truncate_single_line(&tab.display_title(), 18);
            let label = format!(" {}: {}{} ", idx + 1, title, indicator);
            let style = if is_active {
                Style::default()
                    .fg(theme::p().accent)
                    .add_modifier(Modifier::BOLD)
            } else if !tab.pending_approvals.is_empty() {
                Style::default()
                    .fg(theme::p().error)
                    .add_modifier(Modifier::BOLD)
            } else if tab.is_running {
                Style::default().fg(theme::p().warning)
            } else if tab.has_unread {
                Style::default().fg(theme::p().text)
            } else {
                Style::default().fg(theme::p().muted)
            };
            (label, style)
        })
        .collect();

    let new_btn = " + ";
    let mut start = 0;
    let mut end = labels.len();
    let total_tab_width = |s: usize, e: usize| -> usize {
        labels[s..e]
            .iter()
            .map(|(l, _)| l.chars().count() + 3)
            .sum::<usize>()
    };
    let budget = width.saturating_sub(new_btn.len() + 4);
    while total_tab_width(start, end) > budget && end - start > 1 {
        if app.active_tab > start && (end - app.active_tab) <= (app.active_tab - start) {
            start += 1;
        } else {
            end -= 1;
        }
    }

    let mut spans = Vec::new();
    let mut col = area.x;
    if start > 0 {
        let marker = "« ";
        spans.push(Span::styled(marker, Style::default().fg(theme::p().faint)));
        col = col.saturating_add(marker.chars().count() as u16);
    }

    for (idx, (label, style)) in labels.iter().enumerate().take(end).skip(start) {
        let label_len = label.chars().count() as u16;
        let zone_w = label_len.min(area.x + area.width - col);
        if zone_w > 0 {
            app.tab_hit_zones.push(TabHitZone {
                rect: Rect::new(col, area.y, zone_w, 1),
                kind: TabHitKind::Tab(idx),
            });
        }
        spans.push(Span::styled(label.clone(), *style));
        col = col.saturating_add(label_len);
        let close_w = 1u16.min(area.x + area.width - col);
        if close_w > 0 {
            app.tab_hit_zones.push(TabHitZone {
                rect: Rect::new(col, area.y, close_w, 1),
                kind: TabHitKind::CloseTab(idx),
            });
        }
        spans.push(Span::styled(
            "×",
            if idx == app.active_tab {
                Style::default().fg(theme::p().muted)
            } else {
                Style::default().fg(theme::p().faint)
            },
        ));
        col = col.saturating_add(close_w);
        spans.push(Span::raw(" "));
        col = col.saturating_add(1);
    }

    if end < labels.len() {
        let marker = "» ";
        spans.push(Span::styled(marker, Style::default().fg(theme::p().faint)));
        col = col.saturating_add(marker.chars().count() as u16);
    }

    let new_w = (new_btn.len() as u16).min((area.x + area.width).saturating_sub(col));
    if new_w > 0 {
        app.tab_hit_zones.push(TabHitZone {
            rect: Rect::new(col, area.y, new_w, 1),
            kind: TabHitKind::NewTab,
        });
        spans.push(Span::styled(
            new_btn,
            Style::default()
                .fg(theme::p().muted)
                .add_modifier(Modifier::BOLD),
        ));
        col = col.saturating_add(new_w);
    }

    let used = (col.saturating_sub(area.x)) as usize;
    let hint = "/sessions · /new · Ctrl-w close";
    if width.saturating_sub(used) >= hint.len() + 2 {
        let pad = width - used - hint.len() - 1;
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(hint, Style::default().fg(theme::p().faint)));
    }

    let bar = Paragraph::new(Line::from(spans));
    frame.render_widget(bar, area);
}

pub(super) fn capture_screen_lines(frame: &mut Frame<'_>, app: &mut AppState) {
    let area = frame.area();
    let buf = frame.buffer_mut();
    let mut rows = Vec::with_capacity(area.height as usize);
    for row in 0..area.height {
        let y = area.y + row;
        let mut cells = Vec::with_capacity(area.width as usize);
        for col in 0..area.width {
            let x = area.x + col;
            let sym = buf.cell((x, y)).map_or(" ", |c| c.symbol());
            cells.push(sym.to_string());
        }
        rows.push(cells);
    }
    app.screen_lines = rows;
}

pub(super) fn render_selection_highlight(frame: &mut Frame<'_>, app: &AppState) {
    let highlight = Style::default().bg(theme::p().border).fg(theme::p().text);

    if let Some(sel) = app.doc_selection
        && sel.anchor != sel.head
        && app.modal.is_none()
    {
        let inner = app.transcript_inner_area;
        if inner.width > 0 && inner.height > 0 && !app.transcript_wrapped_lines.is_empty() {
            let max_line = app.transcript_wrapped_lines.len() - 1;
            let a = DocPos {
                line: sel.anchor.line.min(max_line),
                col: sel.anchor.col,
            };
            let b = DocPos {
                line: sel.head.line.min(max_line),
                col: sel.head.col,
            };
            let (start, end) = if (a.line, a.col) <= (b.line, b.col) {
                (a, b)
            } else {
                (b, a)
            };
            let vis_start = app.tab().scroll_offset;
            let vis_end = vis_start + inner.height as usize;
            let draw_start = start.line.max(vis_start);
            let draw_end = end.line.min(vis_end.saturating_sub(1));
            if draw_start <= draw_end {
                let min_col = inner.x;
                let max_col = inner.x + inner.width - 1;
                let buf = frame.buffer_mut();
                for doc_line in draw_start..=draw_end {
                    let row = inner.y + (doc_line - vis_start) as u16;
                    let c_start = if doc_line == start.line {
                        inner.x.saturating_add(start.col as u16)
                    } else {
                        min_col
                    }
                    .clamp(min_col, max_col);
                    let c_end = if doc_line == end.line {
                        inner.x.saturating_add(end.col as u16)
                    } else {
                        max_col
                    }
                    .clamp(min_col, max_col);
                    for col in c_start..=c_end {
                        if let Some(cell) = buf.cell_mut((col, row)) {
                            cell.set_style(highlight);
                        }
                    }
                }
            }
            return;
        }
    }

    let Some(sel) = app.selection else {
        return;
    };
    if !sel.active || sel.anchor == sel.head {
        return;
    }
    let bounds = app.selection_bounds(sel.anchor);
    let Some((start, end)) = normalize_selection_in_bounds(sel.anchor, sel.head, bounds) else {
        return;
    };
    let min_col = bounds.x;
    let max_col = bounds.x + bounds.width - 1;
    let buf = frame.buffer_mut();
    for row in start.row..=end.row {
        let c_start = if row == start.row { start.col } else { min_col }.max(min_col);
        let c_end = if row == end.row { end.col } else { max_col }.min(max_col);
        for col in c_start..=c_end {
            if let Some(cell) = buf.cell_mut((col, row)) {
                cell.set_style(highlight);
            }
        }
    }
}
