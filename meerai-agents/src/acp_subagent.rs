use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use meerai_common::hooks::HookEvent;
use meerai_core::{
    EffectClass, PermissionAction, ToolDefinition, ToolError, ToolOutput, ToolPermissionSpec,
    ToolRun, Toolset,
};
use meerai_session::Session;
use serde::Deserialize;
use tokio::sync::{mpsc, oneshot};

use crate::events::{
    AgentCommand, AgentEvent, ApprovalDecision, ItemDelta, ItemId, ItemStatus, SessionItem,
    TurnEndReason,
};
use crate::hooks::{HookContext, HookDecision, HookEngine};
use crate::subagent::{ForwardedApproval, SUBAGENT_BRIDGE};

const TOOLSET_NAME: &str = "acp";
const TOOL_RUN: &str = "acp-run";
const AGENT_ID: &str = "agy";
const DEFAULT_TIMEOUT_SECS: u64 = 600;
const MIN_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 3600;
const CANCEL_GRACE: Duration = Duration::from_secs(2);

pub struct AcpChild {
    pub cmd_tx: mpsc::UnboundedSender<AgentCommand>,
    pub event_rx: mpsc::UnboundedReceiver<AgentEvent>,
}

pub trait AcpSpawner: Send + Sync {
    fn spawn(&self, session: Session, cwd: PathBuf) -> AcpChild;
}

pub struct AcpRunToolset {
    spawner: Arc<dyn AcpSpawner>,
    hooks: Option<Arc<HookEngine>>,
}

impl AcpRunToolset {
    pub fn new(spawner: Arc<dyn AcpSpawner>) -> Self {
        Self {
            spawner,
            hooks: None,
        }
    }

    pub fn with_hooks(mut self, hooks: Arc<HookEngine>) -> Self {
        self.hooks = Some(hooks);
        self
    }
}

