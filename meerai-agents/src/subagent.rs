use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use async_trait::async_trait;
use meerai_common::definitions::{AgentDefinition, validate_agent_model};
use meerai_common::hooks::HookEvent;
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
    AgentCommand, AgentEvent, ApprovalDecision, ApprovalRequest, ItemDelta, ItemId, ItemStatus,
    SessionItem, TurnEndReason,
};
use crate::harness::{AgentHarness, HarnessConfig};
use crate::hooks::{HookContext, HookDecision, HookEngine};
use crate::multi_turn_agent::{ChatStreamer, MultiTurnAgent, MultiTurnAgentConfig};

const TOOLSET_NAME: &str = "subagent";
const TOOL_RUN: &str = "subagent-run";
const CUSTOM_AGENT_MAX_STEPS: usize = 16;
const MIN_CHILD_TOKEN_CAP: u64 = 50_000;
pub(crate) const SUBAGENT_TURN_DEADLINE_MARGIN_SECS: u64 = 5;

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

const BUILTIN_AGENTS: [BuiltinAgent; 6] = [
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
    BuiltinAgent {
        id: "research",
        description: "Read-only research subagent that gathers evidence from the workspace and its documents and returns cited findings",
        effect: EffectClass::Read,
        read_only: true,
        max_steps: 12,
        allowed_tools: Some(READ_ONLY_SUBAGENT_TOOLS),
        system_prompt: "You are a read-only research subagent. Answer one narrow question from the workspace and its documents. Use parallel read-only tool calls, cite exact file:start-end spans, separate evidence from inference, and state what you could not determine. Never edit files or run shell commands.",
    },
    BuiltinAgent {
        id: "review",
        description: "Read-only review of an artifact against stated criteria, with findings by severity and path:line evidence",
        effect: EffectClass::Read,
        read_only: true,
        max_steps: 12,
        allowed_tools: Some(READ_ONLY_SUBAGENT_TOOLS),
        system_prompt: "You are a read-only review subagent. Evaluate the named artifact against the stated criteria. Report findings by severity with path:line evidence and why each finding matters. Do not edit, fix, or run shell commands. If the criteria are missing, state the criteria you applied.",
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
    pub(crate) turn_deadline: Option<Instant>,
    pub(crate) parent_session: Option<Session>,
    pub(crate) parent_policy_mode: ApprovalMode,
    pub(crate) parent_policy_rules: Vec<PermissionRule>,
    pub(crate) parent_policy_grants: Vec<Grant>,
    pub(crate) workspace_root: PathBuf,
    pub(crate) parent_harness_config: HarnessConfig,
    pub(crate) parent_used_tokens: u64,
    pub(crate) routing: Option<Arc<crate::routing::RoutingCoordinator>>,
}

tokio::task_local! {
    pub(crate) static SUBAGENT_BRIDGE: SubagentBridge;
}

#[derive(Debug, Deserialize)]
struct SubagentRunArgs {
    agent: String,
    task: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, deserialize_with = "meerai_core::lenient::option_bool_value")]
    fork_session: Option<bool>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    approval_mode: Option<ApprovalMode>,
    #[serde(default)]
    allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    max_steps: Option<usize>,
    #[serde(default)]
    max_tokens: Option<u64>,
    #[serde(default)]
    max_cost_usd: Option<f64>,
    #[serde(default)]
    item_id: Option<ItemId>,
}

impl SubagentRunArgs {
    fn normalized(mut self) -> Self {
        self.name = self
            .name
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty());
        self.model = self.model.filter(|model| !model.trim().is_empty());
        self.allowed_tools = self.allowed_tools.filter(|tools| !tools.is_empty());
        self.max_steps = self.max_steps.filter(|steps| *steps > 0);
        self.max_tokens = self.max_tokens.filter(|tokens| *tokens > 0);
        self.max_cost_usd = self
            .max_cost_usd
            .filter(|cost| cost.is_finite() && *cost > 0.0);
        self
    }
}

#[derive(Clone, Debug, Default)]
struct SubagentMonitor {
    item_id: ItemId,
    max_steps: usize,
    max_cost_usd: Option<f64>,
    steps_used: usize,
    usage: Usage,
    active_tool: Option<String>,
    turn_end_reason: Option<TurnEndReason>,
}

