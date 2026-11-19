use super::statusline::key_status_style;
use super::*;
use crate::app::{ApprovalClickAction, DiffListState, ProviderConnectionAction};
use crate::diff::DiffRow;

fn auth_file_display() -> String {
    meerai_common::settings::default_data_dir()
        .map(|dir| meerai_client::secrets::auth_file_path(&dir))
        .map(|path| meerai_core::env::display_path(&path))
        .unwrap_or_else(|| "auth.json".to_string())
}

pub(super) fn render_active_modal(frame: &mut Frame<'_>, app: &mut AppState, area: Rect) {
    let Some(modal) = app.modal.clone() else {
        return;
    };
    if area.width < 20 || area.height < 8 {
        return;
    }

    match modal {
        ActiveModal::ProviderSelect { filter, selected } => {
            let popup = cap_width(centered_rect(area, 68, 72, 46, 12), 68);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, "Connect Provider (/connect)", true);

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
            place_input_cursor(frame, filter_area, 8 + filter.chars().count());

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
            place_input_cursor(frame, input_area, input.chars().count());

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
                "•".repeat(api_key.chars().count())
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
                base_url.clone()
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
                place_input_cursor(frame, key_input_area, api_key.chars().count());
            } else {
                place_input_cursor(frame, url_input_area, base_url.chars().count());
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
            selected_recent,
            error,
        } => {
            let active_provider = app.tab().active_provider.clone();
            let active_model = app.tab().model.clone();
            let recent = app.recent_models_for_active_provider();
            let height = if recent.is_empty() {
                12
            } else {
                (12 + recent.len().min(8)) as u16
            };
            let popup = centered_rect_fixed(area, 64, height);
            dim_backdrop(frame, area, popup);
            let title = format!("Set Model [{active_provider}] (/model)");
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
                .map(|m| format!("(Enter keeps '{m}', or type model name)"))
                .unwrap_or_else(|| "(type model name, e.g. gpt-4o)".to_string());
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
            place_input_cursor(frame, input_area, 7 + input.chars().count());

            if recent.is_empty() {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "No recent models saved for this provider.",
                        Style::default().fg(theme::p().faint),
                    ))),
                    rows[3],
                );
            } else {
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        "RECENT MODELS",
                        Style::default()
                            .fg(theme::p().faint)
                            .add_modifier(Modifier::BOLD),
                    )),
                    rows[2],
                );
                let list_items: Vec<ListItem<'_>> = recent
                    .iter()
                    .enumerate()
                    .map(|(idx, m)| {
                        let is_active = m == &active_model;
                        let is_selected = selected_recent == Some(idx);
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
                if let Some(idx) = selected_recent {
                    state.select(Some(idx.min(recent.len() - 1)));
                }
                let list = List::new(list_items).highlight_style(selection_style());
                frame.render_stateful_widget(list, rows[3], &mut state);
                register_list_hit_zones(app, rows[3], state.offset(), recent.len());
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
                    ("↑↓", "recent"),
                    ("ctrl+d", "forget"),
                    ("ctrl+u", "clear"),
                    ("Esc", "close"),
                ])),
                rows[6],
            );
        }
        ActiveModal::DiffView {
            file_index,
            diff,
            scroll,
            list,
        } => {
            let popup = centered_rect(area, 92, 86, 60, 12);
            dim_backdrop(frame, area, popup);
            let file_pos = if app.file_changes.is_empty() {
                String::new()
            } else {
                format!(" [{}/{}]", file_index + 1, app.file_changes.len())
            };
            let title = format!(
                "Diff: {} ({}) +{} -{}{file_pos}",
                diff.path, diff.status, diff.additions, diff.deletions
            );
            let inner = dialog_frame(frame, app, popup, &title, true);

            let rows = Layout::vertical([
                Constraint::Min(3),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);

            let panes_area = match list {
                DiffListState::Open { selected } => {
                    let split = Layout::horizontal([
                        Constraint::Length(32),
                        Constraint::Length(1),
                        Constraint::Min(24),
                    ])
                    .split(rows[0]);
                    render_diff_file_list(frame, app, split[0], selected, file_index);
                    split[2]
                }
                DiffListState::Closed => rows[0],
            };
            let panes = Layout::horizontal([
                Constraint::Percentage(50),
                Constraint::Length(1),
                Constraint::Percentage(50),
            ])
            .split(panes_area);

            let left_body = pane_header(frame, panes[0], &format!("Original (a/{})", diff.path));
            let right_body =
                pane_header(frame, panes[2], &format!("Working Tree (b/{})", diff.path));

            let view_h = left_body.height.max(1) as usize;
            let max_scroll = diff.rows.len().saturating_sub(view_h);
            let effective_scroll = scroll.min(max_scroll);
            let visible: Vec<&DiffRow> = diff
                .rows
                .iter()
                .skip(effective_scroll)
                .take(view_h)
                .collect();
            let language = crate::syntax::language_for_path(diff_new_path(&diff.path));

            let left_lines = diff_pane_lines(&visible, true, left_body.width as usize, language);
            let right_lines = diff_pane_lines(&visible, false, right_body.width as usize, language);

            frame.render_widget(Paragraph::new(left_lines), left_body);
            frame.render_widget(Paragraph::new(right_lines), right_body);
            let hints = if matches!(list, DiffListState::Open { .. }) {
                key_hints_line(&[
                    ("↑↓", "select file"),
                    ("Enter", "open"),
                    ("f", "hide list"),
                    ("←→/[/]", "next/prev"),
                    ("Esc", "hide list"),
                    ("q", "close"),
                ])
            } else {
                key_hints_line(&[
                    ("j/k", "scroll"),
                    ("ctrl+d/u", "page"),
                    ("g/G", "top/bot"),
                    ("f", "files"),
                    ("←→/[/]", "next/prev file"),
                    ("Esc/q", "close"),
                ])
            };
            frame.render_widget(Paragraph::new(hints), rows[2]);
        }
        ActiveModal::Search {
            query,
            mode,
            results,
            selected,
            preview,
            ..
        } => {
            let popup = centered_rect(area, 90, 84, 60, 14);
            dim_backdrop(frame, area, popup);
            let title = format!(
                "Search [{}] ({} matches) (/search)",
                mode.label(),
                results.len()
            );
            let inner = dialog_frame(frame, app, popup, &title, true);

            let layout = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(5),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);

            let placeholder = match mode {
                SearchMode::Files => "(type to fuzzy-match file paths)",
                SearchMode::Content => "(type query to search content with ripgrep)",
            };
            let query_inner = input_band(frame, layout[0], true);
            let mode_badge = format!("[{}]", mode.label());
            let mode_badge_w = mode_badge.chars().count() as u16;
            let input_box = Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("{mode_badge} "),
                    Style::default()
                        .fg(theme::p().interactive)
                        .add_modifier(Modifier::BOLD),
                ),
                if query.is_empty() {
                    Span::styled(placeholder, Style::default().fg(theme::p().faint))
                } else {
                    Span::styled(query.clone(), Style::default().fg(theme::p().text))
                },
            ]));
            frame.render_widget(input_box, query_inner);
            app.modal_hit_zones.push(ModalHitZone {
                rect: Rect::new(query_inner.x, query_inner.y, mode_badge_w, 1),
                kind: ModalHitKind::SearchModeToggle,
            });
            place_input_cursor(
                frame,
                query_inner,
                mode.label().chars().count() + 3 + query.chars().count(),
            );

            let panes = Layout::horizontal([
                Constraint::Percentage(45),
                Constraint::Length(1),
                Constraint::Percentage(55),
            ])
            .split(layout[2]);

            let results_inner = pane_header(frame, panes[0], "Results");
            let results_inner_w = results_inner.width as usize;
            let selected_idx = if results.is_empty() {
                0
            } else {
                selected.min(results.len() - 1)
            };

            let list_items: Vec<ListItem<'_>> = results
                .iter()
                .enumerate()
                .map(|(idx, m)| {
                    let is_hovered = app.hovered_modal_hit == Some(ModalHitKind::ListItem(idx));
                    let text = match m.line {
                        Some(ln) => {
                            let col = m.column.unwrap_or(1);
                            format!("{}:{ln}:{col}  {}", m.path, m.snippet)
                        }
                        None => m.path.clone(),
                    };
                    let row = ListItem::new(Line::from(format!(
                        " {}",
                        truncate_single_line(&text, results_inner_w.saturating_sub(2))
                    )));
                    if is_hovered && idx != selected_idx {
                        row.style(hover_row_style())
                    } else {
                        row
                    }
                })
                .collect();

            let mut list_state = ListState::default();
            if !results.is_empty() {
                list_state.select(Some(selected_idx));
            }
            let list = List::new(list_items).highlight_style(selection_style());
            frame.render_stateful_widget(list, results_inner, &mut list_state);
            register_list_hit_zones(app, results_inner, list_state.offset(), results.len());

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
                            let gutter = format!("{ln:>4}  ");
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
                Paragraph::new(key_hints_line(&[
                    ("Tab", "Files/Content"),
                    ("↑↓", "select"),
                    ("PgUp/PgDn", "scroll preview"),
                    ("Enter", "insert @file"),
                    ("Esc", "close"),
                ])),
                layout[4],
            );
        }
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
            place_input_cursor(frame, filter_area, 2 + filter.chars().count());

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
            let popup = centered_rect_fixed(area, 64, 9);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, "Edit queued message", true);

            let rows = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
            ])
            .split(inner);

            frame.render_widget(
                Paragraph::new(Span::styled(
                    format!("Message {}", index + 1),
                    Style::default().fg(theme::p().muted),
                )),
                rows[0],
            );

            let text = if input.is_empty() {
                Span::styled("Type a new message", Style::default().fg(theme::p().faint))
            } else {
                Span::styled(input.clone(), Style::default().fg(theme::p().text))
            };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("› ", Style::default().fg(theme::p().accent)),
                    text,
                ])),
                rows[1],
            );

            if let Some(message) = error {
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        message.clone(),
                        Style::default().fg(theme::p().error),
                    )),
                    rows[2],
                );
            }

            frame.render_widget(
                Paragraph::new(key_hints_line(&[("Enter", "save"), ("Esc", "cancel")])),
                rows[4],
            );
        }
        ActiveModal::ServeDialog {
            port_input,
            secret_input,
            editing_secret,
            error,
            busy,
        } => {
            let popup = centered_rect_fixed(area, 64, 14);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, "Start Browser UI (/serve)", !busy);

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

            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        "Bind  ",
                        Style::default()
                            .fg(theme::p().muted)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "127.0.0.1 (Basic auth user: meerai)",
                        Style::default().fg(theme::p().text),
                    ),
                ])),
                rows[0],
            );

            let port_focus = !editing_secret;
            let port_label_style = if port_focus {
                Style::default()
                    .fg(theme::p().interactive)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::p().muted)
            };
            frame.render_widget(
                Paragraph::new(Span::styled("Port (optional)", port_label_style)),
                rows[2],
            );
            let port_band_rect = rows[3];
            let port_area = input_band(frame, port_band_rect, port_focus);
            let (port_text, port_style) = if port_input.is_empty() {
                (
                    "(leave blank for default port 28860)".to_string(),
                    Style::default().fg(theme::p().faint),
                )
            } else {
                (port_input.clone(), Style::default().fg(theme::p().text))
            };
            frame.render_widget(
                Paragraph::new(Span::styled(port_text, port_style)),
                port_area,
            );
            app.modal_hit_zones.push(ModalHitZone {
                rect: port_band_rect,
                kind: ModalHitKind::ServeField(false),
            });

            let secret_label_style = if editing_secret {
                Style::default()
                    .fg(theme::p().interactive)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::p().muted)
            };
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "Password (min 8 chars, optional)",
                    secret_label_style,
                )),
                rows[5],
            );
            let secret_band_rect = rows[6];
            let secret_area = input_band(frame, secret_band_rect, editing_secret);
            let (secret_text, secret_style) = if secret_input.is_empty() {
                (
                    "(leave blank for default: 12345678)".to_string(),
                    Style::default().fg(theme::p().faint),
                )
            } else {
                (
                    "•".repeat(secret_input.chars().count()),
                    Style::default().fg(theme::p().text),
                )
            };
            frame.render_widget(
                Paragraph::new(Span::styled(secret_text, secret_style)),
                secret_area,
            );
            app.modal_hit_zones.push(ModalHitZone {
                rect: secret_band_rect,
                kind: ModalHitKind::ServeField(true),
            });

            if port_focus {
                place_input_cursor(frame, port_area, port_input.chars().count());
            } else {
                place_input_cursor(frame, secret_area, secret_input.chars().count());
            }

            if busy {
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        "Starting server...",
                        Style::default().fg(theme::p().warning),
                    )),
                    rows[8],
                );
            } else if let Some(err) = error {
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        truncate_single_line(&err, rows[8].width as usize),
                        Style::default().fg(theme::p().error),
                    )),
                    rows[8],
                );
            }

            frame.render_widget(
                Paragraph::new(key_hints_line(&[
                    ("Enter", "start"),
                    ("Tab", "field"),
                    ("Esc", "cancel"),
                ])),
                rows[9],
            );
        }
        ActiveModal::RewindMenu { selected, .. } => {
            let popup = cap_width(centered_rect(area, 60, 48, 44, 11), 60);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, "Rewind", true);

            let rows = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(2),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);

            frame.render_widget(
                Paragraph::new(Span::styled(
                    "Choose what to restore from the last turn:",
                    Style::default().fg(theme::p().muted),
                )),
                rows[0],
            );

            let options = [
                (
                    "1. Rewind code + conversation",
                    "restore files & shadow turn",
                ),
                ("2. Rewind code only", "restore files, keep chat"),
            ];
            let selected_idx = selected.min(1);
            let list_items: Vec<ListItem<'_>> = options
                .iter()
                .enumerate()
                .map(|(idx, (label, desc))| {
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
                        Span::styled(format!("  — {desc}"), Style::default().fg(theme::p().faint)),
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
                    ("↑↓", "move"),
                    ("Enter", "apply"),
                    ("Esc", "cancel"),
                ])),
                rows[4],
            );
        }
        ActiveModal::Terminal {
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
            let popup = cap_width(centered_rect(area, 72, 72, 48, 14), 76);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, &title, false);

            let q_width = inner.width.max(1) as usize;
            let wrapped_q_lines = wrap_count(&prompt.question, q_width).clamp(1, 4) as u16;
            let rows = Layout::vertical([
                Constraint::Length(wrapped_q_lines),
                Constraint::Length(1),
                Constraint::Min(2),
                Constraint::Length(1),
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

            if y < max_y {
                let other = prompt.options.len();
                let text = custom[current].trim();
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
                let label = if is_focused && editing_custom {
                    format!("Other: {text}▏")
                } else if is_selected {
                    format!("Other: {text}")
                } else {
                    "Other (type your own)".to_string()
                };
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
                let other_rect = Rect::new(rows[2].x, y, rows[2].width, 1);
                if is_focused || is_hovered {
                    frame.render_widget(
                        Block::default().style(Style::default().bg(row_bg)),
                        other_rect,
                    );
                }
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(prefix, label_style),
                        Span::styled(format!("{marker} "), marker_style),
                        Span::styled(label, label_style),
                    ])),
                    other_rect,
                );
                app.modal_hit_zones.push(ModalHitZone {
                    rect: other_rect,
                    kind: ModalHitKind::ListItem(other),
                });
            }

            let action_label = if prompt.multiple { "toggle" } else { "select" };
            frame.render_widget(
                Paragraph::new(key_hints_line(&[
                    ("↑↓", "move"),
                    ("Space", action_label),
                    ("type", "other"),
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
            let popup = cap_width(centered_rect(area, 68, 54, 52, 14), 76);
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
                Constraint::Length(2),
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
                let reason_area = input_band(
                    frame,
                    Rect::new(rows[4].x, rows[4].y, rows[4].width, 1),
                    true,
                );
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(
                            "Reason  ",
                            Style::default()
                                .fg(theme::p().warning)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(reason.clone(), Style::default().fg(theme::p().text)),
                        Span::styled("█", Style::default().fg(theme::p().warning)),
                    ])),
                    reason_area,
                );
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "Enter:send decline reason  Esc:back",
                        Style::default().fg(theme::p().faint),
                    ))),
                    Rect::new(rows[4].x, rows[4].y + 1, rows[4].width, 1),
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
                    ("esc cancel", ApprovalClickAction::Cancel, theme::p().error),
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
        ActiveModal::SessionSelect {
            filter,
            selected,
            sessions,
            loading,
            error,
        } => {
            let popup = cap_width(centered_rect(area, 68, 74, 48, 13), 72);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, "Switch Session (/sessions)", true);

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
                    Span::styled(
                        "(type to filter by id, title, or model)",
                        Style::default().fg(theme::p().faint),
                    )
                } else {
                    Span::styled(filter.clone(), Style::default().fg(theme::p().text))
                },
            ]);
            frame.render_widget(Paragraph::new(filter_line), filter_area);
            place_input_cursor(frame, filter_area, 8 + filter.chars().count());

            let filtered = app.filtered_sessions(&sessions, &filter);
            let selected_idx = selected.min(filtered.len().saturating_sub(1));
            if loading && filtered.is_empty() {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "Loading sessions...",
                        Style::default().fg(theme::p().faint),
                    ))),
                    rows[2],
                );
            } else if let Some(err) = &error {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        format!("Failed to list sessions: {err}"),
                        Style::default().fg(theme::p().error),
                    ))),
                    rows[2],
                );
            } else if filtered.is_empty() {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "No matching sessions.",
                        Style::default().fg(theme::p().faint),
                    ))),
                    rows[2],
                );
            } else {
                let now = now_millis();
                let max_title = (inner.width as usize).saturating_sub(38).max(16);
                let list_items: Vec<ListItem<'_>> = filtered
                    .iter()
                    .enumerate()
                    .map(|(idx, summary)| {
                        let open_idx = app.tab_index_for_session(&summary.id);
                        let is_hovered = app.hovered_modal_hit == Some(ModalHitKind::ListItem(idx));
                        let (tab_badge, badge_style) = match open_idx {
                            Some(tab_i) if tab_i == app.active_tab => (
                                format!("[tab {}*] ", tab_i + 1),
                                Style::default()
                                    .fg(theme::p().success)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Some(tab_i) => (
                                format!("{:<9}", format!("[tab {}]", tab_i + 1)),
                                Style::default().fg(theme::p().accent),
                            ),
                            None => (
                                "         ".to_string(),
                                Style::default().fg(theme::p().faint),
                            ),
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
                            Span::raw(" "),
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
                        let row = ListItem::new(Line::from(item_spans));
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
                frame.render_stateful_widget(list, rows[2], &mut state);
                register_list_hit_zones(app, rows[2], state.offset(), filtered.len());
            }

            let selected_id = filtered.get(selected_idx).map(|summary| summary.id.clone());
            let confirming = app.session_delete_confirm.is_some()
                && app.session_delete_confirm.as_ref() == selected_id.as_ref();
            let confirming_current = confirming
                && selected_id
                    .as_ref()
                    .and_then(|id| app.tab_index_for_session(id))
                    == Some(app.active_tab_index());
            let footer_line = if loading {
                key_hints_line(&[
                    ("Refreshing...", ""),
                    ("Enter", "switch"),
                    ("↑↓", "move"),
                    ("Esc", "close"),
                ])
            } else if confirming {
                let text = if confirming_current && app.tabs().len() == 1 {
                    "Press ctrl+d again to delete and open a new session · Esc cancel"
                } else {
                    "Press ctrl+d again to delete · Esc cancel"
                };
                Line::from(Span::styled(
                    text,
                    Style::default()
                        .fg(theme::p().error)
                        .add_modifier(Modifier::BOLD),
                ))
            } else {
                key_hints_line(&[
                    ("Enter", "switch"),
                    ("ctrl+d", "delete"),
                    ("↑↓", "move"),
                    ("Type", "filter"),
                    ("Esc", "close"),
                ])
            };
            frame.render_widget(Paragraph::new(footer_line), rows[4]);
        }
    }
}

fn place_input_cursor(frame: &mut Frame<'_>, area: Rect, char_offset: usize) {
    if area.width > 0 && area.height > 0 {
        let cursor_x = (area.x + char_offset as u16).min(area.x + area.width.saturating_sub(1));
        frame.set_cursor_position(Position::new(cursor_x, area.y));
    }
}

fn register_list_hit_zones(app: &mut AppState, area: Rect, offset: usize, len: usize) {
    for row in 0..area.height as usize {
        let idx = offset + row;
        if idx >= len {
            break;
        }
        app.modal_hit_zones.push(ModalHitZone {
            rect: Rect::new(area.x, area.y + row as u16, area.width, 1),
            kind: ModalHitKind::ListItem(idx),
        });
    }
}

fn wrap_count(text: &str, width: usize) -> usize {
    if width == 0 {
        return 1;
    }
    text.split('\n')
        .map(|line| {
            let chars = line.chars().count();
            chars.div_ceil(width).max(1)
        })
        .sum::<usize>()
        .max(1)
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

fn diff_new_path(path: &str) -> &str {
    path.split_once(" -> ").map(|(_, new)| new).unwrap_or(path)
}

enum DiffPaneRow {
    Plain(Line<'static>),
    Cell {
        gutter: String,
        body: String,
        style: Style,
    },
}

fn diff_pane_lines(
    rows: &[&DiffRow],
    is_left: bool,
    pane_width: usize,
    language: Option<crate::syntax::Language>,
) -> Vec<Line<'static>> {
    let mut pane = Vec::with_capacity(rows.len());
    let mut source = String::new();
    for (idx, row) in rows.iter().enumerate() {
        if idx > 0 {
            source.push('\n');
        }
        let entry = if let DiffRowKind::HunkHeader(header) = &row.kind {
            DiffPaneRow::Plain(Line::from(Span::styled(
                truncate_single_line(header, pane_width),
                Style::default()
                    .fg(theme::p().accent)
                    .add_modifier(Modifier::BOLD),
            )))
        } else {
            match diff_cell(row, is_left) {
                None => DiffPaneRow::Plain(Line::from(Span::styled(
                    "     · ",
                    Style::default().fg(theme::p().faint),
                ))),
                Some(cell) => {
                    let gutter = diff_gutter(cell.line_no, &row.kind, is_left);
                    let body = truncate_single_line(
                        &cell.text,
                        pane_width.saturating_sub(gutter.chars().count()),
                    );
                    source.push_str(&body);
                    DiffPaneRow::Cell {
                        gutter,
                        body,
                        style: diff_base_style(&row.kind, is_left),
                    }
                }
            }
        };
        pane.push(entry);
    }

    let Some(language) = language else {
        return pane
            .into_iter()
            .map(|row| match row {
                DiffPaneRow::Plain(line) => line,
                DiffPaneRow::Cell {
                    gutter,
                    body,
                    style,
                } => pane_line(gutter, vec![Span::styled(body, style)]),
            })
            .collect();
    };

    let tokens = crate::syntax::token_lines(Some(language), &source);
    pane.into_iter()
        .enumerate()
        .map(|(idx, row)| match row {
            DiffPaneRow::Plain(line) => line,
            DiffPaneRow::Cell {
                gutter,
                body,
                style,
            } => {
                let line_tokens = tokens.get(idx).map(Vec::as_slice).unwrap_or(&[]);
                let body_spans = if line_tokens.is_empty() {
                    vec![Span::styled(body, style)]
                } else {
                    crate::syntax::line_spans(line_tokens, style)
                };
                pane_line(gutter, body_spans)
            }
        })
        .collect()
}

fn diff_cell(row: &DiffRow, is_left: bool) -> Option<&DiffCell> {
    if is_left {
        row.left.as_ref()
    } else {
        row.right.as_ref()
    }
}

fn pane_line(gutter: String, body: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = Vec::with_capacity(body.len() + 1);
    spans.push(Span::styled(gutter, Style::default().fg(theme::p().faint)));
    spans.extend(body);
    Line::from(spans)
}

fn diff_gutter(line_no: Option<usize>, kind: &DiffRowKind, is_left: bool) -> String {
    let prefix = match (kind, is_left) {
        (DiffRowKind::Removed, true) | (DiffRowKind::Modified, true) => "-",
        (DiffRowKind::Added, false) | (DiffRowKind::Modified, false) => "+",
        _ => " ",
    };
    match line_no {
        Some(n) => format!("{n:>4} {prefix} "),
        None => format!("     {prefix} "),
    }
}

fn diff_base_style(kind: &DiffRowKind, is_left: bool) -> Style {
    match (kind, is_left) {
        (DiffRowKind::Removed, true) | (DiffRowKind::Modified, true) => {
            Style::default().fg(theme::p().diff_removed)
        }
        (DiffRowKind::Added, false) | (DiffRowKind::Modified, false) => {
            Style::default().fg(theme::p().diff_added)
        }
        _ => Style::default().fg(theme::p().diff_context),
    }
}

fn render_diff_file_list(
    frame: &mut Frame<'_>,
    app: &mut AppState,
    area: Rect,
    selected: usize,
    file_index: usize,
) {
    let inner = pane_header(frame, area, &format!("Files ({})", app.file_changes.len()));
    if app.file_changes.is_empty() || inner.height == 0 {
        return;
    }
    let width = inner.width as usize;
    let selected_idx = selected.min(app.file_changes.len() - 1);
    let items: Vec<ListItem<'_>> = app
        .file_changes
        .iter()
        .enumerate()
        .map(|(idx, entry)| {
            let is_current = idx == file_index;
            let is_selected = idx == selected_idx;
            let is_hovered = app.hovered_modal_hit == Some(ModalHitKind::ListItem(idx));
            let marker = if is_current { "● " } else { "  " };
            let prefix_width = 1 + marker.chars().count() + entry.status.chars().count() + 1;
            let path_style = if is_selected {
                Style::default()
                    .fg(theme::p().accent)
                    .add_modifier(Modifier::BOLD)
            } else if is_current {
                Style::default()
                    .fg(theme::p().text)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::p().text)
            };
            let row = ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {marker}"),
                    Style::default().fg(if is_current {
                        theme::p().accent
                    } else {
                        theme::p().faint
                    }),
                ),
                Span::styled(
                    format!("{} ", entry.status),
                    Style::default().fg(theme::p().muted),
                ),
                Span::styled(
                    truncate_single_line(&entry.path, width.saturating_sub(prefix_width)),
                    path_style,
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
    state.select(Some(selected_idx));
    let list = List::new(items).highlight_style(Style::default().bg(theme::p().surface_high));
    frame.render_stateful_widget(list, inner, &mut state);
    register_list_hit_zones(app, inner, state.offset(), app.file_changes.len());
}
