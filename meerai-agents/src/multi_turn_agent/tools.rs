use super::*;

#[derive(Default)]
pub(super) struct FailureTracker {
    last: Option<(String, String, String)>,
    consecutive: usize,
    injected: bool,
}

enum BreakerSignal {
    None,
    InjectDiagnostic,
    FailTurn,
}

impl FailureTracker {
    fn record_failure(&mut self, name: &str, args: &str, failure: &str) -> BreakerSignal {
        let key = (name.to_string(), args.to_string(), failure.to_string());
        if self.last.as_ref() == Some(&key) {
            self.consecutive += 1;
        } else {
            self.last = Some(key);
            self.consecutive = 1;
            self.injected = false;
        }

        if self.injected && self.consecutive > 3 {
            BreakerSignal::FailTurn
        } else if self.consecutive == 3 {
            self.injected = true;
            BreakerSignal::InjectDiagnostic
        } else {
            BreakerSignal::None
        }
    }

    fn record_success(&mut self) {
        *self = Self::default();
    }
}

pub(super) fn permission_check(
    tool_name: &str,
    args: &str,
    effect: EffectClass,
    spec: Option<ToolPermissionSpec>,
) -> PermissionCheck {
    let parsed: Option<serde_json::Value> = serde_json::from_str(args).ok();
    let field = |key: &str| {
        parsed
            .as_ref()
            .and_then(|value| value.get(key))
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string()
    };

    let (action, resource) = match spec {
        Some(spec) => {
            let resource = spec.resource_arg.map(field).unwrap_or_default();
            (spec.action, resource)
        }
        None => {
            let action = match effect {
                EffectClass::Read => PermissionAction::Read,
                EffectClass::Write => PermissionAction::Edit,
                EffectClass::Execute => PermissionAction::Shell,
                EffectClass::External => PermissionAction::WebFetch,
            };
            let resource = parsed
                .as_ref()
                .and_then(|value| value.as_object())
                .and_then(|object| object.values().find_map(|value| value.as_str()))
                .map(str::to_string)
                .unwrap_or_else(|| args.to_string());
            (action, resource)
        }
    };

    PermissionCheck {
        action,
        resource,
        effect,
        tool_name: tool_name.to_string(),
    }
}

#[derive(Debug)]
enum Authorization {
    Allowed,
    Refused(String),
    Cancelled,
}

pub(super) struct ApprovalOutcome {
    pub(super) decision: ApprovalDecision,
    pub(super) scope: Option<GrantScope>,
}

impl ApprovalOutcome {
    fn new(decision: ApprovalDecision, scope: Option<GrantScope>) -> Self {
        let scope = effective_scope(&decision, scope);
        Self { decision, scope }
    }
}

fn effective_scope(decision: &ApprovalDecision, scope: Option<GrantScope>) -> Option<GrantScope> {
    match decision {
        ApprovalDecision::AcceptForSession => Some(GrantScope::Session),
        ApprovalDecision::Accept => scope,
        ApprovalDecision::Decline
        | ApprovalDecision::DeclineWithReason(_)
        | ApprovalDecision::Cancel => None,
    }
}

enum CallOutcome {
    Run(ToolRun),
    Failed(anyhow::Error),
}

struct CallPlan {
    tool_call: ToolCall,
    args: String,
    effect: EffectClass,
    item_id: ItemId,
    authorization: Authorization,
    outcome: Option<CallOutcome>,
}

impl CallPlan {
    fn authorized(&self) -> bool {
        matches!(self.authorization, Authorization::Allowed)
    }
}

