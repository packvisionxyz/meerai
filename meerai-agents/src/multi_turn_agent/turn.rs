use super::*;

fn is_context_overflow(message: &str) -> bool {
    let lowered = message.to_lowercase();
    OVERFLOW_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
}

fn is_transient_error(message: &str) -> bool {
    let lowered = message.to_lowercase();
    !NON_TRANSIENT_ERROR_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
}

pub(super) fn provider_backoff_after(failures: usize) -> Duration {
    let factor = 1u64 << failures.saturating_sub(1).min(8);
    Duration::from_millis(PROVIDER_BACKOFF_MS.saturating_mul(factor))
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
            harness.policy.clear_scoped_grants(GrantScope::Turn);
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

        let genai_tools: Vec<Tool> = self
            .tools
            .iter()
            .flat_map(|tool| tool.definition())
            .map(|def| def.to_genai())
            .collect();

        let options = ChatOptions::default()
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

        let harnessed = self.harness.is_some();
        let max_steps = self
            .harness
            .as_ref()
            .map_or(self.config.max_cycles, |harness| {
                harness.config.max_steps_per_turn
            });
        let max_turn_secs = self
            .harness
            .as_ref()
            .map_or(0, |harness| harness.config.max_turn_secs);
        let turn_started = Instant::now();
        let clock = TurnClock {
            started: turn_started,
            max_secs: max_turn_secs,
        };
        let mut pressured = false;
        let mut stop_hook_used = false;
        let mut last_tool_signature: Option<String> = None;
        let mut repeated_tool_batches = 0usize;

        for step_index in 0..max_steps {
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
            if self.pending_compact {
                self.pending_compact = false;
                if let Some(error) = self
                    .run_compaction(CompactionTrigger::Manual, event_tx, &cancel, Some(clock))
                    .await
                {
                    return Err(error);
                }
            }
            self.apply_pending_steers(event_tx).await;

            let final_step = harnessed && step_index + 1 == max_steps;
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

                if !pressured && self.over_high_watermark() {
                    pressured = true;
                    if let Some(error) = self
                        .run_compaction(CompactionTrigger::Pressure, event_tx, &cancel, Some(clock))
                        .await
                    {
                        return Err(error);
                    }
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
                let _ = event_tx.send(AgentEvent::UsageUpdated {
                    usage: step_usage,
                    cumulative: self.cumulative_usage,
                    est_cost_usd: self.estimated_cost(self.cumulative_usage),
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
                    return Err(TurnError::loop_detected(anyhow!(
                        "loop detected: repeated identical tool calls"
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

            if tool_calls.is_empty() {
                if self.drain_inbox(event_tx, inbox.as_deref_mut()).await {
                    return Ok(aborted(event_tx, turn_id));
                }
                if self.apply_pending_steers(event_tx).await {
                    continue;
                }
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
                    let reason = match report.decision {
                        crate::hooks::HookDecision::Deny | crate::hooks::HookDecision::Block => {
                            Some(report.reason().unwrap_or("blocked by hook").to_string())
                        }
                        _ => self.run_turn_stopping(turn_id, step_index),
                    };
                    if let Some(reason) = reason {
                        stop_hook_used = true;
                        self.log_event(
                            event_tx,
                            SessionEvent::UserMessage {
                                source: InputSource::RuntimeContext,
                                content: vec![ContentBlock::Text(reason)],
                            },
                        )
                        .await;
                        continue;
                    }
                }
                return Ok(completed(event_tx, turn_id, final_text));
            }
        }

        let message = match self.harness {
            Some(_) => format!("Exceeded maximum number of steps per turn ({max_steps})"),
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
        let streamer = self.streamer.clone();
        let mut stream = tokio::select! {
            _ = cancel.cancelled() => return Err(StreamFailure::Cancelled),
            result = streamer.stream_chat(&self.model, request, options) => {
                result.map_err(|error| StreamFailure::Provider { error, partial: false })?
            }
        };

        let mut text = String::new();
        let mut reasoning = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut text_item_id: Option<ItemId> = None;
        let mut reasoning_item_id: Option<ItemId> = None;
        let mut end: Option<StreamEnd> = None;

        loop {
            tokio::select! {
                _ = cancel.cancelled() => return Err(StreamFailure::Cancelled),
                next_item = stream.next() => {
                    match next_item {
                        Some(Ok(event)) => match event {
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
                        },
                        Some(Err(error)) => {
                            let partial = !text.is_empty() || !reasoning.is_empty();
                            return Err(StreamFailure::Provider { error, partial });
                        }
                        None => break,
                    }
                }
            }
        }

        Ok(CollectedStream {
            end: end.unwrap_or_default(),
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
                Err(StreamFailure::Provider { error, partial }) if !partial => {
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
                        if let Some(error) = self
                            .run_compaction(
                                CompactionTrigger::ContextOverflow,
                                event_tx,
                                cancel,
                                Some(clock),
                            )
                            .await
                        {
                            return Err(StepFailure::Error(error));
                        }
                        continue;
                    }

                    let transient = !recoverable && is_transient_error(&message);
                    if transient && transient_failures + 1 < MAX_PROVIDER_ATTEMPTS {
                        transient_failures += 1;
                        let delay = provider_backoff_after(transient_failures);
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
                Err(StreamFailure::Provider { error, .. }) => {
                    let message = format!("{error:#}");
                    self.log_event(
                        event_tx,
                        SessionEvent::AssistantAttempt {
                            provider: provider_of(&self.model),
                            model: self.model.clone(),
                            error: message,
                            usage: Usage::default(),
                        },
                    )
                    .await;
                    return Err(StepFailure::Error(TurnError::provider(error)));
                }
            }
        }
    }
}
