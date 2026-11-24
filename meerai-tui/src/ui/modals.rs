use super::statusline::key_status_style;
use super::*;
use crate::app::{ApprovalClickAction, DiffListState, PackTab, ProviderConnectionAction};
use crate::diff::DiffRow;
use meerai_common::settings::EffortLevel;

fn auth_file_display() -> String {
    meerai_common::settings::default_data_dir()
        .map(|dir| dir.join("meerai.db"))
        .map(|path| meerai_core::env::display_path(&path))
        .unwrap_or_else(|| "meerai.db".to_string())
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
                "Diff (/diff): {} ({}) +{} -{}{file_pos}",
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
            cursor,
            error,
        } => {
            let popup_width = area.width.saturating_sub(2).min(64);
            let editor_width = popup_width.saturating_sub(8).max(1) as usize;
            app.queued_editor_width = editor_width as u16;
            let wrapped = wrap_queued_editor(&input, editor_width);
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
            let (cursor_row, cursor_col) = queued_editor_cursor(&wrapped, &input, cursor);
            let visible = editor_area.height as usize;
            let scroll = cursor_row.saturating_sub(visible.saturating_sub(1));
            for (offset, (start, end)) in wrapped.iter().enumerate().skip(scroll).take(visible) {
                let y = editor_area.y + (offset - scroll) as u16;
                let line_rect = Rect::new(editor_area.x, y, editor_area.width, 1);
                let mut spans = vec![Span::styled("› ", Style::default().fg(theme::p().accent))];
                let body = &input[*start..*end];
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
        ActiveModal::ServeDialog {
            port_input,
            secret_input,
            editing_secret,
            error,
            busy,
        } => {
            let popup = centered_rect_fixed(area, 64, 14);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, "Start browser UI (/serve)", !busy);

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
            let inner = dialog_frame(frame, app, popup, "Rewind (/rewind)", true);

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
        ActiveModal::Context(panel) => {
            let popup = cap_width(centered_rect(area, 78, 58, 50, 14), 78);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, "Context (/context)", true);
            let rows = Layout::vertical([
                Constraint::Min(6),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);
            let (text, scroll) = if let Some(input) = panel.focus_input.as_ref() {
                (
                    format!(
                        "Summarize from the selected turn.\n\nFocus (optional): {input}\n\nEnter sends; Esc cancels."
                    ),
                    0,
                )
            } else if let Some(content) = panel.blob_content.as_ref() {
                (
                    format!(
                        "Blob {}\n\n{content}",
                        panel.blob_key.as_deref().unwrap_or("")
                    ),
                    panel.blob_scroll,
                )
            } else {
                let mut lines = Vec::new();
                if let Some(breakdown) = panel.breakdown.as_ref() {
                    lines.push(format!(
                        "used {} / {} tokens ({:.1}%) · watermark {}",
                        breakdown.estimated_total_tokens,
                        breakdown.context_window_tokens,
                        if breakdown.context_window_tokens == 0 {
                            0.0
                        } else {
                            breakdown.estimated_total_tokens as f64 * 100.0
                                / breakdown.context_window_tokens as f64
                        },
                        breakdown.high_watermark_tokens
                    ));
                    lines.push(String::new());
                    lines.push(format!("system     {}", breakdown.system_prompt_tokens));
                    lines.push(format!("tools      {}", breakdown.tool_schemas_tokens));
                    lines.push(format!("memory     {}", breakdown.memory_tokens));
                    lines.push(format!("user       {}", breakdown.user_messages_tokens));
                    lines.push(format!("assistant  {}", breakdown.assistant_tokens));
                    lines.push(format!("results    {}", breakdown.tool_results_tokens));
                    lines.push(format!("reasoning  {}", breakdown.reasoning_tokens));
                    lines.push(format!(
                        "summary    {}",
                        breakdown.compaction_summary_tokens
                    ));
                    lines.push(String::new());
                    lines.push(format!(
                        "verbatim window {} · reasoning pruning {} · clear old results {}",
                        breakdown.verbatim_window_turns,
                        on_off(breakdown.reasoning_pruning),
                        on_off(breakdown.clear_old_tool_results),
                    ));
                    lines.push(format!(
                        "pinned {} · cleared {} · spilled {} · cached {}",
                        breakdown.pinned_turn_ids.len(),
                        breakdown.cleared_tool_results_count,
                        breakdown.spilled_blob_count,
                        breakdown
                            .last_prompt_cached_tokens
                            .map_or_else(|| "n/a".to_string(), |tokens| tokens.to_string())
                    ));
                    lines.push(String::new());
                    lines.push("Turns:".to_string());
                    for (index, turn) in breakdown.turn_ids.iter().enumerate() {
                        let pinned = breakdown.pinned_turn_ids.contains(turn);
                        lines.push(format!(
                            "{} {} {}",
                            if index == panel.selected_turn {
                                ">"
                            } else {
                                " "
                            },
                            if pinned { "★" } else { " " },
                            turn
                        ));
                    }
                    if !breakdown.spilled_blobs.is_empty() {
                        lines.push(String::new());
                        lines.push("Spilled blobs:".to_string());
                        for (index, key) in breakdown.spilled_blobs.iter().enumerate() {
                            lines.push(format!(
                                "{} {}",
                                if index == panel.selected_blob {
                                    ">"
                                } else {
                                    " "
                                },
                                key
                            ));
                        }
                    }
                } else {
                    lines.push(
                        "No context report yet. Press r to request one from the running agent."
                            .to_string(),
                    );
                }
                (lines.join("\n"), panel.scroll)
            };
            frame.render_widget(
                Paragraph::new(text)
                    .style(Style::default().fg(theme::p().text))
                    .scroll((scroll as u16, 0))
                    .wrap(Wrap { trim: false }),
                rows[0],
            );
            frame.render_widget(
                Paragraph::new(Span::styled(
                    panel.error.clone().unwrap_or_else(|| {
                        "Pinned turns stay verbatim; older tool results become stubs.".to_string()
                    }),
                    Style::default().fg(if panel.error.is_some() {
                        theme::p().error
                    } else {
                        theme::p().faint
                    }),
                )),
                rows[1],
            );
            frame.render_widget(
                key_hints_line(&[
                    ("j/k", "select"),
                    ("p", "pin"),
                    ("s", "summarize"),
                    ("f", "focus"),
                    ("b", "blob"),
                    ("t/c/C", "policy"),
                    ("r", "refresh"),
                    ("Esc", "close"),
                ]),
                rows[2],
            );
        }
        ActiveModal::Goal { state, scroll } => {
            let popup = cap_width(centered_rect(area, 78, 58, 50, 14), 78);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, "Goal (/goal)", true);
            let rows = Layout::vertical([Constraint::Min(6), Constraint::Length(1)]).split(inner);
            let text = match state {
                Some(state) => state.render(),
                None => "No active goal.\n\nType @goal <instruction> in the composer to start one."
                    .to_string(),
            };
            frame.render_widget(
                Paragraph::new(text)
                    .style(Style::default().fg(theme::p().text))
                    .scroll((scroll as u16, 0))
                    .wrap(Wrap { trim: false }),
                rows[0],
            );
            frame.render_widget(
                key_hints_line(&[("j/k", "scroll"), ("Esc", "close")]),
                rows[1],
            );
        }
        ActiveModal::Memory {
            entries,
            query,
            selected,
            loading,
            error,
        } => {
            let popup = cap_width(centered_rect(area, 86, 64, 50, 14), 86);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, "Project memory (/memory)", true);
            let rows = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(4),
                Constraint::Length(6),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);
            let header = if loading {
                "Loading...".to_string()
            } else if let Some(error) = error {
                format!("Error: {error}")
            } else if query.is_empty() {
                format!("{} memories", entries.len())
            } else {
                format!("{} matches for \"{query}\"", entries.len())
            };
            frame.render_widget(
                Paragraph::new(Span::styled(header, Style::default().fg(theme::p().muted))),
                rows[0],
            );
            let items: Vec<ListItem<'_>> = entries
                .iter()
                .map(|memory| {
                    let pin = if memory.pinned { "★ " } else { "  " };
                    ListItem::new(Line::from(vec![
                        Span::styled(pin, Style::default().fg(theme::p().accent)),
                        Span::styled(
                            format!("[{}] ", memory.kind.as_str()),
                            Style::default().fg(theme::p().faint),
                        ),
                        Span::styled(memory.title.clone(), Style::default().fg(theme::p().text)),
                    ]))
                })
                .collect();
            let mut state = ListState::default();
            state.select((!entries.is_empty()).then_some(selected));
            frame.render_stateful_widget(
                List::new(items).highlight_style(selection_style()),
                rows[1],
                &mut state,
            );
            let detail = entries
                .get(selected)
                .map(|memory| {
                    format!(
                        "{}\n\n{}\n\nsource {} · updated {}",
                        memory.title,
                        memory.body,
                        memory.source_session_id.as_deref().unwrap_or("session"),
                        memory.updated_at_ms
                    )
                })
                .unwrap_or_default();
            frame.render_widget(
                Paragraph::new(detail)
                    .style(Style::default().fg(theme::p().muted))
                    .wrap(Wrap { trim: true }),
                rows[2],
            );
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "Memories are project-scoped and persist across sessions.",
                    Style::default().fg(theme::p().faint),
                )),
                rows[3],
            );
            frame.render_widget(
                key_hints_line(&[
                    ("j/k", "move"),
                    ("p", "pin"),
                    ("d", "delete"),
                    ("r", "reload"),
                    ("Esc", "close"),
                ]),
                rows[4],
            );
        }
        ActiveModal::Routing {
            routes,
            selected,
            loading,
            error,
        } => {
            let popup = cap_width(centered_rect(area, 80, 58, 50, 12), 80);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, "Model routing (/routing)", true);
            let rows = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(4),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);
            let header = if loading {
                "Loading...".to_string()
            } else if let Some(error) = error {
                format!("Error: {error}")
            } else {
                format!("{} routes", routes.len())
            };
            frame.render_widget(
                Paragraph::new(Span::styled(header, Style::default().fg(theme::p().muted))),
                rows[0],
            );
            let items: Vec<ListItem<'_>> = routes
                .iter()
                .map(|route| {
                    let mut line = format!(
                        "{} -> {}::{}",
                        route.task_type.as_key(),
                        route.provider,
                        route.model
                    );
                    if let Some(cost) = route.max_cost_usd {
                        line.push_str(&format!(" · ${cost:.4}"));
                    }
                    if let Some(tokens) = route.max_tokens_per_turn {
                        line.push_str(&format!(" · {tokens} tokens"));
                    }
                    if let Some(concurrency) = route.max_concurrency {
                        line.push_str(&format!(" · {concurrency} concurrent"));
                    }
                    ListItem::new(Span::styled(line, Style::default().fg(theme::p().text)))
                })
                .collect();
            let mut state = ListState::default();
            state.select((!routes.is_empty()).then_some(selected));
            frame.render_stateful_widget(
                List::new(items).highlight_style(selection_style()),
                rows[1],
                &mut state,
            );
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "Set routes with /routing set <task> <provider::model> [max_cost] [max_tokens] [concurrency].",
                    Style::default().fg(theme::p().faint),
                )),
                rows[2],
            );
            frame.render_widget(
                key_hints_line(&[
                    ("j/k", "move"),
                    ("d", "delete"),
                    ("r", "reload"),
                    ("Esc", "close"),
                ]),
                rows[3],
            );
        }
        ActiveModal::Packs {
            packs,
            definitions,
            tab,
            selected,
            loading,
            error,
        } => {
            let popup = cap_width(centered_rect(area, 84, 64, 50, 14), 84);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, "Skills and packs (/packs)", true);
            let rows = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(4),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);
            let tabs = match tab {
                PackTab::Packs => "[ Packs ]  Definitions",
                PackTab::Definitions => "Packs  [ Definitions ]",
            };
            let header = if loading {
                "Loading...".to_string()
            } else if let Some(error) = error {
                format!("Error: {error}")
            } else {
                tabs.to_string()
            };
            frame.render_widget(
                Paragraph::new(Span::styled(header, Style::default().fg(theme::p().muted))),
                rows[0],
            );
            let items: Vec<ListItem<'_>> = match tab {
                PackTab::Packs => packs
                    .iter()
                    .map(|pack| {
                        ListItem::new(Span::styled(
                            format!(
                                "{} [{}] [{}] rev {}",
                                pack.id,
                                if pack.enabled { "enabled" } else { "disabled" },
                                if pack.trusted { "trusted" } else { "untrusted" },
                                pack.pinned_rev
                            ),
                            Style::default().fg(theme::p().text),
                        ))
                    })
                    .collect(),
                PackTab::Definitions => definitions
                    .agents
                    .iter()
                    .map(|agent| {
                        ListItem::new(Span::styled(
                            format!(
                                "agent/{} ({}) ~{} tokens{}{}",
                                agent.id,
                                agent.source,
                                agent.token_estimate,
                                mode_suffix(agent.mode.as_deref()),
                                supporting_suffix(&agent.supporting_files)
                            ),
                            Style::default().fg(theme::p().text),
                        ))
                    })
                    .chain(definitions.skills.iter().map(|skill| {
                        ListItem::new(Span::styled(
                            format!(
                                "skill/{} ({}) ~{} tokens{}{}",
                                skill.id,
                                skill.source,
                                skill.token_estimate,
                                invocation_suffix(skill.model_invocable),
                                supporting_suffix(&skill.supporting_files)
                            ),
                            Style::default().fg(theme::p().text),
                        ))
                    }))
                    .collect(),
            };
            let mut state = ListState::default();
            state.select((!items.is_empty()).then_some(selected));
            frame.render_stateful_widget(
                List::new(items).highlight_style(selection_style()),
                rows[1],
                &mut state,
            );
            if !definitions.warnings.is_empty() || !definitions.shadows.is_empty() {
                let mut notes = definitions.warnings.clone();
                for shadow in &definitions.shadows {
                    notes.push(format!(
                        "{} {} from {} shadowed by {}",
                        shadow.kind, shadow.id, shadow.source, shadow.kept_source
                    ));
                }
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        notes.join(" · "),
                        Style::default().fg(theme::p().warning),
                    )),
                    rows[2],
                );
            } else {
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        "Packs install from a path or Git URL; trust gates their definitions.",
                        Style::default().fg(theme::p().faint),
                    )),
                    rows[2],
                );
            }
            frame.render_widget(
                key_hints_line(&[
                    ("Tab", "switch"),
                    ("e", "enable"),
                    ("t", "trust"),
                    ("x", "remove"),
                    ("r", "reload"),
                    ("Esc", "close"),
                ]),
                rows[3],
            );
        }
        ActiveModal::Sampling {
            effort,
            temperature,
            field,
            error,
        } => {
            let popup = cap_width(centered_rect(area, 68, 46, 48, 12), 68);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, "Sampling (/sampling)", true);
            let rows = Layout::vertical([
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);
            let effort = effort.unwrap_or_default();
            let tiers = [EffortLevel::Low, EffortLevel::Medium, EffortLevel::High];
            let focused = field == 0;
            let mut tier_spans: Vec<Span<'static>> = Vec::new();
            let mut tier_x = rows[0].x;
            for (index, tier) in tiers.iter().enumerate() {
                if index > 0 {
                    tier_spans.push(Span::raw(" "));
                    tier_x = tier_x.saturating_add(1);
                }
                let label = format!(" {} ", tier.label());
                let selected = *tier == effort;
                let chunk = if selected {
                    format!("[{label}]")
                } else {
                    label
                };
                let width = chunk.chars().count() as u16;
                let mut style = Style::default().fg(if selected {
                    theme::p().interactive
                } else {
                    theme::p().text
                });
                if focused || selected {
                    style = style.add_modifier(Modifier::BOLD);
                }
                if app.hovered_modal_hit == Some(ModalHitKind::EffortTier(*tier)) {
                    style = style.fg(theme::p().accent).add_modifier(Modifier::BOLD);
                }
                tier_spans.push(Span::styled(chunk, style));
                app.modal_hit_zones.push(ModalHitZone {
                    rect: Rect::new(tier_x, rows[0].y.saturating_add(1), width, 1),
                    kind: ModalHitKind::EffortTier(*tier),
                });
                tier_x = tier_x.saturating_add(width);
            }
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled(
                        "Reasoning effort",
                        Style::default().fg(theme::p().muted),
                    )),
                    Line::from(tier_spans),
                    Line::from(Span::styled(
                        "Left/Right cycles · l/m/h picks",
                        Style::default().fg(theme::p().faint),
                    )),
                ]),
                rows[0],
            );
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled(
                        "Temperature override (blank = tier default)",
                        Style::default().fg(theme::p().muted),
                    )),
                    Line::from(Span::styled(
                        format!("{}{}", temperature, if field == 1 { "▌" } else { "" }),
                        Style::default()
                            .fg(if field == 1 {
                                theme::p().interactive
                            } else {
                                theme::p().text
                            })
                            .add_modifier(if field == 1 {
                                Modifier::BOLD
                            } else {
                                Modifier::empty()
                            }),
                    )),
                    Line::from(Span::styled(
                        "0.0..2.0, or type default",
                        Style::default().fg(theme::p().faint),
                    )),
                ]),
                rows[1],
            );
            let effective = temperature
                .trim()
                .parse::<f32>()
                .ok()
                .and_then(|value| meerai_common::settings::Temperature::new(value).effective())
                .unwrap_or_else(|| effort.default_temperature());
            frame.render_widget(
                Paragraph::new(Span::styled(
                    format!("Effective: {} · temp {:.2}", effort.label(), effective),
                    Style::default().fg(theme::p().muted),
                )),
                rows[2],
            );
            if let Some(error) = error {
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        error.clone(),
                        Style::default().fg(theme::p().error),
                    )),
                    rows[3],
                );
            }
            frame.render_widget(
                key_hints_line(&[
                    ("Tab", "field"),
                    ("←/→", "effort"),
                    ("l/m/h", "pick"),
                    ("Enter", "save"),
                    ("Esc", "cancel"),
                ]),
                rows[4],
            );
        }
        ActiveModal::VaultReset { input, error } => {
            let popup = cap_width(centered_rect(area, 72, 45, 46, 12), 72);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(
                frame,
                app,
                popup,
                "Reset credential vault (/vault-reset)",
                true,
            );
            let rows = Layout::vertical([
                Constraint::Length(4),
                Constraint::Length(3),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "This deletes every saved API key and OAuth credential and creates a new vault key. The old ciphertext cannot be recovered.",
                    Style::default().fg(theme::p().error),
                ))
                .wrap(Wrap { trim: true }),
                rows[0],
            );
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled(
                        "Type reset to confirm:",
                        Style::default().fg(theme::p().muted),
                    )),
                    Line::from(Span::styled(
                        if input.is_empty() {
                            "▌".to_string()
                        } else {
                            format!("{input}▌")
                        },
                        Style::default().fg(theme::p().text),
                    )),
                ]),
                rows[1],
            );
            if let Some(error) = error {
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        error.clone(),
                        Style::default().fg(theme::p().error),
                    ))
                    .wrap(Wrap { trim: true }),
                    rows[2],
                );
            }
            frame.render_widget(
                key_hints_line(&[("Enter", "reset"), ("Esc", "cancel")]),
                rows[3],
            );
        }
        ActiveModal::SubagentSpawn(panel) => {
            let popup = cap_width(centered_rect(area, 70, 52, 48, 14), 70);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, "Spawn subagent (/subagent)", true);
            let rows = Layout::vertical([
                Constraint::Min(5),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);
            let mode_label = panel.mode.map_or("parent", |mode| mode.label());
            let fields = [
                format!("Name    {}", blank_as(&panel.name, "optional")),
                format!("Agent   {}", panel.agent),
                format!("Task    {}", panel.task),
                format!(
                    "Model   {}",
                    if panel.model.is_empty() {
                        "parent".to_string()
                    } else {
                        panel.model.clone()
                    }
                ),
                format!("Mode    {mode_label}  (press m)"),
                format!("Steps   {}", blank_as(&panel.steps, "preset")),
                format!("Tokens  {}", blank_as(&panel.tokens, "parent cap")),
                format!("Cost $  {}", blank_as(&panel.cost, "parent cap")),
                format!("Tools   {}", blank_as(&panel.tools, "preset")),
            ];
            let items: Vec<ListItem<'_>> = fields
                .into_iter()
                .enumerate()
                .map(|(idx, text)| {
                    let style = if idx == panel.field {
                        Style::default()
                            .fg(theme::p().interactive)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme::p().text)
                    };
                    ListItem::new(Span::styled(
                        format!("{} {text}", if idx == panel.field { "▸" } else { " " }),
                        style,
                    ))
                })
                .collect();
            let mut state = ListState::default();
            state.select(Some(panel.field));
            frame.render_stateful_widget(List::new(items), rows[0], &mut state);
            if let Some(error) = panel.error.as_ref() {
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        error.clone(),
                        Style::default().fg(theme::p().error),
                    )),
                    rows[1],
                );
            } else {
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        "Budget and permissions are clamped to the parent session.",
                        Style::default().fg(theme::p().faint),
                    )),
                    rows[1],
                );
            }
            frame.render_widget(
                key_hints_line(&[
                    ("Tab", "next field"),
                    ("m", "mode"),
                    ("Enter", "start"),
                    ("Esc", "cancel"),
                ]),
                rows[2],
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
        ActiveModal::Runs { kind, selected } => {
            let title = match kind {
                RunKind::Agents => format!("Agents ({})", app.running_subagents().len()),
                RunKind::Shells => format!("Shells ({})", app.running_shells().len()),
            };
            let popup = cap_width(centered_rect(area, 66, 50, 42, 12), 68);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, &title, true);
            let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);
            let body = rows[0];
            let mut items: Vec<ListItem<'static>> = Vec::new();
            let hovered = app.hovered_modal_hit;
            match kind {
                RunKind::Agents => {
                    for (row_index, index) in app.running_subagents().into_iter().enumerate() {
                        let Some(TranscriptEntry::Subagent(subagent)) =
                            app.tab().transcript.get(index)
                        else {
                            continue;
                        };
                        let (badge, color) = status_label(subagent.status);
                        let steps = if subagent.max_steps > 0 {
                            format!(" {}/{}", subagent.steps_used, subagent.max_steps)
                        } else {
                            String::new()
                        };
                        let name = truncate_single_line(
                            subagent.display_name(),
                            (body.width as usize).saturating_sub(14),
                        );
                        let mut spans =
                            vec![Span::styled(name, Style::default().fg(theme::p().text))];
                        if subagent.named().is_some() {
                            spans.push(Span::styled(
                                format!(" · {}", subagent.agent),
                                Style::default().fg(theme::p().faint),
                            ));
                        }
                        spans.push(Span::styled(
                            format!(" [{badge}]"),
                            Style::default().fg(color),
                        ));
                        spans.push(Span::styled(steps, Style::default().fg(theme::p().faint)));
                        let item = ListItem::new(Line::from(spans));
                        if hovered == Some(ModalHitKind::ListItem(row_index))
                            && row_index != selected
                        {
                            items.push(item.style(hover_row_style()));
                        } else {
                            items.push(item);
                        }
                    }
                }
                RunKind::Shells => {
                    for (row_index, id) in app.running_shells().into_iter().enumerate() {
                        let Some(run) = app.shell_run(id) else {
                            continue;
                        };
                        let elapsed = crate::ui::statusline::format_compact_ms(
                            meerai_core::env::now_millis().saturating_sub(run.started_ms),
                        );
                        let command = truncate_single_line(
                            &format!("!{}", run.command),
                            (body.width as usize).saturating_sub(10),
                        );
                        let item = ListItem::new(Line::from(vec![
                            Span::styled(command, Style::default().fg(theme::p().text)),
                            Span::styled(
                                format!(" · {elapsed}"),
                                Style::default().fg(theme::p().faint),
                            ),
                        ]));
                        if hovered == Some(ModalHitKind::ListItem(row_index))
                            && row_index != selected
                        {
                            items.push(item.style(hover_row_style()));
                        } else {
                            items.push(item);
                        }
                    }
                }
            }
            if items.is_empty() {
                let label = match kind {
                    RunKind::Agents => "No running agents",
                    RunKind::Shells => "No running shells",
                };
                items.push(ListItem::new(Line::from(Span::styled(
                    label,
                    Style::default().fg(theme::p().faint),
                ))));
            }
            let has_rows = match kind {
                RunKind::Agents => !app.running_subagents().is_empty(),
                RunKind::Shells => !app.running_shells().is_empty(),
            };
            let mut state = ListState::default();
            if has_rows {
                state.select(Some(selected.min(items.len().saturating_sub(1))));
            }
            let list = List::new(items).highlight_style(selection_style());
            frame.render_stateful_widget(list, body, &mut state);
            if has_rows {
                register_list_hit_zones(app, body, state.offset(), app.run_count(kind));
            }
            frame.render_widget(
                Paragraph::new(key_hints_line(&[
                    ("Enter", "view"),
                    ("↑↓", "move"),
                    ("Esc", "close"),
                ])),
                rows[1],
            );
        }
        ActiveModal::RunDetail { run, scroll, .. } => match run {
            RunTarget::Subagent(index) => {
                let (title, nested_loaded, loading, report) = {
                    let Some(TranscriptEntry::Subagent(subagent)) = app.tab().transcript.get(index)
                    else {
                        return;
                    };
                    let title = if subagent.named().is_some() {
                        format!("Agent — {} ({})", subagent.display_name(), subagent.agent)
                    } else {
                        format!("Agent — {}", subagent.agent)
                    };
                    (
                        title,
                        subagent.nested.is_some(),
                        subagent.nested_loading,
                        subagent.report.clone(),
                    )
                };
                let popup = if app.transcript_area.width > 0 && app.transcript_area.height > 0 {
                    app.transcript_area
                } else {
                    cap_width(centered_rect(area, 92, 88, 56, 14), 140)
                };
                dim_backdrop(frame, area, popup);
                let inner = dialog_frame(frame, app, popup, &title, true);
                let rows =
                    Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);
                let body = rows[0];
                let width = (body.width as usize).max(1);
                let lines = if nested_loaded {
                    super::transcript::subagent_view_lines(app, index, body.width)
                } else if loading {
                    vec![Line::from(Span::styled(
                        "loading conversation…",
                        Style::default().fg(theme::p().faint),
                    ))]
                } else if let Some(report) = report.filter(|report| !report.trim().is_empty()) {
                    super::transcript::markdown_lines(&report, width)
                } else {
                    vec![Line::from(Span::styled(
                        "conversation unavailable",
                        Style::default().fg(theme::p().faint),
                    ))]
                };
                let max_scroll = lines.len().saturating_sub(body.height as usize);
                app.run_detail_max_scroll = max_scroll;
                let scroll = scroll.min(max_scroll).min(u16::MAX as usize) as u16;
                app.subagent_view_cache.body = body;
                app.subagent_view_cache.scroll = scroll as usize;
                frame.render_widget(
                    Block::default().style(Style::default().bg(theme::p().background)),
                    body,
                );
                frame.render_widget(
                    Paragraph::new(lines)
                        .scroll((scroll, 0))
                        .style(Style::default().fg(theme::p().text)),
                    body,
                );
                if nested_loaded {
                    let first = scroll as usize;
                    let last =
                        (first + body.height as usize).min(app.subagent_view_cache.lines.len());
                    for &(start, end) in &app.subagent_view_cache.bands {
                        let from = start.max(first);
                        let to = end.min(last);
                        for row in from..to {
                            let y = body.y + (row - first) as u16;
                            let buf = frame.buffer_mut();
                            for col in body.x..body.x.saturating_add(body.width) {
                                if let Some(cell) = buf.cell_mut((col, y)) {
                                    cell.set_bg(theme::p().surface);
                                }
                            }
                        }
                    }
                    for &(start, end) in &app.subagent_view_cache.code_bands {
                        let from = start.max(first);
                        let to = end.min(last);
                        let start_x = body.x.saturating_add(2).min(body.x + body.width);
                        for row in from..to {
                            let y = body.y + (row - first) as u16;
                            let buf = frame.buffer_mut();
                            for col in start_x..body.x.saturating_add(body.width) {
                                if let Some(cell) = buf.cell_mut((col, y)) {
                                    cell.set_bg(theme::p().surface_high);
                                }
                            }
                        }
                    }
                }
                if let Some(selection) = app.subagent_view_cache.selection {
                    super::selection::render_doc_selection(
                        frame,
                        body,
                        scroll as usize,
                        &app.subagent_view_cache.doc_lines,
                        selection,
                    );
                }
                frame.render_widget(
                    Paragraph::new(key_hints_line(&[
                        ("drag", "copy"),
                        ("j/k", "scroll"),
                        ("G", "end"),
                        ("Esc", "back"),
                    ])),
                    rows[1],
                );
            }
            RunTarget::Shell(id) => {
                let Some(run) = app.shell_run(id) else {
                    return;
                };
                let elapsed = crate::ui::statusline::format_compact_ms(
                    meerai_core::env::now_millis().saturating_sub(run.started_ms),
                );
                let title = format!("Shell — !{}", truncate_single_line(&run.command, 40));
                let (status, color) = match run.status {
                    ShellRunStatus::Running => (format!("running · {elapsed}"), theme::p().warning),
                    ShellRunStatus::Succeeded => ("finished".to_string(), theme::p().success),
                    ShellRunStatus::Failed => ("failed".to_string(), theme::p().error),
                };
                let mut lines = vec![
                    Line::from(vec![
                        Span::styled("$ ", Style::default().fg(theme::p().accent)),
                        Span::styled(
                            run.command.clone(),
                            Style::default()
                                .fg(theme::p().text)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(Span::styled(status, Style::default().fg(color))),
                    Line::from(""),
                ];
                match &run.output {
                    Some(output) => {
                        lines.extend(output.lines().map(|line| Line::from(line.to_string())))
                    }
                    None => lines.push(Line::from(Span::styled(
                        "waiting for output…",
                        Style::default().fg(theme::p().faint),
                    ))),
                }
                let popup = centered_rect(area, 84, 76, 56, 14);
                dim_backdrop(frame, area, popup);
                let inner = dialog_frame(frame, app, popup, &title, true);
                let rows =
                    Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);
                let body = rows[0];
                let width = (body.width as usize).max(1);
                let mut wrapped_lines: Vec<Line<'static>> = Vec::new();
                for line in lines {
                    let text: String = line
                        .spans
                        .iter()
                        .map(|span| span.content.as_ref())
                        .collect();
                    for (start, end) in wrap_queued_editor(&text, width) {
                        wrapped_lines.push(Line::from(Span::styled(
                            text[start..end].to_string(),
                            Style::default().fg(theme::p().text),
                        )));
                    }
                }
                let max_scroll = wrapped_lines.len().saturating_sub(body.height as usize);
                app.run_detail_max_scroll = max_scroll;
                frame.render_widget(
                    Paragraph::new(wrapped_lines)
                        .scroll((scroll.min(max_scroll).min(u16::MAX as usize) as u16, 0)),
                    body,
                );
                frame.render_widget(
                    Paragraph::new(key_hints_line(&[("j/k", "scroll"), ("Esc", "back")])),
                    rows[1],
                );
            }
        },
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
            let inner = dialog_frame(frame, app, popup, "Switch session (/sessions)", true);

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

fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

fn supporting_suffix(files: &[String]) -> String {
    if files.is_empty() {
        String::new()
    } else {
        format!(" · files: {}", files.join(", "))
    }
}

fn invocation_suffix(model_invocable: Option<bool>) -> &'static str {
    match model_invocable {
        Some(true) => " · model",
        Some(false) => " · manual",
        None => "",
    }
}