impl MultiTurnAgent {
    pub(super) async fn execute_tool_calls(
        &mut self,
        tool_calls: &[ToolCall],
        turn_id: &TurnId,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
        mut inbox: Option<&mut mpsc::UnboundedReceiver<AgentCommand>>,
        clock: TurnClock,
    ) -> Result<Option<String>, TurnError> {
        let mut plans: Vec<CallPlan> = Vec::with_capacity(tool_calls.len());

        for tc in tool_calls {
            if cancel.is_cancelled() {
                return Ok(None);
            }

            let args_str = tc.fn_arguments.to_string();
            let effect = self
                .tools
                .iter()
                .find(|tool| tool.contain(&tc.fn_name))
                .map(|tool| tool.effect_of(&tc.fn_name))
                .unwrap_or(EffectClass::Read);
            let item_id = ItemId::new();

            let _ = event_tx.send(AgentEvent::ItemStarted {
                item: tool_call_item(&item_id, tc, &args_str, effect, ItemStatus::Proposed, None),
            });

            let call_id = CallId::from_raw(tc.call_id.clone());
            self.log_event(
                event_tx,
                SessionEvent::ToolCall {
                    call_id,
                    name: tc.fn_name.clone(),
                    args: args_str.clone(),
                },
            )
            .await;

            let authorization = if self.harness.is_some() {
                let spec = self
                    .tools
                    .iter()
                    .find(|tool| tool.contain(&tc.fn_name))
                    .and_then(|tool| tool.permission_spec(&tc.fn_name));
                let check = permission_check(&tc.fn_name, &args_str, effect, spec);
                self.authorize(
                    &check,
                    &item_id,
                    turn_id,
                    event_tx,
                    cancel,
                    inbox.as_deref_mut(),
                )
                .await
            } else {
                Authorization::Allowed
            };

            if matches!(authorization, Authorization::Allowed) {
                let _ = event_tx.send(AgentEvent::ItemDelta {
                    item_id: item_id.clone(),
                    delta: ItemDelta::StatusChanged(ItemStatus::InProgress),
                });
            }

            plans.push(CallPlan {
                tool_call: tc.clone(),
                args: args_str,
                effect,
                item_id,
                authorization,
                outcome: None,
            });
        }

        let this: &Self = self;
        let mut halted = false;
        let mut cursor = 0;

        while cursor < plans.len() && !halted {
            if cancel.is_cancelled() {
                break;
            }

            if !plans[cursor].authorized() || plans[cursor].effect != EffectClass::Read {
                if plans[cursor].authorized() {
                    let plan = &plans[cursor];
                    let result = match clock.remaining() {
                        Some(remaining) => match tokio::time::timeout(
                            remaining,
                            this.run_tool(&plan.tool_call.fn_name, &plan.args, cancel),
                        )
                        .await
                        {
                            Ok(result) => result,
                            Err(_) => Ok(tool_budget_exceeded(clock.max_secs)),
                        },
                        None if clock.max_secs != 0 => Ok(tool_budget_exceeded(clock.max_secs)),
                        None => {
                            this.run_tool(&plan.tool_call.fn_name, &plan.args, cancel)
                                .await
                        }
                    };
                    halted =
                        matches!(&result, Ok(run) if matches!(run.output, ToolOutput::Stop(_)));
                    plans[cursor].outcome = Some(match result {
                        Ok(run) => CallOutcome::Run(run),
                        Err(error) => CallOutcome::Failed(error),
                    });
                }
                cursor += 1;
                continue;
            }

            let mut end = cursor;
            while end < plans.len()
                && plans[end].authorized()
                && plans[end].effect == EffectClass::Read
            {
                end += 1;
            }

            let reads = (cursor..end).map(|index| {
                let plan = &plans[index];
                async move {
                    let result = this
                        .run_tool(&plan.tool_call.fn_name, &plan.args, cancel)
                        .await;
                    (index, result)
                }
            });
            let results = match clock.remaining() {
                Some(remaining) => match tokio::time::timeout(remaining, join_all(reads)).await {
                    Ok(results) => results,
                    Err(_) => {
                        let timeout = tool_budget_exceeded(clock.max_secs);
                        for plan in plans.iter_mut().take(end).skip(cursor) {
                            plan.outcome = Some(CallOutcome::Run(timeout.clone()));
                        }
                        cursor = end;
                        continue;
                    }
                },
                None if clock.max_secs != 0 => {
                    let timeout = tool_budget_exceeded(clock.max_secs);
                    for plan in plans.iter_mut().take(end).skip(cursor) {
                        plan.outcome = Some(CallOutcome::Run(timeout.clone()));
                    }
                    cursor = end;
                    continue;
                }
                None => join_all(reads).await,
            };
            for (index, result) in results {
                halted = halted
                    || matches!(&result, Ok(run) if matches!(run.output, ToolOutput::Stop(_)));
                plans[index].outcome = Some(match result {
                    Ok(run) => CallOutcome::Run(run),
                    Err(error) => CallOutcome::Failed(error),
                });
            }
            cursor = end;
        }

        let mut stop_reason: Option<String> = None;

        for plan in plans.iter_mut() {
            let call_id = CallId::from_raw(plan.tool_call.call_id.clone());
            let tool_call = plan.tool_call.clone();
            let args_str = plan.args.clone();
            let effect = plan.effect;
            let item_id = plan.item_id.clone();

            let (status, item_output, model_text, observation, run, failure) =
                match &plan.authorization {
                    Authorization::Cancelled => {
                        let observation = ToolObservation::refused("approval cancelled");
                        let text = observation.render();
                        (
                            ItemStatus::Cancelled,
                            text.clone(),
                            text,
                            observation,
                            None,
                            None,
                        )
                    }
                    Authorization::Refused(reason) => {
                        let observation = ToolObservation::refused(reason.clone());
                        let text = observation.render();
                        (
                            ItemStatus::Denied,
                            text.clone(),
                            text,
                            observation,
                            None,
                            None,
                        )
                    }
                    Authorization::Allowed if stop_reason.is_some() && plan.outcome.is_none() => {
                        let observation = ToolObservation::error(NOT_EXECUTED_STOPPED);
                        let text = observation.render();
                        (
                            ItemStatus::Cancelled,
                            text.clone(),
                            text,
                            observation,
                            None,
                            None,
                        )
                    }
                    Authorization::Allowed => match plan.outcome.take() {
                        Some(CallOutcome::Run(mut run)) => {
                            redact_run(&mut run);
                            run.observation = self
                                .spill_observation(
                                    &tool_call.fn_name,
                                    run.observation,
                                    &call_id,
                                    event_tx,
                                )
                                .await;
                            let observation = run.observation.clone();
                            let mut failure = None;
                            let (status, item_output, model_text) = match &run.output {
                                ToolOutput::Text(_) => (
                                    ItemStatus::Succeeded,
                                    observation.stdout.clone(),
                                    observation.stdout.clone(),
                                ),
                                ToolOutput::Fail(text) => {
                                    failure = Some(text.clone());
                                    (
                                        ItemStatus::Failed,
                                        text.clone(),
                                        format!("Tool failed: {text}"),
                                    )
                                }
                                ToolOutput::Stop(_) => (
                                    ItemStatus::Stopped,
                                    observation.stdout.clone(),
                                    observation.stdout.clone(),
                                ),
                            };
                            if matches!(run.output, ToolOutput::Stop(_)) {
                                stop_reason = Some(observation.stdout.clone());
                            }
                            (
                                status,
                                item_output,
                                model_text,
                                observation,
                                Some(run),
                                failure,
                            )
                        }
                        Some(CallOutcome::Failed(error)) => {
                            let text = redact_text(&format!("Tool error: {error:#}"));
                            (
                                ItemStatus::Failed,
                                text.clone(),
                                text.clone(),
                                ToolObservation::error(text.clone()),
                                None,
                                Some(text),
                            )
                        }
                        None => {
                            let observation = ToolObservation::error(NOT_EXECUTED_STOPPED);
                            let text = observation.render();
                            (
                                ItemStatus::Cancelled,
                                text.clone(),
                                text,
                                observation,
                                None,
                                None,
                            )
                        }
                    },
                };

            self.log_event(
                event_tx,
                SessionEvent::ToolResult {
                    call_id,
                    output: observation,
                },
            )
            .await;

            let response = ToolResponse::from_tool_call(&tool_call, model_text.clone())
                .with_fn_name(&tool_call.fn_name);
            self.chat_history.push(ChatMessage::from(response));

            let _ = event_tx.send(AgentEvent::ItemCompleted {
                item: tool_call_item(
                    &item_id,
                    &tool_call,
                    &args_str,
                    effect,
                    status,
                    Some(item_output),
                ),
            });

            if let Some(run) = run
                && !run.file_changes.is_empty()
            {
                let status = if matches!(run.output, ToolOutput::Text(_)) {
                    ItemStatus::Succeeded
                } else {
                    ItemStatus::Failed
                };
                let change_id = ItemId::new();
                let _ = event_tx.send(AgentEvent::ItemStarted {
                    item: SessionItem::FileChange {
                        id: change_id.clone(),
                        changes: run.file_changes.clone(),
                        status: ItemStatus::InProgress,
                    },
                });
                let _ = event_tx.send(AgentEvent::ItemCompleted {
                    item: SessionItem::FileChange {
                        id: change_id,
                        changes: run.file_changes,
                        status,
                    },
                });
            }

            if plan.authorized() {
                self.inject_scoped_instructions(&args_str, event_tx).await;
            }

            if self.harness.is_some() {
                match &failure {
                    Some(failure) => {
                        match self.failure_tracker.record_failure(
                            &tool_call.fn_name,
                            &args_str,
                            failure,
                        ) {
                            BreakerSignal::None => {}
                            BreakerSignal::InjectDiagnostic => {
                                self.log_event(
                                    event_tx,
                                    SessionEvent::CircuitBreaker {
                                        kind: BreakerKind::RepeatedToolFailure,
                                        action: BreakerAction::DiagnosticInjected,
                                    },
                                )
                                .await;
                                self.log_event(
                                    event_tx,
                                    SessionEvent::UserMessage {
                                        source: InputSource::RuntimeContext,
                                        content: vec![ContentBlock::Text(format!(
                                            "tool {} failed identically 3 times with {failure}; \
                                             stop retrying the same call; try a different approach \
                                             or end the turn",
                                            tool_call.fn_name
                                        ))],
                                    },
                                )
                                .await;
                            }
                            BreakerSignal::FailTurn => {
                                self.log_event(
                                    event_tx,
                                    SessionEvent::CircuitBreaker {
                                        kind: BreakerKind::RepeatedToolFailure,
                                        action: BreakerAction::TurnFailed,
                                    },
                                )
                                .await;
                                return Err(TurnError::loop_detected(anyhow!(LOOP_DETECTED)));
                            }
                        }
                    }
                    None => {
                        if matches!(status, ItemStatus::Succeeded | ItemStatus::Stopped) {
                            self.failure_tracker.record_success();
                        }
                    }
                }
            }
        }

        if cancel.is_cancelled() {
            return Ok(None);
        }

        Ok(stop_reason)
    }

