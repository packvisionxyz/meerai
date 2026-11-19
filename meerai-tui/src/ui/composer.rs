use super::*;

pub(super) fn render_composer(frame: &mut Frame<'_>, app: &mut AppState, area: Rect) {
    app.composer_block_area = area;
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().surface)),
        area,
    );
    if area.height < 5 || area.width < 8 {
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
        x: inner.x,
        width: inner.width,
        y: inner.y,
        height: inner.height.saturating_sub(2),
    };
    let footer = Rect::new(
        content.x,
        inner.y + inner.height.saturating_sub(1),
        content.width,
        1,
    );
    app.composer_input_area = content;

    let (mode_badge, mode_color) = match app.tab().approval_mode {
        meerai_policy::ApprovalMode::Plan => (
            approval_mode_badge(app.tab().approval_mode),
            theme::p().info,
        ),
        meerai_policy::ApprovalMode::Ask => (
            approval_mode_badge(app.tab().approval_mode),
            theme::p().warning,
        ),
        meerai_policy::ApprovalMode::Auto => (
            approval_mode_badge(app.tab().approval_mode),
            theme::p().success,
        ),
    };
    let temperature = if matches!(
        app.active_key_status(),
        meerai_client::providers::KeyStatus::OAuth { .. }
    ) {
        "temp plan".to_string()
    } else {
        let effort = app.context_effort.unwrap_or_default();
        let value = app
            .context_temperature
            .unwrap_or_else(|| effort.default_temperature());
        format!("temp {value:.2} · {}", effort.label())
    };
    let model = super::statusline::display_model(app);
    let (input_badge, input_color) = match app.mode {
        InputMode::Insert => ("INS", theme::p().accent),
        InputMode::Normal => ("NOR", theme::p().info),
    };
    let mut footer_spans = vec![
        Span::styled(
            input_badge,
            Style::default()
                .fg(input_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(theme::p().faint)),
        Span::styled(
            mode_badge.to_string(),
            Style::default().fg(mode_color).add_modifier(Modifier::BOLD),
        ),
    ];
    if app.tab().is_acp() {
        let preset = app.tab().acp_preset().unwrap_or("acp");
        footer_spans.push(Span::styled(
            format!(" · {preset} (acp)"),
            Style::default().fg(theme::p().faint),
        ));
    } else {
        footer_spans.push(Span::styled(
            format!(" · {model}"),
            Style::default().fg(theme::p().text),
        ));
        footer_spans.push(Span::styled(
            format!(" · {}", app.tab().active_provider),
            Style::default().fg(theme::p().faint),
        ));
    }
    footer_spans.push(Span::styled(
        format!(" · {temperature}"),
        Style::default().fg(theme::p().warning),
    ));
    frame.render_widget(Paragraph::new(Line::from(footer_spans)), footer);

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

    render_queued_panel(frame, app, area);

    let tab = app.tab();
    let placeholder = match app.mode {
        InputMode::Insert => "Ask meerai anything...",
        InputMode::Normal => "Press i or a to insert...",
    };
    if tab.input.is_empty() {
        app.composer_scroll = 0;
        let lines = Paragraph::new(Line::from(Span::styled(
            placeholder,
            Style::default().fg(theme::p().faint),
        )));
        frame.render_widget(lines, content);
        return;
    }

    let wrapped = wrap_composer(
        &tab.input,
        tab.cursor,
        content.width.max(1) as usize,
        &app.agents,
        &app.skills,
    );
    let visible_rows = content.height.max(1) as usize;
    let scroll_y = wrapped
        .cursor_row
        .saturating_sub(visible_rows.saturating_sub(1));
    app.composer_scroll = scroll_y;
    let lines: Vec<Line<'_>> = wrapped
        .rows
        .iter()
        .map(|row| Line::from(row.spans.clone()))
        .collect();
    frame.render_widget(
        Paragraph::new(lines).scroll((scroll_y.min(u16::MAX as usize) as u16, 0)),
        content,
    );
    let cursor_display_col = wrapped.cursor_col;

    if let Some((start, end)) = app.composer_selection.filter(|(start, end)| start < end) {
        let input = app.tab().input.clone();
        let (start_line, start_col) = wrapped.position(&input, start, &app.agents, &app.skills);
        let (end_line, end_col) = wrapped.position(&input, end, &app.agents, &app.skills);
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
        let rel_row = wrapped.cursor_row.saturating_sub(scroll_y);
        let max_y = content.y + content.height.saturating_sub(1);
        let x = (content.x + cursor_display_col as u16).min(max_x);
        let y = (content.y + rel_row as u16).min(max_y);
        frame.set_cursor_position(Position::new(x, y));
    }
}

