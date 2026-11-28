use super::*;

impl AppState {
    pub(crate) fn keys_chat(
        &mut self,
        modal: ActiveModal,
        key: KeyEvent,
        has_ctrl: bool,
        _has_alt: bool,
    ) -> bool {
        match modal {
            ActiveModal::Approval {
                queue,
                mut reason,
                mut editing_reason,
            } => {
                let request_id = queue.first().map(|request| request.request_id.clone());
                if editing_reason {
                    match key.code {
                        KeyCode::Esc => {
                            reason.clear();
                            editing_reason = false;
                        }
                        _ => match handle_text_key(&mut reason, key, 60, true, true) {
                            TextAction::Submit => {
                                if let Some(request_id) = request_id {
                                    let text = reason.trim().to_string();
                                    let decision = if text.is_empty() {
                                        ApprovalDecision::Decline
                                    } else {
                                        ApprovalDecision::DeclineWithReason(text)
                                    };
                                    let _ = self.tab().cmd_tx.send(AgentCommand::Approve {
                                        request_id,
                                        decision,
                                        scope: None,
                                    });
                                }
                                reason.clear();
                                editing_reason = false;
                            }
                            TextAction::Edited | TextAction::Ignored => {}
                        },
                    }
                } else {
                    let action = match key.code {
                        KeyCode::Char('y') | KeyCode::Enter => {
                            Some((ApprovalDecision::Accept, None))
                        }
                        KeyCode::Char('t') => {
                            Some((ApprovalDecision::Accept, Some(GrantScope::Turn)))
                        }
                        KeyCode::Char('a') => Some((ApprovalDecision::AcceptForSession, None)),
                        KeyCode::Char('n') => Some((ApprovalDecision::Decline, None)),
                        KeyCode::Char('r') => {
                            reason.clear();
                            editing_reason = true;
                            None
                        }
                        KeyCode::Char('c') => Some((ApprovalDecision::Cancel, None)),
                        KeyCode::Esc => Some((ApprovalDecision::Cancel, None)),
                        _ => None,
                    };
                    if let Some((decision, scope)) = action
                        && let Some(request_id) = request_id
                    {
                        let _ = self.tab().cmd_tx.send(AgentCommand::Approve {
                            request_id,
                            decision,
                            scope,
                        });
                    }
                }
                self.modal = Some(ActiveModal::Approval {
                    queue,
                    reason,
                    editing_reason,
                });
                true
            }
            ActiveModal::Terminal {
                output, mut scroll, ..
            } => {
                let max = output
                    .as_deref()
                    .map(|body| body.lines().count().saturating_sub(1))
                    .unwrap_or(0);
                match key.code {
                    KeyCode::Esc => {
                        self.modal = None;
                        self.status_notice = Some(
                            "Shell running in background; output will appear in the transcript"
                                .into(),
                        );
                        return true;
                    }
                    KeyCode::Char('q') => {
                        self.modal = None;
                        return true;
                    }
                    KeyCode::Char('b') if output.is_some() => {
                        self.modal = None;
                        return true;
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        scroll = (scroll + 1).min(max);
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        scroll = scroll.saturating_sub(1);
                    }
                    KeyCode::PageDown | KeyCode::Char('d')
                        if key.code == KeyCode::PageDown || has_ctrl =>
                    {
                        scroll = (scroll + 10).min(max);
                    }
                    KeyCode::PageUp | KeyCode::Char('u')
                        if key.code == KeyCode::PageUp || has_ctrl =>
                    {
                        scroll = scroll.saturating_sub(10);
                    }
                    _ => {}
                }
                if let Some(ActiveModal::Terminal {
                    run_id,
                    command,
                    output,
                    running,
                    ..
                }) = self.modal.take()
                {
                    self.modal = Some(ActiveModal::Terminal {
                        run_id,
                        command,
                        output,
                        running,
                        scroll,
                    });
                }
                true
            }
            ActiveModal::Question {
                request,
                mut answers,
                mut custom,
                mut editing_custom,
                mut current,
                mut focus,
            } => {
                let prompt = &request.questions[current];
                let last = current + 1 >= request.questions.len();
                let other = prompt.options.len();
                let multiple = prompt.multiple;
                let width = self.terminal_width.saturating_sub(12).max(20) as usize;
                let mut submit = false;
                if editing_custom {
                    match key.code {
                        KeyCode::Esc => {
                            editing_custom = false;
                        }
                        KeyCode::Tab => {
                            if last {
                                submit = true;
                            } else {
                                current += 1;
                                focus = 0;
                                editing_custom = false;
                            }
                        }
                        KeyCode::BackTab => {
                            if current > 0 {
                                current -= 1;
                                focus = 0;
                                editing_custom = false;
                            }
                        }
                        KeyCode::Up
                            if !key.modifiers.intersects(
                                KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT,
                            ) =>
                        {
                            if !custom[current].move_up(width) {
                                editing_custom = false;
                                focus = focus.saturating_sub(1);
                            }
                        }
                        KeyCode::Down
                            if !key.modifiers.intersects(
                                KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT,
                            ) =>
                        {
                            if !custom[current].move_down(width) {
                                editing_custom = false;
                            }
                        }
                        _ => match handle_text_key(&mut custom[current], key, width, true, true) {
                            TextAction::Submit => submit = true,
                            TextAction::Edited => {
                                if !multiple {
                                    answers[current].clear();
                                }
                            }
                            TextAction::Ignored => {}
                        },
                    }
                } else {
                    match key.code {
                        KeyCode::Esc => {
                            self.record_question_answer(&request.request_id, None);
                            let _ = self.tab().cmd_tx.send(AgentCommand::AnswerQuestion {
                                request_id: request.request_id.clone(),
                                answers: None,
                            });
                            self.modal = None;
                            self.open_pending_question();
                            return true;
                        }
                        KeyCode::Up | KeyCode::Char('k')
                            if key.code == KeyCode::Up || !has_ctrl =>
                        {
                            focus = focus.saturating_sub(1);
                        }
                        KeyCode::Down | KeyCode::Char('j')
                            if key.code == KeyCode::Down || !has_ctrl =>
                        {
                            focus = (focus + 1).min(other);
                        }
                        KeyCode::Char(' ') if focus < other => {
                            if multiple {
                                if let Some(index) =
                                    answers[current].iter().position(|i| *i == focus)
                                {
                                    answers[current].remove(index);
                                } else {
                                    answers[current].push(focus);
                                    answers[current].sort_unstable();
                                }
                            } else {
                                answers[current] = vec![focus];
                                custom[current].clear();
                            }
                        }
                        KeyCode::Char(' ') => {
                            editing_custom = true;
                        }
                        KeyCode::Char(c @ '1'..='9') if focus < other => {
                            let index = c.to_digit(10).unwrap_or(1) as usize - 1;
                            if index < prompt.options.len() {
                                focus = index;
                                if multiple {
                                    if !answers[current].contains(&index) {
                                        answers[current].push(index);
                                        answers[current].sort_unstable();
                                    }
                                } else {
                                    answers[current] = vec![index];
                                    custom[current].clear();
                                }
                            }
                        }
                        KeyCode::Enter if focus == other => {
                            editing_custom = true;
                        }
                        KeyCode::Enter => {
                            if focus < other && answers[current].is_empty() {
                                answers[current] = vec![focus];
                            }
                            submit = true;
                        }
                        KeyCode::Tab | KeyCode::Right => {
                            if !last {
                                current += 1;
                                focus = 0;
                                editing_custom = false;
                            } else {
                                submit = true;
                            }
                        }
                        KeyCode::BackTab | KeyCode::Left => {
                            if current > 0 {
                                current -= 1;
                                focus = 0;
                                editing_custom = false;
                            }
                        }
                        _ => {
                            if focus == other {
                                editing_custom = true;
                                match handle_text_key(&mut custom[current], key, width, true, true)
                                {
                                    TextAction::Submit => submit = true,
                                    TextAction::Edited => {
                                        if !multiple {
                                            answers[current].clear();
                                        }
                                    }
                                    TextAction::Ignored => {}
                                }
                            }
                        }
                    }
                }
                if submit {
                    if focus < other && answers[current].is_empty() && !editing_custom {
                        answers[current] = vec![focus];
                    }
                    if last {
                        let collected = collect_question_answers(&request, &answers, &custom);
                        self.record_question_answer(&request.request_id, Some(collected.clone()));
                        let _ = self.tab().cmd_tx.send(AgentCommand::AnswerQuestion {
                            request_id: request.request_id.clone(),
                            answers: Some(collected),
                        });
                        self.modal = None;
                        self.open_pending_question();
                        return true;
                    }
                    current += 1;
                    focus = 0;
                    editing_custom = false;
                }
                self.modal = Some(ActiveModal::Question {
                    request,
                    answers,
                    custom,
                    editing_custom,
                    current,
                    focus,
                });
                true
            }
            ActiveModal::UserMessageActions {
                entry_idx,
                mut selected,
            } => {
                let actions = [UserAction::Rewind, UserAction::Copy, UserAction::Fork];
                let run = |state: &mut Self, action| {
                    state.modal = None;
                    state.run_user_action(entry_idx, action);
                };
                match key.code {
                    KeyCode::Esc => {
                        self.modal = None;
                    }
                    KeyCode::Up | KeyCode::Char('p') if key.code == KeyCode::Up || has_ctrl => {
                        selected = if selected == 0 { 2 } else { selected - 1 };
                        self.modal = Some(ActiveModal::UserMessageActions {
                            entry_idx,
                            selected,
                        });
                    }
                    KeyCode::Down | KeyCode::Char('n') if key.code == KeyCode::Down || has_ctrl => {
                        selected = (selected + 1) % 3;
                        self.modal = Some(ActiveModal::UserMessageActions {
                            entry_idx,
                            selected,
                        });
                    }
                    KeyCode::Enter => run(self, actions[selected.min(2)]),
                    KeyCode::Char('w') if !has_ctrl => run(self, UserAction::Rewind),
                    KeyCode::Char('c') if !has_ctrl => run(self, UserAction::Copy),
                    KeyCode::Char('f') if !has_ctrl => run(self, UserAction::Fork),
                    _ => {
                        self.modal = Some(ActiveModal::UserMessageActions {
                            entry_idx,
                            selected,
                        });
                    }
                }
                true
            }
            ActiveModal::CommandPalette {
                mut filter,
                mut selected,
            } => {
                let commands = filtered_commands(&filter);
                match key.code {
                    KeyCode::Esc => {
                        self.modal = None;
                    }
                    KeyCode::Up | KeyCode::Char('p') if key.code == KeyCode::Up || has_ctrl => {
                        if !commands.is_empty() {
                            selected = if selected == 0 {
                                commands.len() - 1
                            } else {
                                selected - 1
                            };
                        }
                        self.modal = Some(ActiveModal::CommandPalette { filter, selected });
                    }
                    KeyCode::Down | KeyCode::Char('n') if key.code == KeyCode::Down || has_ctrl => {
                        if !commands.is_empty() {
                            selected = (selected + 1) % commands.len();
                        }
                        self.modal = Some(ActiveModal::CommandPalette { filter, selected });
                    }
                    KeyCode::Backspace => {
                        filter.backspace();
                        selected = 0;
                        self.modal = Some(ActiveModal::CommandPalette { filter, selected });
                    }
                    _ => match handle_text_key(&mut filter, key, 48, false, false) {
                        TextAction::Submit => {
                            if let Some(command) = commands.get(selected) {
                                let action = command.action;
                                self.modal = None;
                                self.run_palette_action(action);
                            } else {
                                self.modal = Some(ActiveModal::CommandPalette { filter, selected });
                            }
                        }
                        TextAction::Edited => {
                            selected = 0;
                            self.modal = Some(ActiveModal::CommandPalette { filter, selected });
                        }
                        TextAction::Ignored => {
                            self.modal = Some(ActiveModal::CommandPalette { filter, selected });
                        }
                    },
                }
                true
            }
            ActiveModal::QueuedEdit {
                index,
                mut input,
                mut error,
            } => {
                let width = if self.queued_editor_width == 0 {
                    56
                } else {
                    self.queued_editor_width as usize
                };
                let mut keep_open = true;
                match key.code {
                    KeyCode::Esc => {
                        self.modal = None;
                        keep_open = false;
                    }
                    _ => match handle_text_key(&mut input, key, width, true, true) {
                        TextAction::Submit => {
                            let text = input.trim().to_string();
                            if text.is_empty() {
                                error = Some("Message is required".to_string());
                            } else {
                                let updated = {
                                    let tab = self.tab_mut();
                                    match tab.queued.get_mut(index) {
                                        Some(slot) => {
                                            *slot = text.clone();
                                            true
                                        }
                                        None => false,
                                    }
                                };
                                if updated {
                                    self.status_notice = Some("Queued message updated".to_string());
                                }
                                self.modal = None;
                                keep_open = false;
                            }
                        }
                        TextAction::Edited => {
                            error = None;
                        }
                        TextAction::Ignored => {}
                    },
                }
                if keep_open {
                    self.modal = Some(ActiveModal::QueuedEdit {
                        index,
                        input,
                        error,
                    });
                }
                true
            }
            _ => false,
        }
    }
}
