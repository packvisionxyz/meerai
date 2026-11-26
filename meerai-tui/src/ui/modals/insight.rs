use super::*;

pub(super) fn render_insight_modal(
    app: &mut AppState,
    frame: &mut Frame<'_>,
    area: Rect,
    modal: ActiveModal,
) {
    match modal {
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
        ActiveModal::Loop { state, scroll } => {
            let popup = cap_width(centered_rect(area, 78, 58, 50, 14), 78);
            dim_backdrop(frame, area, popup);
            let inner = dialog_frame(frame, app, popup, "Loop (/loop)", true);
            let rows = Layout::vertical([Constraint::Min(6), Constraint::Length(1)]).split(inner);
            let text = match state {
                Some(state) => meerai_skills::loop_engine::render(&state),
                None => "No active loop.\n\nType @goal <instruction> in the composer to start one."
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
            let tiers = app.active_model_effort_levels();
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
        _ => {}
    }
}
