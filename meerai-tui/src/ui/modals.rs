use super::statusline::key_status_style;
use super::*;

pub(super) fn render_active_modal(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let Some(modal) = &app.modal else {
        return;
    };
    if area.width < 20 || area.height < 8 {
        return;
    }

    match modal {
        ActiveModal::ProviderSelect { filter, selected } => {
            let popup = cap_width(centered_rect(area, 66, 68, 42, 10), 64);
            let inner = dialog_frame(frame, popup, "Connect Provider (/connect)");

            let rows = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(1),
            ])
            .split(inner);

            let filter_line = Line::from(vec![
                Span::styled(
                    "Filter ",
                    Style::default()
                        .fg(theme::p().muted)
                        .add_modifier(Modifier::BOLD),
                ),
                if filter.is_empty() {
                    Span::styled("(type to filter)", Style::default().fg(theme::p().faint))
                } else {
                    Span::styled(filter.clone(), Style::default().fg(theme::p().text))
                },
            ]);
            let filter_area = input_band(frame, rows[0]);
            frame.render_widget(Paragraph::new(filter_line), filter_area);

            let providers = app.filtered_providers(filter);
            let active_provider = &app.tab().active_provider;
            let list_items: Vec<ListItem<'_>> = providers
                .iter()
                .map(|(p, status)| {
                    let (badge, color) = key_status_style(status);
                    let active_mark = if p.id == *active_provider { "*" } else { " " };
                    ListItem::new(Line::from(vec![
                        Span::styled(
                            format!("{active_mark} "),
                            Style::default().fg(theme::p().faint),
                        ),
                        Span::styled(
                            p.display_name.clone(),
                            Style::default()
                                .fg(theme::p().text)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(" ({}) ", p.id),
                            Style::default().fg(theme::p().faint),
                        ),
                        Span::styled(format!("[{badge}]"), Style::default().fg(color)),
                    ]))
                })
                .collect();

            let mut state = ListState::default();
            if !providers.is_empty() {
                state.select(Some((*selected).min(providers.len() - 1)));
            }
            let list = List::new(list_items).highlight_style(selection_style());
            frame.render_stateful_widget(list, rows[1], &mut state);

            let footer = Paragraph::new(Line::from(Span::styled(
                "Enter configure · Up/Down move · Type filter · Esc close",
                Style::default().fg(theme::p().faint),
            )));
            frame.render_widget(footer, rows[2]);
        }
        ActiveModal::ApiKeyInput {
            provider_id,
            provider_name,
            api_key,
            base_url,
            editing_base_url,
            existing_key_status,
            error,
        } => {
            let popup = centered_rect_fixed(area, 64, 13);
            let title = format!("Connect {provider_name} ({provider_id})");
            let inner = dialog_frame(frame, popup, &title);

            let (badge, badge_color) = key_status_style(existing_key_status);
            let masked_key = if api_key.is_empty() {
                if existing_key_status.is_configured() {
                    "(press Enter to keep existing key, or type new key)".to_string()
                } else {
                    "(paste or type API key)".to_string()
                }
            } else {
                "•".repeat(api_key.chars().count())
            };

            let default_url = app
                .active_provider_default_base_url(provider_id)
                .unwrap_or_else(|| "provider default".to_string());
            let url_display = if base_url.is_empty() {
                format!("({default_url})")
            } else {
                base_url.clone()
            };

            let key_label_style = if !editing_base_url {
                Style::default()
                    .fg(theme::p().interactive)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::p().muted)
            };
            let url_label_style = if *editing_base_url {
                Style::default()
                    .fg(theme::p().interactive)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::p().muted)
            };

            let mut lines = vec![
                Line::from(vec![
                    Span::styled(
                        "Status    ",
                        Style::default()
                            .fg(theme::p().muted)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("[{badge}]"), Style::default().fg(badge_color)),
                ]),
                Line::default(),
                Line::from(vec![
                    Span::styled("API key   ", key_label_style),
                    Span::styled(masked_key, Style::default().fg(theme::p().text)),
                ]),
                Line::from(vec![
                    Span::styled("Base URL  ", url_label_style),
                    Span::styled(url_display, Style::default().fg(theme::p().text)),
                ]),
                Line::default(),
            ];

            if let Some(err) = error {
                lines.push(Line::from(Span::styled(
                    format!("Error: {err}"),
                    Style::default().fg(theme::p().error),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    "Saved in ~/.local/share/meerai/auth.json (0600).",
                    Style::default().fg(theme::p().faint),
                )));
            }
            lines.push(Line::from(Span::styled(
                "Enter save · Tab switch field · Ctrl-d delete key · Esc back",
                Style::default().fg(theme::p().faint),
            )));

            frame.render_widget(Paragraph::new(lines), inner);
        }
        ActiveModal::ModelInput {
            input,
            selected_recent,
            error,
        } => {
            let active_provider = &app.tab().active_provider;
            let active_model = &app.tab().model;
            let recent = app.recent_models_for_active_provider();
            let height = if recent.is_empty() {
                9
            } else {
                (8 + recent.len().min(8)) as u16
            };
            let popup = centered_rect_fixed(area, 60, height);
            let title = format!("Set Model [{active_provider}] (/model)");
            let inner = dialog_frame(frame, popup, &title);

            let rows = Layout::vertical([
                Constraint::Length(2),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(inner);

            let placeholder = app
                .current_or_default_model(active_provider)
                .map(|m| format!("(Enter keeps '{m}', or type model name)"))
                .unwrap_or_else(|| "(type model name, e.g. gpt-4o)".to_string());
            let input_display = if input.is_empty() {
                Span::styled(placeholder, Style::default().fg(theme::p().faint))
            } else {
                Span::styled(
                    input.as_str().to_string(),
                    Style::default().fg(theme::p().text),
                )
            };
            let input_lines = vec![Line::from(vec![
                Span::styled(
                    "Model  ",
                    Style::default()
                        .fg(theme::p().interactive)
                        .add_modifier(Modifier::BOLD),
                ),
                input_display,
            ])];
            let input_area = input_band(
                frame,
                Rect {
                    height: 1,
                    ..rows[0]
                },
            );
            frame.render_widget(Paragraph::new(input_lines), input_area);

            if recent.is_empty() {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "No recent models saved for this provider.",
                        Style::default().fg(theme::p().faint),
                    ))),
                    rows[1],
                );
            } else {
                let list_items: Vec<ListItem<'_>> = recent
                    .iter()
                    .map(|m| {
                        let active_mark = if m == active_model { "*" } else { " " };
                        ListItem::new(Line::from(vec![
                            Span::styled(
                                format!("{active_mark} "),
                                Style::default().fg(theme::p().faint),
                            ),
                            Span::styled(
                                m.clone(),
                                Style::default()
                                    .fg(theme::p().text)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ]))
                    })
                    .collect();
                let mut state = ListState::default();
                if let Some(idx) = selected_recent {
                    state.select(Some((*idx).min(recent.len() - 1)));
                }
                let list = List::new(list_items).highlight_style(selection_style());
                frame.render_stateful_widget(list, rows[1], &mut state);
            }

            let status_line = if let Some(err) = error {
                Line::from(Span::styled(
                    format!("Error: {err}"),
                    Style::default().fg(theme::p().error),
                ))
            } else {
                Line::from(Span::styled(
                    "Any model name is accepted; validation runs on request.",
                    Style::default().fg(theme::p().faint),
                ))
            };
            let footer_lines = vec![
                status_line,
                Line::from(Span::styled(
                    "Enter apply · Up/Down recent · Ctrl-u clear · Esc close",
                    Style::default().fg(theme::p().faint),
                )),
            ];
            frame.render_widget(Paragraph::new(footer_lines), rows[2]);
        }
        ActiveModal::DiffView {
            file_index,
            diff,
            scroll,
        } => {
            let popup = centered_rect(area, 92, 84, 60, 12);
            let file_pos = if app.file_changes.is_empty() {
                String::new()
            } else {
                format!(" [{}/{}]", file_index + 1, app.file_changes.len())
            };
            let title = format!(
                "Diff: {} ({}) +{} -{}{file_pos}",
                diff.path, diff.status, diff.additions, diff.deletions
            );
            let inner = dialog_frame(frame, popup, &title);

            let rows = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).split(inner);
            let panes = Layout::horizontal([
                Constraint::Percentage(50),
                Constraint::Length(1),
                Constraint::Percentage(50),
            ])
            .split(rows[0]);

            let left_header = pane_header(frame, panes[0], &format!("Original (a/{})", diff.path));
            let right_header =
                pane_header(frame, panes[2], &format!("Working Tree (b/{})", diff.path));

            let left_inner_w = left_header.width as usize;
            let right_inner_w = right_header.width as usize;
            let view_h = left_header.height.max(1) as usize;
            let max_scroll = diff.rows.len().saturating_sub(view_h);
            let effective_scroll = (*scroll).min(max_scroll);

            let mut left_lines = Vec::with_capacity(view_h);
            let mut right_lines = Vec::with_capacity(view_h);
            for row in diff.rows.iter().skip(effective_scroll).take(view_h) {
                left_lines.push(format_diff_pane_line(
                    row.left.as_ref(),
                    &row.kind,
                    true,
                    left_inner_w,
                ));
                right_lines.push(format_diff_pane_line(
                    row.right.as_ref(),
                    &row.kind,
                    false,
                    right_inner_w,
                ));
            }

            frame.render_widget(Paragraph::new(left_lines), left_header);
            frame.render_widget(Paragraph::new(right_lines), right_header);
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "j/k scroll · Ctrl-d/u page · g/G top/bot · Tab/[/] next/prev file · Esc/q close",
                    Style::default().fg(theme::p().faint),
                ))),
                rows[1],
            );
        }
        ActiveModal::Search {
            query,
            mode,
            results,
            selected,
            preview,
            ..
        } => {
            let popup = centered_rect(area, 90, 82, 60, 14);
            let title = format!(
                "Search [{}] ({} matches) (/search)",
                mode.label(),
                results.len()
            );
            let inner = dialog_frame(frame, popup, &title);

            let layout = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(5),
                Constraint::Length(1),
            ])
            .split(inner);

            let mode_color = match mode {
                SearchMode::Files => theme::p().muted,
                SearchMode::Content => theme::p().muted,
            };
            let placeholder = match mode {
                SearchMode::Files => "(type to fuzzy-match file paths)",
                SearchMode::Content => "(type query to search content with ripgrep)",
            };
            let query_inner = input_band(frame, layout[0]);
            let input_box = Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("[{}] ", mode.label()),
                    Style::default().fg(mode_color).add_modifier(Modifier::BOLD),
                ),
                if query.is_empty() {
                    Span::styled(placeholder, Style::default().fg(theme::p().faint))
                } else {
                    Span::styled(query.clone(), Style::default().fg(theme::p().text))
                },
            ]));
            frame.render_widget(input_box, query_inner);
            if query_inner.width > 0 && query_inner.height > 0 {
                let prefix_chars = mode.label().chars().count() + 3;
                let query_chars = query.chars().count();
                let cursor_x = (query_inner.x + (prefix_chars + query_chars) as u16)
                    .min(query_inner.x + query_inner.width.saturating_sub(1));
                frame.set_cursor_position(Position::new(cursor_x, query_inner.y));
            }

            let panes = Layout::horizontal([
                Constraint::Percentage(45),
                Constraint::Length(1),
                Constraint::Percentage(55),
            ])
            .split(layout[1]);

            let results_inner = pane_header(frame, panes[0], "Results");
            let results_inner_w = results_inner.width as usize;

            let list_items: Vec<ListItem<'_>> = results
                .iter()
                .map(|m| {
                    let text = match m.line {
                        Some(ln) => {
                            let col = m.column.unwrap_or(1);
                            format!("{}:{ln}:{col}  {}", m.path, m.snippet)
                        }
                        None => m.path.clone(),
                    };
                    ListItem::new(Line::from(truncate_single_line(&text, results_inner_w)))
                })
                .collect();

            let mut list_state = ListState::default();
            if !results.is_empty() {
                list_state.select(Some((*selected).min(results.len() - 1)));
            }
            let list = List::new(list_items).highlight_style(selection_style());
            frame.render_stateful_widget(list, results_inner, &mut list_state);

            let preview_title = preview
                .as_ref()
                .map(|p| format!("Preview: {}", p.title))
                .unwrap_or_else(|| "Preview".to_string());
            let preview_inner = pane_header(frame, panes[2], &preview_title);
            let preview_w = preview_inner.width as usize;
            let preview_h = preview_inner.height.max(1) as usize;

            let preview_lines: Vec<Line<'_>> = match preview {
                Some(prev) => {
                    let max_scroll = prev.lines.len().saturating_sub(preview_h);
                    let scroll = prev.scroll.min(max_scroll);
                    prev.lines
                        .iter()
                        .skip(scroll)
                        .take(preview_h)
                        .map(|(ln, text, highlighted)| {
                            let gutter = format!("{ln:>4} │ ");
                            let body = truncate_single_line(
                                text,
                                preview_w.saturating_sub(gutter.chars().count()),
                            );
                            if *highlighted {
                                Line::from(vec![
                                    Span::styled(
                                        gutter,
                                        Style::default()
                                            .fg(theme::p().warning)
                                            .bg(theme::p().surface_high)
                                            .add_modifier(Modifier::BOLD),
                                    ),
                                    Span::styled(
                                        body,
                                        Style::default()
                                            .fg(theme::p().text)
                                            .bg(theme::p().surface_high)
                                            .add_modifier(Modifier::BOLD),
                                    ),
                                ])
                                .style(Style::default().bg(theme::p().surface_high))
                            } else {
                                Line::from(vec![
                                    Span::styled(gutter, Style::default().fg(theme::p().faint)),
                                    Span::styled(body, Style::default().fg(theme::p().text)),
                                ])
                            }
                        })
                        .collect()
                }
                None => vec![Line::from(Span::styled(
                    "No matching results.",
                    Style::default().fg(theme::p().faint),
                ))],
            };
            frame.render_widget(Paragraph::new(preview_lines), preview_inner);

            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "Tab Files/Content · Up/Down select · PgUp/PgDn scroll preview · Enter insert @file · Esc close",
                    Style::default().fg(theme::p().faint),
                ))),
                layout[2],
            );
        }
        ActiveModal::UserMessageActions {
            entry_idx,
            selected,
        } => {
            let popup = centered_rect_fixed(area, 40, 8);
            let inner = dialog_frame(frame, popup, "Message");

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
            let list_items: Vec<ListItem<'_>> = items
                .iter()
                .map(|(_, label, key)| {
                    ListItem::new(Line::from(vec![
                        Span::styled(format!(" {label}"), Style::default().fg(theme::p().text)),
                        Span::raw("  "),
                        Span::styled(*key, Style::default().fg(theme::p().faint)),
                    ]))
                })
                .collect();
            let mut state = ListState::default();
            state.select(Some((*selected).min(2)));
            let list = List::new(list_items).highlight_style(selection_style());
            frame.render_stateful_widget(list, rows[0], &mut state);
            let _ = entry_idx;

            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "Enter run · Esc close",
                    Style::default().fg(theme::p().faint),
                ))),
                rows[2],
            );
        }
        ActiveModal::CommandPalette { filter, selected } => {
            let popup = centered_rect(area, 50, 62, 46, 12);
            let popup = Rect {
                x: popup.x + popup.width.saturating_sub(58) / 2,
                width: popup.width.min(58),
                ..popup
            };
            let inner = dialog_frame(frame, popup, "Commands (Ctrl-p)");

            let rows = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(1),
            ])
            .split(inner);

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
            let filter_area = input_band(frame, rows[0]);
            frame.render_widget(Paragraph::new(filter_line), filter_area);

            let commands = crate::app::filtered_commands(filter);
            let width = rows[1].width as usize;
            let mut items: Vec<ListItem<'_>> = Vec::new();
            let mut command_rows: Vec<usize> = Vec::new();
            let mut section = "";
            for command in &commands {
                if command.section != section {
                    section = command.section;
                    items.push(ListItem::new(Line::from(Span::styled(
                        format!(" {} ", section.to_uppercase()),
                        Style::default()
                            .fg(theme::p().faint)
                            .add_modifier(Modifier::BOLD),
                    ))));
                }
                command_rows.push(items.len());
                let label = format!(" {}", command.label);
                let label_len = label.chars().count();
                let hint_len = command.hint.chars().count();
                let gap = width.saturating_sub(label_len + hint_len + 1).max(1);
                let hint_style = if command.hint.starts_with('/') {
                    Style::default().fg(theme::p().faint)
                } else {
                    Style::default().fg(theme::p().muted)
                };
                items.push(ListItem::new(Line::from(vec![
                    Span::styled(label, Style::default().fg(theme::p().text)),
                    Span::raw(" ".repeat(gap)),
                    Span::styled(command.hint, hint_style),
                ])));
            }

            let mut state = ListState::default();
            if !commands.is_empty() {
                let idx = (*selected).min(commands.len() - 1);
                state.select(Some(command_rows[idx]));
            }
            let list = List::new(items).highlight_style(selection_style());
            frame.render_stateful_widget(list, rows[1], &mut state);

            let footer = if commands.is_empty() {
                "No matches · Esc close"
            } else {
                "Enter run · ↑↓ move · Esc close"
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    footer,
                    Style::default().fg(theme::p().faint),
                ))),
                rows[2],
            );
        }
        ActiveModal::RewindMenu { selected, .. } => {
            let popup = cap_width(centered_rect(area, 60, 50, 40, 9), 60);
            let inner = dialog_frame(frame, popup, "Rewind");
            let options = ["Rewind code + conversation", "Rewind code only"];
            let mut lines = vec![Line::from(Span::styled(
                "Choose what to restore from the last turn:",
                Style::default().fg(theme::p().muted),
            ))];
            for (index, option) in options.iter().enumerate() {
                let style = if index == *selected {
                    Style::default()
                        .fg(theme::p().interactive)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::p().text)
                };
                let prefix = if index == *selected { "> " } else { "  " };
                lines.push(Line::from(Span::styled(
                    format!("{prefix}{}. {option}", index + 1),
                    style,
                )));
            }
            lines.push(Line::from(Span::styled(
                "Up/Down move · Enter apply · Esc cancel",
                Style::default().fg(theme::p().faint),
            )));
            for (offset, line) in lines.into_iter().enumerate() {
                if offset as u16 >= inner.height {
                    break;
                }
                frame.render_widget(
                    Paragraph::new(line),
                    Rect {
                        x: inner.x,
                        y: inner.y + offset as u16,
                        width: inner.width,
                        height: 1,
                    },
                );
            }
        }
        ActiveModal::Question {
            request,
            answers,
            current,
            focus,
        } => {
            let prompt = &request.questions[*current];
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
            let popup = cap_width(centered_rect(area, 70, 70, 44, 10), 72);
            let inner = dialog_frame(frame, popup, &title);
            let mut lines = vec![
                Line::from(Span::styled(
                    prompt.question.clone(),
                    Style::default()
                        .fg(theme::p().text)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
            ];
            for (index, option) in prompt.options.iter().enumerate() {
                let selected = answers[*current].contains(&index);
                let marker = if selected { "[x]" } else { "[ ]" };
                let prefix = if index == *focus {
                    format!(">{marker} ")
                } else {
                    format!(" {marker} ")
                };
                let style = if index == *focus {
                    Style::default()
                        .fg(theme::p().interactive)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::p().text)
                };
                lines.push(Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(format!("{}. {}", index + 1, option.label), style),
                ]));
                if let Some(description) = &option.description {
                    lines.push(Line::from(Span::styled(
                        format!("      {description}"),
                        Style::default().fg(theme::p().muted),
                    )));
                }
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                if prompt.multiple {
                    "Up/Down move · Space toggle · Tab next · Enter submit · Esc cancel"
                } else {
                    "Up/Down move · Space select · Tab next · Enter submit · Esc cancel"
                },
                Style::default().fg(theme::p().faint),
            )));
            for (offset, line) in lines.into_iter().enumerate() {
                if offset as u16 >= inner.height {
                    break;
                }
                frame.render_widget(
                    Paragraph::new(line),
                    Rect {
                        x: inner.x,
                        y: inner.y + offset as u16,
                        width: inner.width,
                        height: 1,
                    },
                );
            }
        }
        ActiveModal::Approval {
            queue,
            reason,
            editing_reason,
        } => {
            let Some(request) = queue.first() else {
                return;
            };
            let popup = cap_width(centered_rect(area, 64, 46, 46, 12), 72);
            let title = if queue.len() > 1 {
                format!("Approval required ({})", queue.len())
            } else {
                "Approval required".to_string()
            };
            let inner = dialog_frame_styled(
                frame,
                popup,
                &title,
                Style::default()
                    .fg(theme::p().error)
                    .add_modifier(Modifier::BOLD),
            );

            let rows = Layout::vertical([
                Constraint::Length(2),
                Constraint::Min(3),
                Constraint::Length(3),
            ])
            .split(inner);

            let kind = match request.kind {
                ApprovalKind::ToolCall => "tool call",
                ApprovalKind::Spend => "spend",
                ApprovalKind::SandboxEscape => "sandbox escape",
            };
            let effect = match request.effect {
                Some(EffectClass::Read) => " · effect: read",
                Some(EffectClass::Write) => " · effect: write",
                Some(EffectClass::Execute) => " · effect: execute",
                Some(EffectClass::External) => " · effect: external",
                None => "",
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
                    Style::default().fg(theme::p().faint),
                )),
            ];
            frame.render_widget(Paragraph::new(header), rows[0]);

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
                rows[1],
            );

            let footer = if *editing_reason {
                vec![
                    Line::from(vec![
                        Span::styled("Reason  ", Style::default().fg(theme::p().warning)),
                        Span::styled(
                            reason.as_str().to_string(),
                            Style::default().fg(theme::p().text),
                        ),
                        Span::styled("█", Style::default().fg(theme::p().warning)),
                    ]),
                    Line::from(Span::styled(
                        "Enter:send decline reason  Esc:back",
                        Style::default().fg(theme::p().faint),
                    )),
                ]
            } else {
                vec![
                    Line::from(Span::styled(
                        "y accept · t turn · a session · n deny · r reason · esc cancel",
                        Style::default().fg(theme::p().faint),
                    )),
                    Line::default(),
                ]
            };
            frame.render_widget(Paragraph::new(footer), rows[2]);
        }
        ActiveModal::SessionSelect {
            filter,
            selected,
            sessions,
            loading,
            error,
        } => {
            let popup = cap_width(centered_rect(area, 66, 72, 46, 12), 68);
            let inner = dialog_frame(frame, popup, "Switch Session (/sessions)");

            let rows = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(1),
            ])
            .split(inner);

            let filter_line = Line::from(vec![
                Span::styled(
                    "Filter ",
                    Style::default()
                        .fg(theme::p().muted)
                        .add_modifier(Modifier::BOLD),
                ),
                if filter.is_empty() {
                    Span::styled(
                        "(type to filter by id, title, or model)",
                        Style::default().fg(theme::p().faint),
                    )
                } else {
                    Span::styled(filter.clone(), Style::default().fg(theme::p().text))
                },
            ]);
            let filter_area = input_band(frame, rows[0]);
            frame.render_widget(Paragraph::new(filter_line), filter_area);

            let filtered = app.filtered_sessions(sessions, filter);
            if *loading && filtered.is_empty() {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "Loading sessions...",
                        Style::default().fg(theme::p().faint),
                    ))),
                    rows[1],
                );
            } else if let Some(err) = error {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        format!("Failed to list sessions: {err}"),
                        Style::default().fg(theme::p().error),
                    ))),
                    rows[1],
                );
            } else if filtered.is_empty() {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "No matching sessions.",
                        Style::default().fg(theme::p().faint),
                    ))),
                    rows[1],
                );
            } else {
                let now = now_millis();
                let max_title = (inner.width as usize).saturating_sub(36).max(16);
                let list_items: Vec<ListItem<'_>> = filtered
                    .iter()
                    .map(|summary| {
                        let open_idx = app.tab_index_for_session(&summary.id);
                        let (tab_badge, badge_style) = match open_idx {
                            Some(idx) if idx == app.active_tab => (
                                format!("[tab {}*] ", idx + 1),
                                Style::default()
                                    .fg(theme::p().success)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Some(idx) => (
                                format!("[tab {}] ", idx + 1),
                                Style::default().fg(theme::p().accent),
                            ),
                            None => ("       ".to_string(), Style::default().fg(theme::p().faint)),
                        };
                        let raw_title = summary
                            .title
                            .as_deref()
                            .filter(|t| !t.trim().is_empty())
                            .unwrap_or("(untitled)");
                        let title_text = truncate_single_line(raw_title, max_title);
                        let title_style = if summary
                            .title
                            .as_deref()
                            .is_some_and(|t| !t.trim().is_empty())
                        {
                            Style::default()
                                .fg(theme::p().text)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(theme::p().faint)
                        };
                        let mut item_spans = vec![
                            Span::styled(tab_badge, badge_style),
                            Span::styled(title_text, title_style),
                            Span::styled(
                                format!("  {}", summary.id),
                                Style::default().fg(theme::p().faint),
                            ),
                        ];
                        if let Some(model) =
                            summary.model.as_deref().filter(|m| !m.trim().is_empty())
                        {
                            item_spans.push(Span::styled(
                                format!(" · {}", truncate_single_line(model, 22)),
                                Style::default().fg(theme::p().muted),
                            ));
                        }
                        if summary.updated_at_ms > 0 {
                            item_spans.push(Span::styled(
                                format!(" · {}", format_relative_age(now, summary.updated_at_ms)),
                                Style::default().fg(theme::p().faint),
                            ));
                        }
                        ListItem::new(Line::from(item_spans))
                    })
                    .collect();

                let mut state = ListState::default();
                state.select(Some((*selected).min(filtered.len() - 1)));
                let list = List::new(list_items).highlight_style(selection_style());
                frame.render_stateful_widget(list, rows[1], &mut state);
            }

            let selected_id = filtered
                .get((*selected).min(filtered.len().saturating_sub(1)))
                .map(|summary| summary.id.clone());
            let confirming = app.session_delete_confirm.is_some()
                && app.session_delete_confirm.as_ref() == selected_id.as_ref();
            let footer_text = if *loading {
                "Refreshing... · Enter switch · Up/Down move · Esc close"
            } else if confirming {
                "Press Ctrl-d again to delete · Esc cancel"
            } else {
                "Enter switch · Ctrl-d delete · Up/Down move · Type filter · Esc close"
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    footer_text,
                    Style::default().fg(if confirming {
                        theme::p().error
                    } else {
                        theme::p().faint
                    }),
                ))),
                rows[2],
            );
        }
    }
}