    async fn authorize(
        &mut self,
        check: &PermissionCheck,
        item_id: &ItemId,
        turn_id: &TurnId,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
        inbox: Option<&mut mpsc::UnboundedReceiver<AgentCommand>>,
    ) -> Authorization {
        let Some(harness) = self.harness.as_ref() else {
            return Authorization::Allowed;
        };

        match harness.policy.decide(check) {
            Decision::Allow => Authorization::Allowed,
            Decision::Deny { reason } => Authorization::Refused(reason),
            Decision::Ask => {
                let has_channel = inbox.is_some();
                let request_id = RequestId::new();
                let request = ApprovalRequest {
                    request_id: request_id.clone(),
                    turn_id: turn_id.clone(),
                    kind: ApprovalKind::ToolCall,
                    preview: approval_preview_for(check),
                    effect: Some(check.effect),
                };
                let _ = event_tx.send(AgentEvent::ItemDelta {
                    item_id: item_id.clone(),
                    delta: ItemDelta::StatusChanged(ItemStatus::Pending),
                });
                let _ = event_tx.send(AgentEvent::ApprovalRequested {
                    request: request.clone(),
                });
                self.log_event(
                    event_tx,
                    SessionEvent::ApprovalRequested {
                        request_id: request_id.clone(),
                        kind: request.kind,
                        preview: request.preview,
                    },
                )
                .await;

                let outcome = self
                    .await_decision(&request_id, event_tx, cancel, inbox)
                    .await;

                match outcome.decision {
                    ApprovalDecision::Accept | ApprovalDecision::AcceptForSession => {
                        if let Some(scope) = outcome.scope
                            && let Some(harness) = &self.harness
                        {
                            harness.policy.record_grant(meerai_policy::Grant {
                                action: check.action,
                                resource: check.resource.clone(),
                                scope,
                            });
                        }
                        Authorization::Allowed
                    }
                    ApprovalDecision::Decline => {
                        let reason = if has_channel {
                            USER_DECLINED.to_string()
                        } else {
                            NO_APPROVAL_CHANNEL.to_string()
                        };
                        Authorization::Refused(reason)
                    }
                    ApprovalDecision::DeclineWithReason(reason) => Authorization::Refused(reason),
                    ApprovalDecision::Cancel => {
                        cancel.cancel();
                        Authorization::Cancelled
                    }
                }
            }
        }
    }

