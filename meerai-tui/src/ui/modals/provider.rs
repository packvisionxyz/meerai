use super::*;

pub(super) fn render_provider_modal(
    app: &mut AppState,
    frame: &mut Frame<'_>,
    area: Rect,
    modal: ActiveModal,
) {
    match modal {
        ActiveModal::ProviderSelect { filter, selected } => {
            let popup = cap_width(centered_rect(area, 68, 72, 46, 12), 68);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, "Connect provider (/connect)", true);

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
                    "Filter  ",
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
            frame.render_widget(Paragraph::new(filter_line), filter_area);
            place_input_cursor(frame, filter_area, 8 + filter.cursor_col());

            let providers = app.filtered_providers(&filter);
            let active_provider = app.tab().active_provider.clone();
            let selected_idx = if providers.is_empty() {
                0
            } else {
                selected.min(providers.len() - 1)
            };
            let list_width = rows[2].width as usize;
            let list_items: Vec<ListItem<'_>> = providers
                .iter()
                .enumerate()
                .map(|(idx, (p, status))| {
                    let (badge, color) = key_status_style(status);
                    let badge_text = format!("[{badge}]");
                    let is_active = p.id == active_provider;
                    let is_selected = idx == selected_idx;
                    let is_hovered = app.hovered_modal_hit == Some(ModalHitKind::ListItem(idx));
                    let marker = if is_active { "● " } else { "  " };
                    let marker_color = if is_active {
                        theme::p().accent
                    } else {
                        theme::p().faint
                    };
                    let id_text = format!(" ({})", p.id);
                    let left_len = 2 + p.display_name.chars().count() + id_text.chars().count() + 1;
                    let badge_len = badge_text.chars().count();
                    let gap = list_width.saturating_sub(left_len + badge_len + 1).max(1);
                    let row = ListItem::new(Line::from(vec![
                        Span::styled(" ", Style::default()),
                        Span::styled(marker, Style::default().fg(marker_color)),
                        Span::styled(
                            p.display_name.clone(),
                            Style::default()
                                .fg(theme::p().text)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(id_text, Style::default().fg(theme::p().faint)),
                        Span::raw(" ".repeat(gap)),
                        Span::styled(badge_text, Style::default().fg(color)),
                    ]));
                    if is_hovered && !is_selected {
                        row.style(hover_row_style())
                    } else {
                        row
                    }
                })
                .collect();

            let mut state = ListState::default();
            if !providers.is_empty() {
                state.select(Some(selected_idx));
            }
            let list = List::new(list_items).highlight_style(selection_style());
            frame.render_stateful_widget(list, rows[2], &mut state);
            register_list_hit_zones(app, rows[2], state.offset(), providers.len());

            let footer = Paragraph::new(key_hints_line(&[
                ("Enter", "configure"),
                ("↑↓", "move"),
                ("Type", "filter"),
                ("Esc", "close"),
            ]));
            frame.render_widget(footer, rows[4]);
        }
        ActiveModal::AuthMethodSelect {
            provider_id,
            provider_name,
            selected,
        } => {
            let popup = centered_rect_fixed(area, 66, 11);
            dim_backdrop(frame, area, popup);
            let title = format!("Connect {provider_name} ({provider_id})");
            let inner = dialog_frame(frame, app, popup, &title, true);

            let rows = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(2),
                Constraint::Length(1),
            ])
            .split(inner);

            frame.render_widget(
                Paragraph::new(Span::styled(
                    "ChatGPT Plus/Pro plan usage",
                    Style::default().fg(theme::p().muted),
                )),
                rows[0],
            );

            let options = ["Continue with ChatGPT", "Use an API key"];
            let selected_idx = selected.min(options.len() - 1);
            let list_items: Vec<ListItem<'_>> = options
                .iter()
                .enumerate()
                .map(|(idx, label)| {
                    let is_selected = idx == selected_idx;
                    let is_hovered = app.hovered_modal_hit == Some(ModalHitKind::ListItem(idx));
                    let prefix = if is_selected { "▸ " } else { "  " };
                    let row = ListItem::new(Line::from(vec![
                        Span::styled(
                            prefix,
                            Style::default().fg(if is_selected {
                                theme::p().interactive
                            } else {
                                theme::p().faint
                            }),
                        ),
                        Span::styled(*label, Style::default().fg(theme::p().text)),
                    ]));
                    if is_hovered && !is_selected {
                        row.style(hover_row_style())
                    } else {
                        row
                    }
                })
                .collect();
            let mut state = ListState::default();
            state.select(Some(selected_idx));
            let list = List::new(list_items).highlight_style(selection_style());
            frame.render_stateful_widget(list, rows[2], &mut state);
            register_list_hit_zones(app, rows[2], state.offset(), options.len());

            frame.render_widget(
                Paragraph::new(key_hints_line(&[
                    ("Enter", "select"),
                    ("↑↓", "move"),
                    ("Esc", "back"),
                ])),
                rows[3],
            );
        }
        ActiveModal::ProviderConnection {
            provider_id,
            provider_name,
            status,
            selected,
        } => {
            let options = ProviderConnectionAction::options(&status);
            let popup = centered_rect_fixed(area, 66, 12);
            dim_backdrop(frame, area, popup);
            let title = format!("Connect {provider_name} ({provider_id})");
            let inner = dialog_frame(frame, app, popup, &title, true);

            let rows = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(1),
            ])
            .split(inner);

            let (badge, badge_color) = key_status_style(&status);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        "Status  ",
                        Style::default()
                            .fg(theme::p().muted)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(badge, Style::default().fg(badge_color)),
                ])),
                rows[0],
            );

            let selected_idx = selected.min(options.len().saturating_sub(1));
            let list_items: Vec<ListItem<'_>> = options
                .iter()
                .enumerate()
                .map(|(idx, action)| {
                    let is_selected = idx == selected_idx;
                    let is_hovered = app.hovered_modal_hit == Some(ModalHitKind::ListItem(idx));
                    let prefix = if is_selected { "▸ " } else { "  " };
                    let row = ListItem::new(Line::from(vec![
                        Span::styled(
                            prefix,
                            Style::default().fg(if is_selected {
                                theme::p().interactive
                            } else {
                                theme::p().faint
                            }),
                        ),
                        Span::styled(action.label(), Style::default().fg(theme::p().text)),
                    ]));
                    if is_hovered && !is_selected {
                        row.style(hover_row_style())
                    } else {
                        row
                    }
                })
                .collect();
            let mut state = ListState::default();
            if !options.is_empty() {
                state.select(Some(selected_idx));
            }
            let list = List::new(list_items).highlight_style(selection_style());
            frame.render_stateful_widget(list, rows[2], &mut state);
            register_list_hit_zones(app, rows[2], state.offset(), options.len());

            frame.render_widget(
                Paragraph::new(key_hints_line(&[
                    ("Enter", "select"),
                    ("↑↓", "move"),
                    ("Esc", "back"),
                ])),
                rows[3],
            );
        }
        ActiveModal::ChatGptLogin {
            provider_id,
            provider_name,
            url,
            input,
            error,
            busy,
        } => {
            let popup = centered_rect_fixed(area, 66, 19);
            dim_backdrop(frame, area, popup);
            let title = format!("Sign in with ChatGPT ({provider_id})");
            let inner = dialog_frame(frame, app, popup, &title, true);

            let rows = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(1),
            ])
            .split(inner);

            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        "Provider  ",
                        Style::default()
                            .fg(theme::p().muted)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(provider_name.clone(), Style::default().fg(theme::p().text)),
                ])),
                rows[0],
            );

            let status = if url.is_none() && busy {
                "Starting sign-in..."
            } else {
                "Waiting for the browser callback..."
            };
            frame.render_widget(
                Paragraph::new(Span::styled(
                    status,
                    Style::default().fg(if url.is_some() {
                        theme::p().interactive
                    } else {
                        theme::p().muted
                    }),
                )),
                rows[1],
            );

            let url_text = url.clone().unwrap_or_else(|| "(preparing URL)".to_string());
            frame.render_widget(
                Paragraph::new(Span::styled(
                    url_text,
                    Style::default().fg(theme::p().faint),
                ))
                .wrap(Wrap { trim: false }),
                rows[2],
            );

            if let Some(error) = error {
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        error.clone(),
                        Style::default().fg(theme::p().error),
                    ))
                    .wrap(Wrap { trim: false }),
                    rows[3],
                );
            }

            frame.render_widget(
                Paragraph::new(Span::styled(
                    "Paste the final redirect URL",
                    Style::default().fg(theme::p().muted),
                )),
                rows[4],
            );
            let input_area = input_band(frame, rows[5], true);
            let value = if input.is_empty() {
                Span::styled(
                    "(only needed when the browser cannot reach this host)",
                    Style::default().fg(theme::p().faint),
                )
            } else {
                Span::styled(input.clone(), Style::default().fg(theme::p().text))
            };
            frame.render_widget(Paragraph::new(value), input_area);
            place_input_cursor(frame, input_area, input.cursor_col());

            frame.render_widget(
                Paragraph::new(key_hints_line(&[
                    ("Enter", "paste"),
                    ("ctrl+y", "copy URL"),
                    ("Esc", "cancel"),
                ])),
                rows[7],
            );
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
            let popup = centered_rect_fixed(area, 66, 15);
            dim_backdrop(frame, area, popup);
            let title = format!("Connect {provider_name} ({provider_id})");
            let inner = dialog_frame(frame, app, popup, &title, true);

            let rows = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);

            let (badge, badge_color) = key_status_style(&existing_key_status);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        "Status  ",
                        Style::default()
                            .fg(theme::p().muted)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("[{badge}]"), Style::default().fg(badge_color)),
                ])),
                rows[0],
            );

            let key_focus = !editing_base_url;
            let key_label_style = if key_focus {
                Style::default()
                    .fg(theme::p().interactive)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::p().muted)
            };
            frame.render_widget(
                Paragraph::new(Span::styled("API key", key_label_style)),
                rows[2],
            );
            let key_band_rect = rows[3];
            let key_input_area = input_band(frame, key_band_rect, key_focus);
            let masked_key = if api_key.is_empty() {
                if existing_key_status.is_configured() {
                    "(press Enter to keep existing key, or type new key)".to_string()
                } else {
                    "(paste or type API key)".to_string()
                }
            } else {
                "•".repeat(api_key.cursor_col())
            };
            let key_val_style = if api_key.is_empty() {
                Style::default().fg(theme::p().faint)
            } else {
                Style::default().fg(theme::p().text)
            };
            frame.render_widget(
                Paragraph::new(Span::styled(masked_key, key_val_style)),
                key_input_area,
            );
            app.modal_hit_zones.push(ModalHitZone {
                rect: key_band_rect,
                kind: ModalHitKind::ApiKeyField(false),
            });

            let url_label_style = if editing_base_url {
                Style::default()
                    .fg(theme::p().interactive)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::p().muted)
            };
            frame.render_widget(
                Paragraph::new(Span::styled("Base URL", url_label_style)),
                rows[5],
            );
            let url_band_rect = rows[6];
            let url_input_area = input_band(frame, url_band_rect, editing_base_url);
            let default_url = app
                .active_provider_default_base_url(&provider_id)
                .unwrap_or_else(|| "provider default".to_string());
            let url_display = if base_url.is_empty() {
                format!("({default_url})")
            } else {
                base_url.text.clone()
            };
            let url_val_style = if base_url.is_empty() {
                Style::default().fg(theme::p().faint)
            } else {
                Style::default().fg(theme::p().text)
            };
            frame.render_widget(
                Paragraph::new(Span::styled(url_display, url_val_style)),
                url_input_area,
            );
            app.modal_hit_zones.push(ModalHitZone {
                rect: url_band_rect,
                kind: ModalHitKind::ApiKeyField(true),
            });

            if key_focus {
                place_input_cursor(frame, key_input_area, api_key.cursor_col());
            } else {
                place_input_cursor(frame, url_input_area, base_url.cursor_col());
            }

            let info_line = if let Some(err) = error {
                Line::from(Span::styled(
                    format!("Error: {err}"),
                    Style::default().fg(theme::p().error),
                ))
            } else {
                Line::from(Span::styled(
                    format!("Saved in {} (0600).", auth_file_display()),
                    Style::default().fg(theme::p().faint),
                ))
            };
            frame.render_widget(Paragraph::new(info_line), rows[8]);
            frame.render_widget(
                Paragraph::new(key_hints_line(&[
                    ("Enter", "save"),
                    ("Tab", "switch field"),
                    ("ctrl+d", "delete key"),
                    ("Esc", "back"),
                ])),
                rows[9],
            );
        }
        ActiveModal::ModelInput {
            input,
            selected,
            error,
        } => {
            let active_provider = app.tab().active_provider.clone();
            let active_model = app.tab().model.clone();
            let matches = app.model_candidates(&input);
            let height = if matches.is_empty() {
                12
            } else {
                (12 + matches.len().min(8)) as u16
            };
            let popup = centered_rect_fixed(area, 64, height);
            dim_backdrop(frame, area, popup);
            let title = format!("Set model [{active_provider}] (/model)");
            let inner = dialog_frame(frame, app, popup, &title, true);

            let rows = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);

            let placeholder = app
                .current_or_default_model(&active_provider)
                .map(|m| format!("(type to filter, or Enter keeps '{m}')"))
                .unwrap_or_else(|| "(type to filter, e.g. gpt or claude)".to_string());
            let input_display = if input.is_empty() {
                Span::styled(placeholder, Style::default().fg(theme::p().faint))
            } else {
                Span::styled(input.clone(), Style::default().fg(theme::p().text))
            };
            let input_area = input_band(frame, rows[0], true);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        "Model  ",
                        Style::default()
                            .fg(theme::p().interactive)
                            .add_modifier(Modifier::BOLD),
                    ),
                    input_display,
                ])),
                input_area,
            );
            place_input_cursor(frame, input_area, 7 + input.cursor_col());

            if matches.is_empty() {
                let text = if input.trim().is_empty() {
                    "No models are known for this provider.".to_string()
                } else {
                    format!(
                        "No catalog match for '{}' · Enter applies it as typed.",
                        input.trim()
                    )
                };
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        text,
                        Style::default().fg(theme::p().faint),
                    ))),
                    rows[3],
                );
            } else {
                let heading = if input.trim().is_empty() {
                    "MODELS".to_string()
                } else {
                    format!("MODELS · {} match", matches.len())
                };
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        heading,
                        Style::default()
                            .fg(theme::p().faint)
                            .add_modifier(Modifier::BOLD),
                    )),
                    rows[2],
                );
                let list_items: Vec<ListItem<'_>> = matches
                    .iter()
                    .enumerate()
                    .map(|(idx, m)| {
                        let is_active = m == &active_model;
                        let is_selected = selected == Some(idx);
                        let is_hovered = app.hovered_modal_hit == Some(ModalHitKind::ListItem(idx));
                        let active_mark = if is_active { "● " } else { "  " };
                        let row = ListItem::new(Line::from(vec![
                            Span::raw(" "),
                            Span::styled(
                                active_mark,
                                Style::default().fg(if is_active {
                                    theme::p().accent
                                } else {
                                    theme::p().faint
                                }),
                            ),
                            Span::styled(
                                m.clone(),
                                Style::default()
                                    .fg(theme::p().text)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ]));
                        if is_hovered && !is_selected {
                            row.style(hover_row_style())
                        } else {
                            row
                        }
                    })
                    .collect();
                let mut state = ListState::default();
                if let Some(idx) = selected {
                    state.select(Some(idx.min(matches.len() - 1)));
                }
                let list = List::new(list_items).highlight_style(selection_style());
                frame.render_stateful_widget(list, rows[3], &mut state);
                register_list_hit_zones(app, rows[3], state.offset(), matches.len());
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
            frame.render_widget(Paragraph::new(status_line), rows[5]);
            frame.render_widget(
                Paragraph::new(key_hints_line(&[
                    ("Enter", "apply"),
                    ("Tab", "complete"),
                    ("↑↓", "browse"),
                    ("ctrl+d", "forget"),
                    ("esc", "close"),
                ])),
                rows[6],
            );
        }
        _ => {}
    }
}
