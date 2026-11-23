use super::*;
use meerai_common::settings::EffortLevel;
use meerai_core::ToolDefinition;

fn effort_level(effort: EffortLevel) -> genai::chat::ReasoningEffort {
    match effort {
        EffortLevel::Low => genai::chat::ReasoningEffort::Low,
        EffortLevel::Medium => genai::chat::ReasoningEffort::Medium,
        EffortLevel::High => genai::chat::ReasoningEffort::High,
    }
}

fn is_context_overflow(message: &str) -> bool {
    let lowered = message.to_lowercase();
    OVERFLOW_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
}

pub(super) fn provider_error_status(error: &anyhow::Error) -> Option<u16> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<genai::Error>())
        .and_then(genai_error_status)
}

fn genai_error_status(error: &genai::Error) -> Option<u16> {
    if let Some(status) = error.status() {
        return Some(status.as_u16());
    }
    match error {
        genai::Error::WebStream { error, .. } => error
            .downcast_ref::<genai::Error>()
            .and_then(genai_error_status),
        _ => None,
    }
}

pub(super) fn status_is_transient(status: u16) -> bool {
    matches!(status, 408 | 409 | 425 | 429) || (500..=599).contains(&status)
}

pub(super) fn is_terminal_plan_usage_error(message: &str) -> bool {
    let lowered = message.to_lowercase();
    PLAN_USAGE_TERMINAL_CODES
        .iter()
        .any(|code| lowered.contains(code))
}

pub(super) fn is_transient_provider_error_with_status(status: Option<u16>, message: &str) -> bool {
    if is_terminal_plan_usage_error(message) {
        return false;
    }
    match status {
        Some(status) => status_is_transient(status),
        None => !NON_TRANSIENT_ERROR_MARKERS
            .iter()
            .any(|marker| message.to_lowercase().contains(marker)),
    }
}

fn shorten(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn write_call_summary(tool_calls: &[ToolCall], tools: &[Box<dyn Toolset>]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for call in tool_calls {
        let args = call.fn_arguments.to_string();
        let is_write = tools.iter().any(|tool| {
            tool.contain(&call.fn_name)
                && tool.effect_of_call(&call.fn_name, &args) == EffectClass::Write
        });
        if !is_write {
            continue;
        }
        let target = call
            .fn_arguments
            .get("path")
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                call.fn_arguments
                    .get("command")
                    .and_then(serde_json::Value::as_str)
            });
        let label = match target {
            Some(target) => format!("{} {}", call.fn_name, shorten(target, 80)),
            None => call.fn_name.clone(),
        };
        if !parts.contains(&label) {
            parts.push(label);
        }
    }
    match parts.len() {
        0 => String::new(),
        1 => parts[0].clone(),
        2..=3 => parts.join(", "),
        count => format!("{} (+{} more)", parts[0], count - 1),
    }
}

fn is_transient_provider_error(error: &anyhow::Error, message: &str) -> bool {
    is_transient_provider_error_with_status(provider_error_status(error), message)
}

fn stop_reason_text(reason: &StopReason) -> String {
    let (label, raw) = match reason {
        StopReason::Completed(raw) => ("completed", raw),
        StopReason::MaxTokens(raw) => ("max_tokens", raw),
        StopReason::ToolCall(raw) => ("tool_call", raw),
        StopReason::ContentFilter(raw) => ("content_filter", raw),
        StopReason::StopSequence(raw) => ("stop_sequence", raw),
        StopReason::Other(raw) => ("other", raw),
    };
    if raw.is_empty() {
        label.to_string()
    } else {
        format!("{label}({raw})")
    }
}

fn empty_stream_report(
    model: &str,
    stop_reason: Option<&StopReason>,
    events: usize,
    elapsed: Duration,
) -> (String, bool) {
    let (summary, retryable, hint) = match stop_reason {
        Some(StopReason::ContentFilter(_)) => (
            "provider blocked the response with a content filter before returning content",
            false,
            "Rephrase the prompt or remove content that trips the provider safety filter.",
        ),
        Some(StopReason::MaxTokens(_)) => (
            "provider reached the output token limit before returning content",
            false,
            "Raise the model output limit or shorten the prompt.",
        ),
        Some(StopReason::StopSequence(_)) => (
            "provider stopped at a stop sequence before returning content",
            false,
            "Check the configured stop sequences and retry.",
        ),
        _ => (
            EMPTY_STREAM_ERROR,
            true,
            "Check the provider status, network proxy or VPN, and the selected model.",
        ),
    };
    let reason = stop_reason
        .map(stop_reason_text)
        .unwrap_or_else(|| "none".to_string());
    (
        format!(
            "{summary}\n  provider={} model={model} stop_reason={reason} events={events} elapsed={:.1}s\n  hint: {hint}",
            provider_of(model),
            elapsed.as_secs_f64(),
        ),
        retryable,
    )
}