    pub(super) async fn await_decision(
        &mut self,
        request_id: &RequestId,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
        inbox: Option<&mut mpsc::UnboundedReceiver<AgentCommand>>,
    ) -> ApprovalOutcome {
        let Some(receiver) = inbox else {
            self.resolve_approval(request_id, ApprovalDecision::Decline, None, event_tx)
                .await;
            return ApprovalOutcome::new(ApprovalDecision::Decline, None);
        };

        loop {
            let command = tokio::select! {
                _ = cancel.cancelled() => {
                    self.resolve_approval(
                        request_id,
                        ApprovalDecision::Cancel,
                        None,
                        event_tx,
                    )
                    .await;
                    return ApprovalOutcome::new(ApprovalDecision::Cancel, None);
                }
                command = receiver.recv() => command,
            };
            let Some(command) = command else {
                cancel.cancel();
                self.resolve_approval(request_id, ApprovalDecision::Cancel, None, event_tx)
                    .await;
                return ApprovalOutcome::new(ApprovalDecision::Cancel, None);
            };

            match command {
                AgentCommand::Approve {
                    request_id: id,
                    decision,
                    scope,
                } => {
                    if &id != request_id {
                        continue;
                    }
                    self.resolve_approval(request_id, decision.clone(), scope, event_tx)
                        .await;
                    return ApprovalOutcome::new(decision, scope);
                }
                AgentCommand::Cancel | AgentCommand::Quit => {
                    cancel.cancel();
                    self.resolve_approval(request_id, ApprovalDecision::Cancel, None, event_tx)
                        .await;
                    return ApprovalOutcome::new(ApprovalDecision::Cancel, None);
                }
                AgentCommand::Steer(_)
                | AgentCommand::SetPermissionMode(_)
                | AgentCommand::Compact
                | AgentCommand::ApplyAgent { .. }
                | AgentCommand::ApplySkill { .. } => {
                    self.handle_live_command(command, event_tx).await;
                }
                AgentCommand::SubmitPrompt(_) | AgentCommand::SetModel(_) | AgentCommand::Reset => {
                }
            }
        }
    }