#[derive(Debug, Deserialize)]
struct AcpRunArgs {
    agent: String,
    task: String,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

async fn fire_hooks(
    hooks: &HookEngine,
    parent: &Session,
    workspace_root: &Path,
    args: &str,
    task: &str,
    output: Option<&str>,
    event: HookEvent,
) -> Option<String> {
    let context = HookContext {
        session_id: parent.id().as_str(),
        turn_id: None,
        tool_name: Some(TOOL_RUN),
        tool_input: Some(args),
        tool_output: output,
        prompt: Some(task),
        error: None,
        cwd: workspace_root,
    };
    let report = hooks.run(event, &context).await;
    for firing in &report.firings {
        let _ = parent.append(firing.clone().into_session_event()).await;
    }
    if matches!(report.decision, HookDecision::Deny | HookDecision::Block) {
        return Some(report.reason().unwrap_or("blocked by hook").to_string());
    }
    None
}

fn failure_text(report: &str, error: Option<String>, fallback: &str) -> String {
    if !report.trim().is_empty() {
        report.to_string()
    } else if let Some(error) = error {
        error
    } else {
        fallback.to_string()
    }
}

#[async_trait]
impl Toolset for AcpRunToolset {
    fn name(&self) -> String {
        TOOLSET_NAME.to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            r#type: "function".to_string(),
            name: TOOL_RUN.to_string(),
            description: "Delegate one task to the external 'agy' ACP agent in a child session. The external agent has its own tools, permissions, and policy: meerai policy, path guards, redaction, and sandbox do not apply to it. Its permission requests are forwarded to the user for approval.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agent": {
                        "type": "string",
                        "description": "External ACP agent id (only 'agy')"
                    },
                    "task": {
                        "type": "string",
                        "description": "Task instructions for the external agent"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Wall-clock timeout in seconds (default 600, clamped to 30..=3600)"
                    }
                },
                "required": ["agent", "task"]
            }),
        }]
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == TOOL_RUN
    }

    fn effect_of(&self, _fn_name: &str) -> EffectClass {
        EffectClass::External
    }

    fn effect_of_call(&self, _fn_name: &str, _args: &str) -> EffectClass {
        EffectClass::External
    }

    fn permission_spec(&self, fn_name: &str) -> Option<ToolPermissionSpec> {
        (fn_name == TOOL_RUN).then_some(ToolPermissionSpec {
            action: PermissionAction::Subagent,
            resource_arg: Some("agent"),
        })
    }

    fn should_spill_output(&self, _fn_name: &str) -> bool {
        true
    }

    async fn invoke(&self, fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
        Ok(self.invoke_structured(fn_name, args).await?.output)
    }

    async fn invoke_structured(&self, fn_name: &str, args: &str) -> Result<ToolRun, ToolError> {
        if fn_name != TOOL_RUN {
            return Err(ToolError::InvalidFunctionName(fn_name.to_string()));
        }

        let parsed: AcpRunArgs = serde_json::from_str(args).map_err(ToolError::WrongArguments)?;
        let agent = parsed.agent.trim().to_string();
        if agent != AGENT_ID {
            return Ok(ToolRun::from_output(ToolOutput::Fail(format!(
                "unknown ACP agent '{agent}'; only '{AGENT_ID}' is supported"
            ))));
        }
        let task = parsed.task;
        if task.trim().is_empty() {
            return Ok(ToolRun::from_output(ToolOutput::Fail(
                "acp task must not be empty".to_string(),
            )));
        }

        let Some(bridge) = SUBAGENT_BRIDGE.try_with(Clone::clone).ok() else {
            return Ok(ToolRun::from_output(ToolOutput::Fail(
                "acp-run requires a harness".to_string(),
            )));
        };
        let Some(parent) = bridge.parent_session.clone() else {
            return Ok(ToolRun::from_output(ToolOutput::Fail(
                "acp-run requires a parent session".to_string(),
            )));
        };

        if let Some(hooks) = &self.hooks
            && let Some(reason) = fire_hooks(
                hooks,
                &parent,
                &bridge.workspace_root,
                args,
                &task,
                None,
                HookEvent::SubagentStart,
            )
            .await
        {
            return Ok(ToolRun::from_output(ToolOutput::Fail(reason)));
        }

        let title: String = task
            .lines()
            .next()
            .unwrap_or("acp task")
            .chars()
            .take(60)
            .collect();
        let child_session = parent
            .create_child(Some("acp:agy".to_string()), Some(title))
            .await
            .map_err(ToolError::Unknown)?;
        let child_session_id = child_session.id().to_string();

        let item_id = ItemId::new();
        let _ = bridge.event_tx.send(AgentEvent::ItemStarted {
            item: SessionItem::SubagentRun {
                id: item_id.clone(),
                agent: AGENT_ID.to_string(),
                status: ItemStatus::InProgress,
                report: None,
                child_session_id: Some(child_session_id.clone()),
            },
        });

        let AcpChild {
            cmd_tx,
            mut event_rx,
        } = self
            .spawner
            .spawn(child_session, bridge.workspace_root.clone());
        let _ = cmd_tx.send(AgentCommand::SubmitPrompt(task.clone()));

        let timeout_secs = parsed
            .timeout_secs
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS);

        let mut report = String::new();
        let mut error: Option<String> = None;
        let mut end_reason: Option<TurnEndReason> = None;
        let mut cancelled = false;

        let outcome = tokio::time::timeout(Duration::from_secs(timeout_secs), async {
            loop {
                tokio::select! {
                    _ = bridge.cancel.cancelled() => {
                        let _ = cmd_tx.send(AgentCommand::Cancel);
                        cancelled = true;
                        break;
                    }
                    event = event_rx.recv() => {
                        let Some(event) = event else {
                            break;
                        };
                        match event {
                            AgentEvent::ItemDelta {
                                delta: ItemDelta::AgentText(chunk),
                                ..
                            } => report.push_str(&chunk),
                            AgentEvent::ApprovalRequested { request } => {
                                let request_id = request.request_id.clone();
                                let (reply_tx, reply_rx) = oneshot::channel();
                                let forwarded = ForwardedApproval {
                                    request,
                                    reply: reply_tx,
                                };
                                if bridge.approval_tx.send(forwarded).is_ok() {
                                    let (decision, scope) = reply_rx
                                        .await
                                        .unwrap_or((ApprovalDecision::Cancel, None));
                                    let _ = cmd_tx.send(AgentCommand::Approve {
                                        request_id,
                                        decision,
                                        scope,
                                    });
                                } else {
                                    let _ = cmd_tx.send(AgentCommand::Cancel);
                                }
                            }
                            AgentEvent::TurnCompleted { reason, .. } => {
                                end_reason = Some(reason);
                                break;
                            }
                            AgentEvent::Error { message, .. } => error = Some(message),
                            _ => {}
                        }
                    }
                }
            }
        })
        .await;
        let timed_out = outcome.is_err();

        if timed_out || cancelled {
            let _ = cmd_tx.send(AgentCommand::Cancel);
            let drain = async {
                loop {
                    match event_rx.recv().await {
                        Some(AgentEvent::ItemDelta {
                            delta: ItemDelta::AgentText(chunk),
                            ..
                        }) => report.push_str(&chunk),
                        Some(AgentEvent::TurnCompleted { reason, .. }) => {
                            end_reason = Some(reason);
                            break;
                        }
                        Some(AgentEvent::Error { message, .. }) => error = Some(message),
                        Some(_) => {}
                        None => break,
                    }
                }
            };
            let _ = tokio::time::timeout(CANCEL_GRACE, drain).await;
        }
        let _ = cmd_tx.send(AgentCommand::Quit);

        let (status, text) = if cancelled {
            (ItemStatus::Cancelled, "acp agent cancelled".to_string())
        } else {
            match end_reason {
                Some(TurnEndReason::Completed) => {
                    let text = if report.trim().is_empty() {
                        "acp agent completed with no text".to_string()
                    } else {
                        report
                    };
                    (ItemStatus::Succeeded, text)
                }
                Some(TurnEndReason::Aborted { .. }) => {
                    (ItemStatus::Cancelled, "acp agent cancelled".to_string())
                }
                Some(TurnEndReason::Failed { error }) => (ItemStatus::Failed, error),
                Some(TurnEndReason::MaxTokens) => (
                    ItemStatus::Failed,
                    failure_text(&report, error, "acp agent stopped at max tokens"),
                ),
                Some(_) => (
                    ItemStatus::Failed,
                    failure_text(&report, error, "acp agent did not complete"),
                ),
                None => (
                    ItemStatus::Failed,
                    failure_text(
                        &report,
                        error,
                        &format!("acp agent timed out after {timeout_secs}s"),
                    ),
                ),
            }
        };
        let output = if status == ItemStatus::Succeeded {
            ToolOutput::Text(text.clone())
        } else {
            ToolOutput::Fail(text.clone())
        };

        let _ = bridge.event_tx.send(AgentEvent::ItemCompleted {
            item: SessionItem::SubagentRun {
                id: item_id,
                agent: AGENT_ID.to_string(),
                status,
                report: Some(text.clone()),
                child_session_id: Some(child_session_id.clone()),
            },
        });

        if let Some(hooks) = &self.hooks {
            let _ = fire_hooks(
                hooks,
                &parent,
                &bridge.workspace_root,
                args,
                &task,
                Some(&text),
                HookEvent::SubagentStop,
            )
            .await;
        }

        let mut run = ToolRun::from_output(output);
        run.observation.hint = Some(format!("child_session_id: {child_session_id}"));
        Ok(run)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use meerai_policy::ApprovalMode;
    use meerai_session::SessionId;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::events::{
        AbortCause, ApprovalKind, ApprovalPreview, ApprovalRequest, RequestId, TurnId,
    };
    use crate::harness::HarnessConfig;
    use crate::subagent::SubagentBridge;

    struct FakeSpawner {
        scripted: Mutex<Vec<AgentEvent>>,
        event_tx: Mutex<Option<mpsc::UnboundedSender<AgentEvent>>>,
        commands: Mutex<Option<mpsc::UnboundedReceiver<AgentCommand>>>,
        spawned: AtomicUsize,
    }

    impl FakeSpawner {
        fn new(scripted: Vec<AgentEvent>) -> Self {
            Self {
                scripted: Mutex::new(scripted),
                event_tx: Mutex::new(None),
                commands: Mutex::new(None),
                spawned: AtomicUsize::new(0),
            }
        }

        fn send(&self, event: AgentEvent) {
            if let Some(tx) = self.event_tx.lock().unwrap().as_ref() {
                let _ = tx.send(event);
            }
        }

        fn commands(&self) -> Vec<AgentCommand> {
            let mut guard = self.commands.lock().unwrap();
            let Some(rx) = guard.as_mut() else {
                return Vec::new();
            };
            let mut drained = Vec::new();
            while let Ok(command) = rx.try_recv() {
                drained.push(command);
            }
            drained
        }
    }

    impl AcpSpawner for FakeSpawner {
        fn spawn(&self, _session: Session, _cwd: PathBuf) -> AcpChild {
            self.spawned.fetch_add(1, Ordering::SeqCst);
            let (event_tx, event_rx) = mpsc::unbounded_channel();
            for event in self.scripted.lock().unwrap().drain(..) {
                let _ = event_tx.send(event);
            }
            *self.event_tx.lock().unwrap() = Some(event_tx);
            let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
            *self.commands.lock().unwrap() = Some(cmd_rx);
            AcpChild { cmd_tx, event_rx }
        }
    }

    struct Fixture {
        bridge: SubagentBridge,
        events: mpsc::UnboundedReceiver<AgentEvent>,
        approvals: mpsc::UnboundedReceiver<ForwardedApproval>,
        _grants: mpsc::UnboundedReceiver<meerai_policy::Grant>,
        _budgets: mpsc::UnboundedReceiver<crate::subagent::SubagentBudgetUpdate>,
    }

    fn fixture(parent: &Session, cancel: CancellationToken) -> Fixture {
        let (event_tx, events) = mpsc::unbounded_channel();
        let (approval_tx, approvals) = mpsc::unbounded_channel();
        let (usage_tx, _usage) = mpsc::unbounded_channel();
        let (grant_tx, grants) = mpsc::unbounded_channel();
        let (budget_tx, budgets) = mpsc::unbounded_channel();
        let bridge = SubagentBridge {
            event_tx,
            approval_tx,
            usage_tx,
            grant_tx,
            budget_tx,
            cancel,
            parent_session: Some(parent.clone()),
            parent_policy_mode: ApprovalMode::Ask,
            parent_policy_rules: Vec::new(),
            parent_policy_grants: Vec::new(),
            workspace_root: parent.meta().cwd,
            parent_harness_config: HarnessConfig::default(),
            parent_used_tokens: 0,
        };
        Fixture {
            bridge,
            events,
            approvals,
            _grants: grants,
            _budgets: budgets,
        }
    }

    fn parent_session() -> Session {
        Session::memory(SessionId::new(), PathBuf::from("."))
    }

    fn text_event(text: &str) -> AgentEvent {
        AgentEvent::ItemDelta {
            item_id: ItemId::new(),
            delta: ItemDelta::AgentText(text.to_string()),
        }
    }

    fn completed_event() -> AgentEvent {
        AgentEvent::TurnCompleted {
            turn_id: TurnId::new(),
            reason: TurnEndReason::Completed,
        }
    }

    fn approval_request() -> ApprovalRequest {
        ApprovalRequest {
            request_id: RequestId::new(),
            turn_id: TurnId::new(),
            kind: ApprovalKind::ExternalAgent,
            preview: ApprovalPreview {
                title: "External agent permission: Edit src/lib.rs".to_string(),
                detail: "external agent".to_string(),
            },
            effect: Some(EffectClass::External),
        }
    }

    #[test]
    fn declares_the_external_tool_contract() {
        let tool = AcpRunToolset::new(Arc::new(FakeSpawner::new(Vec::new())));
        assert_eq!(tool.name(), "acp");
        assert!(tool.contain("acp-run"));
        assert!(!tool.contain("acp-other"));
        assert_eq!(tool.effect_of("acp-run"), EffectClass::External);
        assert_eq!(tool.effect_of_call("acp-run", "{}"), EffectClass::External);
        assert!(tool.should_spill_output("acp-run"));
        let spec = tool.permission_spec("acp-run").unwrap();
        assert_eq!(spec.action, PermissionAction::Subagent);
        assert_eq!(spec.resource_arg, Some("agent"));
        let definition = &tool.definition()[0];
        assert_eq!(definition.name, "acp-run");
        assert!(definition.description.contains("own tools"));
        assert!(definition.description.contains("do not apply"));
    }

    #[tokio::test]
    async fn text_run_completes_and_reports_the_child_session() {
        let parent = parent_session();
        let spawner = Arc::new(FakeSpawner::new(vec![text_event("OK"), completed_event()]));
        let tool = AcpRunToolset::new(spawner.clone());
        let Fixture {
            bridge, mut events, ..
        } = fixture(&parent, CancellationToken::new());

        let run = SUBAGENT_BRIDGE
            .scope(
                bridge,
                tool.invoke_structured("acp-run", r#"{"agent":"agy","task":"say OK"}"#),
            )
            .await
            .unwrap();

        assert_eq!(run.output, ToolOutput::Text("OK".to_string()));
        let hint = run.observation.hint.unwrap();
        assert!(hint.starts_with("child_session_id: "));

        let mut collected = Vec::new();
        while let Ok(event) = events.try_recv() {
            collected.push(event);
        }
        assert_eq!(collected.len(), 2);
        match &collected[0] {
            AgentEvent::ItemStarted {
                item:
                    SessionItem::SubagentRun {
                        agent,
                        status,
                        child_session_id,
                        ..
                    },
            } => {
                assert_eq!(agent, "agy");
                assert_eq!(status, &ItemStatus::InProgress);
                assert_eq!(
                    child_session_id.as_deref(),
                    Some(hint.trim_start_matches("child_session_id: "))
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
        match &collected[1] {
            AgentEvent::ItemCompleted {
                item: SessionItem::SubagentRun { status, report, .. },
            } => {
                assert_eq!(status, &ItemStatus::Succeeded);
                assert_eq!(report.as_deref(), Some("OK"));
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let commands = spawner.commands();
        assert!(
            matches!(commands.first(), Some(AgentCommand::SubmitPrompt(task)) if task == "say OK")
        );
        assert!(matches!(commands.last(), Some(AgentCommand::Quit)));
    }

    #[tokio::test]
    async fn permission_requests_are_forwarded_and_approved() {
        let parent = parent_session();
        let request = approval_request();
        let request_id = request.request_id.clone();
        let spawner = Arc::new(FakeSpawner::new(vec![
            AgentEvent::ApprovalRequested { request },
            completed_event(),
        ]));
        let tool = Arc::new(AcpRunToolset::new(spawner.clone()));
        let Fixture {
            bridge,
            mut approvals,
            ..
        } = fixture(&parent, CancellationToken::new());

        let handle = tokio::spawn(SUBAGENT_BRIDGE.scope(bridge, async move {
            tool.invoke_structured("acp-run", r#"{"agent":"agy","task":"edit"}"#)
                .await
        }));

        let forwarded = approvals.recv().await.expect("forwarded approval");
        assert_eq!(forwarded.request.request_id, request_id);
        let _ = forwarded.reply.send((ApprovalDecision::Accept, None));

        let run = handle.await.unwrap().unwrap();
        assert_eq!(
            run.output,
            ToolOutput::Text("acp agent completed with no text".to_string())
        );

        let commands = spawner.commands();
        assert!(commands.iter().any(|command| matches!(
            command,
            AgentCommand::Approve {
                request_id: id,
                decision: ApprovalDecision::Accept,
                scope: None,
            } if id == &request_id
        )));
    }

    #[tokio::test]
    async fn parent_cancel_sends_cancel_to_the_child() {
        let parent = parent_session();
        let spawner = Arc::new(FakeSpawner::new(Vec::new()));
        let tool = AcpRunToolset::new(spawner.clone());
        let cancel = CancellationToken::new();
        cancel.cancel();
        let Fixture { bridge, .. } = fixture(&parent, cancel);

        let notifier = spawner.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            notifier.send(AgentEvent::TurnCompleted {
                turn_id: TurnId::new(),
                reason: TurnEndReason::Aborted {
                    cause: AbortCause::User,
                },
            });
        });

        let run = SUBAGENT_BRIDGE
            .scope(
                bridge,
                tool.invoke_structured("acp-run", r#"{"agent":"agy","task":"work"}"#),
            )
            .await
            .unwrap();

        assert_eq!(
            run.output,
            ToolOutput::Fail("acp agent cancelled".to_string())
        );
        let commands = spawner.commands();
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, AgentCommand::Cancel))
        );
        assert!(matches!(commands.last(), Some(AgentCommand::Quit)));
    }

    #[tokio::test]
    async fn unknown_agent_fails_without_spawning() {
        let parent = parent_session();
        let spawner = Arc::new(FakeSpawner::new(Vec::new()));
        let tool = AcpRunToolset::new(spawner.clone());
        let Fixture { bridge, .. } = fixture(&parent, CancellationToken::new());

        let run = SUBAGENT_BRIDGE
            .scope(
                bridge,
                tool.invoke_structured("acp-run", r#"{"agent":"claude","task":"x"}"#),
            )
            .await
            .unwrap();

        assert_eq!(
            run.output,
            ToolOutput::Fail("unknown ACP agent 'claude'; only 'agy' is supported".to_string())
        );
        assert_eq!(spawner.spawned.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn empty_task_fails_without_spawning() {
        let parent = parent_session();
        let spawner = Arc::new(FakeSpawner::new(Vec::new()));
        let tool = AcpRunToolset::new(spawner.clone());
        let Fixture { bridge, .. } = fixture(&parent, CancellationToken::new());

        let run = SUBAGENT_BRIDGE
            .scope(
                bridge,
                tool.invoke_structured("acp-run", r#"{"agent":"agy","task":"   "}"#),
            )
            .await
            .unwrap();

        assert_eq!(
            run.output,
            ToolOutput::Fail("acp task must not be empty".to_string())
        );
        assert_eq!(spawner.spawned.load(Ordering::SeqCst), 0);
    }
}