pub(super) fn provider_backoff_after(failures: usize) -> Duration {
    let factor = 1u64 << failures.saturating_sub(1).min(8);
    Duration::from_millis(PROVIDER_BACKOFF_MS.saturating_mul(factor))
}

pub(super) fn provider_backoff_with_jitter(failures: usize) -> Duration {
    let base = provider_backoff_after(failures);
    let cap = (base.as_millis() as u64 / 2).max(1);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or(0);
    base + Duration::from_millis(u64::from(nanos) % cap)
}

impl MultiTurnAgent {
    pub(super) async fn run_turn(
        &mut self,
        prompt: &str,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancel: CancellationToken,
        inbox: Option<&mut mpsc::UnboundedReceiver<AgentCommand>>,
    ) -> (TurnId, Result<String, TurnError>) {
        let turn_id = TurnId::new();
        self.injected_instruction_dirs.clear();
        let title = self.new_session_title(prompt);
        let _ = event_tx.send(AgentEvent::TurnStarted {
            turn_id: turn_id.clone(),
            model: self.model.clone(),
        });
        self.log_event(
            event_tx,
            SessionEvent::TurnStarted {
                turn_id: turn_id.clone(),
                trigger: TurnTrigger::UserPrompt,
            },
        )
        .await;
        let prompt_report = self
            .run_hook(
                HookEvent::UserPromptSubmit,
                Some(&turn_id),
                None,
                Some(prompt),
                None,
                Some(prompt),
                event_tx,
            )
            .await;
        if matches!(
            prompt_report.decision,
            crate::hooks::HookDecision::Deny | crate::hooks::HookDecision::Block
        ) {
            let reason = prompt_report
                .reason()
                .unwrap_or("blocked by hook")
                .to_string();
            self.log_event(
                event_tx,
                SessionEvent::Error {
                    code: ErrorCode::Config,
                    message: reason.clone(),
                    retryable: false,
                },
            )
            .await;
            self.log_event(
                event_tx,
                SessionEvent::TurnEnded {
                    turn_id: turn_id.clone(),
                    reason: TurnEndReason::Failed {
                        error: reason.clone(),
                    },
                },
            )
            .await;
            return (turn_id, Err(TurnError::config(anyhow!(reason))));
        }
        self.log_event(
            event_tx,
            SessionEvent::UserMessage {
                source: InputSource::User,
                content: vec![ContentBlock::Text(prompt.to_string())],
            },
        )
        .await;
        if let Some(title) = title
            && let Some(harness) = &self.harness
        {
            let _ = harness.session.set_title(title).await;
        }
        self.sync_system_message(event_tx).await;

        let outcome = match self
            .stream_turn(prompt, &turn_id, event_tx, cancel, inbox)
            .await
        {
            Ok((text, reason)) => {
                self.log_event(
                    event_tx,
                    SessionEvent::TurnEnded {
                        turn_id: turn_id.clone(),
                        reason,
                    },
                )
                .await;
                Ok(text)
            }
            Err(err) => {
                let message = err.message();
                self.log_event(
                    event_tx,
                    SessionEvent::Error {
                        code: err.code,
                        message: message.clone(),
                        retryable: err.retryable,
                    },
                )
                .await;
                self.run_hook_with_error(
                    HookEvent::TurnFailed,
                    Some(&turn_id),
                    None,
                    None,
                    None,
                    None,
                    Some(&message),
                    event_tx,
                )
                .await;
                self.log_event(
                    event_tx,
                    SessionEvent::TurnEnded {
                        turn_id: turn_id.clone(),
                        reason: TurnEndReason::Failed {
                            error: message.clone(),
                        },
                    },
                )
                .await;
                Err(err)
            }
        };
        if let Some(harness) = &self.harness {
            if let Err(error) = harness.session.heal_dangling_tool_calls().await {
                self.log_event(
                    event_tx,
                    SessionEvent::Error {
                        code: ErrorCode::Internal,
                        message: format!("failed to repair dangling tool calls: {error:#}"),
                        retryable: false,
                    },
                )
                .await;
            }
            harness.policy.clear_scoped_grants(GrantScope::Turn);
        }
        if self.sandbox_escape == Some(GrantScope::Turn) {
            self.sandbox_escape = None;
        }
        (turn_id, outcome)
    }