    pub(super) async fn handle_live_command(
        &mut self,
        command: AgentCommand,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) {
        match command {
            AgentCommand::Steer(text) => self.pending_steers.push_back(text),
            AgentCommand::SetPermissionMode(mode) => {
                if let Some(harness) = &mut self.harness {
                    harness.policy.set_mode(mode);
                }
            }
            AgentCommand::Compact => self.pending_compact = true,
            AgentCommand::ApplyAgent {
                name,
                instructions,
                model,
            } => {
                self.apply_agent_instructions(&name, &instructions, model, event_tx)
                    .await;
            }
            AgentCommand::ApplySkill { name, instructions } => {
                self.apply_skill_instructions(&name, &instructions, event_tx)
                    .await;
            }
            AgentCommand::Approve { .. }
            | AgentCommand::SubmitPrompt(_)
            | AgentCommand::SetModel(_)
            | AgentCommand::Reset
            | AgentCommand::Cancel
            | AgentCommand::Quit => {}
        }
    }

    async fn resolve_approval(
        &self,
        request_id: &RequestId,
        decision: ApprovalDecision,
        scope: Option<GrantScope>,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) {
        let scope = effective_scope(&decision, scope);
        self.log_event(
            event_tx,
            SessionEvent::ApprovalDecided {
                request_id: request_id.clone(),
                decision,
                scope,
            },
        )
        .await;
        let _ = event_tx.send(AgentEvent::ApprovalResolved {
            request_id: request_id.clone(),
        });
    }

