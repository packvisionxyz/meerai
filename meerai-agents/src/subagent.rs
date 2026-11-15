use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use meerai_common::definitions::{AgentDefinition, validate_agent_model};
use meerai_core::{
    EffectClass, GrantScope, PermissionAction, ToolDefinition, ToolError, ToolOutput,
    ToolPermissionSpec, ToolRun, Toolset,
};
use meerai_policy::{ApprovalMode, Grant, PermissionRule, PolicyEngine};
use meerai_session::{Session, SessionId, Usage};
use serde::Deserialize;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::events::{
    AgentCommand, AgentEvent, ApprovalDecision, ApprovalRequest, ItemId, ItemStatus, SessionItem,
    TurnEndReason,
};
use crate::harness::{AgentHarness, HarnessConfig};
use crate::multi_turn_agent::{ChatStreamer, MultiTurnAgent, MultiTurnAgentConfig};

const TOOLSET_NAME: &str = "subagent";
const TOOL_RUN: &str = "subagent-run";
const CUSTOM_AGENT_MAX_STEPS: usize = 16;

pub const READ_ONLY_SUBAGENT_TOOLS: &[&str] = &[
    "code-read_file",
    "code-list_dir",
    "code-grep",
    "code-glob",
    "retrieval-repo_map",
    "lsp-go_to_definition",
    "lsp-find_references",
    "lsp-get_diagnostics",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuiltinAgent {
    pub id: &'static str,
    pub description: &'static str,
    pub effect: EffectClass,
    pub read_only: bool,
    pub max_steps: usize,
    pub allowed_tools: Option<&'static [&'static str]>,
    pub system_prompt: &'static str,
}

const BUILTIN_AGENTS: [BuiltinAgent; 4] = [
    BuiltinAgent {
        id: "explore",
        description: "Fast read-only codebase search returning file:start-end spans and synthesis",
        effect: EffectClass::Read,
        read_only: true,
        max_steps: 4,
        allowed_tools: Some(READ_ONLY_SUBAGENT_TOOLS),
        system_prompt: "You are a fast, read-only codebase exploration subagent. Issue parallel read-only tool calls (code-read_file, code-list_dir, code-grep, code-glob, and any retrieval or LSP tools) to locate relevant code. Never attempt edits or shell commands. Return exact file:start-end spans and a single concise paragraph synthesizing your findings.",
    },
    BuiltinAgent {
        id: "plan",
        description: "Read-only architecture analysis and implementation planning without mutations",
        effect: EffectClass::Read,
        read_only: true,
        max_steps: 16,
        allowed_tools: Some(READ_ONLY_SUBAGENT_TOOLS),
        system_prompt: "You are a read-only planning subagent. Inspect the codebase using read-only tools without making any file or environment mutations. Return a concrete, step-by-step plan citing exact file paths, symbols, edge cases, and verification commands.",
    },
    BuiltinAgent {
        id: "build",
        description: "Focused implementation and verification subagent with write and shell tools",
        effect: EffectClass::Write,
        read_only: false,
        max_steps: 32,
        allowed_tools: None,
        system_prompt: "You are a focused implementation subagent. Make only the requested changes using the available tools, keep edits minimal and clean, and run relevant verification commands before reporting. Summarize modified files, verification results, and any remaining blockers.",
    },
    BuiltinAgent {
        id: "general",
        description: "General-purpose multi-step subagent with full non-subagent tools",
        effect: EffectClass::Write,
        read_only: false,
        max_steps: 16,
        allowed_tools: None,
        system_prompt: "You are a general-purpose subagent. Complete the assigned task accurately using the available tools and return a concise summary of what you did and observed.",
    },
];

pub fn builtin_agents() -> &'static [BuiltinAgent] {
    &BUILTIN_AGENTS
}

pub fn find_builtin_agent(id: &str) -> Option<&'static BuiltinAgent> {
    let trimmed = id.trim();
    BUILTIN_AGENTS.iter().find(|agent| agent.id == trimmed)
}