    async fn stream_turn(
        &mut self,
        prompt: &str,
        turn_id: &TurnId,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancel: CancellationToken,
        mut inbox: Option<&mut mpsc::UnboundedReceiver<AgentCommand>>,
    ) -> Result<(String, TurnEndReason), TurnError> {
        self.chat_history.push(ChatMessage::user(prompt));

        let plan_mode = self
            .harness
            .as_ref()
            .is_some_and(|harness| harness.policy.mode() == ApprovalMode::Plan);
        let definitions: Vec<ToolDefinition> = self
            .tools
            .iter()
            .flat_map(|tool| {
                tool.definition().into_iter().filter(|definition| {
                    !plan_mode || tool.effect_of(&definition.name) == EffectClass::Read
                })
            })
            .collect();
        let genai_tools: Vec<Tool> = definitions.iter().map(ToolDefinition::to_genai).collect();
        self.tool_schema_tokens = definitions
            .iter()
            .map(|definition| {
                estimate_tokens(&definition.name)
                    + estimate_tokens(&definition.description)
                    + estimate_tokens(&definition.parameters.to_string())
            })
            .sum();

        let plan_usage = self
            .harness
            .as_ref()
            .is_some_and(|harness| harness.config.plan_usage);
        let mut options = ChatOptions::default()
            .with_capture_content(true)
            .with_capture_tool_calls(true)
            .with_capture_usage(true)
            .with_capture_reasoning_content(true)
            .with_stream_header_timeout(STREAM_HEADER_TIMEOUT)
            .with_stream_read_timeout(STREAM_READ_TIMEOUT)
            .with_extra_headers(Headers::from([
                ("x-opencode-session", self.session_id.as_str()),
                ("user-agent", MEERAI_USER_AGENT),
            ]));
        if !plan_usage
            && self
                .harness
                .as_ref()
                .is_none_or(|harness| harness.config.prompt_caching)
        {
            options = options.with_cache_control(CacheControl::Ephemeral);
        }
        if !plan_usage
            && let Some(temperature) = self
                .harness
                .as_ref()
                .and_then(|harness| harness.config.temperature)
        {
            options = options.with_temperature(temperature as f64);
        }
        let route = self
            .harness
            .as_ref()
            .and_then(|harness| harness.config.routing.route(&self.task_type))
            .cloned();
        let route_effort = route
            .as_ref()
            .and_then(|route| route.effort.as_deref())
            .and_then(crate::routing::parse_effort);
        let effort = route_effort.or_else(|| self.harness.as_ref().and_then(|h| h.config.effort));
        if !plan_usage && let Some(effort) = effort {
            options = options.with_reasoning_effort(effort_level(effort));
        }
        if let Some(max_tokens) = route
            .as_ref()
            .and_then(|route| route.max_tokens_per_turn)
            .and_then(|max_tokens| u32::try_from(max_tokens).ok())
        {
            options = options.with_max_tokens(max_tokens);
        }

        let harnessed = self.harness.is_some();
        let max_steps = self
            .harness
            .as_ref()
            .map_or(self.config.max_cycles, |harness| {
                harness.config.max_steps_per_turn
            })
            .max(1);
        let continuations = if harnessed {
            self.harness
                .as_ref()
                .map_or(0, |harness| harness.config.step_limit_continuations)
        } else {
            0
        };
        let total_steps = max_steps.saturating_mul(continuations.saturating_add(1));
        let max_turn_secs = self
            .harness
            .as_ref()
            .map_or(0, |harness| harness.config.max_turn_secs);
        let turn_started = Instant::now();
        let clock = TurnClock {
            started: turn_started,
            max_secs: max_turn_secs,
        };
        let mut pressure_compactions = 0usize;
        let mut pressure_step: Option<usize> = None;
        let progress_tracking = self
            .harness
            .as_ref()
            .is_some_and(|harness| harness.policy.mode() != ApprovalMode::Plan)
            && self.tools.iter().any(|tool| {
                tool.definition()
                    .iter()
                    .any(|definition| tool.effect_of(&definition.name) == EffectClass::Write)
            });
        let mut no_progress_steps = 0usize;
        let mut no_progress_diagnosed = false;
        let no_progress_fail_steps = self
            .harness
            .as_ref()
            .map_or(NO_PROGRESS_FAIL_STEPS, |harness| {
                harness.config.no_progress_fail_steps
            })
            .max(3);
        let no_progress_diagnostic_steps = (no_progress_fail_steps * 2 / 3).max(1);
        let mut stop_hook_used = false;
        let mut last_tool_signature: Option<String> = None;
        let mut repeated_tool_batches = 0usize;

        for step_index in 0..total_steps {
            if cancel.is_cancelled() {
                return Ok(aborted(event_tx, turn_id));
            }
            if let Some(ended) = self
                .enforce_turn_deadline(turn_started, max_turn_secs, turn_id, event_tx)
                .await
            {
                return Ok(ended);
            }
            if self.drain_inbox(event_tx, inbox.as_deref_mut()).await {
                cancel.cancel();
                return Ok(aborted(event_tx, turn_id));
            }
            if let Some(pending) = self.pending_compact.take() {
                self.run_compaction(
                    CompactionTrigger::Manual,
                    event_tx,
                    &cancel,
                    Some(clock),
                    pending.focus,
                    pending.from_turn_id,
                )
                .await?;
            }
            self.apply_pending_steers(event_tx).await;

            let final_step = harnessed && step_index + 1 == total_steps;
            let continuation_boundary =
                harnessed && step_index > 0 && step_index % max_steps == 0 && !final_step;
            let mut plan = StepPlan {
                turn_id: turn_id.clone(),
                step_index,
                model: self.model.clone(),
                final_step,
                tool_count: self.tools.len(),
            };
            if let StepAction::Stop { reason } = self.run_before_step(&mut plan) {
                return Ok(completed(event_tx, turn_id, reason.unwrap_or_default()));
            }

            if harnessed {
                match self
                    .enforce_token_budget(turn_id, event_tx, &cancel, inbox.as_deref_mut())
                    .await
                {
                    BudgetOutcome::Continue => {}
                    BudgetOutcome::Cancelled => return Ok(aborted(event_tx, turn_id)),
                    BudgetOutcome::Declined { detail } => {
                        let _ = event_tx.send(AgentEvent::Error {
                            code: ErrorCode::Budget,
                            message: detail.clone(),
                        });
                        self.log_event(
                            event_tx,
                            SessionEvent::Error {
                                code: ErrorCode::Budget,
                                message: detail,
                                retryable: false,
                            },
                        )
                        .await;
                        return Ok(failed(event_tx, turn_id, TOKEN_BUDGET_DECLINED.to_string()));
                    }
                }

                if let Some(coordinator) = self
                    .harness
                    .as_ref()
                    .and_then(|harness| harness.routing.clone())
                    && coordinator.over_cap()
                {
                    match self
                        .enforce_spend_cap(
                            turn_id,
                            &coordinator,
                            event_tx,
                            &cancel,
                            inbox.as_deref_mut(),
                        )
                        .await
                    {
                        BudgetOutcome::Continue => {}
                        BudgetOutcome::Cancelled => return Ok(aborted(event_tx, turn_id)),
                        BudgetOutcome::Declined { detail } => {
                            let _ = event_tx.send(AgentEvent::Error {
                                code: ErrorCode::Budget,
                                message: detail.clone(),
                            });
                            return Ok(failed(event_tx, turn_id, detail));
                        }
                    }
                }

                if !self.over_high_watermark() {
                    self.pressure_wasted = false;
                }
                if self.over_high_watermark()
                    && !self.pressure_wasted
                    && pressure_compactions < MAX_PRESSURE_COMPACTIONS_PER_TURN
                    && pressure_step != Some(step_index)
                {
                    pressure_step = Some(step_index);
                    match self
                        .run_compaction(
                            CompactionTrigger::Pressure,
                            event_tx,
                            &cancel,
                            Some(clock),
                            None,
                            None,
                        )
                        .await
                    {
                        Ok(true) => pressure_compactions += 1,
                        Ok(false) => {}
                        Err(error) => return Err(error),
                    }
                }

                if continuation_boundary {
                    self.log_event(
                        event_tx,
                        SessionEvent::UserMessage {
                            source: InputSource::RuntimeContext,
                            content: vec![ContentBlock::Text(STEP_CONTINUE_NOTICE.to_string())],
                        },
                    )
                    .await;
                }
                if final_step {
                    self.log_event(
                        event_tx,
                        SessionEvent::UserMessage {
                            source: InputSource::RuntimeContext,
                            content: vec![ContentBlock::Text(STEP_LIMIT_NOTICE.to_string())],
                        },
                    )
                    .await;
                }
            }

            let step_id = StepId::new();
            self.log_event(
                event_tx,
                SessionEvent::StepStarted {
                    step_id: step_id.clone(),
                    turn_id: turn_id.clone(),
                },
            )
            .await;
            let step_started = Instant::now();

            let collected = match self
                .request_step(&genai_tools, &options, final_step, event_tx, &cancel, clock)
                .await
            {
                Ok(collected) => collected,
                Err(StepFailure::Cancelled) => {
                    self.log_step_ended(
                        event_tx,
                        &step_id,
                        StepFinish::Aborted {
                            cause: AbortCause::User,
                        },
                    )
                    .await;
                    return Ok(aborted(event_tx, turn_id));
                }
                Err(StepFailure::Error(error)) => {
                    self.log_step_ended(
                        event_tx,
                        &step_id,
                        StepFinish::Failed {
                            error: error.message(),
                        },
                    )
                    .await;
                    return Err(error);
                }
            };

            let end = collected.end;
            let step_usage = end
                .captured_usage
                .clone()
                .map(Usage::from)
                .unwrap_or_default();
            if end.captured_usage.is_some() {
                self.cumulative_usage.add(step_usage);
                let est_cost_usd = self.estimated_cost(self.cumulative_usage);
                self.note_routing_cost(est_cost_usd);
                self.record_usage_row(Some(turn_id), &step_usage, est_cost_usd)
                    .await;
                let _ = event_tx.send(AgentEvent::UsageUpdated {
                    usage: step_usage,
                    cumulative: self.cumulative_usage,
                    est_cost_usd,
                });
            }

            let final_text = if collected.text.is_empty() {
                end.captured_first_text().unwrap_or_default().to_string()
            } else {
                collected.text
            };

            if !final_text.is_empty() {
                let _ = event_tx.send(AgentEvent::ItemCompleted {
                    item: SessionItem::AgentMessage {
                        id: collected.text_item_id.unwrap_or_default(),
                        text: final_text.clone(),
                    },
                });
            }
            if !collected.reasoning.is_empty() {
                let _ = event_tx.send(AgentEvent::ItemCompleted {
                    item: SessionItem::Reasoning {
                        id: collected.reasoning_item_id.unwrap_or_default(),
                        summary: collected.reasoning.clone(),
                    },
                });
            }

            let mut tool_calls: Vec<ToolCall> = collected.tool_calls;
            for call in end.captured_tool_calls().unwrap_or_default() {
                match tool_calls
                    .iter_mut()
                    .find(|existing| existing.call_id == call.call_id)
                {
                    Some(existing) => {
                        if existing.thought_signatures.is_none() {
                            existing.thought_signatures = call.thought_signatures.clone();
                        }
                    }
                    None => tool_calls.push(call.clone()),
                }
            }
            let core_calls: Vec<meerai_core::ToolCall> =
                tool_calls.iter().map(meerai_core::ToolCall::from).collect();
            let call_signatures: Vec<Option<Vec<String>>> = tool_calls
                .iter()
                .map(|call| call.thought_signatures.clone())
                .collect();

            let mut content = Vec::new();
            if !final_text.is_empty() {
                content.push(ContentBlock::Text(final_text.clone()));
            }
            if !collected.reasoning.is_empty() {
                content.push(ContentBlock::Reasoning(collected.reasoning.clone()));
            }

            self.log_event(
                event_tx,
                SessionEvent::AssistantMessage {
                    provider: provider_of(&self.model),
                    model: self.model.clone(),
                    content,
                    tool_calls: core_calls,
                    call_signatures,
                    usage: step_usage,
                    stream_digest: StreamDigest {
                        final_text: final_text.clone(),
                        duration_ms: step_started.elapsed().as_millis() as u64,
                        usage: step_usage,
                    },
                },
            )
            .await;

            if tool_calls.is_empty() {
                if !final_text.is_empty() {
                    self.chat_history
                        .push(ChatMessage::assistant(final_text.clone()));
                }
            } else if let Some(assistant_msg) = end.into_assistant_message_for_tool_use() {
                self.chat_history.push(assistant_msg);
            } else {
                self.chat_history
                    .push(ChatMessage::from(tool_calls.clone()));
            }

            if tool_calls.is_empty() {
                last_tool_signature = None;
                repeated_tool_batches = 0;
            } else {
                let edit_batch = tool_calls.iter().all(|call| {
                    matches!(call.fn_name.as_str(), "code-edit_file" | "code-write_file")
                });
                let mut parts: Vec<String> = tool_calls
                    .iter()
                    .map(|call| format!("{}:{}", call.fn_name, call.fn_arguments))
                    .collect();
                parts.sort();
                let signature = parts.join("|");
                if last_tool_signature.as_deref() == Some(signature.as_str()) {
                    repeated_tool_batches += 1;
                } else {
                    repeated_tool_batches = 0;
                    last_tool_signature = Some(signature);
                }
                if repeated_tool_batches == 2 {
                    if edit_batch {
                        self.log_event(
                            event_tx,
                            SessionEvent::CircuitBreaker {
                                kind: BreakerKind::RepeatedIdenticalEdit,
                                action: BreakerAction::DiagnosticInjected,
                            },
                        )
                        .await;
                    }
                    self.log_event(
                        event_tx,
                        SessionEvent::UserMessage {
                            source: InputSource::RuntimeContext,
                            content: vec![ContentBlock::Text(
                                "You already called these tools with identical arguments. Read the results, change the arguments, or end the turn."
                                    .to_string(),
                            )],
                        },
                    )
                    .await;
                }
                if repeated_tool_batches >= 4 {
                    if edit_batch {
                        self.log_event(
                            event_tx,
                            SessionEvent::CircuitBreaker {
                                kind: BreakerKind::RepeatedIdenticalEdit,
                                action: BreakerAction::TurnFailed,
                            },
                        )
                        .await;
                    }
                    let mut names: Vec<&str> = tool_calls
                        .iter()
                        .map(|call| call.fn_name.as_str())
                        .collect();
                    names.sort_unstable();
                    names.dedup();
                    let offender = match names.len() {
                        0 => String::new(),
                        1 => names[0].to_string(),
                        2..=3 => names.join(", "),
                        count => format!("{} (+{} more)", names[0], count - 1),
                    };
                    return Err(TurnError::loop_detected(anyhow!(
                        "loop detected: repeated identical tool calls ({offender})"
                    )));
                }
            }

            let stop_reason = if tool_calls.is_empty() {
                None
            } else {
                match self
                    .execute_tool_calls(
                        &tool_calls,
                        turn_id,
                        step_index,
                        event_tx,
                        &cancel,
                        inbox.as_deref_mut(),
                        clock,
                    )
                    .await
                {
                    Ok(stop_reason) => stop_reason,
                    Err(error) => {
                        self.log_step_ended(
                            event_tx,
                            &step_id,
                            StepFinish::Failed {
                                error: error.message(),
                            },
                        )
                        .await;
                        return Err(error);
                    }
                }
            };

            self.log_event(
                event_tx,
                SessionEvent::UsageUpdated {
                    usage: step_usage,
                    cumulative: self.cumulative_usage,
                },
            )
            .await;

            let finish = if cancel.is_cancelled() {
                StepFinish::Aborted {
                    cause: AbortCause::User,
                }
            } else {
                StepFinish::Completed
            };
            self.log_step_ended(event_tx, &step_id, finish).await;
            self.emit_context_updated(event_tx);
            self.emit_goal_updated(event_tx);

            if cancel.is_cancelled() {
                return Ok(aborted(event_tx, turn_id));
            }

            if let Some(ended) = self
                .enforce_turn_deadline(turn_started, max_turn_secs, turn_id, event_tx)
                .await
            {
                return Ok(ended);
            }

            if let Some(reason) = stop_reason {
                return Ok(completed(event_tx, turn_id, reason));
            }

            let step_write = tool_calls.iter().any(|call| {
                let args = call.fn_arguments.to_string();
                self.tools.iter().any(|tool| {
                    tool.contain(&call.fn_name)
                        && tool.effect_of_call(&call.fn_name, &args) == EffectClass::Write
                })
            });

            if !tool_calls.is_empty() && progress_tracking {
                let delegated = tool_calls
                    .iter()
                    .any(|call| matches!(call.fn_name.as_str(), "subagent-run" | "acp-run"));
                if delegated {
                    no_progress_steps = 0;
                    no_progress_diagnosed = false;
                } else if step_write {
                    if self.file_changes_this_step > 0 {
                        no_progress_steps = 0;
                        no_progress_diagnosed = false;
                    } else {
                        no_progress_steps += 1;
                        if no_progress_steps == no_progress_diagnostic_steps
                            && !no_progress_diagnosed
                        {
                            no_progress_diagnosed = true;
                            self.log_event(
                                event_tx,
                                SessionEvent::CircuitBreaker {
                                    kind: BreakerKind::NoProgress,
                                    action: BreakerAction::DiagnosticInjected,
                                },
                            )
                            .await;
                            self.log_event(
                                event_tx,
                                SessionEvent::UserMessage {
                                    source: InputSource::RuntimeContext,
                                    content: vec![ContentBlock::Text(
                                        NO_PROGRESS_NOTICE.to_string(),
                                    )],
                                },
                            )
                            .await;
                        }
                        if no_progress_steps >= no_progress_fail_steps {
                            self.log_event(
                                event_tx,
                                SessionEvent::CircuitBreaker {
                                    kind: BreakerKind::NoProgress,
                                    action: BreakerAction::TurnFailed,
                                },
                            )
                            .await;
                            let offender = write_call_summary(&tool_calls, &self.tools);
                            let message = if offender.is_empty() {
                                format!(
                                    "no progress: {} write steps without a file change",
                                    no_progress_steps
                                )
                            } else {
                                format!(
                                    "no progress: {} write steps without a file change (last write: {offender})",
                                    no_progress_steps
                                )
                            };
                            return Err(TurnError::loop_detected(anyhow!(message)));
                        }
                    }
                }
            }

            if tool_calls.is_empty() {
                if self.drain_inbox(event_tx, inbox.as_deref_mut()).await {
                    return Ok(aborted(event_tx, turn_id));
                }
                if self.apply_pending_steers(event_tx).await {
                    continue;
                }
                let mut reason = None;
                let mut blocked = false;
                if !stop_hook_used {
                    let report = self
                        .run_hook(
                            HookEvent::Stop,
                            Some(turn_id),
                            None,
                            None,
                            None,
                            None,
                            event_tx,
                        )
                        .await;
                    blocked = matches!(
                        report.decision,
                        crate::hooks::HookDecision::Deny | crate::hooks::HookDecision::Block
                    );
                    if blocked {
                        reason = Some(report.reason().unwrap_or("blocked by hook").to_string());
                    }
                }
                if reason.is_none() {
                    reason = self.run_turn_stopping(turn_id, step_index);
                }
                if let Some(reason) = reason.as_mut() {
                    if blocked {
                        stop_hook_used = true;
                        if let Some(notice) =
                            self.rollback_after_verify_failure(turn_id, event_tx).await
                        {
                            reason.push_str("\n\n");
                            reason.push_str(&notice);
                        }
                    }
                    self.log_event(
                        event_tx,
                        SessionEvent::UserMessage {
                            source: InputSource::RuntimeContext,
                            content: vec![ContentBlock::Text(reason.clone())],
                        },
                    )
                    .await;
                    continue;
                }
                return Ok(completed(event_tx, turn_id, final_text));
            }
        }

        let message = match self.harness {
            Some(_) => format!("Exceeded maximum number of steps per turn ({total_steps})"),
            None => format!(
                "Exceeded maximum number of cycles ({})",
                self.config.max_cycles
            ),
        };
        Err(TurnError::budget(anyhow!(message)))
    }

