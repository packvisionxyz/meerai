use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use meerai_agents::events::{
    ApprovalDecision, ApprovalKind, ApprovalPreview, ContentBlock, GrantScope, RequestId,
};
use meerai_agents::{
    AbortCause, AgentCommand, AgentEvent, ApprovalRequest, CallId, ErrorCode, ItemDelta, ItemId,
    ItemStatus, PlanStep, PlanStepStatus, SessionItem, TurnEndReason, TurnId,
};
use meerai_core::{EffectClass, ToolObservation};
use meerai_session::{InputSource, Session, SessionEvent, StreamDigest, TurnTrigger, Usage};
use tokio::sync::mpsc;

use crate::events::{
    AcpEvent, AcpPermissionKind, AcpPermissionOption, AcpPermissionRequest, AcpPlanStep,
    AcpToolCall, AcpToolCallUpdate,
};
use crate::{AcpPreset, AcpSession, AcpStart, connect_with};

const EXTERNAL_AGENT_DETAIL: &str = "ACP permission request from the external agent. meerai policy, path guards, redaction, and sandbox do not apply.";

pub struct AcpWorkerConfig {
    pub session: Session,
    pub preset: AcpPreset,
    pub cwd: PathBuf,
    pub start: AcpStart,
    pub resumed: Arc<AtomicBool>,
}

pub fn spawn_acp_worker(
    config: AcpWorkerConfig,
) -> (
    mpsc::UnboundedSender<AgentCommand>,
    mpsc::UnboundedReceiver<AgentEvent>,
) {
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    tokio::spawn(run(config, command_rx, event_tx));
    (command_tx, event_rx)
}

async fn run(
    config: AcpWorkerConfig,
    mut commands: mpsc::UnboundedReceiver<AgentCommand>,
    events: mpsc::UnboundedSender<AgentEvent>,
) {
    let AcpWorkerConfig {
        session,
        preset,
        cwd,
        start,
        resumed,
    } = config;
    resumed.store(false, Ordering::Relaxed);
    let mut state = AcpTurnState::new(preset.id);
    let mut acp = match connect_with(preset, cwd, start).await {
        Ok(acp) => acp,
        Err(error) => {
            let _ = events.send(AgentEvent::Error {
                code: ErrorCode::Provider,
                message: error.to_string(),
            });
            return;
        }
    };
    resumed.store(acp.handshake().resumed, Ordering::Relaxed);
    let _ = session
        .set_remote_session(state.harness(), acp.session_id.clone())
        .await;
    let mut incoming = detach_events(&mut acp);
    while let Some(event) = incoming.recv().await {
        if matches!(event, AcpEvent::Connected { .. }) {
            break;
        }
    }
    let mut queued: VecDeque<String> = VecDeque::new();

    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else {
                    break;
                };
                match command {
                    AgentCommand::SubmitPrompt(prompt) => {
                        if state.is_active() {
                            queued.push_back(prompt);
                        } else {
                            begin(&mut state, &acp, &session, &events, prompt).await;
                        }
                    }
                    AgentCommand::Cancel => {
                        if let Err(error) = acp.cancel().await {
                            let _ = events.send(AgentEvent::Error {
                                code: ErrorCode::Provider,
                                message: error.to_string(),
                            });
                        }
                    }
                    AgentCommand::Approve {
                        request_id,
                        decision,
                        scope,
                    } => {
                        if let Some((emission, reply)) =
                            state.resolve_permission(&request_id, &decision, scope)
                        {
                            apply(&session, &events, emission).await;
                            match reply {
                                PermissionReply::Selected {
                                    acp_request_id,
                                    option_id,
                                } => {
                                    let _ = acp.reply_permission(acp_request_id, option_id).await;
                                }
                                PermissionReply::Cancelled { acp_request_id } => {
                                    let _ = acp.cancel_permission(acp_request_id).await;
                                }
                            }
                        }
                    }
                    AgentCommand::Quit => {
                        let _ = acp.shutdown().await;
                        break;
                    }
                    AgentCommand::Steer(_)
                    | AgentCommand::SetModel(_)
                    | AgentCommand::SetPermissionMode(_)
                    | AgentCommand::Compact
                    | AgentCommand::ApplyAgent { .. }
                    | AgentCommand::ApplySkill { .. }
                    | AgentCommand::Reload { .. }
                    | AgentCommand::SetHooks(_)
                    | AgentCommand::Rewind { .. }
                    | AgentCommand::AnswerQuestion { .. }
                    | AgentCommand::Reset => {}
                }
            }
            event = incoming.recv() => {
                let Some(event) = event else {
                    break;
                };
                match event {
                    AcpEvent::TextDelta(chunk) => {
                        apply(&session, &events, state.text_delta(&chunk)).await;
                    }
                    AcpEvent::ThoughtDelta(chunk) => {
                        apply(&session, &events, state.thought_delta(&chunk)).await;
                    }
                    AcpEvent::ToolCall(call) => {
                        apply(&session, &events, state.tool_call(&call)).await;
                    }
                    AcpEvent::ToolCallUpdate(update) => {
                        apply(&session, &events, state.tool_call_update(&update)).await;
                    }
                    AcpEvent::Plan(steps) => {
                        apply(&session, &events, state.plan(&steps)).await;
                    }
                    AcpEvent::PermissionRequest(request) => match state.permission(&request) {
                        Some(emission) => apply(&session, &events, emission).await,
                        None => {
                            let _ = acp.cancel_permission(request.request_id).await;
                        }
                    },
                    AcpEvent::Finished { stop_reason } => {
                        apply(&session, &events, state.finished(&stop_reason)).await;
                        if let Some(prompt) = queued.pop_front() {
                            begin(&mut state, &acp, &session, &events, prompt).await;
                        }
                    }
                    AcpEvent::Error(message) => {
                        apply(&session, &events, state.error(&message)).await;
                    }
                    AcpEvent::Connected { .. } | AcpEvent::Usage { .. } => {}
                }
            }
        }
    }
}