#[derive(Clone, Debug, Default)]
struct WrappedRow {
    logical_line: usize,
    logical_start: usize,
    spans: Vec<Span<'static>>,
}

#[derive(Clone, Debug, Default)]
struct WrappedComposer {
    rows: Vec<WrappedRow>,
    cursor_row: usize,
    cursor_col: usize,
}

impl WrappedComposer {
    fn row_for(&self, logical_line: usize, display_col: usize) -> (usize, usize) {
        let mut row = None;
        for (idx, wrapped) in self.rows.iter().enumerate() {
            if wrapped.logical_line != logical_line {
                continue;
            }
            row = Some(idx);
            if display_col <= wrapped.logical_start + wrapped_width(wrapped) {
                break;
            }
        }
        let row = row.unwrap_or(0);
        let start = self.rows.get(row).map(|w| w.logical_start).unwrap_or(0);
        (row, display_col.saturating_sub(start))
    }

    fn position(
        &self,
        input: &str,
        byte: usize,
        agents: &[AgentDefinition],
        skills: &[SkillDefinition],
    ) -> (usize, usize) {
        let (logical_line, display_col) = display_pos_for_byte(input, byte, agents, skills);
        self.row_for(logical_line, display_col)
    }
}

fn wrapped_width(wrapped: &WrappedRow) -> usize {
    wrapped
        .spans
        .iter()
        .map(|span| span.content.chars().count())
        .sum()
}

