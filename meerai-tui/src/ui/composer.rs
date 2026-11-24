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
        let highlight = TEXT_SELECTION_BG;
        let highlight_fg = TEXT_SELECTION_FG;
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
                    cell.set_fg(highlight_fg);
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

#[derive(Default)]
pub(crate) struct ComposerRow {
    pub logical_line: usize,
    pub start: usize,
    pub width: usize,
}

pub(crate) fn composer_row(
    input: &str,
    cursor: usize,
    width: usize,
    row: usize,
    agents: &[AgentDefinition],
    skills: &[SkillDefinition],
) -> ComposerRow {
    let wrapped = wrap_composer(input, cursor, width.max(1), agents, skills);
    let row = row.min(wrapped.rows.len().saturating_sub(1));
    wrapped
        .rows
        .get(row)
        .map(|target| ComposerRow {
            logical_line: target.logical_line,
            start: target.logical_start,
            width: wrapped_width(target),
        })
        .unwrap_or_default()
}

pub(crate) fn composer_byte_at(
    input: &str,
    logical_line: usize,
    display_col: usize,
    agents: &[AgentDefinition],
    skills: &[SkillDefinition],
) -> usize {
    let mut line_start = 0usize;
    let mut line = "";
    for (index, text) in input.split('\n').enumerate() {
        if index == logical_line {
            line = text;
            break;
        }
        line_start += text.len() + 1;
    }
    let char_idx = display_col_to_char_index(line, display_col, agents, skills);
    line_start + byte_at_char(line, char_idx)
}

pub(crate) fn composer_vertical_move(
    input: &str,
    cursor: usize,
    width: usize,
    delta: isize,
    goal_col: Option<usize>,
    agents: &[AgentDefinition],
    skills: &[SkillDefinition],
) -> Option<(usize, usize)> {
    let wrapped = wrap_composer(input, cursor, width.max(1), agents, skills);
    let target_row = wrapped.cursor_row.checked_add_signed(delta)?;
    let target = wrapped.rows.get(target_row)?;
    let goal = goal_col.unwrap_or(wrapped.cursor_col);
    let col = goal.min(wrapped_width(target));
    Some((
        composer_byte_at(
            input,
            target.logical_line,
            target.logical_start + col,
            agents,
            skills,
        ),
        goal,
    ))
}

fn byte_at_char(line: &str, char_index: usize) -> usize {
    line.char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(line.len())
}