fn detach_events(acp: &mut AcpSession) -> mpsc::UnboundedReceiver<AcpEvent> {
    let (_placeholder_tx, placeholder) = mpsc::unbounded_channel();
    std::mem::replace(&mut acp.events, placeholder)
}

async fn begin(
    state: &mut AcpTurnState,
    acp: &AcpSession,
    session: &Session,
    events: &mpsc::UnboundedSender<AgentEvent>,
    prompt: String,
) {
    let emission = state.start_turn(&prompt);
    apply(session, events, emission).await;
    if let Err(error) = acp.prompt(prompt).await {
        let message = error.to_string();
        apply(session, events, state.error(&message)).await;
    }
}

async fn apply(
    session: &Session,
    events: &mpsc::UnboundedSender<AgentEvent>,
    emission: TurnEmission,
) {
    for event in emission.log {
        let _ = session.append(event).await;
    }
    for event in emission.events {
        let _ = events.send(event);
    }
}

struct AcpTurnState {
    model: String,
    turn: Option<ActiveTurn>,
    permissions: HashMap<RequestId, PendingPermission>,
}

struct ActiveTurn {
    turn_id: TurnId,
    text: String,
    text_item: Option<ItemId>,
    reasoning: String,
    reasoning_item: Option<ItemId>,
    calls: HashMap<String, ToolItem>,
}

#[derive(Clone)]
struct ToolItem {
    item_id: ItemId,
    name: String,
    args: String,
}

struct PendingPermission {
    acp_request_id: u64,
    options: Vec<AcpPermissionOption>,
}

#[derive(Default)]
struct TurnEmission {
    log: Vec<SessionEvent>,
    events: Vec<AgentEvent>,
}

enum PermissionReply {
    Selected {
        acp_request_id: u64,
        option_id: String,
    },
    Cancelled {
        acp_request_id: u64,
    },
}

