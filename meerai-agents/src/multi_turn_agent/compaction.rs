use super::*;

fn message_text(message: &ChatMessage) -> String {
    message.content.texts().join("\n")
}

pub(super) fn estimate_context_tokens(messages: &[ChatMessage]) -> u64 {
    messages
        .iter()
        .map(|message| {
            let calls: u64 = message
                .content
                .tool_calls()
                .iter()
                .map(|call| estimate_tokens(&call.fn_arguments.to_string()))
                .sum();
            let results: u64 = message
                .content
                .tool_responses()
                .iter()
                .map(|response| estimate_tokens(&response.content))
                .sum();
            estimate_tokens(&message_text(message)) + calls + results
        })
        .sum()
}

pub(super) fn truncate_tokens(text: &str, max_tokens: u64) -> String {
    let limit = (max_tokens.saturating_mul(4)) as usize;
    if text.len() <= limit {
        return text.to_string();
    }

    let end = floor_char_boundary(text, limit);
    format!("{}\n... [truncated]", &text[..end])
}

fn role_label(role: &ChatRole) -> &'static str {
    match role {
        ChatRole::System => "system",
        ChatRole::User => "user",
        ChatRole::Assistant => "assistant",
        ChatRole::Tool => "tool",
    }
}

fn render_transcript(messages: &[ChatMessage], max_tokens: u64) -> String {
    let mut out = String::new();
    for message in messages {
        let label = role_label(&message.role);
        let text = message_text(message);
        if !text.is_empty() {
            out.push_str(label);
            out.push_str(": ");
            out.push_str(&truncate_tokens(&text, max_tokens));
            out.push('\n');
        }
        for call in message.content.tool_calls() {
            out.push_str(&format!(
                "{label} tool_call {} {}\n",
                call.fn_name,
                truncate_tokens(&call.fn_arguments.to_string(), max_tokens)
            ));
        }
        for response in message.content.tool_responses() {
            out.push_str(&format!(
                "{label} tool_result {}\n",
                truncate_tokens(&response.content, max_tokens)
            ));
        }
    }
    out
}

