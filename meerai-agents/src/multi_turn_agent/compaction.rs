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
    ) -> Option<TurnError> {
        let (events, verbatim_window_turns) = self.harness.as_ref().map(|harness| {
            (
                harness.session.events(),
                harness.config.verbatim_window_turns,
            )
        })?;

        if has_open_compaction(&events) {
            return None;
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

        let Some(range) = plan_compaction_boundary(&events, verbatim_window_turns) else {
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
            return None;
        };

        let end_index = events
            .iter()
            .position(|logged| logged.seq == range.end_inclusive)
            .map_or(events.len(), |index| index + 1);
        let transcript = render_transcript(
            &derive_messages(&events[..end_index], &self.projection_policy()),
            SUMMARY_MESSAGE_TOKEN_LIMIT,
        );
        let before = estimate_context_tokens(&self.request_messages());
        let request = ChatRequest::new(vec![ChatMessage::user(transcript)])
            .with_system(SUMMARY_INSTRUCTION.to_string());

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
                let item_id = ItemId::new();
                let _ = event_tx.send(AgentEvent::ItemStarted {
                    item: SessionItem::Compaction {
                        id: item_id.clone(),
                        trigger,
                        freed_tokens: 0,
                    },
                });
                let _ = event_tx.send(AgentEvent::ItemCompleted {
                    item: SessionItem::Compaction {
                        id: item_id,
                        trigger,
                        freed_tokens,
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
                None
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
                budget_error
            }
        }
    }

    async fn summarize(
        &self,
        request: ChatRequest,
        cancel: &CancellationToken,
    ) -> Result<(String, Option<Usage>)> {
        let options = ChatOptions::default()
            .with_capture_content(true)
            .with_capture_usage(true);
        let model = self
            .harness
            .as_ref()
            .and_then(|harness| harness.config.summarizer_model.clone())
            .unwrap_or_else(|| self.model.clone());
        let streamer = self.streamer.clone();
        let mut stream = tokio::select! {
            _ = cancel.cancelled() => return Err(anyhow!("compaction cancelled")),
            result = streamer.stream_chat(&model, request, &options) => result?,
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
        let _ = event_tx.send(AgentEvent::UsageUpdated {
            usage,
            cumulative: self.cumulative_usage,
            est_cost_usd: None,
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

    pub(super) fn over_high_watermark(&self) -> bool {
        let Some(harness) = &self.harness else {
            return false;
        };
        let threshold = harness
            .config
            .context_window_tokens
            .saturating_mul(u64::from(harness.config.high_watermark_pct))
            / 100;
        let estimated = estimate_context_tokens(&self.request_messages())
            + estimate_tokens(&self.effective_system_prompt());
        estimated > threshold
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