pub type SubagentToolFactory =
    Arc<dyn Fn(bool /* read_only */) -> Vec<Box<dyn Toolset>> + Send + Sync>;

pub(crate) struct ForwardedApproval {
    pub(crate) request: ApprovalRequest,
    pub(crate) reply: oneshot::Sender<(ApprovalDecision, Option<GrantScope>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SubagentBudgetUpdate {
    Increase(u64),
    Unlimited,
}

#[derive(Clone)]
pub(crate) struct SubagentBridge {
    pub(crate) event_tx: mpsc::UnboundedSender<AgentEvent>,
    pub(crate) approval_tx: mpsc::UnboundedSender<ForwardedApproval>,
    pub(crate) usage_tx: mpsc::UnboundedSender<Usage>,
    pub(crate) grant_tx: mpsc::UnboundedSender<Grant>,
    pub(crate) budget_tx: mpsc::UnboundedSender<SubagentBudgetUpdate>,
    pub(crate) cancel: CancellationToken,
    pub(crate) parent_session: Option<Session>,
    pub(crate) parent_policy_mode: ApprovalMode,
    pub(crate) parent_policy_rules: Vec<PermissionRule>,
    pub(crate) parent_policy_grants: Vec<Grant>,
    pub(crate) workspace_root: PathBuf,
    pub(crate) parent_harness_config: HarnessConfig,
    pub(crate) parent_used_tokens: u64,
}

tokio::task_local! {
    pub(crate) static SUBAGENT_BRIDGE: SubagentBridge;
}

#[derive(Debug, Deserialize)]
struct SubagentRunArgs {
    agent: String,
    task: String,
    #[serde(default, deserialize_with = "meerai_core::lenient::option_bool_value")]
    fork_session: Option<bool>,
}

enum ResolvedAgent<'a> {
    Builtin(&'static BuiltinAgent),
    Custom(&'a AgentDefinition),
}

impl ResolvedAgent<'_> {
    fn id(&self) -> &str {
        match self {
            Self::Builtin(agent) => agent.id,
            Self::Custom(agent) => &agent.id,
        }
    }

    fn effect(&self) -> EffectClass {
        match self {
            Self::Builtin(agent) => agent.effect,
            Self::Custom(_) => EffectClass::Read,
        }
    }

    fn read_only(&self) -> bool {
        match self {
            Self::Builtin(agent) => agent.read_only,
            Self::Custom(_) => true,
        }
    }

    fn max_steps(&self) -> usize {
        match self {
            Self::Builtin(agent) => agent.max_steps,
            Self::Custom(_) => CUSTOM_AGENT_MAX_STEPS,
        }
    }

    fn allowed_tools(&self) -> Option<&'static [&'static str]> {
        match self {
            Self::Builtin(agent) => agent.allowed_tools,
            Self::Custom(_) => None,
        }
    }

    fn system_prompt(&self) -> String {
        match self {
            Self::Builtin(agent) => agent.system_prompt.to_string(),
            Self::Custom(agent) => {
                if agent.prompt.trim().is_empty() {
                    "You are a read-only custom subagent. Complete the task using only read-only tools."
                        .to_string()
                } else {
                    agent.prompt.clone()
                }
            }
        }
    }

    fn model(&self, parent_model: &str) -> String {
        match self {
            Self::Builtin(_) => parent_model.to_string(),
            Self::Custom(agent) => match validate_agent_model(agent.model.as_deref()) {
                Ok(Some(raw)) => apply_child_model(parent_model, raw),
                Ok(None) | Err(_) => parent_model.to_string(),
            },
        }
    }
}

fn apply_child_model(parent_model: &str, model_id: &str) -> String {
    if let Some((provider, _)) = parent_model.split_once("::")
        && !provider.is_empty()
    {
        return format!("{provider}::{model_id}");
    }
    if let Some((provider, _)) = parent_model.split_once(':')
        && !provider.is_empty()
    {
        return format!("{provider}:{model_id}");
    }
    model_id.to_string()
}

struct ChildToolsetFilter {
    inner: Box<dyn Toolset>,
    read_only: bool,
    allowed_tools: Option<&'static [&'static str]>,
}