impl AcpTurnState {
    fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            turn: None,
            permissions: HashMap::new(),
        }
    }

    fn harness(&self) -> String {
        format!("acp:{}", self.model)
    }

    fn is_active(&self) -> bool {
        self.turn.is_some()
    }

    fn start_turn(&mut self, prompt: &str) -> TurnEmission {
        let turn_id = TurnId::new();
        self.turn = Some(ActiveTurn {
            turn_id: turn_id.clone(),
            text: String::new(),
            text_item: None,
            reasoning: String::new(),
            reasoning_item: None,
            calls: HashMap::new(),
        });
        let mut emission = TurnEmission::default();
        emission.log.push(SessionEvent::TurnStarted {
            turn_id: turn_id.clone(),
            trigger: TurnTrigger::UserPrompt,
        });
        emission.log.push(SessionEvent::UserMessage {
            source: InputSource::User,
            content: vec![ContentBlock::Text(prompt.to_string())],
        });
        emission.events.push(AgentEvent::TurnStarted {
            turn_id,
            model: self.harness(),
        });
        emission
    }

    fn text_delta(&mut self, chunk: &str) -> TurnEmission {
        let mut emission = TurnEmission::default();
        let Some(turn) = self.turn.as_mut() else {
            return emission;
        };
        let item_id = match turn.text_item.clone() {
            Some(id) => id,
            None => {
                let id = ItemId::new();
                turn.text_item = Some(id.clone());
                emission.events.push(AgentEvent::ItemStarted {
                    item: SessionItem::AgentMessage {
                        id: id.clone(),
                        text: String::new(),
                    },
                });
                id
            }
        };
        turn.text.push_str(chunk);
        emission.events.push(AgentEvent::ItemDelta {
            item_id,
            delta: ItemDelta::AgentText(chunk.to_string()),
        });
        emission
    }

    fn thought_delta(&mut self, chunk: &str) -> TurnEmission {
        let mut emission = TurnEmission::default();
        let Some(turn) = self.turn.as_mut() else {
            return emission;
        };
        let item_id = match turn.reasoning_item.clone() {
            Some(id) => id,
            None => {
                let id = ItemId::new();
                turn.reasoning_item = Some(id.clone());
                emission.events.push(AgentEvent::ItemStarted {
                    item: SessionItem::Reasoning {
                        id: id.clone(),
                        summary: String::new(),
                    },
                });
                id
            }
        };
        turn.reasoning.push_str(chunk);
        emission.events.push(AgentEvent::ItemDelta {
            item_id,
            delta: ItemDelta::ReasoningSummary(chunk.to_string()),
        });
        emission
    }

    fn tool_call(&mut self, call: &AcpToolCall) -> TurnEmission {
        let mut emission = TurnEmission::default();
        let Some(turn) = self.turn.as_mut() else {
            return emission;
        };
        let item_id = ItemId::new();
        let args = call.raw_input.clone().unwrap_or_default();
        turn.calls.insert(
            call.id.clone(),
            ToolItem {
                item_id: item_id.clone(),
                name: call.title.clone(),
                args: args.clone(),
            },
        );
        let call_id = CallId::from_raw(call.id.clone());
        emission.log.push(SessionEvent::ToolCall {
            call_id: call_id.clone(),
            name: call.title.clone(),
            args: args.clone(),
        });
        emission.events.push(AgentEvent::ItemStarted {
            item: SessionItem::ToolCall {
                id: item_id,
                call_id,
                name: call.title.clone(),
                args,
                effect: EffectClass::External,
                status: item_status(&call.status),
                output: None,
            },
        });
        emission
    }

    fn tool_call_update(&mut self, update: &AcpToolCallUpdate) -> TurnEmission {
        let mut emission = TurnEmission::default();
        let Some(turn) = self.turn.as_mut() else {
            return emission;
        };
        let record = match turn.calls.get(&update.id).cloned() {
            Some(record) => record,
            None => {
                let record = ToolItem {
                    item_id: ItemId::new(),
                    name: String::new(),
                    args: String::new(),
                };
                emission.events.push(AgentEvent::ItemStarted {
                    item: SessionItem::ToolCall {
                        id: record.item_id.clone(),
                        call_id: CallId::from_raw(update.id.clone()),
                        name: record.name.clone(),
                        args: record.args.clone(),
                        effect: EffectClass::External,
                        status: item_status(&update.status),
                        output: None,
                    },
                });
                turn.calls.insert(update.id.clone(), record.clone());
                record
            }
        };
        let call_id = CallId::from_raw(update.id.clone());
        emission.log.push(SessionEvent::ToolResult {
            call_id: call_id.clone(),
            output: observation(update),
        });
        emission.events.push(AgentEvent::ItemCompleted {
            item: SessionItem::ToolCall {
                id: record.item_id.clone(),
                call_id,
                name: record.name.clone(),
                args: record.args.clone(),
                effect: EffectClass::External,
                status: item_status(&update.status),
                output: update.output.clone(),
            },
        });
        emission
    }

    fn plan(&mut self, steps: &[AcpPlanStep]) -> TurnEmission {
        let mut emission = TurnEmission::default();
        let item = SessionItem::Plan {
            id: ItemId::new(),
            steps: steps.iter().map(plan_step).collect(),
        };
        emission
            .events
            .push(AgentEvent::ItemStarted { item: item.clone() });
        emission.events.push(AgentEvent::ItemCompleted { item });
        emission
    }

    fn permission(&mut self, request: &AcpPermissionRequest) -> Option<TurnEmission> {
        let turn_id = self.turn.as_ref()?.turn_id.clone();
        let request_id = RequestId::new();
        let preview = ApprovalPreview {
            title: format!("External agent permission: {}", request.tool_call.title),
            detail: EXTERNAL_AGENT_DETAIL.to_string(),
        };
        self.permissions.insert(
            request_id.clone(),
            PendingPermission {
                acp_request_id: request.request_id,
                options: request.options.clone(),
            },
        );
        let mut emission = TurnEmission::default();
        emission.log.push(SessionEvent::ApprovalRequested {
            request_id: request_id.clone(),
            kind: ApprovalKind::ExternalAgent,
            preview: preview.clone(),
        });
        emission.events.push(AgentEvent::ApprovalRequested {
            request: ApprovalRequest {
                request_id,
                turn_id,
                kind: ApprovalKind::ExternalAgent,
                preview,
                effect: Some(EffectClass::External),
            },
        });
        Some(emission)
    }

    fn resolve_permission(
        &mut self,
        request_id: &RequestId,
        decision: &ApprovalDecision,
        scope: Option<GrantScope>,
    ) -> Option<(TurnEmission, PermissionReply)> {
        let pending = self.permissions.remove(request_id)?;
        let reply = match select_option(&pending.options, decision) {
            Some(option_id) => PermissionReply::Selected {
                acp_request_id: pending.acp_request_id,
                option_id,
            },
            None => PermissionReply::Cancelled {
                acp_request_id: pending.acp_request_id,
            },
        };
        let mut emission = TurnEmission::default();
        emission.log.push(SessionEvent::ApprovalDecided {
            request_id: request_id.clone(),
            decision: decision.clone(),
            scope,
        });
        emission.events.push(AgentEvent::ApprovalResolved {
            request_id: request_id.clone(),
        });
        Some((emission, reply))
    }

    fn finished(&mut self, stop_reason: &str) -> TurnEmission {
        let mut emission = TurnEmission::default();
        let Some(turn) = self.turn.take() else {
            return emission;
        };
        if let Some(id) = turn.text_item {
            emission.events.push(AgentEvent::ItemCompleted {
                item: SessionItem::AgentMessage {
                    id,
                    text: turn.text.clone(),
                },
            });
        }
        if let Some(id) = turn.reasoning_item {
            emission.events.push(AgentEvent::ItemCompleted {
                item: SessionItem::Reasoning {
                    id,
                    summary: turn.reasoning.clone(),
                },
            });
        }
        if !turn.text.is_empty() {
            emission.log.push(SessionEvent::AssistantMessage {
                provider: self.harness(),
                model: self.model.clone(),
                content: vec![ContentBlock::Text(turn.text)],
                tool_calls: Vec::new(),
                usage: Usage::default(),
                stream_digest: StreamDigest::default(),
            });
        }
        let reason = turn_end_reason(stop_reason);
        emission.log.push(SessionEvent::TurnEnded {
            turn_id: turn.turn_id.clone(),
            reason: reason.clone(),
        });
        emission.events.push(AgentEvent::TurnCompleted {
            turn_id: turn.turn_id,
            reason,
        });
        emission
    }

    fn error(&mut self, message: &str) -> TurnEmission {
        let mut emission = TurnEmission::default();
        emission.events.push(AgentEvent::Error {
            code: ErrorCode::Provider,
            message: message.to_string(),
        });
        let Some(turn) = self.turn.take() else {
            return emission;
        };
        let error = message.to_string();
        emission.log.push(SessionEvent::Error {
            code: ErrorCode::Provider,
            message: error.clone(),
            retryable: false,
        });
        emission.log.push(SessionEvent::TurnEnded {
            turn_id: turn.turn_id.clone(),
            reason: TurnEndReason::Failed {
                error: error.clone(),
            },
        });
        emission.events.push(AgentEvent::TurnCompleted {
            turn_id: turn.turn_id,
            reason: TurnEndReason::Failed { error },
        });
        emission
    }
}

