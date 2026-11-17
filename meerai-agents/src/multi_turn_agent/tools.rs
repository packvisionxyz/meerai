use super::*;
use crate::harness::HarnessConfig;
use crate::question::{QUESTION_TOOL, format_answers, parse_questions};
use crate::subagent::{ForwardedApproval, SUBAGENT_BRIDGE, SubagentBridge, SubagentBudgetUpdate};

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
            let mut resource = spec.resource_arg.map(field).unwrap_or_default();
            if resource.is_empty() && tool_name == "sac-run_script" {
                let lang = field("language");
                let script = field("script");
                resource = if !script.is_empty() {
                    let preview: String = script
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(80)
                        .collect();
                    if lang.is_empty() {
                        preview
                    } else {
                        format!("{lang}: {preview}")
                    }
                } else {
                    lang
                };
            }
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

const LSP_DIAGNOSTICS_TOOL: &str = "lsp-get_diagnostics";
const LSP_DIAGNOSTICS_TIMEOUT: Duration = Duration::from_secs(5);
const LSP_DIAGNOSTICS_MAX_BYTES: usize = 4096;

pub(super) async fn collect_post_edit_diagnostics(
    tools: &[Box<dyn Toolset>],
    path: &str,
) -> Option<String> {
    let tool = tools
        .iter()
        .find(|tool| tool.contain(LSP_DIAGNOSTICS_TOOL))?;
    let args = serde_json::json!({ "path": path }).to_string();
    let run = tokio::time::timeout(
        LSP_DIAGNOSTICS_TIMEOUT,
        tool.invoke_structured(LSP_DIAGNOSTICS_TOOL, &args),
    )
    .await
    .ok()?
    .ok()?;
    let ToolOutput::Text(text) = run.output else {
        return None;
    };
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let end = floor_char_boundary(text, LSP_DIAGNOSTICS_MAX_BYTES);
    Some(crate::screen::screen_untrusted_text(
        "lsp_diagnostics",
        &text[..end],
    ))
}

const POST_EDIT_DIAGNOSTIC_TOOLS: [&str; 2] = ["code-edit_file", "code-write_file"];

fn should_collect_post_edit_diagnostics(
    enabled: bool,
    status: ItemStatus,
    tool_name: &str,
    path: Option<&str>,
    policy_allowed: bool,
) -> bool {
    enabled
        && status == ItemStatus::Succeeded
        && POST_EDIT_DIAGNOSTIC_TOOLS.contains(&tool_name)
        && path.is_some()
        && policy_allowed
}

#[cfg(test)]
mod post_edit_diagnostics_tests {
    use super::should_collect_post_edit_diagnostics;
    use crate::events::ItemStatus;

    #[test]
    fn collects_after_a_policy_allowed_edit() {
        assert!(should_collect_post_edit_diagnostics(
            true,
            ItemStatus::Succeeded,
            "code-edit_file",
            Some("src/lib.rs"),
            true
        ));
        assert!(should_collect_post_edit_diagnostics(
            true,
            ItemStatus::Succeeded,
            "code-write_file",
            Some("src/lib.rs"),
            true
        ));
    }

    #[test]
    fn skips_when_the_setting_is_disabled() {
        assert!(!should_collect_post_edit_diagnostics(
            false,
            ItemStatus::Succeeded,
            "code-edit_file",
            Some("src/lib.rs"),
            true
        ));
    }

    #[test]
    fn skips_when_the_tool_did_not_succeed() {
        for status in [
            ItemStatus::Proposed,
            ItemStatus::Pending,
            ItemStatus::InProgress,
            ItemStatus::Failed,
            ItemStatus::Stopped,
            ItemStatus::Denied,
            ItemStatus::Cancelled,
        ] {
            assert!(!should_collect_post_edit_diagnostics(
                true,
                status,
                "code-edit_file",
                Some("src/lib.rs"),
                true
            ));
        }
    }

    #[test]
    fn skips_tools_that_do_not_write_files() {
        assert!(!should_collect_post_edit_diagnostics(
            true,
            ItemStatus::Succeeded,
            "code-read_file",
            Some("src/lib.rs"),
            true
        ));
    }

    #[test]
    fn skips_when_the_call_has_no_path() {
        assert!(!should_collect_post_edit_diagnostics(
            true,
            ItemStatus::Succeeded,
            "code-edit_file",
            None,
            true
        ));
    }