impl MultiTurnAgent {
    pub(super) async fn run_compaction(
        &mut self,
        trigger: CompactionTrigger,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
        clock: Option<TurnClock>,
        focus: Option<String>,
        from_turn_id: Option<TurnId>,
    ) -> Result<bool, TurnError> {
        let Some((events, verbatim_window_turns)) = self.harness.as_ref().map(|harness| {
            (
                harness.session.events(),
                harness.config.verbatim_window_turns,
            )
        }) else {
            return Ok(false);
        };

        if has_open_compaction(&events) {
            return Ok(false);
        }

        let compaction_id = CompactionId::new();
        self.log_event(
            event_tx,
            SessionEvent::CompactionStarted {
                compaction_id: compaction_id.clone(),
                trigger,
            },
        )
        .await;

        let pre_compact = self
            .run_hook(
                HookEvent::PreCompact,
                HookArgs {
                    turn_id: None,
                    ..HookArgs::default()
                },
                event_tx,
            )
            .await;
        if matches!(
            pre_compact.decision,
            crate::hooks::HookDecision::Deny | crate::hooks::HookDecision::Block
        ) {
            let reason = pre_compact
                .reason()
                .unwrap_or("blocked by hook")
                .to_string();
            self.log_event(
                event_tx,
                SessionEvent::CompactionEnded {
                    compaction_id,
                    result: CompactionResult::Skipped { reason },
                },
            )
            .await;
            self.run_hook(
                HookEvent::PostCompact,
                HookArgs {
                    turn_id: None,
                    ..HookArgs::default()
                },
                event_tx,
            )
            .await;
            return Ok(false);
        }

        let range = match &from_turn_id {
            Some(turn_id) => meerai_session::plan_compaction_boundary_from(&events, turn_id),
            None => plan_compaction_boundary(&events, verbatim_window_turns),
        };
        let Some(range) = range else {
            self.log_event(
                event_tx,
                SessionEvent::CompactionEnded {
                    compaction_id,
                    result: CompactionResult::Skipped {
                        reason: COMPACTION_EMPTY_SKIP.to_string(),
                    },
                },
            )
            .await;
            self.run_hook(
                HookEvent::PostCompact,
                HookArgs {
                    turn_id: None,
                    ..HookArgs::default()
                },
                event_tx,
            )
            .await;
            return Ok(false);
        };

        let end_index = events
            .iter()
            .position(|logged| logged.seq == range.end_inclusive)
            .map_or(events.len(), |index| index + 1);
        let projection_policy = self.projection_policy();
        let distinct_model = self
            .harness
            .as_ref()
            .is_some_and(|harness| harness.config.summarizer_model.is_some());
        let instruction = match focus.as_deref().map(str::trim) {
            Some(focus) if !focus.is_empty() => {
                format!("{SUMMARY_INSTRUCTION}\n\nFocus this summary on: {focus}")
            }
            _ => SUMMARY_INSTRUCTION.to_string(),
        };
        let request = if distinct_model {
            let mut transcript_policy = projection_policy.clone();
            transcript_policy.clear_old_tool_results = false;
            let transcript = render_transcript(
                &derive_messages(&events[..end_index], &transcript_policy),
                SUMMARY_MESSAGE_TOKEN_LIMIT,
            );
            ChatRequest::new(vec![ChatMessage::user(transcript)]).with_system(instruction.clone())
        } else {
            let mut messages = self.request_messages();
            messages.push(ChatMessage::user(instruction.clone()));
            let tools: Vec<Tool> = self
                .tools
                .iter()
                .flat_map(|tool| tool.definition())
                .map(|definition| definition.to_genai())
                .collect();
            let mut request = ChatRequest::new(messages);
            let system = self.effective_system_prompt();
            if !system.is_empty() {
                request = request.with_system(system);
            }
            if !tools.is_empty() {
                request = request.with_tools(tools);
            }
            request
        };
        let before = estimate_context_tokens(&self.request_messages());

        let mut budget_error: Option<TurnError> = None;
        let summary = match clock.and_then(|clock| clock.remaining().map(|left| (clock, left))) {
            Some((clock, remaining)) => {
                match tokio::time::timeout(remaining, self.summarize(request, cancel)).await {
                    Ok(result) => result,
                    Err(_) => {
                        let error = clock.error();
                        budget_error = Some(error);
                        Err(anyhow!("turn exceeded {}s", clock.max_secs))
                    }
                }
            }
            None => match clock {
                Some(clock) if clock.max_secs != 0 => {
                    let error = clock.error();
                    budget_error = Some(error);
                    Err(anyhow!("turn exceeded {}s", clock.max_secs))
                }
                _ => self.summarize(request, cancel).await,
            },
        };

        match summary {
            Ok((state_block, usage)) => {
                let auto_memory = self
                    .harness
                    .as_ref()
                    .is_some_and(|harness| harness.config.auto_memory);
                if auto_memory && let Some(capture) = &self.memory_capture {
                    capture.capture_summary(&state_block);
                }
                self.record_summary_usage(usage, event_tx).await;
                self.log_event(
                    event_tx,
                    SessionEvent::CompactionSummary {
                        compaction_id: compaction_id.clone(),
                        replaced: range,
                        summary: summary_message(&state_block),
                    },
                )
                .await;

                let after = estimate_context_tokens(&self.request_messages());
                let freed_tokens = before.saturating_sub(after);
                if trigger == CompactionTrigger::Pressure {
                    self.pressure_wasted = freed_tokens == 0;
                }
                let item_id = ItemId::new();
                let _ = event_tx.send(AgentEvent::ItemStarted {
                    item: SessionItem::Compaction {
                        id: item_id.clone(),
                        trigger,
                        freed_tokens: 0,
                    },
                });
                self.log_event(
                    event_tx,
                    SessionEvent::CompactionEnded {
                        compaction_id,
                        result: CompactionResult::Applied { freed_tokens },
                    },
                )
                .await;
                let _ = event_tx.send(AgentEvent::ItemCompleted {
                    item: SessionItem::Compaction {
                        id: item_id,
                        trigger,
                        freed_tokens,
                    },
                });
                self.run_hook(
                    HookEvent::PostCompact,
                    HookArgs {
                        turn_id: None,
                        ..HookArgs::default()
                    },
                    event_tx,
                )
                .await;
                self.emit_context_updated(event_tx);
                Ok(true)
            }
            Err(error) => {
                let message = format!("{error:#}");
                self.log_event(
                    event_tx,
                    SessionEvent::CompactionEnded {
                        compaction_id,
                        result: CompactionResult::Failed {
                            error: message.clone(),
                        },
                    },
                )
                .await;
                self.run_hook(
                    HookEvent::PostCompact,
                    HookArgs {
                        turn_id: None,
                        ..HookArgs::default()
                    },
                    event_tx,
                )
                .await;
                if budget_error.is_none() {
                    self.log_event(
                        event_tx,
                        SessionEvent::Error {
                            code: ErrorCode::Internal,
                            message,
                            retryable: false,
                        },
                    )
                    .await;
                }
                match budget_error {
                    Some(error) => Err(error),
                    None => Ok(false),
                }
            }
        }
    }