fn select_option(options: &[AcpPermissionOption], decision: &ApprovalDecision) -> Option<String> {
    let (primary, fallback) = match decision {
        ApprovalDecision::Accept => (AcpPermissionKind::AllowOnce, AcpPermissionKind::AllowAlways),
        ApprovalDecision::AcceptForSession => {
            (AcpPermissionKind::AllowAlways, AcpPermissionKind::AllowOnce)
        }
        ApprovalDecision::Decline | ApprovalDecision::DeclineWithReason(_) => (
            AcpPermissionKind::RejectOnce,
            AcpPermissionKind::RejectAlways,
        ),
        ApprovalDecision::Cancel => return None,
    };
    find_option(options, primary).or_else(|| find_option(options, fallback))
}

fn find_option(options: &[AcpPermissionOption], kind: AcpPermissionKind) -> Option<String> {
    options
        .iter()
        .find(|option| option.kind == kind)
        .map(|option| option.option_id.clone())
}

fn item_status(status: &str) -> ItemStatus {
    match status {
        "pending" => ItemStatus::Proposed,
        "in_progress" => ItemStatus::InProgress,
        "completed" => ItemStatus::Succeeded,
        "failed" => ItemStatus::Failed,
        _ => ItemStatus::Pending,
    }
}

fn observation(update: &AcpToolCallUpdate) -> ToolObservation {
    let output = update.output.clone().unwrap_or_default();
    match update.status.as_str() {
        "failed" => ToolObservation::error(output),
        _ => ToolObservation::success(output),
    }
}

fn plan_step(step: &AcpPlanStep) -> PlanStep {
    PlanStep {
        text: step.content.clone(),
        status: plan_status(&step.status),
    }
}

fn plan_status(status: &str) -> PlanStepStatus {
    match status {
        "in_progress" => PlanStepStatus::InProgress,
        "completed" => PlanStepStatus::Completed,
        _ => PlanStepStatus::Pending,
    }
}