fn mode_suffix(mode: Option<&str>) -> String {
    match mode {
        Some(mode) if mode != "all" => format!(" · {mode}"),
        _ => String::new(),
    }
}

fn blank_as<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}

pub(crate) fn wrap_queued_editor(text: &str, width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line_start = 0usize;
    for segment in text.split('\n') {
        let seg_end = line_start + segment.len();
        let mut start = line_start;
        if start == seg_end {
            lines.push((start, start));
        }
        while start < seg_end {
            let mut end = start;
            let mut last_space = None;
            for (used, (offset, ch)) in text[start..seg_end].char_indices().enumerate() {
                if used == width {
                    break;
                }
                if ch == ' ' && offset > 0 {
                    last_space = Some(start + offset);
                }
                end = start + offset + ch.len_utf8();
            }
            if end < seg_end
                && !text[end..].starts_with(' ')
                && let Some(space) = last_space
            {
                end = space;
            }
            lines.push((start, end));
            start = end;
            while start < seg_end && text[start..].starts_with(' ') {
                start += 1;
            }
        }
        line_start = seg_end + 1;
    }
    if lines.is_empty() {
        lines.push((0, 0));
    }
    lines
}

pub(crate) fn queued_editor_cursor(
    lines: &[(usize, usize)],
    text: &str,
    cursor: usize,
) -> (usize, usize) {
    let cursor = cursor.min(text.len());
    let row = lines
        .iter()
        .rposition(|(start, _)| *start <= cursor)
        .unwrap_or(0);
    let (start, end) = lines[row];
    let cursor = cursor.min(end);
    (row, text[start..cursor].chars().count())
}

