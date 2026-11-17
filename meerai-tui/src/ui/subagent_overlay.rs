use super::*;

pub(super) fn render_subagent_overlay(
    frame: &mut Frame<'_>,
    app: &mut AppState,
    composer_area: Rect,
) {
    app.subagent_overlay_zones.clear();
    if app.modal.is_some() {
        return;
    }
    let Some(overlay) = app.subagent_overlay.clone() else {
        return;
    };
    if overlay.entries.is_empty() {
        return;
    }

    let count = overlay.entries.len() as u16;
    let height = (count + 3).min(16);
    let width = composer_area.width.saturating_sub(4).clamp(24, 58);
    let x = composer_area.x.saturating_add(2);
    let y = composer_area.y.saturating_sub(height + 1);
    let area = Rect::new(x, y, width, height);
    if area.width < 8 || area.height < 3 {
        return;
    }

    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().surface_high)),
        area,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            format!(" Subagents ({})", overlay.entries.len()),
            Style::default()
                .fg(theme::p().text)
                .bg(theme::p().surface_high)
                .add_modifier(Modifier::BOLD),
        )),
        Rect::new(area.x, area.y, area.width, 1),
    );

    let rows_height = area.height.saturating_sub(2);
    for (offset, entry_idx) in overlay
        .entries
        .iter()
        .take(rows_height as usize)
        .enumerate()
    {
        let Some(TranscriptEntry::Subagent(subagent)) = app.tab().transcript.get(*entry_idx) else {
            continue;
        };
        let row = Rect::new(area.x, area.y + 1 + offset as u16, area.width, 1);
        let selected = offset == overlay.selected;
        let bg = theme::p().surface_high;
        if selected {
            frame.render_widget(
                Block::default().style(
                    Style::default()
                        .bg(theme::p().surface_high)
                        .fg(theme::p().accent),
                ),
                row,
            );
        }
        let (badge, color) = status_label(subagent.status);
        let prefix = if selected { "▸ " } else { "  " };
        let name_style = if selected {
            Style::default()
                .fg(theme::p().accent)
                .bg(bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::p().text).bg(bg)
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(prefix, name_style),
                Span::styled(
                    truncate_single_line(&subagent.agent, (area.width as usize).saturating_sub(14)),
                    name_style,
                ),
                Span::styled(format!(" [{badge}]"), Style::default().fg(color).bg(bg)),
            ])),
            row,
        );
        app.subagent_overlay_zones.push((row, offset));
    }

    let hint = "click view · Esc close";
    frame.render_widget(
        Paragraph::new(Span::styled(
            format!(" {hint}"),
            Style::default().fg(theme::p().faint),
        )),
        Rect::new(area.x, area.y + area.height - 1, area.width, 1),
    );
}

fn status_label(status: ToolCallStatus) -> (&'static str, Color) {
    match status {
        ToolCallStatus::Proposed | ToolCallStatus::Running => ("running", theme::p().warning),
        ToolCallStatus::Succeeded => ("done", theme::p().success),
        ToolCallStatus::Failed => ("failed", theme::p().error),
        ToolCallStatus::Stopped | ToolCallStatus::Cancelled => ("stopped", theme::p().muted),
    }
}