    async fn spill_observation(
        &self,
        tool_name: &str,
        mut observation: ToolObservation,
        call_id: &CallId,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) -> ToolObservation {
        let Some(harness) = &self.harness else {
            return observation;
        };
        let should_spill = self
            .tools
            .iter()
            .find(|tool| tool.contain(tool_name))
            .is_none_or(|tool| tool.should_spill_output(tool_name));
        if !should_spill {
            return observation;
        }
        if !matches!(
            observation.status,
            ObservationStatus::Success | ObservationStatus::Stopped
        ) {
            return observation;
        }

        let text = observation.stdout.clone();
        if estimate_tokens(&text) <= harness.config.spill_max_inline_tokens {
            return observation;
        }
        let Some(blobs) = harness.session.blob_store() else {
            return observation;
        };

        let outcome = apply_spill(&text, harness.config.spill_max_inline_tokens, &blobs).await;
        if let Some(warning) = outcome.warning {
            self.log_event(
                event_tx,
                SessionEvent::Error {
                    code: ErrorCode::Internal,
                    message: warning,
                    retryable: false,
                },
            )
            .await;
            return observation;
        }
        let Some(blob_path) = outcome.blob_path else {
            return observation;
        };

        observation.stdout = outcome.retained;
        self.log_event(
            event_tx,
            SessionEvent::SpillApplied {
                call_id: call_id.clone(),
                bytes_omitted: outcome.bytes_omitted,
                blob_path,
            },
        )
        .await;
        observation
    }

    async fn run_tool(
        &self,
        name: &str,
        args: &str,
        cancel: &CancellationToken,
    ) -> Result<ToolRun> {
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.contain(name))
            .ok_or_else(|| anyhow!("Tool not found: {name}"))?;

        let timeout_ms = self
            .harness
            .as_ref()
            .map(|harness| harness.config.tool_timeout_ms);
        let max_attempts = self.config.max_retries.max(1);

        for attempt in 1..=max_attempts {
            if cancel.is_cancelled() {
                return Err(anyhow!("Tool execution cancelled"));
            }

            let invocation = tool.invoke_structured(name, args);
            let result = match timeout_ms {
                Some(ms) => match tokio::time::timeout(Duration::from_millis(ms), invocation).await
                {
                    Ok(result) => result,
                    Err(_) => {
                        return Ok(ToolRun::from_output(ToolOutput::Fail(format!(
                            "tool timed out after {ms}ms"
                        ))));
                    }
                },
                None => invocation.await,
            };

            match result {
                Ok(run) => return Ok(run),
                Err(err) if attempt == max_attempts => {
                    return Err(anyhow!(
                        "Failed to invoke tool '{name}' after {max_attempts} attempts: {err}"
                    ));
                }
                Err(_) => {
                    tokio::select! {
                        _ = cancel.cancelled() => return Err(anyhow!("Tool execution cancelled")),
                        _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                    }
                }
            }
        }

        unreachable!("loop always returns on final attempt")
    }
}
