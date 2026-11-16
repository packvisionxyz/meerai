use super::*;

pub(super) fn render_composer(frame: &mut Frame<'_>, app: &mut AppState, area: Rect) {
    app.composer_block_area = area;
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().surface)),
        area,
    );
    if area.height < 3 || area.width < 8 {
        return;
    }

    let stripe_color = if app.tab().is_running {
        theme::p().warning
    } else {
        match app.mode {
            InputMode::Insert => theme::p().accent,
            InputMode::Normal => theme::p().info,
        }
    };
    for y in area.y..area.y + area.height {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "▌",
                Style::default().fg(stripe_color).bg(theme::p().surface),
            )),
            Rect::new(area.x, y, 1, 1),
        );
    }

    let inner = Rect {
        x: area.x.saturating_add(2),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(2),
    };
    let content = Rect {
        x: inner.x.saturating_add(2),
        width: inner.width.saturating_sub(2),
        ..inner
    };
    app.composer_input_area = content;

    let marker_color = match app.mode {
        InputMode::Insert => theme::p().interactive,
        InputMode::Normal => theme::p().muted,
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "❯",
            Style::default()
                .fg(marker_color)
                .add_modifier(Modifier::BOLD),
        ))),
        Rect::new(inner.x, inner.y, 2, 1),
    );

    if !app.tab().queued.is_empty() && area.width >= 30 {
        let badge = format!("{} queued · ctrl+g steer", app.tab().queued.len());
        let badge_w = badge.chars().count() as u16;
        if badge_w + 4 < area.width {
            frame.render_widget(
                Paragraph::new(Span::styled(badge, Style::default().fg(theme::p().warning))),
                Rect::new(area.x + area.width - badge_w - 2, area.y, badge_w, 1),
            );
        }
    }

    let (cursor_row, cursor_col) = cursor_row_col(&app.tab().input, app.tab().cursor);
    let visible_rows = content.height.max(1) as usize;
    let scroll_y = cursor_row.saturating_sub(visible_rows.saturating_sub(1));
    app.composer_scroll = scroll_y;

    let tab = app.tab();
    let mut cursor_display_col = cursor_col;
    let placeholder = match app.mode {
        InputMode::Insert => "Ask meerai anything...",
        InputMode::Normal => "Press i or a to insert...",
    };
    let lines = if tab.input.is_empty() {
        Paragraph::new(Line::from(Span::styled(
            placeholder,
            Style::default().fg(theme::p().faint),
        )))
    } else {
        let lines: Vec<Line<'_>> = tab
            .input
            .split('\n')
            .enumerate()
            .map(|(index, line)| {
                let cursor = (index == cursor_row).then_some(cursor_col);
                let (spans, display_col) =
                    render_composer_line(line, cursor, &app.agents, &app.skills);
                if index == cursor_row {
                    cursor_display_col = display_col;
                }
                Line::from(spans)
            })
            .collect();
        Paragraph::new(lines).scroll((scroll_y.min(u16::MAX as usize) as u16, 0))
    };

    frame.render_widget(lines, content);

    if let Some((start, end)) = app.composer_selection.filter(|(start, end)| start < end) {
        let input = app.tab().input.clone();
        let (start_line, start_col) = display_pos_for_byte(&input, start, &app.agents, &app.skills);
        let (end_line, end_col) = display_pos_for_byte(&input, end, &app.agents, &app.skills);
        let highlight = theme::p().surface_high;
        let buf = frame.buffer_mut();
        for line in start_line..=end_line {
            if line < scroll_y || line >= scroll_y + content.height as usize {
                continue;
            }
            let y = content.y + (line - scroll_y) as u16;
            let from = if line == start_line { start_col } else { 0 };
            let to = if line == end_line {
                end_col
            } else {
                content.width as usize
            };
            for col in from..to {
                if let Some(cell) = buf.cell_mut((content.x + col as u16, y)) {
                    cell.set_bg(highlight);
                }
            }
        }
    }

    if app.modal.is_none()
        && app.mode == InputMode::Insert
        && content.width > 0
        && content.height > 0
    {
        let max_x = content.x + content.width.saturating_sub(1);
        let rel_row = cursor_row.saturating_sub(scroll_y);
        let max_y = content.y + content.height.saturating_sub(1);
        let x = (content.x + cursor_display_col as u16).min(max_x);
        let y = (content.y + rel_row as u16).min(max_y);
        frame.set_cursor_position(Position::new(x, y));
    }
}