    #[test]
    fn skips_when_policy_denies_the_read() {
        assert!(!should_collect_post_edit_diagnostics(
            true,
            ItemStatus::Succeeded,
            "code-edit_file",
            Some("src/lib.rs"),
            false
        ));
    }
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
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn execute_tool_calls(
        &mut self,
        tool_calls: &[ToolCall],
        turn_id: &TurnId,
        step_index: usize,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
        mut inbox: Option<&mut mpsc::UnboundedReceiver<AgentCommand>>,
        clock: TurnClock,
    ) -> Result<Option<String>, TurnError> {
        self.file_changes_this_step = 0;
        let mut plans: Vec<CallPlan> = Vec::with_capacity(tool_calls.len());

        for tc in tool_calls {
            if cancel.is_cancelled() {
                return Ok(None);
            }

            let normalized_args = normalized_tool_args(&tc.fn_arguments);
            let args_str = coerce_tool_args(&self.tools, &tc.fn_name, &normalized_args);
            let effect = self
                .tools
                .iter()
                .find(|tool| tool.contain(&tc.fn_name))
                .map(|tool| tool.effect_of_call(&tc.fn_name, &args_str))
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
                    &args_str,
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

        let questions_registered = self.tools.iter().any(|tool| tool.contain(QUESTION_TOOL));
        for plan in plans.iter_mut() {
            if questions_registered && plan.authorized() && plan.tool_call.fn_name == QUESTION_TOOL
            {
                let run = self
                    .ask_questions(
                        &plan.tool_call,
                        turn_id,
                        event_tx,
                        cancel,
                        inbox.as_deref_mut(),
                    )
                    .await;
                plan.outcome = Some(CallOutcome::Run(run));
            }
        }

        let (workspace, snapshots) = match self.harness.as_ref() {
            Some(harness) => (
                Some(harness.policy.workspace_root().to_path_buf()),
                harness.session.snapshot_store(),
            ),
            None => (None, None),
        };

        if let Some(workspace) = workspace.as_deref() {
            for plan in plans.iter().filter(|plan| plan.authorized()) {
                let tool_plan = ToolPlan {
                    turn_id,
                    step_index,
                    tool_name: &plan.tool_call.fn_name,
                    call_id: &plan.tool_call.call_id,
                    args: &plan.args,
                    effect: plan.effect,
                };
                let context = ToolContext {
                    workspace,
                    snapshots: snapshots.as_ref(),
                };
                for event in self.run_before_tool(&tool_plan, &context).await {
                    self.log_event(event_tx, event).await;
                }
            }
        }

        let tools = &self.tools;
        let timeout_ms = self
            .harness
            .as_ref()
            .map(|harness| harness.config.tool_timeout_ms);
        let max_retries = self.config.max_retries;
        let mut ctx = DecisionCtx {
            harness: &mut self.harness,
            hooks: &mut self.hooks,
            pending_steers: &mut self.pending_steers,
            pending_compact: &mut self.pending_compact,
            chat_history: &mut self.chat_history,
            model: &mut self.model,
            shared_model: &self.shared_model,
            cumulative_usage: &mut self.cumulative_usage,
        };
        let mut halted = false;
        let mut cursor = 0;

        while cursor < plans.len() && !halted {
            if cancel.is_cancelled() {
                break;
            }

            if plans[cursor].outcome.is_some() {
                cursor += 1;
                continue;
            }

            if !plans[cursor].authorized() || plans[cursor].effect != EffectClass::Read {
                if plans[cursor].authorized() {
                    let result = {
                        let fn_name = &plans[cursor].tool_call.fn_name;
                        let args = &plans[cursor].args;
                        run_with_bridge(
                            &mut ctx,
                            event_tx,
                            cancel,
                            inbox.as_deref_mut(),
                            clock,
                            run_tool_from(tools, timeout_ms, max_retries, fn_name, args, cancel),
                        )
                        .await
                        .unwrap_or_else(|()| Ok(tool_budget_exceeded(clock.max_secs)))
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
                && plans[end].outcome.is_none()
                && plans[end].authorized()
                && plans[end].effect == EffectClass::Read
            {
                end += 1;
            }

            let results = {
                let reads = (cursor..end).map(|index| {
                    let fn_name = &plans[index].tool_call.fn_name;
                    let args = &plans[index].args;
                    async move {
                        let result =
                            run_tool_from(tools, timeout_ms, max_retries, fn_name, args, cancel)
                                .await;
                        (index, result)
                    }
                });
                run_with_bridge(
                    &mut ctx,
                    event_tx,
                    cancel,
                    inbox.as_deref_mut(),
                    clock,
                    join_all(reads),
                )
                .await
            };
            let results = match results {
                Ok(results) => results,
                Err(()) => {
                    let timeout = tool_budget_exceeded(clock.max_secs);
                    for plan in plans.iter_mut().take(end).skip(cursor) {
                        plan.outcome = Some(CallOutcome::Run(timeout.clone()));
                    }
                    cursor = end;
                    continue;
                }
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
                            crate::screen::screen_run(&mut run);
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
                            let text = crate::screen::screen_untrusted_text(
                                "tool_output",
                                &redact_text(&format!("Tool error: {error:#}")),
                            );
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

            let outcome = ToolOutcome {
                turn_id: turn_id.clone(),
                step_index,
                tool_name: tool_call.fn_name.clone(),
                call_id: tool_call.call_id.clone(),
                status,
                observation: observation.clone(),
            };
            for feedback in self.run_after_tool(&outcome) {
                self.log_event(
                    event_tx,
                    SessionEvent::UserMessage {
                        source: InputSource::RuntimeContext,
                        content: vec![ContentBlock::Text(feedback)],
                    },
                )
                .await;
            }

            let hook_event = if matches!(status, ItemStatus::Succeeded | ItemStatus::Stopped) {
                HookEvent::PostToolUse
            } else {
                HookEvent::PostToolUseFailure
            };
            let report = self
                .run_hook(
                    hook_event,
                    Some(turn_id),
                    Some(&tool_call.fn_name),
                    Some(&args_str),
                    Some(&item_output),
                    None,
                    event_tx,
                )
                .await;
            if matches!(
                report.decision,
                crate::hooks::HookDecision::Deny | crate::hooks::HookDecision::Block
            ) {
                self.log_event(
                    event_tx,
                    SessionEvent::UserMessage {
                        source: InputSource::RuntimeContext,
                        content: vec![ContentBlock::Text(
                            report.reason().unwrap_or("blocked by hook").to_string(),
                        )],
                    },
                )
                .await;
            }

            let repair_hint = argument_repair_hint(&self.tools, &tool_call.fn_name, &observation);
            self.log_event(
                event_tx,
                SessionEvent::ToolResult {
                    call_id,
                    output: observation,
                },
            )
            .await;

            if let Some(hint) = repair_hint {
                self.log_event(
                    event_tx,
                    SessionEvent::UserMessage {
                        source: InputSource::RuntimeContext,
                        content: vec![ContentBlock::Text(hint)],
                    },
                )
                .await;
            }

            if self.post_edit_diagnostics
                && matches!(status, ItemStatus::Succeeded)
                && POST_EDIT_DIAGNOSTIC_TOOLS.contains(&tool_call.fn_name.as_str())
                && let Some(path) = crate::tool_args::tool_write_path(&args_str)
                && should_collect_post_edit_diagnostics(
                    self.post_edit_diagnostics,
                    status,
                    &tool_call.fn_name,
                    Some(&path),
                    self.diagnostics_allowed(&path),
                )
                && let Some(text) = collect_post_edit_diagnostics(&self.tools, &path).await
            {
                self.log_event(
                    event_tx,
                    SessionEvent::UserMessage {
                        source: InputSource::RuntimeContext,
                        content: vec![ContentBlock::Text(format!(
                            "diagnostics for {path}:\n{text}"
                        ))],
                    },
                )
                .await;
            }

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
                self.file_changes_this_step = self
                    .file_changes_this_step
                    .saturating_add(run.file_changes.len());
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

    fn diagnostics_allowed(&self, path: &str) -> bool {
        let Some(harness) = self.harness.as_ref() else {
            return false;
        };
        let action = self
            .tools
            .iter()
            .find(|tool| tool.contain(LSP_DIAGNOSTICS_TOOL))
            .and_then(|tool| tool.permission_spec(LSP_DIAGNOSTICS_TOOL))
            .map_or(PermissionAction::Read, |spec| spec.action);
        let check = PermissionCheck {
            action,
            resource: path.to_string(),
            effect: EffectClass::Read,
            tool_name: LSP_DIAGNOSTICS_TOOL.to_string(),
        };
        matches!(harness.policy.decide(&check), Decision::Allow)
    }

    async fn ask_questions(
        &mut self,
        tool_call: &ToolCall,
        turn_id: &TurnId,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
        inbox: Option<&mut mpsc::UnboundedReceiver<AgentCommand>>,
    ) -> ToolRun {
        if self.question_cancels >= 2 {
            return ToolRun::from_output(ToolOutput::Fail(
                "question-ask is disabled after repeated cancellations; ask in plain text or proceed with a stated assumption"
                    .to_string(),
            ));
        }
        let args = normalized_tool_args(&tool_call.fn_arguments);
        let questions = match parse_questions(&args) {
            Ok(questions) => questions,
            Err(error) => {
                return ToolRun::from_output(ToolOutput::Fail(format!(
                    "invalid question arguments: {error}"
                )));
            }
        };
        let request_id = RequestId::new();
        let request = QuestionRequest {
            request_id: request_id.clone(),
            turn_id: turn_id.clone(),
            questions: questions.clone(),
        };
        let _ = event_tx.send(AgentEvent::QuestionAsked {
            request: request.clone(),
        });
        self.log_event(
            event_tx,
            SessionEvent::QuestionAsked {
                request_id: request_id.clone(),
                questions,
            },
        )
        .await;

        let answers = self
            .await_answer(&request_id, event_tx, cancel, inbox)
            .await;

        let _ = event_tx.send(AgentEvent::QuestionResolved {
            request_id: request_id.clone(),
        });
        self.log_event(
            event_tx,
            SessionEvent::QuestionAnswered {
                request_id,
                answers: answers.clone(),
            },
        )
        .await;

        match answers {
            Some(answers) => {
                self.question_cancels = 0;
                ToolRun::from_output(ToolOutput::Text(format_answers(&answers)))
            }
            None => {
                self.question_cancels += 1;
                ToolRun::from_output(ToolOutput::Fail(
                    "question cancelled by the user".to_string(),
                ))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn authorize(
        &mut self,
        check: &PermissionCheck,
        args: &str,
        item_id: &ItemId,
        turn_id: &TurnId,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
        inbox: Option<&mut mpsc::UnboundedReceiver<AgentCommand>>,
    ) -> Authorization {
        let outside = self
            .tools
            .iter()
            .find(|tool| tool.contain(&check.tool_name))
            .and_then(|tool| tool.permission_spec(&check.tool_name))
            .filter(|spec| {
                matches!(
                    spec.action,
                    meerai_core::PermissionAction::Read | meerai_core::PermissionAction::Edit
                )
            })
            .and(self.harness.as_ref())
            .and_then(|harness| {
                outside_workspace(harness.policy.workspace_root(), &check.resource)
            });
        let outside = outside.as_deref();

        let (mode, matched, mut decision) = {
            let Some(harness) = self.harness.as_ref() else {
                return Authorization::Allowed;
            };
            let mode = harness.policy.mode();
            let matched = harness.policy.matched_effect(check);
            (mode, matched, harness.policy.decide(check))
        };

        if auto_approve_safe_shell(check, &decision, matched, mode) {
            decision = Decision::Allow;
        }
        if let Some(path) = outside
            && !matches!(decision, Decision::Deny { .. })
            && !self.outside_granted(path)
        {
            decision = Decision::Ask;
        }

        let report = self
            .run_hook(
                HookEvent::PreToolUse,
                Some(turn_id),
                Some(&check.tool_name),
                Some(args),
                None,
                None,
                event_tx,
            )
            .await;
        decision = apply_hook_decision(decision, matched, mode, outside.is_some(), &report);
        if matches!(decision, Decision::Ask) {
            let report = self
                .run_hook(
                    HookEvent::PermissionRequest,
                    Some(turn_id),
                    Some(&check.tool_name),
                    Some(args),
                    None,
                    None,
                    event_tx,
                )
                .await;
            decision = apply_hook_decision(decision, matched, mode, outside.is_some(), &report);
        }

        match decision {
            Decision::Allow => Authorization::Allowed,
            Decision::Deny { reason } => Authorization::Refused(reason),
            Decision::Ask => {
                let has_channel = inbox.is_some();
                let request_id = RequestId::new();
                let mut preview = approval_preview_for(check);
                if let Some(path) = outside {
                    preview
                        .detail
                        .push_str(&format!("\noutside workspace: {}", path.display()));
                }
                let request = ApprovalRequest {
                    request_id: request_id.clone(),
                    turn_id: turn_id.clone(),
                    kind: ApprovalKind::ToolCall,
                    preview,
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
                        if let Some(path) = outside {
                            self.grant_outside_path(&check.tool_name, path);
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

    fn outside_granted(&self, path: &std::path::Path) -> bool {
        self.outside_grants
            .iter()
            .any(|root| path.starts_with(root))
    }

    fn grant_outside_path(&mut self, tool_name: &str, path: &std::path::Path) {
        let root = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };
        if let Some(tool) = self.tools.iter().find(|tool| tool.contain(tool_name)) {
            tool.grant_path_root(&root);
        }
        self.outside_grants.insert(root);
    }

    pub(super) async fn await_decision(
        &mut self,
        request_id: &RequestId,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
        inbox: Option<&mut mpsc::UnboundedReceiver<AgentCommand>>,
    ) -> ApprovalOutcome {
        self.decision_ctx()
            .await_decision(request_id, event_tx, cancel, inbox)
            .await
    }

    pub(super) async fn await_answer(
        &mut self,
        request_id: &RequestId,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
        inbox: Option<&mut mpsc::UnboundedReceiver<AgentCommand>>,
    ) -> Option<Vec<crate::events::QuestionAnswer>> {
        self.decision_ctx()
            .await_answer(request_id, event_tx, cancel, inbox)
            .await
    }

    pub(super) async fn handle_live_command(
        &mut self,
        command: AgentCommand,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) {
        self.decision_ctx()
            .handle_live_command(command, event_tx)
            .await;
    }

    fn decision_ctx(&mut self) -> DecisionCtx<'_> {
        DecisionCtx {
            harness: &mut self.harness,
            hooks: &mut self.hooks,
            pending_steers: &mut self.pending_steers,
            pending_compact: &mut self.pending_compact,
            chat_history: &mut self.chat_history,
            model: &mut self.model,
            shared_model: &self.shared_model,
            cumulative_usage: &mut self.cumulative_usage,
        }
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
}

struct DecisionCtx<'a> {
    harness: &'a mut Option<AgentHarness>,
    hooks: &'a mut Arc<HookEngine>,
    pending_steers: &'a mut VecDeque<String>,
    pending_compact: &'a mut bool,
    chat_history: &'a mut Vec<ChatMessage>,
    model: &'a mut String,
    shared_model: &'a Arc<std::sync::RwLock<String>>,
    cumulative_usage: &'a mut Usage,
}

impl DecisionCtx<'_> {
    async fn await_decision(
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
                | AgentCommand::ApplySkill { .. }
                | AgentCommand::Reload { .. }
                | AgentCommand::SetHooks(_) => {
                    self.handle_live_command(command, event_tx).await;
                }
                AgentCommand::AnswerQuestion { .. }
                | AgentCommand::Rewind { .. }
                | AgentCommand::SubmitPrompt(_)
                | AgentCommand::SetModel(_)
                | AgentCommand::Reset => {}
            }
        }
    }

    async fn await_answer(
        &mut self,
        request_id: &RequestId,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
        inbox: Option<&mut mpsc::UnboundedReceiver<AgentCommand>>,
    ) -> Option<Vec<crate::events::QuestionAnswer>> {
        let receiver = inbox?;

        loop {
            let command = tokio::select! {
                _ = cancel.cancelled() => return None,
                command = receiver.recv() => command,
            };
            let Some(command) = command else {
                cancel.cancel();
                return None;
            };

            match command {
                AgentCommand::AnswerQuestion {
                    request_id: id,
                    answers,
                } => {
                    if &id == request_id {
                        return answers;
                    }
                }
                AgentCommand::Cancel | AgentCommand::Quit => {
                    cancel.cancel();
                    return None;
                }
                AgentCommand::Approve { .. }
                | AgentCommand::Rewind { .. }
                | AgentCommand::SubmitPrompt(_)
                | AgentCommand::SetModel(_)
                | AgentCommand::Reset => {}
                AgentCommand::Steer(_)
                | AgentCommand::SetPermissionMode(_)
                | AgentCommand::Compact
                | AgentCommand::ApplyAgent { .. }
                | AgentCommand::ApplySkill { .. }
                | AgentCommand::Reload { .. }
                | AgentCommand::SetHooks(_) => {
                    self.handle_live_command(command, event_tx).await;
                }
            }
        }
    }

    async fn handle_live_command(
        &mut self,
        command: AgentCommand,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) {
        match command {
            AgentCommand::Steer(text) => self.pending_steers.push_back(text),
            AgentCommand::SetPermissionMode(mode) => {
                if let Some(harness) = self.harness.as_mut() {
                    harness.policy.set_mode(mode);
                    let _ = harness.session.set_approval_mode(mode.label()).await;
                }
            }
            AgentCommand::Compact => *self.pending_compact = true,
            AgentCommand::ApplyAgent {
                name,
                instructions,
                model,
            } => {
                self.apply_instructions(
                    format!("Agent instructions ({name}):\n\n{instructions}"),
                    model,
                    event_tx,
                )
                .await;
            }
            AgentCommand::ApplySkill { name, instructions } => {
                self.apply_instructions(
                    format!("Skill instructions ({name}):\n\n{instructions}"),
                    None,
                    event_tx,
                )
                .await;
            }
            AgentCommand::Reload { .. } => {}
            AgentCommand::SetHooks(entries) => {
                *self.hooks = Arc::new(HookEngine::new(entries));
            }
            AgentCommand::Approve { .. }
            | AgentCommand::AnswerQuestion { .. }
            | AgentCommand::Rewind { .. }
            | AgentCommand::SubmitPrompt(_)
            | AgentCommand::SetModel(_)
            | AgentCommand::Reset
            | AgentCommand::Cancel
            | AgentCommand::Quit => {}
        }
    }

    async fn apply_instructions(
        &mut self,
        text: String,
        model: Option<String>,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) {
        let text = crate::screen::screen_untrusted_text("definition_instructions", &text);
        log_harness_event(
            self.harness.as_ref(),
            event_tx,
            SessionEvent::UserMessage {
                source: InputSource::System,
                content: vec![ContentBlock::Text(text.clone())],
            },
        )
        .await;
        self.chat_history.push(ChatMessage::user(text));
        if let Some(model) = model {
            *self.model = model.clone();
            *self
                .shared_model
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = model;
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
        log_harness_event(
            self.harness.as_ref(),
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
}

async fn log_harness_event(
    harness: Option<&AgentHarness>,
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
    event: SessionEvent,
) {
    let Some(harness) = harness else {
        return;
    };
    if let Err(error) = harness.session.append(event).await {
        let _ = event_tx.send(AgentEvent::Error {
            code: ErrorCode::Internal,
            message: format!("failed to append session event: {error:#}"),
        });
    }
}

fn record_child_usage(
    usage: Usage,
    cumulative_usage: &mut Usage,
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
) {
    cumulative_usage.add(usage);
    let _ = event_tx.send(AgentEvent::UsageUpdated {
        usage,
        cumulative: *cumulative_usage,
        est_cost_usd: None,
    });
}

fn drain_child_usage(
    usage_rx: &mut mpsc::UnboundedReceiver<Usage>,
    cumulative_usage: &mut Usage,
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
) {
    while let Ok(usage) = usage_rx.try_recv() {
        record_child_usage(usage, cumulative_usage, event_tx);
    }
}

fn drain_child_harness_updates(
    grant_rx: &mut mpsc::UnboundedReceiver<meerai_policy::Grant>,
    budget_rx: &mut mpsc::UnboundedReceiver<SubagentBudgetUpdate>,
    harness: Option<&mut AgentHarness>,
) {
    let Some(harness) = harness else {
        return;
    };
    while let Ok(grant) = grant_rx.try_recv() {
        harness.policy.record_grant(grant);
    }
    while let Ok(update) = budget_rx.try_recv() {
        match update {
            SubagentBudgetUpdate::Unlimited => harness.config.max_session_tokens = None,
            SubagentBudgetUpdate::Increase(delta) => {
                if let Some(cap) = harness.config.max_session_tokens {
                    harness.config.max_session_tokens = Some(cap.saturating_add(delta));
                }
            }
        }
    }
}

async fn run_with_bridge<F, T>(
    ctx: &mut DecisionCtx<'_>,
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
    cancel: &CancellationToken,
    mut inbox: Option<&mut mpsc::UnboundedReceiver<AgentCommand>>,
    clock: TurnClock,
    fut: F,
) -> std::result::Result<T, ()>
where
    F: std::future::Future<Output = T>,
{
    if clock.max_secs != 0 && clock.remaining().is_none() {
        return Err(());
    }

    let (approval_tx, mut approval_rx) = mpsc::unbounded_channel::<ForwardedApproval>();
    let (usage_tx, mut usage_rx) = mpsc::unbounded_channel::<Usage>();
    let (grant_tx, mut grant_rx) = mpsc::unbounded_channel::<meerai_policy::Grant>();
    let (budget_tx, mut budget_rx) = mpsc::unbounded_channel::<SubagentBudgetUpdate>();
    let bridge_cancel = cancel.child_token();
    let bridge = SubagentBridge {
        event_tx: event_tx.clone(),
        approval_tx,
        usage_tx,
        grant_tx,
        budget_tx,
        cancel: bridge_cancel.clone(),
        parent_session: ctx.harness.as_ref().map(|h| h.session.clone()),
        parent_policy_mode: ctx
            .harness
            .as_ref()
            .map_or(ApprovalMode::Ask, |h| h.policy.mode()),
        parent_policy_rules: ctx
            .harness
            .as_ref()
            .map_or_else(Vec::new, |h| h.policy.rules().to_vec()),
        parent_policy_grants: ctx
            .harness
            .as_ref()
            .map_or_else(Vec::new, |h| h.policy.grants()),
        workspace_root: ctx.harness.as_ref().map_or_else(
            || PathBuf::from("."),
            |h| h.policy.workspace_root().to_path_buf(),
        ),
        parent_harness_config: ctx
            .harness
            .as_ref()
            .map_or_else(HarnessConfig::default, |h| h.config.clone()),
        parent_used_tokens: ctx
            .cumulative_usage
            .total_tokens
            .map(|tokens| tokens.max(0) as u64)
            .unwrap_or(0),
    };

    let mut scoped = std::pin::pin!(SUBAGENT_BRIDGE.scope(bridge, fut));
    let has_deadline = clock.max_secs != 0;
    let deadline = clock
        .remaining()
        .map(|left| tokio::time::Instant::now() + left)
        .unwrap_or_else(tokio::time::Instant::now);
    let sleep = tokio::time::sleep_until(deadline);
    tokio::pin!(sleep);

    let mut approval_open = true;
    let mut usage_open = true;

    let result = loop {
        tokio::select! {
            res = &mut scoped => break Ok(res),
            _ = &mut sleep, if has_deadline => {
                bridge_cancel.cancel();
                let shutdown = async {
                    loop {
                        tokio::select! {
                            _ = &mut scoped => break,
                            maybe_req = approval_rx.recv(), if approval_open => {
                                match maybe_req {
                                    Some(req) => {
                                        let _ = req.reply.send((ApprovalDecision::Cancel, None));
                                    }
                                    None => approval_open = false,
                                }
                            }
                        }
                    }
                };
                let _ = tokio::time::timeout(Duration::from_secs(2), shutdown).await;
                break Err(());
            }
            maybe_req = approval_rx.recv(), if approval_open => {
                match maybe_req {
                    Some(req) => {
                        let _ = event_tx.send(AgentEvent::ApprovalRequested {
                            request: req.request.clone(),
                        });
                        log_harness_event(
                            ctx.harness.as_ref(),
                            event_tx,
                            SessionEvent::ApprovalRequested {
                                request_id: req.request.request_id.clone(),
                                kind: req.request.kind,
                                preview: req.request.preview.clone(),
                            },
                        )
                        .await;
                        let outcome = ctx
                            .await_decision(
                                &req.request.request_id,
                                event_tx,
                                cancel,
                                inbox.as_deref_mut(),
                            )
                            .await;
                        let _ = req.reply.send((outcome.decision, outcome.scope));
                        drain_child_usage(&mut usage_rx, ctx.cumulative_usage, event_tx);
                    }
                    None => approval_open = false,
                }
            }
            maybe_usage = usage_rx.recv(), if usage_open => {
                match maybe_usage {
                    Some(usage) => record_child_usage(usage, ctx.cumulative_usage, event_tx),
                    None => usage_open = false,
                }
            }
        }
    };

    drain_child_usage(&mut usage_rx, ctx.cumulative_usage, event_tx);
    drain_child_harness_updates(&mut grant_rx, &mut budget_rx, ctx.harness.as_mut());
    result
}

async fn run_tool_from(
    tools: &[Box<dyn Toolset>],
    timeout_ms: Option<u64>,
    max_retries: usize,
    name: &str,
    args: &str,
    cancel: &CancellationToken,
) -> Result<ToolRun> {
    let tool = tools
        .iter()
        .find(|tool| tool.contain(name))
        .ok_or_else(|| anyhow!("Tool not found: {name}"))?;

    let max_attempts = max_retries.max(1);
    let effective_timeout = if name.starts_with("subagent-") {
        None
    } else {
        timeout_ms
    };

    for attempt in 1..=max_attempts {
        if cancel.is_cancelled() {
            return Err(anyhow!("Tool execution cancelled"));
        }

        let invocation = tool.invoke_structured(name, args);
        tokio::pin!(invocation);
        let limit = effective_timeout.map(Duration::from_millis);
        let invoked = async {
            match limit {
                Some(limit) => tokio::time::timeout(limit, &mut invocation).await,
                None => Ok::<_, tokio::time::error::Elapsed>((&mut invocation).await),
            }
        };
        let result = tokio::select! {
            _ = cancel.cancelled() => return Err(anyhow!("Tool execution cancelled")),
            result = invoked => result,
        };
        let result = match result {
            Ok(result) => result,
            Err(_) => {
                let ms = effective_timeout.unwrap_or(0);
                return Ok(ToolRun::from_output(ToolOutput::Fail(format!(
                    "tool timed out after {ms}ms"
                ))));
            }
        };

        match result {
            Ok(run) => return Ok(run),
            Err(err @ meerai_core::ToolError::WrongArguments(_))
            | Err(err @ meerai_core::ToolError::InvalidFunctionName(_)) => {
                return Err(anyhow!("Failed to invoke tool '{name}': {err}"));
            }
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

fn outside_workspace(workspace: &std::path::Path, resource: &str) -> Option<std::path::PathBuf> {
    use std::path::{Component, Path, PathBuf};

    let raw = resource.trim();
    if raw.is_empty() {
        return None;
    }
    let path = Path::new(raw);
    let has_parent = path
        .components()
        .any(|component| matches!(component, Component::ParentDir));
    if !path.is_absolute() && !has_parent {
        return None;
    }
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    if normalized.starts_with(workspace) {
        None
    } else {
        Some(normalized)
    }
}

#[cfg(test)]
mod outside_tests {
    use std::path::Path;

    use super::outside_workspace;

    #[test]
    fn detects_paths_outside_the_workspace() {
        let workspace = Path::new("/tmp/work");
        assert!(outside_workspace(workspace, "src/main.rs").is_none());
        assert!(outside_workspace(workspace, "./src/main.rs").is_none());
        assert_eq!(
            outside_workspace(workspace, "../secret.txt"),
            Some(Path::new("/tmp/secret.txt").to_path_buf())
        );
        assert_eq!(
            outside_workspace(workspace, "/etc/passwd"),
            Some(Path::new("/etc/passwd").to_path_buf())
        );
    }
}

fn auto_approve_safe_shell(
    check: &PermissionCheck,
    decision: &Decision,
    rule_effect: Option<meerai_policy::Effect>,
    mode: meerai_policy::ApprovalMode,
) -> bool {
    matches!(decision, Decision::Ask)
        && rule_effect.is_none()
        && mode != meerai_policy::ApprovalMode::Plan
        && check.action == meerai_core::PermissionAction::Shell
        && crate::shell_safety::command_is_safe(&check.resource)
}

fn apply_hook_decision(
    decision: Decision,
    matched: Option<meerai_policy::Effect>,
    mode: ApprovalMode,
    outside_forced: bool,
    report: &HookReport,
) -> Decision {
    if matches!(decision, Decision::Deny { .. }) {
        return decision;
    }
    if mode == ApprovalMode::Plan {
        return match report.decision {
            crate::hooks::HookDecision::Deny | crate::hooks::HookDecision::Block => {
                Decision::Deny {
                    reason: report.reason().unwrap_or("blocked by hook").to_string(),
                }
            }
            _ => decision,
        };
    }
    match report.decision {
        crate::hooks::HookDecision::Deny | crate::hooks::HookDecision::Block => Decision::Deny {
            reason: report.reason().unwrap_or("blocked by hook").to_string(),
        },
        crate::hooks::HookDecision::Ask => match decision {
            Decision::Allow => Decision::Ask,
            other => other,
        },
        crate::hooks::HookDecision::Allow => match decision {
            Decision::Ask if matched.is_none() && !outside_forced => Decision::Allow,
            other => other,
        },
        crate::hooks::HookDecision::Pass => decision,
    }
}

#[cfg(test)]
mod hook_decision_tests {
    use super::apply_hook_decision;
    use crate::hooks::{HookDecision, HookReport};
    use meerai_policy::{ApprovalMode, Decision, Effect};

    fn report(decision: HookDecision) -> HookReport {
        HookReport {
            decision,
            firings: Vec::new(),
        }
    }

    #[test]
    fn hook_allow_does_not_override_outside_workspace() {
        let decision = apply_hook_decision(
            Decision::Ask,
            None,
            ApprovalMode::Ask,
            true,
            &report(HookDecision::Allow),
        );
        assert_eq!(decision, Decision::Ask);
    }

    #[test]
    fn plan_mode_hook_denies_a_read() {
        let decision = apply_hook_decision(
            Decision::Allow,
            None,
            ApprovalMode::Plan,
            false,
            &report(HookDecision::Deny),
        );
        assert!(matches!(decision, Decision::Deny { .. }));
    }

    #[test]
    fn hook_allow_overrides_only_tier_default_ask() {
        let decision = apply_hook_decision(
            Decision::Ask,
            None,
            ApprovalMode::Ask,
            false,
            &report(HookDecision::Allow),
        );
        assert_eq!(decision, Decision::Allow);
    }

    #[test]
    fn explicit_ask_rule_is_not_loosened() {
        let decision = apply_hook_decision(
            Decision::Ask,
            Some(Effect::Ask),
            ApprovalMode::Ask,
            false,
            &report(HookDecision::Allow),
        );
        assert_eq!(decision, Decision::Ask);
    }
}

#[cfg(test)]
mod auto_approve_tests {
    use super::auto_approve_safe_shell;
    use meerai_core::{EffectClass, PermissionAction};
    use meerai_policy::{ApprovalMode, Decision, Effect, PermissionCheck};

    fn check(command: &str) -> PermissionCheck {
        PermissionCheck {
            action: PermissionAction::Shell,
            resource: command.to_string(),
            effect: EffectClass::Execute,
            tool_name: "code-shell".to_string(),
        }
    }

    #[test]
    fn auto_approves_safe_shell_only_without_an_explicit_rule() {
        let safe = check("git status && git diff");
        assert!(auto_approve_safe_shell(
            &safe,
            &Decision::Ask,
            None,
            ApprovalMode::Ask
        ));
        assert!(!auto_approve_safe_shell(
            &safe,
            &Decision::Ask,
            Some(Effect::Ask),
            ApprovalMode::Ask
        ));
        assert!(!auto_approve_safe_shell(
            &safe,
            &Decision::Ask,
            None,
            ApprovalMode::Plan
        ));
        assert!(!auto_approve_safe_shell(
            &check("cargo test"),
            &Decision::Ask,
            None,
            ApprovalMode::Ask
        ));
        assert!(!auto_approve_safe_shell(
            &safe,
            &Decision::Deny { reason: "x".into() },
            None,
            ApprovalMode::Ask
        ));
    }

    #[test]
    fn sac_inline_permission_check_populates_resource_preview() {
        let check = super::permission_check(
            "sac-run_script",
            r#"{"language":"python","script":"print(1)\nprint(2)"}"#,
            meerai_core::EffectClass::Execute,
            Some(meerai_core::ToolPermissionSpec {
                action: meerai_core::PermissionAction::Shell,
                resource_arg: Some("path"),
            }),
        );
        assert_eq!(check.resource, "python: print(1)");
    }
}

fn normalized_tool_args(value: &serde_json::Value) -> String {
    meerai_core::lenient::normalize_tool_arguments(value).to_string()
}

fn coerce_tool_args(tools: &[Box<dyn Toolset>], fn_name: &str, args: &str) -> String {
    let Some(tool) = tools.iter().find(|tool| tool.contain(fn_name)) else {
        return args.to_string();
    };
    let Some(definition) = tool
        .definition()
        .into_iter()
        .find(|definition| definition.name == fn_name)
    else {
        return args.to_string();
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(args) else {
        return args.to_string();
    };
    if meerai_core::lenient::coerce_to_schema(&mut value, &definition.parameters) {
        value.to_string()
    } else {
        args.to_string()
    }
}

fn argument_repair_hint(
    tools: &[Box<dyn Toolset>],
    fn_name: &str,
    observation: &ToolObservation,
) -> Option<String> {
    if observation.status != ObservationStatus::Error {
        return None;
    }
    let message = if observation.stderr.is_empty() {
        observation.stdout.as_str()
    } else {
        observation.stderr.as_str()
    };
    let looks_like_arguments = message.contains("arguments for tool failed to parse")
        || message.contains("invalid type")
        || message.contains("missing field")
        || message.contains("unknown field");
    if !looks_like_arguments {
        return None;
    }
    let schema = tools
        .iter()
        .find(|tool| tool.contain(fn_name))?
        .definition()
        .into_iter()
        .find(|definition| definition.name == fn_name)?
        .parameters
        .to_string();
    let schema = &schema[..floor_char_boundary(&schema, 4_096)];
    let error = &message[..floor_char_boundary(message, 2_048)];
    Some(format!(
        "Tool '{fn_name}' rejected its arguments. Retry the same tool with corrected JSON. Use this parameter schema (truncated to 4 KiB):\n{schema}\nError: {error}"
    ))
}

#[cfg(test)]
mod cancel_tests {
    use std::time::Duration;

    use meerai_core::{ToolDefinition, ToolError, ToolOutput, Toolset, async_trait};
    use tokio_util::sync::CancellationToken;

    use super::run_tool_from;

    struct SlowTool;

    #[async_trait]
    impl Toolset for SlowTool {
        fn name(&self) -> String {
            "slow".to_string()
        }

        fn definition(&self) -> Vec<ToolDefinition> {
            Vec::new()
        }

        fn contain(&self, fn_name: &str) -> bool {
            fn_name == "slow-run"
        }

        async fn invoke(&self, _fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(ToolOutput::Text("done".to_string()))
        }
    }

    #[tokio::test]
    async fn cancel_stops_an_in_flight_tool() {
        let tools: Vec<Box<dyn Toolset>> = vec![Box::new(SlowTool)];
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let task = tokio::spawn(async move {
            run_tool_from(&tools, Some(60_000), 1, "slow-run", "{}", &cancel).await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        token.cancel();
        let result = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("tool did not stop after cancel")
            .expect("task panicked");
        let error = result.expect_err("cancelled tool should fail");
        assert!(error.to_string().contains("cancelled"), "{error:#}");
    }
}