fn turn_end_reason(stop_reason: &str) -> TurnEndReason {
    match stop_reason {
        "max_tokens" => TurnEndReason::MaxTokens,
        "cancelled" => TurnEndReason::Aborted {
            cause: AbortCause::User,
        },
        "refusal" => TurnEndReason::Failed {
            error: "agent refused".to_string(),
        },
        _ => TurnEndReason::Completed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_call() -> AcpToolCall {
        AcpToolCall {
            id: "call-1".to_string(),
            title: "Edit src/lib.rs".to_string(),
            kind: "edit".to_string(),
            status: "pending".to_string(),
            raw_input: Some("{\"path\":\"src/lib.rs\"}".to_string()),
        }
    }

    fn permission_options() -> Vec<AcpPermissionOption> {
        vec![
            AcpPermissionOption {
                option_id: "allow-once".to_string(),
                name: "Allow once".to_string(),
                kind: AcpPermissionKind::AllowOnce,
            },
            AcpPermissionOption {
                option_id: "allow-always".to_string(),
                name: "Always allow".to_string(),
                kind: AcpPermissionKind::AllowAlways,
            },
            AcpPermissionOption {
                option_id: "reject-once".to_string(),
                name: "Reject".to_string(),
                kind: AcpPermissionKind::RejectOnce,
            },
            AcpPermissionOption {
                option_id: "reject-always".to_string(),
                name: "Always reject".to_string(),
                kind: AcpPermissionKind::RejectAlways,
            },
        ]
    }

    fn permission_request() -> AcpPermissionRequest {
        AcpPermissionRequest {
            request_id: 3,
            tool_call: tool_call(),
            options: permission_options(),
        }
    }

    fn text_of(event: &AgentEvent) -> Option<&str> {
        match event {
            AgentEvent::ItemDelta {
                delta: ItemDelta::AgentText(text),
                ..
            } => Some(text),
            _ => None,
        }
    }

    #[test]
    fn start_turn_logs_prompt_and_emits_turn_started() {
        let mut state = AcpTurnState::new("agy");
        let emission = state.start_turn("hello");
        assert!(state.is_active());
        assert!(matches!(
            emission.log[0],
            SessionEvent::TurnStarted {
                trigger: TurnTrigger::UserPrompt,
                ..
            }
        ));
        assert_eq!(
            emission.log[1],
            SessionEvent::UserMessage {
                source: InputSource::User,
                content: vec![ContentBlock::Text("hello".to_string())],
            }
        );
        match &emission.events[0] {
            AgentEvent::TurnStarted { model, .. } => assert_eq!(model, "acp:agy"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn text_deltas_open_one_item_and_finish_with_assistant_message() {
        let mut state = AcpTurnState::new("agy");
        state.start_turn("hi");
        let first = state.text_delta("OK");
        assert_eq!(first.events.len(), 2);
        let item_id = match &first.events[0] {
            AgentEvent::ItemStarted {
                item: SessionItem::AgentMessage { id, text },
            } => {
                assert!(text.is_empty());
                id.clone()
            }
            other => panic!("unexpected event: {other:?}"),
        };
        match &first.events[1] {
            AgentEvent::ItemDelta {
                item_id: delta_id,
                delta,
            } => {
                assert_eq!(delta_id, &item_id);
                assert_eq!(delta, &ItemDelta::AgentText("OK".to_string()));
            }
            other => panic!("unexpected event: {other:?}"),
        }
        let second = state.text_delta("!");
        assert_eq!(second.events.len(), 1);
        assert_eq!(text_of(&second.events[0]), Some("!"));

        let done = state.finished("end_turn");
        assert!(!state.is_active());
        match &done.events[0] {
            AgentEvent::ItemCompleted {
                item: SessionItem::AgentMessage { id, text },
            } => {
                assert_eq!(id, &item_id);
                assert_eq!(text, "OK!");
            }
            other => panic!("unexpected event: {other:?}"),
        }
        let assistant = done
            .log
            .iter()
            .find_map(|event| match event {
                SessionEvent::AssistantMessage {
                    provider,
                    model,
                    content,
                    ..
                } => Some((provider.clone(), model.clone(), content.clone())),
                _ => None,
            })
            .expect("assistant message logged");
        assert_eq!(assistant.0, "acp:agy");
        assert_eq!(assistant.1, "agy");
        assert_eq!(assistant.2, vec![ContentBlock::Text("OK!".to_string())]);
        assert!(
            done.log
                .iter()
                .any(|event| matches!(event, SessionEvent::TurnEnded { .. }))
        );
        match done.events.last() {
            Some(AgentEvent::TurnCompleted { reason, .. }) => {
                assert_eq!(reason, &TurnEndReason::Completed);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn finished_without_text_logs_no_assistant_message() {
        let mut state = AcpTurnState::new("agy");
        state.start_turn("hi");
        let done = state.finished("end_turn");
        assert!(
            !done
                .log
                .iter()
                .any(|event| matches!(event, SessionEvent::AssistantMessage { .. }))
        );
        assert!(
            done.log
                .iter()
                .any(|event| matches!(event, SessionEvent::TurnEnded { .. }))
        );
    }

    #[test]
    fn thought_deltas_open_one_reasoning_item() {
        let mut state = AcpTurnState::new("agy");
        state.start_turn("hi");
        let first = state.thought_delta("plan");
        match &first.events[0] {
            AgentEvent::ItemStarted {
                item: SessionItem::Reasoning { summary, .. },
            } => assert!(summary.is_empty()),
            other => panic!("unexpected event: {other:?}"),
        }
        assert_eq!(
            first.events[1],
            AgentEvent::ItemDelta {
                item_id: match &first.events[0] {
                    AgentEvent::ItemStarted {
                        item: SessionItem::Reasoning { id, .. },
                    } => id.clone(),
                    _ => unreachable!(),
                },
                delta: ItemDelta::ReasoningSummary("plan".to_string()),
            }
        );
        let second = state.thought_delta("ning");
        assert_eq!(second.events.len(), 1);
        let done = state.finished("end_turn");
        match &done.events[0] {
            AgentEvent::ItemCompleted {
                item: SessionItem::Reasoning { summary, .. },
            } => assert_eq!(summary, "planning"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn tool_call_starts_an_external_item_and_logs_the_call() {
        let mut state = AcpTurnState::new("agy");
        state.start_turn("hi");
        let emission = state.tool_call(&tool_call());
        assert_eq!(
            emission.log,
            vec![SessionEvent::ToolCall {
                call_id: CallId::from_raw("call-1"),
                name: "Edit src/lib.rs".to_string(),
                args: "{\"path\":\"src/lib.rs\"}".to_string(),
            }]
        );
        match &emission.events[0] {
            AgentEvent::ItemStarted {
                item:
                    SessionItem::ToolCall {
                        call_id,
                        name,
                        args,
                        effect,
                        status,
                        output,
                        ..
                    },
            } => {
                assert_eq!(call_id, &CallId::from_raw("call-1"));
                assert_eq!(name, "Edit src/lib.rs");
                assert_eq!(args, "{\"path\":\"src/lib.rs\"}");
                assert_eq!(effect, &EffectClass::External);
                assert_eq!(status, &ItemStatus::Proposed);
                assert_eq!(output, &None);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn tool_call_update_completes_the_item_and_logs_the_result() {
        let mut state = AcpTurnState::new("agy");
        state.start_turn("hi");
        let started = state.tool_call(&tool_call());
        let item_id = match &started.events[0] {
            AgentEvent::ItemStarted {
                item: SessionItem::ToolCall { id, .. },
            } => id.clone(),
            other => panic!("unexpected event: {other:?}"),
        };
        let update = AcpToolCallUpdate {
            id: "call-1".to_string(),
            status: "completed".to_string(),
            output: Some("wrote 3 lines".to_string()),
        };
        let emission = state.tool_call_update(&update);
        assert_eq!(
            emission.log,
            vec![SessionEvent::ToolResult {
                call_id: CallId::from_raw("call-1"),
                output: ToolObservation::success("wrote 3 lines"),
            }]
        );
        match &emission.events[0] {
            AgentEvent::ItemCompleted {
                item:
                    SessionItem::ToolCall {
                        id, status, output, ..
                    },
            } => {
                assert_eq!(id, &item_id);
                assert_eq!(status, &ItemStatus::Succeeded);
                assert_eq!(output.as_deref(), Some("wrote 3 lines"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn failed_tool_call_update_logs_an_error_observation() {
        let mut state = AcpTurnState::new("agy");
        state.start_turn("hi");
        state.tool_call(&tool_call());
        let update = AcpToolCallUpdate {
            id: "call-1".to_string(),
            status: "failed".to_string(),
            output: Some("permission denied".to_string()),
        };
        let emission = state.tool_call_update(&update);
        assert_eq!(
            emission.log,
            vec![SessionEvent::ToolResult {
                call_id: CallId::from_raw("call-1"),
                output: ToolObservation::error("permission denied"),
            }]
        );
        match &emission.events[0] {
            AgentEvent::ItemCompleted {
                item: SessionItem::ToolCall { status, .. },
            } => assert_eq!(status, &ItemStatus::Failed),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn plan_emits_started_and_completed_items() {
        let mut state = AcpTurnState::new("agy");
        state.start_turn("hi");
        let steps = vec![
            AcpPlanStep {
                content: "read".to_string(),
                status: "completed".to_string(),
                priority: "high".to_string(),
            },
            AcpPlanStep {
                content: "write".to_string(),
                status: "in_progress".to_string(),
                priority: "low".to_string(),
            },
            AcpPlanStep {
                content: "test".to_string(),
                status: "weird".to_string(),
                priority: "medium".to_string(),
            },
        ];
        let emission = state.plan(&steps);
        assert_eq!(emission.events.len(), 2);
        let expected = vec![
            PlanStep {
                text: "read".to_string(),
                status: PlanStepStatus::Completed,
            },
            PlanStep {
                text: "write".to_string(),
                status: PlanStepStatus::InProgress,
            },
            PlanStep {
                text: "test".to_string(),
                status: PlanStepStatus::Pending,
            },
        ];
        match &emission.events[0] {
            AgentEvent::ItemStarted {
                item: SessionItem::Plan { steps, .. },
            } => assert_eq!(steps, &expected),
            other => panic!("unexpected event: {other:?}"),
        }
        match &emission.events[1] {
            AgentEvent::ItemCompleted {
                item: SessionItem::Plan { steps, .. },
            } => assert_eq!(steps, &expected),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn permission_request_maps_to_an_external_agent_approval() {
        let mut state = AcpTurnState::new("agy");
        let turn_id = match &state.start_turn("hi").events[0] {
            AgentEvent::TurnStarted { turn_id, .. } => turn_id.clone(),
            other => panic!("unexpected event: {other:?}"),
        };
        let emission = state
            .permission(&permission_request())
            .expect("permission mapped");
        let request = match &emission.events[0] {
            AgentEvent::ApprovalRequested { request } => request,
            other => panic!("unexpected event: {other:?}"),
        };
        assert_eq!(request.turn_id, turn_id);
        assert_eq!(request.kind, ApprovalKind::ExternalAgent);
        assert_eq!(request.effect, Some(EffectClass::External));
        assert_eq!(
            request.preview.title,
            "External agent permission: Edit src/lib.rs"
        );
        assert_eq!(request.preview.detail, EXTERNAL_AGENT_DETAIL);
        assert_eq!(
            emission.log[0],
            SessionEvent::ApprovalRequested {
                request_id: request.request_id.clone(),
                kind: ApprovalKind::ExternalAgent,
                preview: request.preview.clone(),
            }
        );
    }

    #[test]
    fn permission_without_a_turn_is_not_mapped() {
        let mut state = AcpTurnState::new("agy");
        assert!(state.permission(&permission_request()).is_none());
    }

    #[test]
    fn approval_decisions_select_options_by_kind() {
        let options = permission_options();
        assert_eq!(
            select_option(&options, &ApprovalDecision::Accept).as_deref(),
            Some("allow-once")
        );
        assert_eq!(
            select_option(&options, &ApprovalDecision::AcceptForSession).as_deref(),
            Some("allow-always")
        );
        assert_eq!(
            select_option(&options, &ApprovalDecision::Decline).as_deref(),
            Some("reject-once")
        );
        assert_eq!(
            select_option(&options, &ApprovalDecision::DeclineWithReason("no".into())).as_deref(),
            Some("reject-once")
        );
        assert_eq!(select_option(&options, &ApprovalDecision::Cancel), None);
    }

    #[test]
    fn approval_decisions_fall_back_without_guessing() {
        let allow_always_only = vec![AcpPermissionOption {
            option_id: "allow-always".to_string(),
            name: "Always allow".to_string(),
            kind: AcpPermissionKind::AllowAlways,
        }];
        assert_eq!(
            select_option(&allow_always_only, &ApprovalDecision::Accept).as_deref(),
            Some("allow-always")
        );

        let reject_always_only = vec![AcpPermissionOption {
            option_id: "reject-always".to_string(),
            name: "Always reject".to_string(),
            kind: AcpPermissionKind::RejectAlways,
        }];
        assert_eq!(
            select_option(
                &reject_always_only,
                &ApprovalDecision::DeclineWithReason("no".into())
            )
            .as_deref(),
            Some("reject-always")
        );

        let allow_always_only = vec![AcpPermissionOption {
            option_id: "allow-always".to_string(),
            name: "Always allow".to_string(),
            kind: AcpPermissionKind::AllowAlways,
        }];
        assert_eq!(
            select_option(&allow_always_only, &ApprovalDecision::AcceptForSession).as_deref(),
            Some("allow-always")
        );

        let allow_once_only = vec![AcpPermissionOption {
            option_id: "allow-once".to_string(),
            name: "Allow once".to_string(),
            kind: AcpPermissionKind::AllowOnce,
        }];
        assert_eq!(
            select_option(&allow_once_only, &ApprovalDecision::AcceptForSession).as_deref(),
            Some("allow-once")
        );

        let allow_once_only = vec![AcpPermissionOption {
            option_id: "allow-once".to_string(),
            name: "Allow once".to_string(),
            kind: AcpPermissionKind::AllowOnce,
        }];
        assert_eq!(
            select_option(&allow_once_only, &ApprovalDecision::Decline),
            None
        );
    }

    #[test]
    fn resolving_a_permission_emits_approval_resolved_and_replies() {
        let mut state = AcpTurnState::new("agy");
        state.start_turn("hi");
        let emission = state.permission(&permission_request()).expect("mapped");
        let request_id = match &emission.events[0] {
            AgentEvent::ApprovalRequested { request } => request.request_id.clone(),
            other => panic!("unexpected event: {other:?}"),
        };
        let (resolved, reply) = state
            .resolve_permission(&request_id, &ApprovalDecision::Accept, None)
            .expect("resolved");
        assert_eq!(
            resolved.events,
            vec![AgentEvent::ApprovalResolved {
                request_id: request_id.clone()
            }]
        );
        assert_eq!(
            resolved.log,
            vec![SessionEvent::ApprovalDecided {
                request_id: request_id.clone(),
                decision: ApprovalDecision::Accept,
                scope: None,
            }]
        );
        match reply {
            PermissionReply::Selected {
                acp_request_id,
                option_id,
            } => {
                assert_eq!(acp_request_id, 3);
                assert_eq!(option_id, "allow-once");
            }
            PermissionReply::Cancelled { .. } => panic!("expected a selected option"),
        }
        assert!(
            state
                .resolve_permission(&request_id, &ApprovalDecision::Accept, None)
                .is_none()
        );
    }

    #[test]
    fn cancel_decision_cancels_the_acp_permission() {
        let mut state = AcpTurnState::new("agy");
        state.start_turn("hi");
        let emission = state.permission(&permission_request()).expect("mapped");
        let request_id = match &emission.events[0] {
            AgentEvent::ApprovalRequested { request } => request.request_id.clone(),
            other => panic!("unexpected event: {other:?}"),
        };
        let (_, reply) = state
            .resolve_permission(&request_id, &ApprovalDecision::Cancel, None)
            .expect("resolved");
        match reply {
            PermissionReply::Cancelled { acp_request_id } => assert_eq!(acp_request_id, 3),
            PermissionReply::Selected { .. } => panic!("expected a cancelled permission"),
        }
    }

    #[test]
    fn stop_reasons_map_to_turn_end_reasons() {
        assert_eq!(turn_end_reason("end_turn"), TurnEndReason::Completed);
        assert_eq!(turn_end_reason("max_tokens"), TurnEndReason::MaxTokens);
        assert_eq!(
            turn_end_reason("cancelled"),
            TurnEndReason::Aborted {
                cause: AbortCause::User
            }
        );
        assert_eq!(
            turn_end_reason("refusal"),
            TurnEndReason::Failed {
                error: "agent refused".to_string()
            }
        );
        assert_eq!(
            turn_end_reason("max_turn_requests"),
            TurnEndReason::Completed
        );
        assert_eq!(turn_end_reason("unknown"), TurnEndReason::Completed);
    }

    #[test]
    fn error_fails_the_active_turn() {
        let mut state = AcpTurnState::new("agy");
        state.start_turn("hi");
        let emission = state.error("adapter crashed");
        assert!(!state.is_active());
        assert_eq!(
            emission.events[0],
            AgentEvent::Error {
                code: ErrorCode::Provider,
                message: "adapter crashed".to_string(),
            }
        );
        assert_eq!(
            emission.log,
            vec![
                SessionEvent::Error {
                    code: ErrorCode::Provider,
                    message: "adapter crashed".to_string(),
                    retryable: false,
                },
                SessionEvent::TurnEnded {
                    turn_id: match &emission.events[1] {
                        AgentEvent::TurnCompleted { turn_id, .. } => turn_id.clone(),
                        other => panic!("unexpected event: {other:?}"),
                    },
                    reason: TurnEndReason::Failed {
                        error: "adapter crashed".to_string(),
                    },
                },
            ]
        );
        match &emission.events[1] {
            AgentEvent::TurnCompleted { reason, .. } => assert_eq!(
                reason,
                &TurnEndReason::Failed {
                    error: "adapter crashed".to_string()
                }
            ),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn error_without_a_turn_only_emits_an_error_event() {
        let mut state = AcpTurnState::new("agy");
        let emission = state.error("connect failed");
        assert!(emission.log.is_empty());
        assert_eq!(emission.events.len(), 1);
    }

    #[test]
    fn deltas_without_a_turn_are_ignored() {
        let mut state = AcpTurnState::new("agy");
        assert!(state.text_delta("ignored").events.is_empty());
        assert!(state.thought_delta("ignored").events.is_empty());
        assert!(state.tool_call(&tool_call()).events.is_empty());
    }
}