impl ChildToolsetFilter {
    fn allows(&self, fn_name: &str) -> bool {
        !fn_name.starts_with("subagent-")
            && self.inner.contain(fn_name)
            && (!self.read_only || self.inner.effect_of(fn_name) == EffectClass::Read)
            && self
                .allowed_tools
                .is_none_or(|allowed| allowed.contains(&fn_name))
    }
}

#[async_trait]
impl Toolset for ChildToolsetFilter {
    fn name(&self) -> String {
        self.inner.name()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        self.inner
            .definition()
            .into_iter()
            .filter(|def| self.allows(&def.name))
            .collect()
    }

    fn contain(&self, fn_name: &str) -> bool {
        self.allows(fn_name)
    }

    fn effect_of(&self, fn_name: &str) -> EffectClass {
        self.inner.effect_of(fn_name)
    }

    fn effect_of_call(&self, fn_name: &str, args: &str) -> EffectClass {
        self.inner.effect_of_call(fn_name, args)
    }

    fn permission_spec(&self, fn_name: &str) -> Option<ToolPermissionSpec> {
        self.inner.permission_spec(fn_name)
    }

    fn should_spill_output(&self, fn_name: &str) -> bool {
        self.inner.should_spill_output(fn_name)
    }

    async fn invoke(&self, fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
        if !self.allows(fn_name) {
            return Err(ToolError::InvalidFunctionName(fn_name.to_string()));
        }
        self.inner.invoke(fn_name, args).await
    }

    async fn invoke_structured(&self, fn_name: &str, args: &str) -> Result<ToolRun, ToolError> {
        if !self.allows(fn_name) {
            return Err(ToolError::InvalidFunctionName(fn_name.to_string()));
        }
        self.inner.invoke_structured(fn_name, args).await
    }
}

pub struct SubagentToolset {
    streamer: Arc<dyn ChatStreamer>,
    model: Arc<RwLock<String>>,
    tool_factory: SubagentToolFactory,
    custom_agents: Vec<AgentDefinition>,
}

impl SubagentToolset {
    pub fn new(
        streamer: Arc<dyn ChatStreamer>,
        model: Arc<RwLock<String>>,
        tool_factory: SubagentToolFactory,
    ) -> Self {
        Self {
            streamer,
            model,
            tool_factory,
            custom_agents: Vec::new(),
        }
    }

    pub fn from_agent(agent: &MultiTurnAgent, tool_factory: SubagentToolFactory) -> Self {
        Self::new(agent.streamer(), agent.model_handle(), tool_factory)
    }

    pub fn with_custom_agents(mut self, agents: Vec<AgentDefinition>) -> Self {
        self.custom_agents = agents
            .into_iter()
            .filter(|agent| find_builtin_agent(&agent.id).is_none())
            .collect();
        self
    }

    fn resolve_agent(&self, id: &str) -> Option<ResolvedAgent<'_>> {
        let trimmed = id.trim();
        if let Some(builtin) = find_builtin_agent(trimmed) {
            return Some(ResolvedAgent::Builtin(builtin));
        }
        self.custom_agents
            .iter()
            .find(|agent| agent.id == trimmed)
            .map(ResolvedAgent::Custom)
    }

    fn available_agent_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = BUILTIN_AGENTS
            .iter()
            .map(|agent| agent.id.to_string())
            .collect();
        for agent in &self.custom_agents {
            ids.push(agent.id.clone());
        }
        ids
    }

    fn build_child_tools(
        &self,
        read_only: bool,
        allowed_tools: Option<&'static [&'static str]>,
    ) -> Vec<Box<dyn Toolset>> {
        (self.tool_factory)(read_only)
            .into_iter()
            .filter(|tool| tool.name() != TOOLSET_NAME)
            .map(|tool| {
                Box::new(ChildToolsetFilter {
                    inner: tool,
                    read_only,
                    allowed_tools,
                }) as Box<dyn Toolset>
            })
            .filter(|tool| !tool.definition().is_empty())
            .collect()
    }

    fn current_model(&self) -> String {
        self.model
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn tool_description(&self) -> String {
        let mut parts: Vec<String> = BUILTIN_AGENTS
            .iter()
            .map(|agent| format!("{} ({})", agent.id, agent.description))
            .collect();
        for agent in &self.custom_agents {
            let desc = agent
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .unwrap_or("read-only custom agent");
            parts.push(format!("{} ({})", agent.id, desc));
        }
        format!(
            "Run an isolated subagent for a focused task. Available agents: {}.",
            parts.join("; ")
        )
    }
}

