use super::*;

struct QueuedPrompt {
    text: String,
    model: Option<String>,
}

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
                AgentCommand::SetHooks(entries) => {
                    self.set_hooks(Arc::new(HookEngine::new(entries)));
                }
                AgentCommand::Approve { .. }
                | AgentCommand::AnswerQuestion { .. }
                | AgentCommand::Rewind { .. }
                | AgentCommand::SubmitPrompt(_)
                | AgentCommand::SetModel { .. }
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
        let mut queued: VecDeque<QueuedPrompt> = VecDeque::new();

        agent.start_session(&event_tx).await;

        'worker: while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                AgentCommand::SubmitPrompt(prompt) | AgentCommand::Steer(prompt) => {
                    queued.push_back(QueuedPrompt {
                        text: prompt,
                        model: None,
                    });
                    let mut recovery_attempts = 0usize;

                    while let Some(QueuedPrompt {
                        text: prompt,
                        model: turn_model,
                    }) = queued.pop_front()
                    {
                        let harnessed = agent.harness.is_some();
                        let cancel = CancellationToken::new();
                        let mut pending_quit = false;
                        let mut pending_reset = false;
                        let mut pending_model: Option<String> = None;
                        let mut pending_model_window: Option<u64> = None;
                        let mut recovery: Option<QueuedPrompt> = None;
                        let recovery_model = agent
                            .harness
                            .as_ref()
                            .and_then(|harness| harness.config.recovery_model.clone());
                        let previous_model = turn_model.as_ref().map(|_| agent.model().to_string());
                        if let Some(model) = &turn_model {
                            agent.set_model(model.clone());
                        }
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
                                        match outcome {
                                            Ok(_) => recovery_attempts = 0,
                                            Err(err) => {
                                                let message = err.message();
                                                let recoverable = match err.code {
                                                    ErrorCode::Provider => err.retryable,
                                                    ErrorCode::LoopDetected => true,
                                                    _ => false,
                                                };
                                                let will_recover = recoverable
                                                    && recovery_attempts < MAX_RECOVERY_ATTEMPTS
                                                    && !pending_quit
                                                    && !pending_reset;
                                                if will_recover {
                                                    let _ = event_tx.send(
                                                        AgentEvent::RecoveryStarted {
                                                            attempt: recovery_attempts + 1,
                                                        },
                                                    );
                                                }
                                                let _ = event_tx.send(AgentEvent::Error {
                                                    code: err.code,
                                                    message: message.clone(),
                                                });
                                                let _ = event_tx.send(AgentEvent::TurnCompleted {
                                                    turn_id,
                                                    reason: TurnEndReason::Failed { error: message.clone() },
                                                });
                                                if will_recover {
                                                    recovery_attempts += 1;
                                                    recovery = Some(QueuedPrompt {
                                                        text: recovery_prompt(&message),
                                                        model: recovery_model.clone(),
                                                    });
                                                }
                                            }
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
                                            Some(AgentCommand::SetModel {
                                                model,
                                                context_window_tokens,
                                            }) => {
                                                pending_model = Some(model);
                                                pending_model_window = context_window_tokens;
                                            }
                                            Some(AgentCommand::SubmitPrompt(prompt)) => {
                                                queued.push_back(QueuedPrompt {
                                                    text: prompt,
                                                    model: None,
                                                });
                                            }
                                            Some(AgentCommand::Steer(text)) if !harnessed => {
                                                queued.push_back(QueuedPrompt {
                                                    text,
                                                    model: None,
                                                });
                                            }
                                            Some(
                                                command @ (AgentCommand::Steer(_)
                                                | AgentCommand::Approve { .. }
                                                | AgentCommand::AnswerQuestion { .. }
                                                | AgentCommand::SetPermissionMode(_)
                                                | AgentCommand::Compact
                                                | AgentCommand::ApplyAgent { .. }
                                                | AgentCommand::ApplySkill { .. }
                                                | AgentCommand::Reload { .. }
                                                | AgentCommand::SetHooks(_)
                                                | AgentCommand::Rewind { .. }),
                                            ) => {
                                                let _ = inbox_tx.send(command);
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if let Some(previous) = previous_model {
                            agent.set_model(previous);
                        }
                        if let Some(model) = pending_model {
                            agent.set_model(model);
                            if let Some(tokens) = pending_model_window {
                                agent.set_context_window_tokens(tokens);
                            }
                            agent.persist_model().await;
                        }
                        if pending_reset {
                            agent.end_session(&event_tx).await;
                            agent.reset();
                            agent.start_session(&event_tx).await;
                        }
                        if !pending_quit
                            && !pending_reset
                            && let Some(recovery) = recovery.take()
                        {
                            queued.push_front(recovery);
                        }
                        if pending_quit {
                            break 'worker;
                        }
                    }
                }
                AgentCommand::SetModel {
                    model,
                    context_window_tokens,
                } => {
                    agent.set_model(model);
                    if let Some(tokens) = context_window_tokens {
                        agent.set_context_window_tokens(tokens);
                    }
                    agent.persist_model().await;
                }
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
                    context_window_tokens,
                } => {
                    agent
                        .apply_agent_instructions(
                            &name,
                            &instructions,
                            model,
                            context_window_tokens,
                            &event_tx,
                        )
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
                AgentCommand::SetHooks(entries) => {
                    agent.set_hooks(Arc::new(HookEngine::new(entries)));
                }
                AgentCommand::Rewind { turn_id, scope } => {
                    let outcome = agent.rewind(&turn_id, scope).await;
                    let (restored, error) = match outcome {
                        Ok(restored) => (restored, None),
                        Err(err) => (Vec::new(), Some(format!("{err:#}"))),
                    };
                    let _ = event_tx.send(AgentEvent::RewindCompleted {
                        turn_id,
                        scope,
                        restored,
                        error,
                    });
                }
                AgentCommand::Reset => {
                    agent.end_session(&event_tx).await;
                    agent.reset();
                    agent.start_session(&event_tx).await;
                }
                AgentCommand::Cancel => {}
                AgentCommand::Approve { .. } => {}
                AgentCommand::AnswerQuestion { .. } => {}
                AgentCommand::Quit => break,
            }
        }

        agent.end_session(&event_tx).await;
    });

    (cmd_tx, event_rx)
}

fn recovery_prompt(message: &str) -> String {
    format!(
        "The previous turn failed: {message}\n\n\
         Do not repeat the failed action. Diagnose the cause, change the approach, and make one \
         concrete change or check. If the failure is not recoverable, stop and report it."
    )
}