    async fn collect_stream(
        &self,
        request: ChatRequest,
        options: &ChatOptions,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
    ) -> Result<CollectedStream, StreamFailure> {
        let _routing_permit = match self
            .harness
            .as_ref()
            .and_then(|harness| harness.routing.clone())
        {
            Some(coordinator) => {
                let provider = crate::routing::provider_of(&self.model);
                let permit = tokio::select! {
                    _ = cancel.cancelled() => return Err(StreamFailure::Cancelled),
                    permit = coordinator.acquire(&provider) => permit,
                };
                match permit {
                    Some(permit) => Some(permit),
                    None => {
                        return Err(StreamFailure::Provider {
                            error: anyhow!("routing coordinator closed"),
                            items: Vec::new(),
                        });
                    }
                }
            }
            None => None,
        };
        let streamer = self.streamer.clone();
        let mut stream = tokio::select! {
            _ = cancel.cancelled() => return Err(StreamFailure::Cancelled),
            result = streamer.stream_chat(&self.model, request, options) => {
                result.map_err(|error| StreamFailure::Provider { error, items: Vec::new() })?
            }
        };

        let mut text = String::new();
        let mut reasoning = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut text_item_id: Option<ItemId> = None;
        let mut reasoning_item_id: Option<ItemId> = None;
        let mut end: Option<StreamEnd> = None;
        let started = Instant::now();
        let mut events = 0usize;

        loop {
            tokio::select! {
                _ = cancel.cancelled() => return Err(StreamFailure::Cancelled),
                next_item = stream.next() => {
                    match next_item {
                        Some(Ok(event)) => {
                            events += 1;
                            match event {
                                ChatStreamEvent::Chunk(chunk) => {
                                    if !chunk.content.is_empty() {
                                        text.push_str(&chunk.content);
                                        let item_id = text_item_id
                                            .get_or_insert_with(|| start_text_item(event_tx));
                                        let _ = event_tx.send(AgentEvent::ItemDelta {
                                            item_id: item_id.clone(),
                                            delta: ItemDelta::AgentText(chunk.content),
                                        });
                                    }
                                }
                                ChatStreamEvent::ReasoningChunk(chunk) => {
                                    if !chunk.content.is_empty() {
                                        reasoning.push_str(&chunk.content);
                                        let item_id = reasoning_item_id
                                            .get_or_insert_with(|| start_reasoning_item(event_tx));
                                        let _ = event_tx.send(AgentEvent::ItemDelta {
                                            item_id: item_id.clone(),
                                            delta: ItemDelta::ReasoningSummary(chunk.content),
                                        });
                                    }
                                }
                                ChatStreamEvent::End(stream_end) => end = Some(stream_end),
                                ChatStreamEvent::ToolCallChunk(chunk) => {
                                    if let Some(existing) = tool_calls
                                        .iter_mut()
                                        .find(|call| call.call_id == chunk.tool_call.call_id)
                                    {
                                        *existing = chunk.tool_call;
                                    } else {
                                        tool_calls.push(chunk.tool_call);
                                    }
                                }
                                ChatStreamEvent::Start
                                | ChatStreamEvent::ThoughtSignatureChunk(_)
                                | ChatStreamEvent::Heartbeat => {}
                            }
                        }
                        Some(Err(error)) => {
                            let items = [reasoning_item_id.clone(), text_item_id.clone()]
                                .into_iter()
                                .flatten()
                                .collect();
                            return Err(StreamFailure::Provider { error, items });
                        }
                        None => break,
                    }
                }
            }
        }

        let end = end.unwrap_or_default();
        if reasoning.is_empty()
            && let Some(captured) = end.captured_reasoning_content.as_deref()
            && !captured.is_empty()
        {
            reasoning = captured.to_string();
        }
        let has_content = !text.is_empty()
            || !reasoning.is_empty()
            || !tool_calls.is_empty()
            || end
                .captured_first_text()
                .is_some_and(|text| !text.is_empty())
            || end
                .captured_tool_calls()
                .is_some_and(|calls| !calls.is_empty());
        if !has_content && end.captured_usage.is_none() {
            let (detail, retryable) = empty_stream_report(
                &self.model,
                end.captured_stop_reason.as_ref(),
                events,
                started.elapsed(),
            );
            return Err(StreamFailure::EmptyStream { detail, retryable });
        }

        Ok(CollectedStream {
            end,
            text,
            reasoning,
            tool_calls,
            text_item_id,
            reasoning_item_id,
        })
    }