fn display_width_of(line: &str, agents: &[AgentDefinition], skills: &[SkillDefinition]) -> usize {
    let mut width = 0usize;
    let mut index = 0usize;
    while index < line.len() {
        if let Some((kind, id, end)) = parse_mention_token(line, index)
            && mention_is_known(kind, id, agents, skills)
        {
            width += id.chars().count();
            index = end;
            continue;
        }
        let Some(ch) = line[index..].chars().next() else {
            break;
        };
        width += 1;
        index += ch.len_utf8();
    }
    width
}

fn display_pos_for_byte(
    input: &str,
    byte: usize,
    agents: &[AgentDefinition],
    skills: &[SkillDefinition],
) -> (usize, usize) {
    let clamped = byte.min(input.len());
    let before = &input[..clamped];
    let line = before.matches('\n').count();
    let line_start = before.rfind('\n').map(|index| index + 1).unwrap_or(0);
    let width = display_width_of(&input[line_start..clamped], agents, skills);
    (line, width)
}

pub(super) fn render_composer_line(
    line: &str,
    cursor_char: Option<usize>,
    agents: &[AgentDefinition],
    skills: &[SkillDefinition],
) -> (Vec<Span<'static>>, usize) {
    let cursor_byte = cursor_char.map(|col| {
        line.char_indices()
            .nth(col)
            .map(|(idx, _)| idx)
            .unwrap_or(line.len())
    });

    let mut spans = Vec::new();
    let mut display_col = 0usize;
    let mut cursor_col = None;
    let mut plain = String::new();
    let mut plain_start = 0usize;
    let mut index = 0usize;

    while index < line.len() {
        let Some(ch) = line[index..].chars().next() else {
            break;
        };
        if ch == '@'
            && let Some((kind, id, end)) = parse_mention_token(line, index)
            && mention_is_known(kind, id, agents, skills)
        {
            if !plain.is_empty() {
                push_composer_text(
                    &mut spans,
                    &mut plain,
                    plain_start,
                    index,
                    cursor_byte,
                    &mut display_col,
                    &mut cursor_col,
                    line,
                );
            }
            if cursor_col.is_none()
                && let Some(cursor) = cursor_byte
            {
                if cursor <= index {
                    cursor_col = Some(display_col);
                } else if cursor <= end {
                    cursor_col = Some(display_col + id.chars().count());
                }
            }
            display_col += id.chars().count();
            spans.push(Span::styled(id.to_string(), mention_chip_style(kind)));
            index = end;
            plain_start = end;
            continue;
        }
        if plain.is_empty() {
            plain_start = index;
        }
        plain.push(ch);
        index += ch.len_utf8();
    }

    if !plain.is_empty() {
        push_composer_text(
            &mut spans,
            &mut plain,
            plain_start,
            line.len(),
            cursor_byte,
            &mut display_col,
            &mut cursor_col,
            line,
        );
    }

    (spans, cursor_col.unwrap_or(display_col))
}

#[allow(clippy::too_many_arguments)]
fn push_composer_text(
    spans: &mut Vec<Span<'static>>,
    plain: &mut String,
    start: usize,
    end: usize,
    cursor_byte: Option<usize>,
    display_col: &mut usize,
    cursor_col: &mut Option<usize>,
    line: &str,
) {
    let text = std::mem::take(plain);
    if cursor_col.is_none()
        && let Some(cursor) = cursor_byte
    {
        if cursor <= start {
            *cursor_col = Some(*display_col);
        } else if cursor < end {
            *cursor_col = Some(*display_col + line[start..cursor].chars().count());
        }
    }
    *display_col += text.chars().count();
    spans.push(Span::raw(text));
}