    async fn summarize(
        &self,
        request: ChatRequest,
        cancel: &CancellationToken,
    ) -> Result<(String, Option<Usage>)> {
        let mut options = ChatOptions::default()
            .with_capture_content(true)
            .with_capture_usage(true)
            .with_stream_header_timeout(STREAM_HEADER_TIMEOUT)
            .with_stream_read_timeout(STREAM_READ_TIMEOUT);
        let plan_usage = self
            .harness
            .as_ref()
            .is_some_and(|harness| harness.config.plan_usage);
        if !plan_usage
            && self
                .harness
                .as_ref()
                .is_none_or(|harness| harness.config.prompt_caching)
        {
            options = options.with_cache_control(CacheControl::Ephemeral);
        }
        let model = self
            .harness
            .as_ref()
            .and_then(|harness| harness.config.summarizer_model.clone())
            .or_else(|| {
                self.harness
                    .as_ref()
                    .and_then(|harness| {
                        harness
                            .config
                            .routing
                            .route(&meerai_common::settings::TaskType::Compact)
                    })
                    .map(|route| route.model_spec())
            })
            .unwrap_or_else(|| self.model.clone());
        let _routing_permit = match self
            .harness
            .as_ref()
            .and_then(|harness| harness.routing.clone())
        {
            Some(coordinator) => {
                let provider = crate::routing::provider_of(&model);
                coordinator.acquire(&provider).await
            }
            None => None,
        };
        let streamer = self.streamer.clone();
        let mut stream = tokio::select! {
            _ = cancel.cancelled() => return Err(anyhow!("compaction cancelled")),
            result = streamer.stream_chat(self.provider.as_deref(), &model, request, &options) => result?,
        };

        let mut text = String::new();
        let mut end: Option<StreamEnd> = None;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return Err(anyhow!("compaction cancelled")),
                next_item = stream.next() => match next_item {
                    Some(Ok(ChatStreamEvent::Chunk(chunk))) => text.push_str(&chunk.content),
                    Some(Ok(ChatStreamEvent::End(stream_end))) => end = Some(stream_end),
                    Some(Ok(_)) => {}
                    Some(Err(error)) => return Err(error),
                    None => break,
                }
            }
        }

        let usage = end
            .as_ref()
            .and_then(|end| end.captured_usage.clone())
            .map(Usage::from);

        if text.trim().is_empty()
            && let Some(end) = end
        {
            text = end.captured_first_text().unwrap_or_default().to_string();
        }
        if text.trim().is_empty() {
            return Err(anyhow!(COMPACTION_EMPTY_SUMMARY));
        }
        Ok((text, usage))
    }

    async fn record_summary_usage(
        &mut self,
        usage: Option<Usage>,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) {
        let Some(usage) = usage else {
            return;
        };
        self.cumulative_usage.add(usage);
        let est_cost_usd = self.estimated_cost(self.cumulative_usage);
        self.note_routing_cost(est_cost_usd);
        let _ = event_tx.send(AgentEvent::UsageUpdated {
            usage,
            cumulative: self.cumulative_usage,
            est_cost_usd,
        });
        self.log_event(
            event_tx,
            SessionEvent::UsageUpdated {
                usage,
                cumulative: self.cumulative_usage,
            },
        )
        .await;
    }

    pub(super) fn over_high_watermark(&mut self) -> bool {
        let Some(threshold) = self.harness.as_ref().map(|harness| {
            harness
                .config
                .context_window_tokens
                .saturating_mul(u64::from(harness.config.high_watermark_pct))
                / 100
        }) else {
            return false;
        };
        let system_tokens = estimate_tokens(&self.effective_system_prompt());
        let messages = self.request_messages();
        let estimated =
            estimate_context_tokens(&messages) + system_tokens + self.tool_schema_tokens;
        estimated > threshold
    }

    pub(super) async fn enforce_spend_cap(
        &mut self,
        turn_id: &TurnId,
        coordinator: &std::sync::Arc<crate::routing::RoutingCoordinator>,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
        inbox: Option<&mut mpsc::UnboundedReceiver<AgentCommand>>,
    ) -> BudgetOutcome {
        let spent = coordinator.spent_usd();
        let cap = coordinator.cap().unwrap_or(0.0);
        let request_id = RequestId::new();
        let preview = ApprovalPreview {
            title: "Session spend cap reached".to_string(),
            detail: format!("session spent ${spent:.4} of the ${cap:.4} cap"),
        };
        let _ = event_tx.send(AgentEvent::ApprovalRequested {
            request: ApprovalRequest {
                request_id: request_id.clone(),
                turn_id: turn_id.clone(),
                kind: ApprovalKind::Spend,
                preview: preview.clone(),
                effect: None,
            },
        });
        self.log_event(
            event_tx,
            SessionEvent::ApprovalRequested {
                request_id: request_id.clone(),
                kind: ApprovalKind::Spend,
                preview,
            },
        )
        .await;

        let outcome = self
            .await_decision(&request_id, event_tx, cancel, inbox)
            .await;
        match outcome.decision {
            ApprovalDecision::Accept => {
                coordinator.set_cap(Some(spent + cap.max(1.0)));
                BudgetOutcome::Continue
            }
            ApprovalDecision::AcceptForSession => {
                coordinator.set_cap(None);
                BudgetOutcome::Continue
            }
            ApprovalDecision::Cancel => {
                cancel.cancel();
                BudgetOutcome::Cancelled
            }
            ApprovalDecision::Decline => BudgetOutcome::Declined {
                detail: SPEND_CAP_DECLINED.to_string(),
            },
            ApprovalDecision::DeclineWithReason(reason) => BudgetOutcome::Declined {
                detail: format!("{SPEND_CAP_DECLINED}: {reason}"),
            },
        }
    }

    pub(super) async fn enforce_token_budget(
        &mut self,
        turn_id: &TurnId,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
        inbox: Option<&mut mpsc::UnboundedReceiver<AgentCommand>>,
    ) -> BudgetOutcome {
        let Some((cap, window)) = self.harness.as_ref().and_then(|harness| {
            harness
                .config
                .max_session_tokens
                .map(|cap| (cap, harness.config.context_window_tokens))
        }) else {
            return BudgetOutcome::Continue;
        };
        if cap == 0 {
            return BudgetOutcome::Continue;
        }

        let used = self
            .cumulative_usage
            .total_tokens
            .map(|tokens| tokens.max(0) as u64)
            .unwrap_or(0);
        if used < cap {
            return BudgetOutcome::Continue;
        }

        let request_id = RequestId::new();
        let preview = ApprovalPreview {
            title: "Session token budget reached".to_string(),
            detail: format!("session used {used} of {cap} tokens"),
        };
        let _ = event_tx.send(AgentEvent::ApprovalRequested {
            request: ApprovalRequest {
                request_id: request_id.clone(),
                turn_id: turn_id.clone(),
                kind: ApprovalKind::Spend,
                preview: preview.clone(),
                effect: None,
            },
        });
        self.log_event(
            event_tx,
            SessionEvent::ApprovalRequested {
                request_id: request_id.clone(),
                kind: ApprovalKind::Spend,
                preview,
            },
        )
        .await;

        let outcome = self
            .await_decision(&request_id, event_tx, cancel, inbox)
            .await;

        match outcome.decision {
            ApprovalDecision::Accept => {
                if let Some(harness) = &mut self.harness {
                    harness.config.max_session_tokens = Some(cap.saturating_add(window));
                }
                BudgetOutcome::Continue
            }
            ApprovalDecision::AcceptForSession => {
                if let Some(harness) = &mut self.harness {
                    harness.config.max_session_tokens = None;
                }
                BudgetOutcome::Continue
            }
            ApprovalDecision::Cancel => {
                cancel.cancel();
                BudgetOutcome::Cancelled
            }
            ApprovalDecision::Decline => BudgetOutcome::Declined {
                detail: TOKEN_BUDGET_DECLINED.to_string(),
            },
            ApprovalDecision::DeclineWithReason(reason) => BudgetOutcome::Declined {
                detail: format!("{TOKEN_BUDGET_DECLINED}: {reason}"),
            },
        }
    }
}