impl SubagentMonitor {
    fn new(item_id: ItemId, max_steps: usize, max_cost_usd: Option<f64>) -> Self {
        Self {
            item_id,
            max_steps,
            max_cost_usd,
            ..Self::default()
        }
    }
}

fn estimated_cost(usage: Usage, config: &HarnessConfig) -> Option<f64> {
    let pricing = config.token_pricing?;
    let prompt = usage.prompt_tokens.unwrap_or(0).max(0) as f64;
    let completion = usage.completion_tokens.unwrap_or(0).max(0) as f64;
    Some(
        prompt / 1_000_000.0 * pricing.input_per_mtok
            + completion / 1_000_000.0 * pricing.output_per_mtok,
    )
}

fn clamp_mode(
    requested: Option<ApprovalMode>,
    parent: ApprovalMode,
    read_only: bool,
) -> ApprovalMode {
    if read_only {
        return ApprovalMode::Plan;
    }
    let rank = |mode: ApprovalMode| match mode {
        ApprovalMode::Plan => 0,
        ApprovalMode::Ask => 1,
        ApprovalMode::Auto => 2,
    };
    match requested {
        Some(requested) if rank(requested) < rank(parent) => requested,
        _ => parent,
    }
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
            Self::Custom(_) => EffectClass::Write,
        }
    }

    fn read_only(&self) -> bool {
        match self {
            Self::Builtin(agent) => agent.read_only,
            Self::Custom(_) => false,
        }
    }

    fn agent_rules(&self) -> Vec<PermissionRule> {
        match self {
            Self::Builtin(_) => Vec::new(),
            Self::Custom(agent) => agent.permissions.clone(),
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

fn intersect_allowed_tools(
    preset: Option<Vec<String>>,
    requested: Option<Vec<String>>,
) -> Option<Vec<String>> {
    match (preset, requested) {
        (Some(preset), Some(requested)) => Some(
            preset
                .into_iter()
                .filter(|tool| requested.iter().any(|requested| requested == tool))
                .collect(),
        ),
        (Some(preset), None) => Some(preset),
        (None, requested) => requested,
    }
}

fn subagent_agents(agents: Vec<AgentDefinition>) -> Vec<AgentDefinition> {
    agents
        .into_iter()
        .filter(|agent| agent.mode.runnable_as_subagent())
        .filter(|agent| find_builtin_agent(&agent.id).is_none())
        .collect()
}

fn subagent_task_type(agent_id: &str) -> meerai_common::settings::TaskType {
    use meerai_common::settings::TaskType;
    match agent_id {
        "explore" => TaskType::Explore,
        "plan" => TaskType::Plan,
        "build" => TaskType::Build,
        "review" => TaskType::Review,
        _ => TaskType::Default,
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
    allowed_tools: Option<Vec<String>>,
}

impl ChildToolsetFilter {
    fn allows(&self, fn_name: &str) -> bool {
        !fn_name.starts_with("subagent-")
            && self.inner.contain(fn_name)
            && (!self.read_only || self.inner.effect_of(fn_name) == EffectClass::Read)
            && self
                .allowed_tools
                .as_ref()
                .is_none_or(|allowed| allowed.iter().any(|name| name == fn_name))
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
    hooks: Option<Arc<HookEngine>>,
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
            hooks: None,
        }
    }

    pub fn from_agent(agent: &MultiTurnAgent, tool_factory: SubagentToolFactory) -> Self {
        Self::new(agent.streamer(), agent.model_handle(), tool_factory)
    }

    pub fn with_custom_agents(mut self, agents: Vec<AgentDefinition>) -> Self {
        self.custom_agents = subagent_agents(agents);
        self
    }

    pub fn with_hooks(mut self, hooks: Arc<HookEngine>) -> Self {
        self.hooks = Some(hooks);
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
        allowed_tools: Option<Vec<String>>,
    ) -> Vec<Box<dyn Toolset>> {
        (self.tool_factory)(read_only)
            .into_iter()
            .filter(|tool| tool.name() != TOOLSET_NAME)
            .map(|tool| {
                Box::new(ChildToolsetFilter {
                    inner: tool,
                    read_only,
                    allowed_tools: allowed_tools.clone(),
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
        let mut ids: Vec<&str> = BUILTIN_AGENTS.iter().map(|agent| agent.id).collect();
        ids.extend(self.custom_agents.iter().map(|agent| agent.id.as_str()));
        format!(
            "Run an isolated subagent for a focused task. Available agents: {}.",
            ids.join(", ")
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

fn child_session_token_cap(
    parent_cap: Option<u64>,
    parent_used: u64,
    seeded: u64,
    requested: Option<u64>,
) -> Option<u64> {
    let requested = requested.map(|tokens| tokens.max(MIN_CHILD_TOKEN_CAP));
    match parent_cap {
        Some(cap) => {
            let remaining = cap.saturating_sub(parent_used);
            Some(seeded.saturating_add(remaining.min(requested.unwrap_or(u64::MAX))))
        }
        None => requested.map(|tokens| seeded.saturating_add(tokens)),
    }
}

pub(crate) fn child_turn_secs(deadline: Option<Instant>, now: Instant) -> u64 {
    let Some(deadline) = deadline else {
        return 0;
    };
    deadline
        .saturating_duration_since(now)
        .as_secs()
        .saturating_sub(SUBAGENT_TURN_DEADLINE_MARGIN_SECS)
        .max(1)
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
                    "name": {
                        "type": "string",
                        "description": "Optional short display name for this run, shown in the agents list and transcript"
                    },
                    "fork_session": {
                        "type": "boolean",
                        "description": "Fork the parent session transcript instead of starting a fresh child session (default: false)"
                    },
                    "model": {
                        "type": "string",
                        "description": "Optional model override for the child; it stays on the parent provider"
                    },
                    "approval_mode": {
                        "type": "string",
                        "enum": ["plan", "ask", "auto"],
                        "description": "Optional child approval mode; it is clamped to the parent mode and plan mode stays read-only"
                    },
                    "allowed_tools": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Optional tool name subset for the child; it can only narrow the agent preset. Omit it or send an empty list to keep the preset"
                    },
                    "max_steps": {
                        "type": "integer",
                        "description": "Optional step cap for the child; it is clamped to the preset and the parent turn cap. Omit it or send 0 to use the preset"
                    },
                    "max_tokens": {
                        "type": "integer",
                        "description": "Optional child session token cap, at least 50000 and clamped to the parent remaining budget. Omit it or send 0 for no cap"
                    },
                    "max_cost_usd": {
                        "type": "number",
                        "description": "Optional child cost cap in USD; the child is cancelled when it is exceeded. Omit it or send 0 for no cap"
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

        let parsed = serde_json::from_str::<SubagentRunArgs>(args)
            .map_err(ToolError::WrongArguments)?
            .normalized();
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
        let preset_tools: Option<Vec<String>> = resolved
            .allowed_tools()
            .map(|tools| tools.iter().map(|tool| tool.to_string()).collect());
        let allowed_tools = intersect_allowed_tools(preset_tools, parsed.allowed_tools.clone());
        let system_prompt = resolved.system_prompt();
        let parent_model = self.current_model();
        let mut child_model = resolved.model(&parent_model);

        let bridge = SUBAGENT_BRIDGE.try_with(Clone::clone).ok();
        if let Some(bridge) = &bridge
            && let Some(route) = bridge
                .parent_harness_config
                .routing
                .route(&subagent_task_type(&agent_id))
        {
            child_model = route.model_spec();
        }
        if let Some(requested) = parsed.model.as_deref().map(str::trim)
            && !requested.is_empty()
            && !requested.contains(':')
        {
            child_model = apply_child_model(&parent_model, requested);
        }

        if let Some(hooks) = &self.hooks
            && let Some(bridge) = &bridge
            && let Some(parent_session) = &bridge.parent_session
        {
            let context = HookContext {
                session_id: parent_session.id().as_str(),
                turn_id: None,
                tool_name: Some(TOOL_RUN),
                tool_input: Some(args),
                tool_output: None,
                prompt: Some(&parsed.task),
                error: None,
                cwd: &bridge.workspace_root,
            };
            let report = hooks.run(HookEvent::SubagentStart, &context).await;
            for firing in &report.firings {
                let _ = parent_session
                    .append(firing.clone().into_session_event())
                    .await;
            }
            if matches!(report.decision, HookDecision::Deny | HookDecision::Block) {
                let reason = report.reason().unwrap_or("blocked by hook").to_string();
                return Ok(ToolRun::from_output(ToolOutput::Fail(reason)));
            }
        }

        let item_id = parsed.item_id.clone().unwrap_or_default();
        let requested_steps = parsed.max_steps.unwrap_or(preset_max_steps).max(1);
        let max_steps = bridge.as_ref().map_or(requested_steps, |bridge| {
            requested_steps.min(bridge.parent_harness_config.max_steps_per_turn)
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
        child_agent.set_task_type(subagent_task_type(&agent_id));
        if let Some(hooks) = &self.hooks {
            child_agent.set_hooks(hooks.clone());
        }

        let mut child_mode = if read_only {
            ApprovalMode::Plan
        } else {
            bridge
                .as_ref()
                .map_or(ApprovalMode::Ask, |bridge| bridge.parent_policy_mode)
        };
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
            child_mode = clamp_mode(parsed.approval_mode, bridge.parent_policy_mode, read_only);
            let child_policy = PolicyEngine::new(child_mode)
                .with_workspace_root(bridge.workspace_root.clone())
                .with_rules(bridge.parent_policy_rules.clone())
                .with_agent_rules(resolved.agent_rules());
            for grant in &bridge.parent_policy_grants {
                child_policy.record_grant(grant.clone());
            }
            let mut child_harness_config = bridge.parent_harness_config.clone();
            child_harness_config.max_steps_per_turn = max_steps;
            child_harness_config.step_limit_continuations = 0;
            child_harness_config.max_turn_secs =
                child_turn_secs(bridge.turn_deadline, Instant::now());
            child_agent.set_harness(AgentHarness {
                session: child_session,
                policy: child_policy,
                config: child_harness_config,
                routing: bridge.routing.clone(),
            });
            if let Some(cap) = child_session_token_cap(
                bridge.parent_harness_config.max_session_tokens,
                bridge.parent_used_tokens,
                child_agent.cumulative_usage_tokens(),
                parsed.max_tokens,
            ) {
                child_initial_cap = Some(cap);
                child_agent.set_max_session_tokens(Some(cap));
            }
        }

        if let Some(bridge) = &bridge {
            let _ = bridge.event_tx.send(AgentEvent::ItemStarted {
                item: SessionItem::SubagentRun {
                    id: item_id.clone(),
                    name: parsed.name.clone(),
                    agent: agent_id.clone(),
                    status: ItemStatus::InProgress,
                    report: None,
                    child_session_id: child_session_id.clone(),
                    model: Some(child_model.clone()),
                    approval_mode: child_mode,
                    steps_used: 0,
                    max_steps,
                    usage: Usage::default(),
                    est_cost_usd: None,
                    active_tool: None,
                },
            });
        }

        let (child_event_tx, mut child_event_rx) = mpsc::unbounded_channel::<AgentEvent>();
        let (child_cmd_tx, mut child_cmd_rx) = mpsc::unbounded_channel::<AgentCommand>();
        let child_cancel = bridge
            .as_ref()
            .map(|bridge| bridge.cancel.child_token())
            .unwrap_or_default();

        let max_cost_usd = parsed.max_cost_usd;
        let mut monitor = SubagentMonitor::new(item_id.clone(), max_steps, max_cost_usd);
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
                            &mut monitor,
                        );
                    }
                }
            }
        };

        while let Ok(event) = child_event_rx.try_recv() {
            handle_child_event(event, bridge.as_ref(), &child_cmd_tx, &mut monitor);
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

        let (final_status, report, output) = match (child_result, monitor.turn_end_reason.take()) {
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

        if let Some(hooks) = &self.hooks
            && let Some(bridge) = &bridge
            && let Some(parent_session) = &bridge.parent_session
        {
            let context = HookContext {
                session_id: parent_session.id().as_str(),
                turn_id: None,
                tool_name: Some(TOOL_RUN),
                tool_input: Some(args),
                tool_output: Some(&report),
                prompt: Some(&parsed.task),
                error: None,
                cwd: &bridge.workspace_root,
            };
            let stop_report = hooks.run(HookEvent::SubagentStop, &context).await;
            for firing in &stop_report.firings {
                let _ = parent_session
                    .append(firing.clone().into_session_event())
                    .await;
            }
        }

        if let Some(bridge) = &bridge {
            let _ = bridge.event_tx.send(AgentEvent::ItemCompleted {
                item: SessionItem::SubagentRun {
                    id: item_id,
                    name: parsed.name.clone(),
                    agent: agent_id,
                    status: final_status,
                    report: Some(report),
                    child_session_id: child_session_id.clone(),
                    model: Some(child_model),
                    approval_mode: child_mode,
                    steps_used: 0,
                    max_steps,
                    usage: child_agent.cumulative_usage,
                    est_cost_usd: child_agent.estimated_cost(child_agent.cumulative_usage),
                    active_tool: None,
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
    monitor: &mut SubagentMonitor,
) {
    match event {
        AgentEvent::UsageUpdated {
            usage, cumulative, ..
        } => {
            monitor.steps_used = monitor.steps_used.saturating_add(1);
            monitor.usage = cumulative;
            if let Some(bridge) = bridge {
                let _ = bridge.usage_tx.send(usage);
                if emit_subagent_progress(bridge, monitor) {
                    let _ = child_cmd_tx.send(AgentCommand::Cancel);
                }
            }
        }
        AgentEvent::ItemStarted {
            item: SessionItem::ToolCall { name, .. },
        } => {
            monitor.active_tool = Some(name);
            if let Some(bridge) = bridge {
                emit_subagent_progress(bridge, monitor);
            }
        }
        AgentEvent::ItemCompleted {
            item: SessionItem::ToolCall { .. },
        } => {
            monitor.active_tool = None;
            if let Some(bridge) = bridge {
                emit_subagent_progress(bridge, monitor);
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
            monitor.turn_end_reason = Some(reason);
        }
        _ => {}
    }
}

fn emit_subagent_progress(bridge: &SubagentBridge, monitor: &SubagentMonitor) -> bool {
    let est_cost_usd = estimated_cost(monitor.usage, &bridge.parent_harness_config);
    let _ = bridge.event_tx.send(AgentEvent::ItemDelta {
        item_id: monitor.item_id.clone(),
        delta: ItemDelta::SubagentProgress {
            steps_used: monitor.steps_used,
            max_steps: monitor.max_steps,
            usage: monitor.usage,
            est_cost_usd,
            active_tool: monitor.active_tool.clone(),
        },
    });
    match (monitor.max_cost_usd, est_cost_usd) {
        (Some(cap), Some(cost)) => cost > cap,
        _ => false,
    }
}

#[cfg(test)]
mod lenient_tests {
    use super::*;

    #[test]
    fn plan_mode_clamps_child_approval_modes() {
        assert_eq!(
            clamp_mode(Some(ApprovalMode::Auto), ApprovalMode::Plan, false),
            ApprovalMode::Plan
        );
        assert_eq!(
            clamp_mode(Some(ApprovalMode::Auto), ApprovalMode::Ask, false),
            ApprovalMode::Ask
        );
        assert_eq!(
            clamp_mode(Some(ApprovalMode::Ask), ApprovalMode::Auto, false),
            ApprovalMode::Ask
        );
        assert_eq!(
            clamp_mode(Some(ApprovalMode::Auto), ApprovalMode::Auto, true),
            ApprovalMode::Plan
        );
    }

    #[test]
    fn requested_tools_only_narrow_the_preset() {
        let preset = Some(vec!["code-read_file".to_string(), "shell".to_string()]);
        let requested = Some(vec!["shell".to_string(), "code-write_file".to_string()]);
        assert_eq!(
            intersect_allowed_tools(preset.clone(), requested),
            Some(vec!["shell".to_string()])
        );
        assert_eq!(
            intersect_allowed_tools(preset, None),
            Some(vec!["code-read_file".to_string(), "shell".to_string()])
        );
        assert_eq!(
            intersect_allowed_tools(None, Some(vec!["shell".to_string()])),
            Some(vec!["shell".to_string()])
        );
        assert_eq!(intersect_allowed_tools(None, None), None);
    }

    #[test]
    fn zero_and_empty_optional_limits_mean_unset() {
        let args = serde_json::from_str::<SubagentRunArgs>(
            r#"{"agent":"explore","task":"x","model":"  ","allowed_tools":[],"max_steps":0,"max_tokens":0,"max_cost_usd":0}"#,
        )
        .unwrap()
        .normalized();
        assert_eq!(args.model, None);
        assert_eq!(args.allowed_tools, None);
        assert_eq!(args.max_steps, None);
        assert_eq!(args.max_tokens, None);
        assert_eq!(args.max_cost_usd, None);
    }

    #[test]
    fn positive_optional_limits_are_kept() {
        let args = serde_json::from_str::<SubagentRunArgs>(
            r#"{"agent":"explore","task":"x","model":"gpt-6","allowed_tools":["code-grep"],"max_steps":12,"max_tokens":4000,"max_cost_usd":1.5}"#,
        )
        .unwrap()
        .normalized();
        assert_eq!(args.model.as_deref(), Some("gpt-6"));
        assert_eq!(args.allowed_tools, Some(vec!["code-grep".to_string()]));
        assert_eq!(args.max_steps, Some(12));
        assert_eq!(args.max_tokens, Some(4000));
        assert_eq!(args.max_cost_usd, Some(1.5));
    }

    #[test]
    fn child_token_cap_floors_small_requests_and_respects_the_parent() {
        assert_eq!(
            child_session_token_cap(None, 0, 0, Some(4_500)),
            Some(MIN_CHILD_TOKEN_CAP)
        );
        assert_eq!(child_session_token_cap(None, 0, 0, None), None);
        assert_eq!(
            child_session_token_cap(Some(80_000), 10_000, 0, Some(4_500)),
            Some(MIN_CHILD_TOKEN_CAP)
        );
        assert_eq!(
            child_session_token_cap(Some(20_000), 5_000, 0, Some(4_500)),
            Some(15_000)
        );
        assert_eq!(
            child_session_token_cap(None, 0, 1_000, Some(60_000)),
            Some(61_000)
        );
    }

    #[test]
    fn task_types_follow_the_agent_id() {
        use meerai_common::settings::TaskType;
        assert_eq!(subagent_task_type("explore"), TaskType::Explore);
        assert_eq!(subagent_task_type("plan"), TaskType::Plan);
        assert_eq!(subagent_task_type("build"), TaskType::Build);
        assert_eq!(subagent_task_type("review"), TaskType::Review);
        assert_eq!(subagent_task_type("general"), TaskType::Default);
    }

    #[test]
    fn accepts_string_booleans_for_fork_session() {
        let args: SubagentRunArgs =
            serde_json::from_str(r#"{"agent":"explore","task":"x","fork_session":"true"}"#)
                .unwrap();
        assert_eq!(args.fork_session, Some(true));
    }

    fn definition(id: &str, mode: meerai_common::definitions::AgentMode) -> AgentDefinition {
        use meerai_common::definitions::DefinitionSource;

        AgentDefinition {
            id: id.to_string(),
            source: DefinitionSource::Meerai,
            description: None,
            model: None,
            mode,
            steps: None,
            permissions: Vec::new(),
            prompt: String::new(),
            path: PathBuf::from(format!(".meerai/agents/{id}.md")),
            token_estimate: 0,
            warnings: Vec::new(),
            supporting_files: Vec::new(),
        }
    }

    #[test]
    fn only_subagent_mode_definitions_run_as_subagents() {
        use meerai_common::definitions::AgentMode;

        let agents = subagent_agents(vec![
            definition("all", AgentMode::All),
            definition("sub", AgentMode::Subagent),
            definition("primary", AgentMode::Primary),
            definition("explore", AgentMode::All),
        ]);

        let ids: Vec<&str> = agents.iter().map(|agent| agent.id.as_str()).collect();
        assert_eq!(ids, ["all", "sub"]);
    }

    #[test]
    fn custom_agents_are_writable_and_keep_their_rules() {
        use meerai_common::definitions::AgentMode;

        let mut agent = definition("implementer", AgentMode::Subagent);
        agent.permissions = vec![PermissionRule {
            action: meerai_policy::ActionSelector::Action(PermissionAction::Edit),
            resource: "src/generated/**".to_string(),
            effect: meerai_policy::Effect::Deny,
        }];
        let resolved = ResolvedAgent::Custom(&agent);

        assert!(!resolved.read_only());
        assert_eq!(resolved.effect(), EffectClass::Write);
        assert_eq!(resolved.agent_rules(), agent.permissions);
        assert!(
            ResolvedAgent::Builtin(find_builtin_agent("explore").unwrap())
                .agent_rules()
                .is_empty()
        );
    }
}