fn mention_chip_style(kind: MentionKind) -> Style {
    let color = match kind {
        MentionKind::Agent => theme::p().accent,
        MentionKind::Skill => theme::p().interactive,
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

pub(super) fn render_completion_popup(
    frame: &mut Frame<'_>,
    app: &mut AppState,
    transcript_area: Rect,
    composer_area: Rect,
) {
    if app.modal.is_some() || app.mode != InputMode::Insert || !app.completion.is_active() {
        return;
    }

    let items = app.completion.items();
    if items.is_empty() || transcript_area.height < 3 || composer_area.width < 12 {
        return;
    }

    let max_visible = (transcript_area.height.saturating_sub(2) as usize).clamp(1, 8);
    let height = items.len().min(max_visible) as u16 + 2;
    let (width, x) = if app.completion.kind() == Some(CompletionKind::File) {
        (transcript_area.width, transcript_area.x)
    } else {
        (
            composer_area.width.saturating_sub(4).min(68),
            composer_area.x + 2,
        )
    };
    let y = composer_area.y.saturating_sub(height);
    let popup_area = Rect::new(x, y, width, height);

    let title = match app.completion.kind() {
        Some(CompletionKind::SlashCommand) => {
            format!(
                "Commands ({}/{})",
                app.completion.selected() + 1,
                items.len()
            )
        }
        Some(CompletionKind::Model) => {
            format!("Models ({}/{})", app.completion.selected() + 1, items.len())
        }
        Some(CompletionKind::File) => {
            format!(
                "Mentions ({}/{})",
                app.completion.selected() + 1,
                items.len()
            )
        }
        None => String::new(),
    };

    let inner = dialog_frame_compact(frame, popup_area, &title);
    let label_width = items
        .iter()
        .map(|item| item.label.chars().count())
        .max()
        .unwrap_or(12)
        .min(((inner.width as usize).saturating_mul(2) / 5).max(12));

    let selected_idx = app.completion.selected();
    let hovered_idx = app.hovered_completion;

    let list_items: Vec<ListItem<'_>> = items
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            let is_selected = idx == selected_idx;
            let is_hovered = hovered_idx == Some(idx);
            let label = truncate_single_line(&item.label, label_width);
            let label_len = label.chars().count();
            let pad = label_width.saturating_sub(label_len) + 2;
            let max_desc = (inner.width as usize).saturating_sub(2 + label_len + pad + 2);

            let lead = if is_selected { "▸ " } else { "  " };
            let mut spans = vec![Span::styled(
                lead,
                Style::default().fg(if is_selected {
                    theme::p().interactive
                } else {
                    theme::p().faint
                }),
            )];
            let label_color = match item.item_kind {
                CompletionItemKind::Agent => theme::p().accent,
                CompletionItemKind::Skill => theme::p().interactive,
                CompletionItemKind::Plain | CompletionItemKind::File => theme::p().text,
            };
            spans.push(Span::styled(
                label,
                Style::default()
                    .fg(label_color)
                    .add_modifier(Modifier::BOLD),
            ));
            if !item.description.is_empty() && max_desc > 0 {
                spans.push(Span::raw(" ".repeat(pad)));
                spans.push(Span::styled(
                    truncate_single_line(&item.description, max_desc),
                    Style::default().fg(if is_selected || is_hovered {
                        theme::p().muted
                    } else {
                        theme::p().faint
                    }),
                ));
            }
            let row = ListItem::new(Line::from(spans));
            if is_hovered && !is_selected {
                row.style(hover_row_style())
            } else {
                row
            }
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(selected_idx));

    let list = List::new(list_items).highlight_style(selection_style());
    frame.render_stateful_widget(list, inner, &mut list_state);

    let offset = list_state.offset();
    for row in 0..inner.height as usize {
        let idx = offset + row;
        if idx >= items.len() {
            break;
        }
        app.completion_hit_zones.push(CompletionHitZone {
            rect: Rect::new(inner.x, inner.y + row as u16, inner.width, 1),
            index: idx,
        });
    }
}