async fn create_child_session(
    parent: &Session,
    child_model: &str,
    fork_session: bool,
) -> Option<Session> {
    if fork_session
        && let Some(config) = parent.config()
        && let Ok(forked) = Session::fork_latest(config, parent).await
    {
        let _ = forked
            .update_meta(|meta| {
                meta.parent_session_id = Some(parent.id().clone());
                meta.model = Some(child_model.to_string());
            })
            .await;
        return Some(forked);
    }
    parent
        .create_child(Some(child_model.to_string()), None)
        .await
        .ok()
}

#[async_trait]
impl Toolset for SubagentToolset {
    fn name(&self) -> String {
        TOOLSET_NAME.to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            r#type: "function".to_string(),
            name: TOOL_RUN.to_string(),
            description: self.tool_description(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agent": {
                        "type": "string",
                        "description": "Subagent preset or custom agent id (built-ins: explore, plan, build, general)"
                    },
                    "task": {
                        "type": "string",
                        "description": "Focused task instructions for the subagent"
                    },
                    "fork_session": {
                        "type": "boolean",
                        "description": "Fork the parent session transcript instead of starting a fresh child session (default: false)"
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
        EffectClass::Write
    }

    fn effect_of_call(&self, fn_name: &str, args: &str) -> EffectClass {
        if fn_name != TOOL_RUN {
            return EffectClass::Read;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(args) else {
            return EffectClass::Read;
        };
        let Some(agent_id) = value.get("agent").and_then(|v| v.as_str()) else {
            return EffectClass::Read;
        };
        self.resolve_agent(agent_id)
            .map(|resolved| resolved.effect())
            .unwrap_or(EffectClass::Read)
    }

    fn permission_spec(&self, fn_name: &str) -> Option<ToolPermissionSpec> {
        (fn_name == TOOL_RUN).then_some(ToolPermissionSpec {
            action: PermissionAction::Subagent,
            resource_arg: Some("agent"),
        })
    }

    async fn invoke(&self, fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
        Ok(self.invoke_structured(fn_name, args).await?.output)
    }

    async fn invoke_structured(&self, fn_name: &str, args: &str) -> Result<ToolRun, ToolError> {
        if fn_name != TOOL_RUN {
            return Err(ToolError::InvalidFunctionName(fn_name.to_string()));
        }

        let parsed: SubagentRunArgs =
            serde_json::from_str(args).map_err(ToolError::WrongArguments)?;
        if parsed.task.trim().is_empty() {
            return Ok(ToolRun::from_output(ToolOutput::Fail(
                "subagent task must not be empty".to_string(),
            )));
        }

        let Some(resolved) = self.resolve_agent(&parsed.agent) else {
            return Ok(ToolRun::from_output(ToolOutput::Fail(format!(
                "unknown subagent '{}'; available agents: {}",
                parsed.agent.trim(),
                self.available_agent_ids().join(", ")
            ))));
        };

        let agent_id = resolved.id().to_string();
        let read_only = resolved.read_only();
        let preset_max_steps = resolved.max_steps();
        let allowed_tools = resolved.allowed_tools();
        let system_prompt = resolved.system_prompt();
        let parent_model = self.current_model();
        let child_model = resolved.model(&parent_model);

        let bridge = SUBAGENT_BRIDGE.try_with(Clone::clone).ok();
        let item_id = ItemId::new();
        if let Some(bridge) = &bridge {
            let _ = bridge.event_tx.send(AgentEvent::ItemStarted {
                item: SessionItem::SubagentRun {
                    id: item_id.clone(),
                    agent: agent_id.clone(),
                    status: ItemStatus::InProgress,
                    report: None,
                },
            });
        }

        let max_steps = bridge.as_ref().map_or(preset_max_steps, |bridge| {
            preset_max_steps.min(bridge.parent_harness_config.max_steps_per_turn)
        });
        let child_tools = self.build_child_tools(read_only, allowed_tools);
        let child_config = MultiTurnAgentConfig {
            max_cycles: max_steps,
            ..MultiTurnAgentConfig::default()
        };
        let mut child_agent = MultiTurnAgent::with_streamer(
            self.streamer.clone(),
            child_model.clone(),
            child_tools,
            system_prompt,
            child_config,
        );

        let mut child_session_id: Option<String> = None;
        let mut child_initial_cap: Option<u64> = None;
        if let Some(bridge) = &bridge {
            let persisted = if let Some(parent_session) = &bridge.parent_session {
                create_child_session(
                    parent_session,
                    &child_model,
                    parsed.fork_session.unwrap_or(false),
                )
                .await
            } else {
                None
            };
            let child_session = match persisted {
                Some(session) => {
                    child_session_id = Some(session.id().to_string());
                    session
                }
                None => Session::memory(SessionId::new(), bridge.workspace_root.clone()),
            };
            let policy_mode = if read_only {
                ApprovalMode::Plan
            } else {
                bridge.parent_policy_mode
            };
            let child_policy = PolicyEngine::new(policy_mode)
                .with_workspace_root(bridge.workspace_root.clone())
                .with_rules(bridge.parent_policy_rules.clone());
            for grant in &bridge.parent_policy_grants {
                child_policy.record_grant(grant.clone());
            }
            let mut child_harness_config = bridge.parent_harness_config.clone();
            child_harness_config.max_steps_per_turn = max_steps;
            child_harness_config.max_turn_secs = 0;
            child_agent.set_harness(AgentHarness {
                session: child_session,
                policy: child_policy,
                config: child_harness_config,
            });
            if let Some(cap) = bridge.parent_harness_config.max_session_tokens {
                let child_seeded = child_agent.cumulative_usage_tokens();
                let remaining = cap.saturating_sub(bridge.parent_used_tokens);
                let adjusted = child_seeded.saturating_add(remaining);
                child_initial_cap = Some(adjusted);
                child_agent.set_max_session_tokens(Some(adjusted));
            }
        }

        let (child_event_tx, mut child_event_rx) = mpsc::unbounded_channel::<AgentEvent>();
        let (child_cmd_tx, mut child_cmd_rx) = mpsc::unbounded_channel::<AgentCommand>();
        let child_cancel = bridge
            .as_ref()
            .map(|bridge| bridge.cancel.child_token())
            .unwrap_or_default();

        let mut turn_end_reason: Option<TurnEndReason> = None;
        let child_result = {
            let mut turn_fut = std::pin::pin!(child_agent.prompt_streaming_with_commands(
                &parsed.task,
                &child_event_tx,
                child_cancel,
                &mut child_cmd_rx,
            ));

            loop {
                tokio::select! {
                    res = &mut turn_fut => break res,
                    Some(event) = child_event_rx.recv() => {
                        handle_child_event(
                            event,
                            bridge.as_ref(),
                            &child_cmd_tx,
                            &mut turn_end_reason,
                        );
                    }
                }
            }
        };

        while let Ok(event) = child_event_rx.try_recv() {
            handle_child_event(event, bridge.as_ref(), &child_cmd_tx, &mut turn_end_reason);
        }

        if let Some(bridge) = &bridge
            && let Some(child_harness) = child_agent.harness()
        {
            for grant in child_harness.policy.grants() {
                if !bridge.parent_policy_grants.contains(&grant) {
                    let _ = bridge.grant_tx.send(grant);
                }
            }
            if let Some(initial_cap) = child_initial_cap {
                match child_harness.config.max_session_tokens {
                    None => {
                        let _ = bridge.budget_tx.send(SubagentBudgetUpdate::Unlimited);
                    }
                    Some(final_cap) if final_cap > initial_cap => {
                        let _ = bridge
                            .budget_tx
                            .send(SubagentBudgetUpdate::Increase(final_cap - initial_cap));
                    }
                    Some(_) => {}
                }
            }
        }

        let (final_status, report, output) = match (child_result, turn_end_reason) {
            (Ok(_), Some(TurnEndReason::Aborted { .. })) => {
                let msg = "subagent cancelled".to_string();
                (ItemStatus::Cancelled, msg.clone(), ToolOutput::Fail(msg))
            }
            (Ok(_), Some(TurnEndReason::Failed { error })) => {
                (ItemStatus::Failed, error.clone(), ToolOutput::Fail(error))
            }
            (Ok(text), _) => (ItemStatus::Succeeded, text.clone(), ToolOutput::Text(text)),
            (Err(err), _) => {
                let msg = format!("{err:#}");
                (ItemStatus::Failed, msg.clone(), ToolOutput::Fail(msg))
            }
        };

        if let Some(bridge) = &bridge {
            let _ = bridge.event_tx.send(AgentEvent::ItemCompleted {
                item: SessionItem::SubagentRun {
                    id: item_id,
                    agent: agent_id,
                    status: final_status,
                    report: Some(report),
                },
            });
        }

        let mut run = ToolRun::from_output(output);
        run.observation.hint = child_session_id.map(|id| format!("child_session_id: {id}"));
        Ok(run)
    }
}

fn handle_child_event(
    event: AgentEvent,
    bridge: Option<&SubagentBridge>,
    child_cmd_tx: &mpsc::UnboundedSender<AgentCommand>,
    turn_end_reason: &mut Option<TurnEndReason>,
) {
    match event {
        AgentEvent::UsageUpdated { usage, .. } => {
            if let Some(bridge) = bridge {
                let _ = bridge.usage_tx.send(usage);
            }
        }
        AgentEvent::ApprovalRequested { request } => {
            if let Some(bridge) = bridge {
                let (reply_tx, reply_rx) = oneshot::channel();
                let request_id = request.request_id.clone();
                if bridge
                    .approval_tx
                    .send(ForwardedApproval {
                        request,
                        reply: reply_tx,
                    })
                    .is_ok()
                {
                    let cmd_tx = child_cmd_tx.clone();
                    tokio::spawn(async move {
                        let (decision, scope) =
                            reply_rx.await.unwrap_or((ApprovalDecision::Cancel, None));
                        let _ = cmd_tx.send(AgentCommand::Approve {
                            request_id,
                            decision,
                            scope,
                        });
                    });
                } else {
                    let _ = child_cmd_tx.send(AgentCommand::Approve {
                        request_id,
                        decision: ApprovalDecision::Cancel,
                        scope: None,
                    });
                }
            } else {
                let _ = child_cmd_tx.send(AgentCommand::Approve {
                    request_id: request.request_id,
                    decision: ApprovalDecision::Decline,
                    scope: None,
                });
            }
        }
        AgentEvent::TurnCompleted { reason, .. } => {
            *turn_end_reason = Some(reason);
        }
        _ => {}
    }
}

#[cfg(test)]
mod lenient_tests {
    use super::*;

    #[test]
    fn accepts_string_booleans_for_fork_session() {
        let args: SubagentRunArgs =
            serde_json::from_str(r#"{"agent":"explore","task":"x","fork_session":"true"}"#)
                .unwrap();
        assert_eq!(args.fork_session, Some(true));
    }
}