fn format_relative_age(now_ms: u64, updated_ms: u64) -> String {
    let diff_secs = now_ms.saturating_sub(updated_ms) / 1000;
    if diff_secs < 60 {
        "just now".to_string()
    } else if diff_secs < 3600 {
        format!("{}m ago", diff_secs / 60)
    } else if diff_secs < 86_400 {
        format!("{}h ago", diff_secs / 3600)
    } else {
        format!("{}d ago", diff_secs / 86_400)
    }
}

fn format_diff_pane_line(
    cell: Option<&DiffCell>,
    kind: &DiffRowKind,
    is_left: bool,
    pane_width: usize,
) -> Line<'static> {
    if let DiffRowKind::HunkHeader(header) = kind {
        return Line::from(Span::styled(
            truncate_single_line(header, pane_width),
            Style::default()
                .fg(theme::p().accent)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let Some(cell) = cell else {
        return Line::from(Span::styled("~", Style::default().fg(theme::p().faint)));
    };

    let prefix = match (kind, is_left) {
        (DiffRowKind::Removed, true) | (DiffRowKind::Modified, true) => "-",
        (DiffRowKind::Added, false) | (DiffRowKind::Modified, false) => "+",
        _ => " ",
    };
    let line_num = match cell.line_no {
        Some(n) => format!("{n:>4} {prefix} "),
        None => format!("     {prefix} "),
    };
    let body = truncate_single_line(
        &cell.text,
        pane_width.saturating_sub(line_num.chars().count()),
    );

    let style = match (kind, is_left) {
        (DiffRowKind::Removed, true) | (DiffRowKind::Modified, true) => {
            Style::default().fg(theme::p().diff_removed)
        }
        (DiffRowKind::Added, false) | (DiffRowKind::Modified, false) => {
            Style::default().fg(theme::p().diff_added)
        }
        _ => Style::default().fg(theme::p().diff_context),
    };

    Line::from(vec![
        Span::styled(line_num, Style::default().fg(theme::p().faint)),
        Span::styled(body, style),
    ])
}
