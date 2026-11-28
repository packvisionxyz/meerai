use super::*;

pub(super) fn render_chat_modal(
    app: &mut AppState,
    frame: &mut Frame<'_>,
    area: Rect,
    modal: ActiveModal,
) {
    match modal {
        ActiveModal::UserMessageActions {
            entry_idx,
            selected,
        } => {
            let popup = centered_rect_fixed(area, 44, 10);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, "Message", true);

            let rows = Layout::vertical([
                Constraint::Length(3),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(inner);

            let items = [
                (UserAction::Rewind, "Rewind", "w"),
                (UserAction::Copy, "Copy", "c"),
                (UserAction::Fork, "Fork", "f"),
            ];
            let width = rows[0].width as usize;
            let selected_idx = selected.min(2);
            let list_items: Vec<ListItem<'_>> = items
                .iter()
                .enumerate()
                .map(|(idx, (_, label, key))| {
                    let is_hovered = app.hovered_modal_hit == Some(ModalHitKind::ListItem(idx));
                    let left = format!(" {label}");
                    let gap = width
                        .saturating_sub(left.chars().count() + key.len() + 1)
                        .max(2);
                    let row = ListItem::new(Line::from(vec![
                        Span::styled(left, Style::default().fg(theme::p().text)),
                        Span::raw(" ".repeat(gap)),
                        Span::styled(*key, Style::default().fg(theme::p().muted)),
                    ]));
                    if is_hovered && idx != selected_idx {
                        row.style(hover_row_style())
                    } else {
                        row
                    }
                })
                .collect();
            let mut state = ListState::default();
            state.select(Some(selected_idx));
            let list = List::new(list_items).highlight_style(selection_style());
            frame.render_stateful_widget(list, rows[0], &mut state);
            register_list_hit_zones(app, rows[0], state.offset(), items.len());
            let _ = entry_idx;

            frame.render_widget(
                Paragraph::new(key_hints_line(&[("Enter", "run"), ("Esc", "close")])),
                rows[2],
            );
        }
        ActiveModal::CommandPalette { filter, selected } => {
            let popup = cap_width(centered_rect(area, 54, 66, 48, 14), 62);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, "Commands (ctrl+p)", true);

            let rows = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);

            let filter_area = input_band(frame, rows[0], true);
            let filter_line = Line::from(vec![
                Span::styled(
                    "> ",
                    Style::default()
                        .fg(theme::p().interactive)
                        .add_modifier(Modifier::BOLD),
                ),
                if filter.is_empty() {
                    Span::styled("filter commands", Style::default().fg(theme::p().faint))
                } else {
                    Span::styled(filter.clone(), Style::default().fg(theme::p().text))
                },
            ]);
            frame.render_widget(Paragraph::new(filter_line), filter_area);
            place_input_cursor(frame, filter_area, 2 + filter.cursor_col());

            let commands = crate::app::filtered_commands(&filter);
            let width = rows[2].width as usize;
            let selected_idx = if commands.is_empty() {
                0
            } else {
                selected.min(commands.len() - 1)
            };
            let mut items: Vec<ListItem<'_>> = Vec::new();
            let mut command_rows: Vec<usize> = Vec::new();
            let mut visual_to_command: Vec<Option<usize>> = Vec::new();
            let mut section = "";
            for (cmd_idx, command) in commands.iter().enumerate() {
                if command.section != section {
                    if !section.is_empty() {
                        items.push(ListItem::new(Line::default()));
                        visual_to_command.push(None);
                    }
                    section = command.section;
                    items.push(ListItem::new(Line::from(Span::styled(
                        format!(" {} ", section.to_uppercase()),
                        Style::default()
                            .fg(theme::p().faint)
                            .add_modifier(Modifier::BOLD),
                    ))));
                    visual_to_command.push(None);
                }
                command_rows.push(items.len());
                visual_to_command.push(Some(cmd_idx));
                let is_hovered = app.hovered_modal_hit == Some(ModalHitKind::ListItem(cmd_idx));
                let label = format!(" {}", command.label);
                let label_len = label.chars().count();
                let hint = command.display_hint();
                let hint_len = hint.chars().count();
                let gap = width.saturating_sub(label_len + hint_len + 1).max(1);
                let hint_style = if hint.starts_with('/') {
                    Style::default().fg(theme::p().faint)
                } else {
                    Style::default().fg(theme::p().muted)
                };
                let row = ListItem::new(Line::from(vec![
                    Span::styled(label, Style::default().fg(theme::p().text)),
                    Span::raw(" ".repeat(gap)),
                    Span::styled(hint, hint_style),
                ]));
                items.push(if is_hovered && cmd_idx != selected_idx {
                    row.style(hover_row_style())
                } else {
                    row
                });
            }

            let mut state = ListState::default();
            if !commands.is_empty() {
                state.select(Some(command_rows[selected_idx]));
            }
            let list = List::new(items).highlight_style(selection_style());
            frame.render_stateful_widget(list, rows[2], &mut state);

            let offset = state.offset();
            for row_offset in 0..rows[2].height as usize {
                let visual_idx = offset + row_offset;
                if let Some(Some(cmd_idx)) = visual_to_command.get(visual_idx) {
                    app.modal_hit_zones.push(ModalHitZone {
                        rect: Rect::new(rows[2].x, rows[2].y + row_offset as u16, rows[2].width, 1),
                        kind: ModalHitKind::ListItem(*cmd_idx),
                    });
                }
            }

            let footer = if commands.is_empty() {
                key_hints_line(&[("No matches", ""), ("Esc", "close")])
            } else {
                key_hints_line(&[("Enter", "run"), ("↑↓", "move"), ("Esc", "close")])
            };
            frame.render_widget(Paragraph::new(footer), rows[4]);
        }
        ActiveModal::QueuedEdit {
            index,
            input,
            error,
        } => {
            let popup_width = area.width.saturating_sub(2).min(64);
            let editor_width = popup_width.saturating_sub(8).max(1) as usize;
            app.queued_editor_width = editor_width as u16;
            let wrapped = wrap_queued_editor(&input.text, editor_width);
            let editor_lines = wrapped.len() as u16;
            let popup_height = (editor_lines + 6)
                .clamp(8, 14)
                .min(area.height.saturating_sub(2));
            let popup = centered_rect_fixed(area, popup_width, popup_height);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, "Edit queued message", true);

            let has_error = error.is_some();
            let mut constraints = vec![Constraint::Length(1), Constraint::Min(1)];
            if has_error {
                constraints.push(Constraint::Length(1));
            }
            constraints.push(Constraint::Length(1));
            let rows = Layout::vertical(constraints).split(inner);
            let editor_area = rows[1];
            let error_area = has_error.then(|| rows[2]);
            let hint_area = rows[rows.len() - 1];

            frame.render_widget(
                Paragraph::new(Span::styled(
                    format!("Message {}", index + 1),
                    Style::default().fg(theme::p().muted),
                )),
                rows[0],
            );

            let text_style = Style::default().fg(theme::p().text);
            let cursor_style = Style::default()
                .fg(theme::p().surface)
                .bg(theme::p().accent)
                .add_modifier(Modifier::BOLD);
            let (cursor_row, cursor_col) =
                queued_editor_cursor(&wrapped, &input.text, input.cursor);
            let visible = editor_area.height as usize;
            let scroll = cursor_row.saturating_sub(visible.saturating_sub(1));
            for (offset, (start, end)) in wrapped.iter().enumerate().skip(scroll).take(visible) {
                let y = editor_area.y + (offset - scroll) as u16;
                let line_rect = Rect::new(editor_area.x, y, editor_area.width, 1);
                let mut spans = vec![Span::styled("› ", Style::default().fg(theme::p().accent))];
                let body = &input.text[*start..*end];
                if offset == cursor_row {
                    let before: String = body.chars().take(cursor_col).collect();
                    let mut after = body.chars().skip(cursor_col);
                    spans.push(Span::styled(before, text_style));
                    match after.next() {
                        Some(ch) => {
                            spans.push(Span::styled(ch.to_string(), cursor_style));
                            let rest: String = after.collect();
                            if !rest.is_empty() {
                                spans.push(Span::styled(rest, text_style));
                            }
                        }
                        None => spans.push(Span::styled(" ", cursor_style)),
                    }
                } else {
                    spans.push(Span::styled(body.to_string(), text_style));
                }
                frame.render_widget(Paragraph::new(Line::from(spans)), line_rect);
            }

            if let Some(message) = error
                && let Some(error_area) = error_area
            {
                frame.render_widget(
                    Paragraph::new(Span::styled(message, Style::default().fg(theme::p().error))),
                    error_area,
                );
            }

            frame.render_widget(
                Paragraph::new(key_hints_line(&[
                    ("Enter", "save"),
                    ("alt+enter", "newline"),
                    ("Esc", "cancel"),
                ])),
                hint_area,
            );
        }
        ActiveModal::Terminal {
            run_id: _,
            command,
            output,
            running,
            scroll,
        } => {
            let popup = centered_rect(area, 80, 70, 50, 10);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, "Terminal", true);
            let rows = Layout::vertical([
                Constraint::Length(2),
                Constraint::Min(3),
                Constraint::Length(1),
            ])
            .split(inner);
            let status = if running {
                "running… (shell timeout 120s)".to_string()
            } else {
                String::new()
            };
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(vec![
                        Span::styled("$ ", Style::default().fg(theme::p().accent)),
                        Span::styled(
                            command.clone(),
                            Style::default()
                                .fg(theme::p().text)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(Span::styled(status, Style::default().fg(theme::p().muted))),
                ]),
                rows[0],
            );
            let body = output.as_deref().unwrap_or("");
            frame.render_widget(
                Paragraph::new(body)
                    .wrap(Wrap { trim: false })
                    .scroll(((scroll).min(u16::MAX as usize) as u16, 0)),
                rows[1],
            );
            frame.render_widget(
                Paragraph::new(key_hints_line(&[
                    ("j/k", "scroll"),
                    ("Esc", "background"),
                    ("q", "close"),
                ])),
                rows[2],
            );
        }
        ActiveModal::Question {
            request,
            answers,
            custom,
            editing_custom,
            current,
            focus,
        } => {
            let prompt = &request.questions[current];
            let title = if request.questions.len() > 1 {
                format!(
                    "Question {}/{} — {}",
                    current + 1,
                    request.questions.len(),
                    prompt.header
                )
            } else {
                format!("Question — {}", prompt.header)
            };
            let popup = cap_width(centered_rect(area, 72, 72, 48, 18), 76);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, &title, false);

            let q_width = inner.width.max(1) as usize;
            let wrapped_q_lines = wrap_count(&prompt.question, q_width).clamp(1, 4) as u16;
            let rows = Layout::vertical([
                Constraint::Length(wrapped_q_lines),
                Constraint::Length(1),
                Constraint::Min(2),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(inner);

            frame.render_widget(
                Paragraph::new(prompt.question.clone())
                    .style(
                        Style::default()
                            .fg(theme::p().text)
                            .add_modifier(Modifier::BOLD),
                    )
                    .wrap(Wrap { trim: false }),
                rows[0],
            );

            let mode_label = if prompt.multiple {
                "Select all that apply · Other accepts typed text"
            } else {
                "Select one · Other accepts typed text"
            };
            frame.render_widget(
                Paragraph::new(Span::styled(
                    mode_label,
                    Style::default().fg(theme::p().muted),
                )),
                rows[1],
            );

            let mut y = rows[2].y;
            let max_y = rows[2].y + rows[2].height;
            for (index, option) in prompt.options.iter().enumerate() {
                if y >= max_y {
                    break;
                }
                let is_selected = answers[current].contains(&index);
                let is_focused = index == focus;
                let is_hovered = app.hovered_modal_hit == Some(ModalHitKind::ListItem(index));
                let marker = if prompt.multiple {
                    if is_selected { "[x]" } else { "[ ]" }
                } else if is_selected {
                    "(•)"
                } else {
                    "( )"
                };
                let row_bg = if is_focused || is_hovered {
                    theme::p().surface_high
                } else {
                    theme::p().surface
                };
                let prefix = if is_focused { "▸ " } else { "  " };
                let label_style = if is_focused {
                    Style::default()
                        .fg(theme::p().interactive)
                        .bg(row_bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::p().text).bg(row_bg)
                };
                let marker_style = Style::default()
                    .fg(if is_selected {
                        theme::p().accent
                    } else if is_focused {
                        theme::p().interactive
                    } else {
                        theme::p().muted
                    })
                    .bg(row_bg)
                    .add_modifier(Modifier::BOLD);

                let option_rect = Rect::new(rows[2].x, y, rows[2].width, 1);
                if is_focused || is_hovered {
                    frame.render_widget(
                        Block::default().style(Style::default().bg(row_bg)),
                        option_rect,
                    );
                }
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(prefix, label_style),
                        Span::styled(format!("{marker} "), marker_style),
                        Span::styled(format!("{}. {}", index + 1, option.label), label_style),
                    ])),
                    option_rect,
                );
                app.modal_hit_zones.push(ModalHitZone {
                    rect: option_rect,
                    kind: ModalHitKind::ListItem(index),
                });
                y += 1;

                if let Some(description) = &option.description
                    && y < max_y
                {
                    let desc_rect = Rect::new(rows[2].x, y, rows[2].width, 1);
                    frame.render_widget(
                        Paragraph::new(Span::styled(
                            format!(
                                "      {}",
                                truncate_single_line(
                                    description,
                                    (rows[2].width as usize).saturating_sub(6)
                                )
                            ),
                            Style::default().fg(theme::p().muted),
                        )),
                        desc_rect,
                    );
                    y += 1;
                }
            }

            let other = prompt.options.len();
            let input = &custom[current];
            let text = input.text.trim();
            let is_selected = !text.is_empty();
            let is_focused = focus == other;
            let is_hovered = app.hovered_modal_hit == Some(ModalHitKind::ListItem(other));
            let row_bg = if is_focused || is_hovered {
                theme::p().surface_high
            } else {
                theme::p().surface
            };
            let prefix = if is_focused { "▸ " } else { "  " };
            let marker = if prompt.multiple {
                if is_selected { "[x]" } else { "[ ]" }
            } else if is_selected {
                "(•)"
            } else {
                "( )"
            };
            let other_area = rows[3];
            if is_focused || is_hovered {
                frame.render_widget(
                    Block::default().style(Style::default().bg(row_bg)),
                    other_area,
                );
            }
            let label_style = if is_focused {
                Style::default()
                    .fg(theme::p().interactive)
                    .bg(row_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::p().text).bg(row_bg)
            };
            let marker_style = Style::default()
                .fg(if is_selected {
                    theme::p().accent
                } else if is_focused {
                    theme::p().interactive
                } else {
                    theme::p().muted
                })
                .bg(row_bg)
                .add_modifier(Modifier::BOLD);
            let prefix_text = format!("{prefix}{marker} Other: ");
            let prefix_width = prefix_text.chars().count() as u16;
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(prefix, label_style),
                    Span::styled(format!("{marker} "), marker_style),
                    Span::styled("Other: ", label_style),
                ])),
                Rect::new(other_area.x, other_area.y, other_area.width, 1),
            );
            let text_area = Rect::new(
                other_area.x.saturating_add(prefix_width),
                other_area.y,
                other_area.width.saturating_sub(prefix_width),
                other_area.height,
            );
            if editing_custom || !text.is_empty() {
                let mut scroll = 0;
                render_text_block(
                    frame,
                    text_area,
                    input,
                    Style::default().fg(theme::p().text).bg(row_bg),
                    &mut scroll,
                    editing_custom && is_focused,
                );
            } else if text_area.width > 0 {
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        "Other (type your own)",
                        Style::default().fg(theme::p().faint).bg(row_bg),
                    )),
                    Rect::new(text_area.x, text_area.y, text_area.width, 1),
                );
            }
            app.modal_hit_zones.push(ModalHitZone {
                rect: other_area,
                kind: ModalHitKind::ListItem(other),
            });

            let action_label = if prompt.multiple { "toggle" } else { "select" };
            frame.render_widget(
                Paragraph::new(key_hints_line(&[
                    ("↑↓", "move"),
                    ("Space", action_label),
                    ("type", "other"),
                    ("shift+enter", "newline"),
                    ("Tab", "next"),
                    ("Enter", "submit"),
                    ("Esc", "cancel"),
                ])),
                rows[4],
            );
        }
        ActiveModal::Approval {
            queue,
            reason,
            editing_reason,
        } => {
            let Some(request) = queue.first() else {
                return;
            };
            let popup = cap_width(centered_rect(area, 68, 54, 52, 16), 76);
            dim_backdrop(frame, area, popup);
            let title = if queue.len() > 1 {
                format!("Approval required ({})", queue.len())
            } else {
                "Approval required".to_string()
            };
            let inner = dialog_frame_styled(
                frame,
                app,
                popup,
                &title,
                Style::default()
                    .fg(theme::p().error)
                    .add_modifier(Modifier::BOLD),
                false,
            );

            let rows = Layout::vertical([
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(1),
                Constraint::Length(if editing_reason { 4 } else { 2 }),
            ])
            .split(inner);

            let kind = match request.kind {
                ApprovalKind::ToolCall => "tool call",
                ApprovalKind::Spend => "spend",
                ApprovalKind::SandboxEscape => "sandbox escape",
                ApprovalKind::ExternalAgent => "external agent permission",
            };
            let effect = match request.effect {
                Some(EffectClass::Read) => " · effect: read",
                Some(EffectClass::Write) => " · effect: write",
                Some(EffectClass::Execute) => " · effect: execute",
                Some(EffectClass::External) => " · effect: external",
                None => "",
            };
            let effect_color = match request.effect {
                Some(EffectClass::Write) => theme::p().warning,
                Some(EffectClass::Execute) | Some(EffectClass::External) => theme::p().error,
                Some(EffectClass::Read) | None => theme::p().muted,
            };
            let header = vec![
                Line::from(Span::styled(
                    request.preview.title.clone(),
                    Style::default()
                        .fg(theme::p().text)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    format!("kind: {kind}{effect}"),
                    Style::default().fg(effect_color),
                )),
            ];
            frame.render_widget(Paragraph::new(header), rows[0]);

            frame.render_widget(
                Block::default().style(Style::default().bg(theme::p().background)),
                rows[2],
            );
            let detail_inner = Rect {
                x: rows[2].x.saturating_add(1),
                y: rows[2].y,
                width: rows[2].width.saturating_sub(2),
                height: rows[2].height,
            };
            let detail = if request.preview.detail.is_empty() {
                vec![Line::from(Span::styled(
                    "(no details)",
                    Style::default().fg(theme::p().faint),
                ))]
            } else {
                request
                    .preview
                    .detail
                    .lines()
                    .map(|line| Line::from(line.to_string()))
                    .collect()
            };
            frame.render_widget(
                Paragraph::new(detail)
                    .style(Style::default().fg(theme::p().text))
                    .wrap(Wrap { trim: false }),
                detail_inner,
            );

            if editing_reason {
                let reason_area = input_band(frame, rows[4], true);
                let prefix = "Reason  ";
                let prefix_width = prefix.chars().count() as u16;
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        prefix,
                        Style::default()
                            .fg(theme::p().warning)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Rect::new(reason_area.x, reason_area.y, reason_area.width, 1),
                );
                let text_area = Rect::new(
                    reason_area.x.saturating_add(prefix_width),
                    reason_area.y,
                    reason_area.width.saturating_sub(prefix_width),
                    reason_area.height.saturating_sub(1),
                );
                let mut scroll = 0;
                render_text_block(
                    frame,
                    text_area,
                    &reason,
                    Style::default().fg(theme::p().text),
                    &mut scroll,
                    true,
                );
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "Enter:send decline reason · shift+enter:newline · Esc:back",
                        Style::default().fg(theme::p().faint),
                    ))),
                    Rect::new(
                        rows[4].x,
                        rows[4].y + rows[4].height.saturating_sub(1),
                        rows[4].width,
                        1,
                    ),
                );
            } else {
                let actions = [
                    ("y accept", ApprovalClickAction::Accept, theme::p().success),
                    (
                        "t turn",
                        ApprovalClickAction::AcceptTurn,
                        theme::p().interactive,
                    ),
                    (
                        "a session",
                        ApprovalClickAction::AcceptSession,
                        theme::p().info,
                    ),
                    ("n deny", ApprovalClickAction::Decline, theme::p().warning),
                    (
                        "r reason",
                        ApprovalClickAction::DeclineReason,
                        theme::p().muted,
                    ),
                    ("c cancel", ApprovalClickAction::Cancel, theme::p().error),
                ];
                let mut spans = Vec::new();
                let mut col = rows[4].x;
                for (idx, (label, action, color)) in actions.iter().enumerate() {
                    if idx > 0 {
                        spans.push(Span::styled(" · ", Style::default().fg(theme::p().faint)));
                        col = col.saturating_add(3);
                    }
                    let w = label.chars().count() as u16;
                    let hovered =
                        app.hovered_modal_hit == Some(ModalHitKind::ApprovalAction(*action));
                    let style = if hovered {
                        Style::default()
                            .fg(theme::p().text)
                            .bg(theme::p().surface_high)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(*color).add_modifier(Modifier::BOLD)
                    };
                    if col + w <= rows[4].x + rows[4].width {
                        app.modal_hit_zones.push(ModalHitZone {
                            rect: Rect::new(col, rows[4].y, w, 1),
                            kind: ModalHitKind::ApprovalAction(*action),
                        });
                    }
                    spans.push(Span::styled(*label, style));
                    col = col.saturating_add(w);
                }
                frame.render_widget(
                    Paragraph::new(Line::from(spans)),
                    Rect::new(rows[4].x, rows[4].y, rows[4].width, 1),
                );
            }
        }
        _ => {}
    }
}
