use super::*;

pub(super) fn render_misc_modal(
    app: &mut AppState,
    frame: &mut Frame<'_>,
    area: Rect,
    modal: ActiveModal,
) {
    match modal {
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
                format!("Agent   {}", panel.agent.text),
                format!("Task    {}", panel.task.text),
                format!(
                    "Model   {}",
                    if panel.model.is_empty() {
                        "parent".to_string()
                    } else {
                        panel.model.text.clone()
                    }
                ),
                format!("Mode    {mode_label}  (press m)"),
                format!("Steps   {}", blank_as(panel.steps.as_str(), "preset")),
                format!("Tokens  {}", blank_as(panel.tokens.as_str(), "parent cap")),
                format!("Cost $  {}", blank_as(panel.cost.as_str(), "parent cap")),
                format!("Tools   {}", blank_as(panel.tools.as_str(), "preset")),
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
        _ => {}
    }
}