pub(crate) fn display_col_to_char_index(
    line: &str,
    display_col: usize,
    agents: &[AgentDefinition],
    skills: &[SkillDefinition],
) -> usize {
    let mut display = 0usize;
    let mut index = 0usize;
    while index < line.len() && display < display_col {
        if let Some((kind, id, end)) = parse_mention_token(line, index)
            && mention_is_known(kind, id, agents, skills)
        {
            let width = id.chars().count();
            if display + width > display_col {
                return line[..end].chars().count();
            }
            display += width;
            index = end;
            continue;
        }
        let Some(ch) = line[index..].chars().next() else {
            break;
        };
        display += 1;
        index += ch.len_utf8();
    }
    line[..index].chars().count()
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

pub(crate) fn composer_rows(
    input: &str,
    width: usize,
    agents: &[AgentDefinition],
    skills: &[SkillDefinition],
) -> usize {
    wrap_composer(input, input.len(), width.max(1), agents, skills)
        .rows
        .len()
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
        MentionKind::Skill => theme::p().warning,
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

    let max_visible = (transcript_area.height.saturating_sub(2) as usize).clamp(1, 12);
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
            let badge = item.item_kind.badge();
            let badge_width = badge.map_or(0, |_| 6);
            let max_desc =
                (inner.width as usize).saturating_sub(2 + badge_width + label_len + pad + 2);

            let lead = if is_selected { "▸ " } else { "  " };
            let mut spans = vec![Span::styled(
                lead,
                Style::default().fg(if is_selected {
                    theme::p().interactive
                } else {
                    theme::p().faint
                }),
            )];
            if let Some(badge) = badge {
                spans.push(Span::styled(
                    format!("{badge:<5} "),
                    Style::default().fg(theme::p().faint),
                ));
            }
            let label_color = match item.item_kind {
                CompletionItemKind::Agent => theme::p().accent,
                CompletionItemKind::Skill => theme::p().warning,
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

const QUEUED_BAND_MAX_ROWS: u16 = 6;
const QUEUED_ACTIONS_WIDTH: u16 = 10;

fn queued_band_blocks(
    queued: &[String],
    width: u16,
    max_rows: u16,
) -> (Vec<(usize, Vec<String>)>, usize) {
    let text_w = width.saturating_sub(3 + QUEUED_ACTIONS_WIDTH).max(8) as usize;
    let wrapped: Vec<Vec<String>> = queued
        .iter()
        .map(|text| wrap_queued_preview(text, text_w, text_w, 2))
        .collect();
    let total: u16 = wrapped.iter().map(|lines| lines.len() as u16).sum();
    let budget = if total > max_rows {
        max_rows.saturating_sub(1)
    } else {
        max_rows
    };
    let mut blocks: Vec<(usize, Vec<String>)> = Vec::new();
    let mut used = 0u16;
    for (idx, lines) in wrapped.into_iter().enumerate() {
        let block_h = lines.len() as u16;
        if used + block_h > budget {
            break;
        }
        used += block_h;
        blocks.push((idx, lines));
    }
    let hidden = queued.len().saturating_sub(blocks.len());
    (blocks, hidden)
}

fn queued_band_height(blocks: &[(usize, Vec<String>)], hidden: usize) -> u16 {
    let rows: u16 = blocks.iter().map(|(_, lines)| lines.len() as u16).sum();
    rows + u16::from(hidden > 0)
}

pub(super) fn queued_band_rows(app: &AppState, width: u16) -> u16 {
    if app.completion.is_active() || width < 40 {
        return 0;
    }
    let queued = &app.tab().queued;
    if queued.is_empty() {
        return 0;
    }
    let (blocks, hidden) = queued_band_blocks(queued, width, QUEUED_BAND_MAX_ROWS);
    queued_band_height(&blocks, hidden)
}

pub(super) fn render_queued_band(frame: &mut Frame<'_>, app: &mut AppState, area: Rect) {
    app.queued_hit_zones.clear();
    app.queued_band_area = Rect::default();
    if area.height == 0 || area.width < 40 {
        return;
    }
    let queued = app.tab().queued.clone();
    if queued.is_empty() {
        return;
    }
    let (blocks, hidden) =
        queued_band_blocks(&queued, area.width, area.height.min(QUEUED_BAND_MAX_ROWS));
    if blocks.is_empty() && hidden == 0 {
        return;
    }

    let band = Rect {
        height: queued_band_height(&blocks, hidden).min(area.height),
        ..area
    };
    app.queued_band_area = band;
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::p().surface_high)),
        band,
    );

    let hovered = app.hovered_queued;
    let mut row = band.y;
    for (idx, lines) in blocks {
        let edit = QueuedHitKind::Edit(idx);
        let remove = QueuedHitKind::Remove(idx);
        let edit_rect = Rect::new(band.right().saturating_sub(9), row, 4, 1);
        let remove_rect = Rect::new(band.right().saturating_sub(2), row, 1, 1);
        let label_style = if hovered == Some(edit) || hovered == Some(remove) {
            Style::default().fg(theme::p().text)
        } else {
            Style::default().fg(theme::p().muted)
        };
        for (line_idx, line) in lines.iter().enumerate() {
            let prefix = if line_idx == 0 { "↳ " } else { "  " };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(prefix, Style::default().fg(theme::p().warning)),
                    Span::styled(line.clone(), label_style),
                ])),
                Rect::new(band.x + 1, row + line_idx as u16, band.width - 2, 1),
            );
        }
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
        row += lines.len() as u16;
    }
    if hidden > 0 {
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!("… {hidden} more queued"),
                Style::default().fg(theme::p().faint),
            )),
            Rect::new(band.x + 1, row, band.width.saturating_sub(2), 1),
        );
    }
}

fn wrap_queued_preview(text: &str, first_w: usize, next_w: usize, max_lines: usize) -> Vec<String> {
    let width_for = |line: usize| -> usize {
        if line == 0 {
            first_w.max(4)
        } else {
            next_w.max(4)
        }
    };
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch == '\n' {
            lines.push(std::mem::take(&mut current));
            continue;
        }
        if current.chars().count() < width_for(lines.len()) {
            current.push(ch);
            continue;
        }
        match current.char_indices().rfind(|(_, c)| *c == ' ') {
            Some((idx, _)) if ch != ' ' => {
                let tail = current[idx + 1..].to_string();
                current.truncate(idx);
                lines.push(std::mem::take(&mut current));
                current = tail;
                current.push(ch);
            }
            _ => {
                lines.push(std::mem::take(&mut current));
                if ch != ' ' {
                    current.push(ch);
                }
            }
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        let limit = width_for(max_lines.saturating_sub(1));
        if let Some(last) = lines.last_mut() {
            while last.chars().count() >= limit {
                last.pop();
            }
            last.push('…');
        }
    }
    lines
}

#[cfg(test)]
mod cursor_tests {
    use super::*;

    #[test]
    fn goal_arguments_keep_the_cursor_at_the_end() {
        let skills = meerai_common::definitions::builtin_skills();
        let input = "/goal 1234";
        assert_eq!(
            display_pos_for_byte(input, input.len(), &[], &skills),
            (0, 10)
        );
        let wrapped = wrap_composer(input, input.len(), 40, &[], &skills);
        assert_eq!(wrapped.cursor_row, 0);
        assert_eq!(wrapped.cursor_col, 10);
    }
}