    async fn request_step(
        &mut self,
        tools: &[Tool],
        options: &ChatOptions,
        final_step: bool,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
        clock: TurnClock,
    ) -> Result<CollectedStream, StepFailure> {
        let mut overflow_retried = false;
        let mut transient_failures = 0usize;

        loop {
            let mut request = ChatRequest::new(self.request_messages());
            let system_prompt = self.effective_system_prompt();
            if !system_prompt.is_empty() {
                request = request.with_system(system_prompt);
            }
            if !final_step && !tools.is_empty() {
                request = request.with_tools(tools.to_vec());
            }

            let collected = match clock.remaining() {
                Some(remaining) => match tokio::time::timeout(
                    remaining,
                    self.collect_stream(request, options, event_tx, cancel),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => return Err(StepFailure::Error(clock.error())),
                },
                None if clock.max_secs != 0 => return Err(StepFailure::Error(clock.error())),
                None => {
                    self.collect_stream(request, options, event_tx, cancel)
                        .await
                }
            };

            match collected {
                Ok(collected) => return Ok(collected),
                Err(StreamFailure::Cancelled) => return Err(StepFailure::Cancelled),
                Err(StreamFailure::EmptyStream { detail, retryable }) => {
                    self.log_event(
                        event_tx,
                        SessionEvent::AssistantAttempt {
                            provider: provider_of(&self.model),
                            model: self.model.clone(),
                            error: detail.clone(),
                            usage: Usage::default(),
                        },
                    )
                    .await;

                    if retryable && transient_failures + 1 < MAX_PROVIDER_ATTEMPTS {
                        transient_failures += 1;
                        let delay = provider_backoff_with_jitter(transient_failures);
                        tokio::select! {
                            _ = cancel.cancelled() => return Err(StepFailure::Cancelled),
                            _ = tokio::time::sleep(delay) => {}
                        }
                        continue;
                    }

                    let error = anyhow::Error::msg(detail);
                    return Err(StepFailure::Error(if retryable {
                        TurnError::transient_provider(error)
                    } else {
                        TurnError::provider(error)
                    }));
                }
                Err(StreamFailure::Provider { error, items }) if items.is_empty() => {
                    let message = format!("{error:#}");
                    let recoverable = self.harness.is_some() && is_context_overflow(&message);
                    self.log_event(
                        event_tx,
                        SessionEvent::AssistantAttempt {
                            provider: provider_of(&self.model),
                            model: self.model.clone(),
                            error: message.clone(),
                            usage: Usage::default(),
                        },
                    )
                    .await;

                    if recoverable && !overflow_retried {
                        overflow_retried = true;
                        let compacted = match self
                            .run_compaction(
                                CompactionTrigger::ContextOverflow,
                                event_tx,
                                cancel,
                                Some(clock),
                                None,
                                None,
                            )
                            .await
                        {
                            Ok(compacted) => compacted,
                            Err(error) => return Err(StepFailure::Error(error)),
                        };
                        if compacted {
                            continue;
                        }
                    }

                    let transient = !recoverable && is_transient_provider_error(&error, &message);
                    if transient && transient_failures + 1 < MAX_PROVIDER_ATTEMPTS {
                        transient_failures += 1;
                        let delay = provider_backoff_with_jitter(transient_failures);
                        tokio::select! {
                            _ = cancel.cancelled() => return Err(StepFailure::Cancelled),
                            _ = tokio::time::sleep(delay) => {}
                        }
                        continue;
                    }

                    let error = if recoverable {
                        TurnError::context_overflow(error)
                    } else if transient {
                        TurnError::transient_provider(error)
                    } else {
                        TurnError::provider(error)
                    };
                    return Err(StepFailure::Error(error));
                }
                Err(StreamFailure::Provider { error, items }) => {
                    let message = format!("{error:#}");
                    self.log_event(
                        event_tx,
                        SessionEvent::AssistantAttempt {
                            provider: provider_of(&self.model),
                            model: self.model.clone(),
                            error: message.clone(),
                            usage: Usage::default(),
                        },
                    )
                    .await;

                    let transient = is_transient_provider_error(&error, &message);
                    if transient {
                        discard_partial_items(event_tx, &items);
                        if transient_failures + 1 < MAX_PROVIDER_ATTEMPTS {
                            transient_failures += 1;
                            let delay = provider_backoff_with_jitter(transient_failures);
                            tokio::select! {
                                _ = cancel.cancelled() => return Err(StepFailure::Cancelled),
                                _ = tokio::time::sleep(delay) => {}
                            }
                            continue;
                        }
                    }

                    return Err(StepFailure::Error(if transient {
                        TurnError::transient_provider(error)
                    } else {
                        TurnError::provider(error)
                    }));
                }
            }
        }
    }
}
