use super::*;

impl MultiTurnAgent {
    pub(super) async fn drain_inbox(
        &mut self,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        inbox: Option<&mut mpsc::UnboundedReceiver<AgentCommand>>,
    ) -> bool {
        let Some(receiver) = inbox else {
            return false;
        };

        while let Ok(command) = receiver.try_recv() {
            match command {
                AgentCommand::Cancel | AgentCommand::Quit => return true,
                AgentCommand::Steer(_)
                | AgentCommand::SetPermissionMode(_)
                | AgentCommand::Compact
                | AgentCommand::ApplyAgent { .. }
                | AgentCommand::ApplySkill { .. }
                | AgentCommand::Reload { .. } => {
                    self.handle_live_command(command, event_tx).await;
                }
                AgentCommand::Approve { .. }
                | AgentCommand::SubmitPrompt(_)
                | AgentCommand::SetModel(_)
                | AgentCommand::Revert { .. }
                | AgentCommand::Reset => {}
            }
        }

        false
    }
}

pub fn spawn_worker(
    mut agent: MultiTurnAgent,
) -> (
    mpsc::UnboundedSender<AgentCommand>,
    mpsc::UnboundedReceiver<AgentEvent>,
) {
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        let mut queued: VecDeque<String> = VecDeque::new();

        'worker: while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                AgentCommand::SubmitPrompt(prompt) | AgentCommand::Steer(prompt) => {
                    queued.push_back(prompt);

                    while let Some(prompt) = queued.pop_front() {
                        let harnessed = agent.harness.is_some();
                        let cancel = CancellationToken::new();
                        let mut pending_quit = false;
                        let mut pending_reset = false;
                        let mut pending_model: Option<String> = None;
                        let (inbox_tx, mut inbox_rx) = mpsc::unbounded_channel();

                        {
                            let mut turn_fut = std::pin::pin!(agent.run_turn(
                                &prompt,
                                &event_tx,
                                cancel.clone(),
                                Some(&mut inbox_rx),
                            ));

                            loop {
                                tokio::select! {
                                    res = &mut turn_fut => {
                                        let (turn_id, outcome) = res;
                                        if let Err(err) = outcome {
                                            let message = err.message();
                                            let _ = event_tx.send(AgentEvent::Error {
                                                code: err.code,
                                                message: message.clone(),
                                            });
                                            let _ = event_tx.send(AgentEvent::TurnCompleted {
                                                turn_id,
                                                reason: TurnEndReason::Failed { error: message },
                                            });
                                        }
                                        break;
                                    }
                                    maybe_cmd = cmd_rx.recv() => {
                                        match maybe_cmd {
                                            Some(AgentCommand::Cancel) => cancel.cancel(),
                                            Some(AgentCommand::Quit) | None => {
                                                cancel.cancel();
                                                pending_quit = true;
                                            }
                                            Some(AgentCommand::Reset) => {
                                                cancel.cancel();
                                                pending_reset = true;
                                            }
                                            Some(AgentCommand::SetModel(model)) => {
                                                pending_model = Some(model);
                                            }
                                            Some(AgentCommand::SubmitPrompt(prompt)) => {
                                                queued.push_back(prompt);
                                            }
                                            Some(AgentCommand::Steer(text)) if !harnessed => {
                                                queued.push_back(text);
                                            }
                                            Some(
                                                command @ (AgentCommand::Steer(_)
                                                | AgentCommand::Approve { .. }
                                                | AgentCommand::SetPermissionMode(_)
                                                | AgentCommand::Compact
                                                | AgentCommand::ApplyAgent { .. }
                                                | AgentCommand::ApplySkill { .. }
                                                | AgentCommand::Reload { .. }
                                                | AgentCommand::Revert { .. }),
                                            ) => {
                                                let _ = inbox_tx.send(command);
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if let Some(model) = pending_model {
                            agent.set_model(model);
                        }
                        if pending_reset {
                            agent.reset();
                        }
                        if pending_quit {
                            break 'worker;
                        }
                    }
                }
                AgentCommand::SetModel(model) => agent.set_model(model),
                AgentCommand::SetPermissionMode(mode) => {
                    agent.set_permission_mode(mode);
                    if let Some(harness) = agent.harness.as_ref() {
                        let _ = harness.session.set_approval_mode(mode.label()).await;
                    }
                }
                AgentCommand::Compact => {
                    let cancel = CancellationToken::new();
                    let _ = agent
                        .run_compaction(CompactionTrigger::Manual, &event_tx, &cancel, None)
                        .await;
                }
                AgentCommand::ApplyAgent {
                    name,
                    instructions,
                    model,
                } => {
                    agent
                        .apply_agent_instructions(&name, &instructions, model, &event_tx)
                        .await;
                }
                AgentCommand::ApplySkill { name, instructions } => {
                    agent
                        .apply_skill_instructions(&name, &instructions, &event_tx)
                        .await;
                }
                AgentCommand::Reload {
                    rules,
                    context,
                    system_prompt,
                } => agent.reload_config(rules, context, system_prompt),
                AgentCommand::Revert { turn_id } => {
                    if let Err(err) = agent.revert_to_turn(&turn_id).await {
                        let _ = event_tx.send(AgentEvent::Error {
                            code: ErrorCode::Internal,
                            message: format!("revert failed: {err}"),
                        });
                    }
                }
                AgentCommand::Reset => agent.reset(),
                AgentCommand::Cancel => {}
                AgentCommand::Approve { .. } => {}
                AgentCommand::Quit => break,
            }
        }
    });

    (cmd_tx, event_rx)
}