fn wrapped_chars(spans: &[Span<'static>]) -> Vec<(char, Style)> {
    spans
        .iter()
        .flat_map(|span| span.content.chars().map(move |ch| (ch, span.style)))
        .collect()
}

pub(crate) fn composer_row_start(
    input: &str,
    cursor: usize,
    width: usize,
    row: usize,
    agents: &[AgentDefinition],
    skills: &[SkillDefinition],
) -> (usize, usize) {
    let wrapped = wrap_composer(input, cursor, width.max(1), agents, skills);
    let row = row.min(wrapped.rows.len().saturating_sub(1));
    wrapped
        .rows
        .get(row)
        .map(|target| (target.logical_line, target.logical_start))
        .unwrap_or((0, 0))
}

fn wrap_line(
    spans: &[Span<'static>],
    width: usize,
    logical_line: usize,
    rows: &mut Vec<WrappedRow>,
) {
    let chars = wrapped_chars(spans);
    if chars.is_empty() {
        rows.push(WrappedRow {
            logical_line,
            logical_start: 0,
            spans: Vec::new(),
        });
        return;
    }
    let mut start = 0usize;
    while start < chars.len() {
        let remaining = &chars[start..];
        let hard_end = start + remaining.len().min(width);
        let end = if hard_end < chars.len() && chars[hard_end].0 != ' ' {
            remaining[..hard_end - start]
                .iter()
                .rposition(|(ch, _)| *ch == ' ')
                .map(|idx| start + idx)
                .filter(|idx| *idx > start)
                .unwrap_or(hard_end)
        } else {
            hard_end
        };
        let spans = chars[start..end]
            .iter()
            .map(|(ch, style)| Span::styled(ch.to_string(), *style))
            .collect();
        rows.push(WrappedRow {
            logical_line,
            logical_start: start,
            spans,
        });
        start = end;
        if start < chars.len() && chars[start].0 == ' ' {
            start += 1;
        }
    }
}

fn wrap_composer(
    input: &str,
    cursor: usize,
    width: usize,
    agents: &[AgentDefinition],
    skills: &[SkillDefinition],
) -> WrappedComposer {
    let width = width.max(1);
    let (cursor_line, cursor_col) = display_pos_for_byte(input, cursor, agents, skills);
    let mut wrapped = WrappedComposer {
        rows: Vec::new(),
        cursor_row: 0,
        cursor_col: 0,
    };
    for (line_index, line) in input.split('\n').enumerate() {
        let (spans, _) = render_composer_line(line, None, agents, skills);
        wrap_line(&spans, width, line_index, &mut wrapped.rows);
    }
    if wrapped.rows.is_empty() {
        wrapped.rows.push(WrappedRow::default());
    }
    (wrapped.cursor_row, wrapped.cursor_col) = wrapped.row_for(cursor_line, cursor_col);
    wrapped
}

pub(crate) fn composer_rows(input: &str, width: usize) -> usize {
    let width = width.max(1);
    if input.is_empty() {
        return 1;
    }
    input
        .split('\n')
        .map(|line| line.chars().count().max(1).div_ceil(width))
        .sum::<usize>()
        .max(1)
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

fn render_queued_panel(frame: &mut Frame<'_>, app: &mut AppState, area: Rect) {
    app.queued_hit_zones.clear();
    app.queued_panel_area = Rect::default();
    let queued = app.tab().queued.clone();
    if queued.is_empty() || area.width < 32 || area.y < 4 || app.completion.is_active() {
        return;
    }
    let max_rows = 4u16;
    let shown = queued.len().min(max_rows as usize);
    let height = shown as u16;
    let panel = Rect {
        x: area.x.saturating_add(2),
        y: area.y.saturating_sub(height),
        width: area.width.saturating_sub(4),
        height,
    };
    app.queued_panel_area = panel;
    frame.render_widget(Clear, panel);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().surface)),
        panel,
    );

    let hovered = app.hovered_queued;
    for row in 0..height {
        let idx = row as usize;
        let Some(text) = queued.get(idx) else {
            break;
        };
        let rect = Rect::new(panel.x, panel.y + row, panel.width, 1);
        let edit = QueuedHitKind::Edit(idx);
        let remove = QueuedHitKind::Remove(idx);
        let remove_w = 3u16;
        let edit_x = panel.x + panel.width.saturating_sub(remove_w + 5);
        let edit_rect = Rect::new(edit_x, rect.y, 5, 1);
        let remove_rect = Rect::new(
            panel.x + panel.width.saturating_sub(remove_w),
            rect.y,
            remove_w,
            1,
        );

        let preview_w = edit_x.saturating_sub(panel.x).saturating_sub(1) as usize;
        let preview =
            meerai_core::text::truncate_single_line(text, preview_w.saturating_sub(2).max(2));
        let label_style = if hovered == Some(edit) || hovered == Some(remove) {
            Style::default().fg(theme::p().text)
        } else {
            Style::default().fg(theme::p().muted)
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("↳ ", Style::default().fg(theme::p().warning)),
                Span::styled(preview, label_style),
            ])),
            Rect::new(rect.x, rect.y, preview_w as u16, 1),
        );
        frame.render_widget(
            Paragraph::new(Span::styled(
                "edit",
                if hovered == Some(edit) {
                    Style::default()
                        .fg(theme::p().accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::p().faint)
                },
            )),
            edit_rect,
        );
        frame.render_widget(
            Paragraph::new(Span::styled(
                "✕",
                if hovered == Some(remove) {
                    Style::default()
                        .fg(theme::p().error)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::p().faint)
                },
            )),
            remove_rect,
        );

        app.queued_hit_zones.push(QueuedHitZone {
            rect: edit_rect,
            kind: edit,
        });
        app.queued_hit_zones.push(QueuedHitZone {
            rect: remove_rect,
            kind: remove,
        });
    }
}
