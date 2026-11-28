use super::*;

pub(super) fn render_panels_modal(
    app: &mut AppState,
    frame: &mut Frame<'_>,
    area: Rect,
    modal: ActiveModal,
) {
    match modal {
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
            let action = app
                .file_changes
                .get(file_index)
                .map(|entry| entry.action_label())
                .unwrap_or("");
            let title = format!(
                "Diff (/diff): {} ({}) +{} -{}{file_pos}{}",
                diff.path,
                diff.status,
                diff.additions,
                diff.deletions,
                if action.is_empty() {
                    String::new()
                } else {
                    format!(" · {action}")
                }
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
                    ("s", "stage"),
                    ("u", "unstage"),
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
                    ("s", "stage"),
                    ("u", "unstage"),
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
                mode.label().chars().count() + 3 + query.cursor_col(),
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
                (
                    port_input.text.clone(),
                    Style::default().fg(theme::p().text),
                )
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
                    "•".repeat(secret_input.cursor_col()),
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
                place_input_cursor(frame, port_area, port_input.cursor_col());
            } else {
                place_input_cursor(frame, secret_area, secret_input.cursor_col());
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
        ActiveModal::McpServers { selected } => {
            let connected = app.mcp_servers.iter().filter(|s| s.is_connected()).count();
            let failed = app.mcp_servers.iter().filter(|s| s.is_failed()).count();
            let untrusted = app.mcp_servers.iter().filter(|s| s.needs_trust()).count();
            let detail = if untrusted > 0 {
                format!("{untrusted} need trust")
            } else {
                format!("{failed} failed")
            };
            let title = format!(
                "MCP servers ({connected} connected, {detail}, {} total)",
                app.mcp_servers.len()
            );
            let popup = cap_width(centered_rect(area, 76, 60, 48, 14), 78);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, &title, true);
            let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);
            let body = rows[0];
            let hovered = app.hovered_modal_hit;
            let mut items: Vec<ListItem<'static>> = Vec::new();

            let ordered: Vec<&crate::app::McpServerView> = app
                .mcp_servers
                .iter()
                .filter(|server| server.is_failed())
                .chain(app.mcp_servers.iter().filter(|s| s.needs_trust()))
                .chain(app.mcp_servers.iter().filter(|s| s.is_connected()))
                .chain(
                    app.mcp_servers
                        .iter()
                        .filter(|s| !s.is_failed() && !s.is_connected() && !s.needs_trust()),
                )
                .collect();

            for (row_index, server) in ordered.iter().enumerate() {
                let (badge, color) = if server.is_connected() {
                    ("connected", theme::p().success)
                } else if server.is_failed() {
                    ("failed", theme::p().error)
                } else if server.needs_trust() {
                    ("needs trust", theme::p().warning)
                } else if server.status == "disabled" {
                    ("disabled", theme::p().faint)
                } else if server.status == "unsupported" {
                    ("unsupported", theme::p().warning)
                } else {
                    (server.badge(), theme::p().faint)
                };
                let name =
                    truncate_single_line(&server.name, (body.width as usize).saturating_sub(24));
                let tool_count = if server.tools.is_empty() {
                    String::new()
                } else {
                    format!(" · {} tools", server.tools.len())
                };
                let mut spans = vec![
                    Span::styled(name, Style::default().fg(theme::p().text)),
                    Span::styled(
                        format!(" ({})", server.kind),
                        Style::default().fg(theme::p().faint),
                    ),
                    Span::styled(format!("  {badge}"), Style::default().fg(color)),
                    Span::styled(tool_count, Style::default().fg(theme::p().faint)),
                ];
                if let Some(error) = &server.error {
                    spans.push(Span::styled(
                        format!(
                            "  {}",
                            truncate_single_line(error, (body.width as usize).saturating_sub(28))
                        ),
                        Style::default().fg(theme::p().error),
                    ));
                }
                let item = ListItem::new(Line::from(spans));
                if hovered == Some(ModalHitKind::ListItem(row_index)) && row_index != selected {
                    items.push(item.style(hover_row_style()));
                } else {
                    items.push(item);
                }
            }

            if items.is_empty() {
                items.push(ListItem::new(Line::from(Span::styled(
                    "No MCP servers configured. Add .mcp.json to the project root.",
                    Style::default().fg(theme::p().faint),
                ))));
            }
            let has_rows = !ordered.is_empty();
            let mut state = ListState::default();
            if has_rows {
                state.select(Some(selected.min(items.len().saturating_sub(1))));
            }
            let list = List::new(items).highlight_style(selection_style());
            frame.render_stateful_widget(list, body, &mut state);
            if has_rows {
                register_list_hit_zones(app, body, state.offset(), ordered.len());
            }
            let hints: Vec<(&str, &str)> = if untrusted > 0 {
                vec![
                    ("↑↓", "move"),
                    ("t", "trust"),
                    ("r", "reload"),
                    ("Esc", "close"),
                ]
            } else {
                vec![("↑↓", "move"), ("r", "reload"), ("Esc", "close")]
            };
            frame.render_widget(Paragraph::new(key_hints_line(&hints)), rows[1]);
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
            place_input_cursor(frame, filter_area, 8 + filter.cursor_col());

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
        _ => {}
    }
}