pub(crate) fn queued_editor_vertical(
    text: &str,
    cursor: usize,
    width: usize,
    delta: isize,
    goal_col: Option<usize>,
) -> Option<(usize, usize)> {
    let lines = wrap_queued_editor(text, width.max(1));
    let (row, col) = queued_editor_cursor(&lines, text, cursor);
    let target_row = row.checked_add_signed(delta)?;
    let (start, end) = *lines.get(target_row)?;
    let goal = goal_col.unwrap_or(col);
    let body = &text[start..end];
    let offset = body
        .char_indices()
        .nth(goal)
        .map(|(index, _)| index)
        .unwrap_or(body.len());
    Some((start + offset, goal))
}

#[cfg(test)]
mod tests {
    use super::{invocation_suffix, mode_suffix};

    #[test]
    fn invocation_suffix_marks_model_and_manual_skills() {
        assert_eq!(invocation_suffix(Some(true)), " · model");
        assert_eq!(invocation_suffix(Some(false)), " · manual");
        assert_eq!(invocation_suffix(None), "");
    }

    #[test]
    fn mode_suffix_marks_non_default_agent_modes() {
        assert_eq!(mode_suffix(Some("subagent")), " · subagent");
        assert_eq!(mode_suffix(Some("primary")), " · primary");
        assert_eq!(mode_suffix(Some("all")), "");
        assert_eq!(mode_suffix(None), "");
    }
}
