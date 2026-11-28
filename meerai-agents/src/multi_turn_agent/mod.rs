use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::harness::HarnessConfig;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use futures::future::join_all;
use futures::{Stream, StreamExt};
use genai::Headers;
use genai::chat::{
    CacheControl, ChatMessage, ChatOptions, ChatRequest, ChatRole, ChatStreamEvent, StopReason,
    StreamEnd, Tool, ToolCall, ToolResponse,
};
use meerai_common::hooks::HookEvent;
use meerai_common::settings::{ContextSettings, EffortLevel};
use meerai_core::env::{MEERAI_USER_AGENT, generate_opencode_session_id, resolve_lexical_subpath};
use meerai_core::text::floor_char_boundary;
use meerai_core::{
    EffectClass, ObservationStatus, ToolObservation, ToolOutput, ToolPermissionSpec, ToolRun,
    Toolset,
};
use meerai_policy::{
    ApprovalMode, Decision, PermissionAction, PermissionCheck, PermissionRule, PolicyEngine,
    approval_preview_for,
};
use meerai_session::{
    ApprovalPreview, BreakerAction, BreakerKind, CompactionId, CompactionResult, CompactionTrigger,
    ContentBlock, ContextBreakdown, InputSource, ProjectionPolicy, PromptSection, SessionEvent,
    StepFinish, StepId, StreamDigest, Usage, apply_spill, context_breakdown, derive_messages,
    estimate_tokens, has_open_compaction, plan_compaction_boundary, summary_message,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::events::{
    AbortCause, AgentCommand, AgentEvent, ApprovalDecision, ApprovalKind, ApprovalRequest, CallId,
    ErrorCode, GrantScope, ItemDelta, ItemId, ItemStatus, QuestionRequest, RequestId, RewindScope,
    SessionItem, SubagentSpec, TurnEndReason, TurnId, TurnTrigger,
};
use crate::harness::AgentHarness;
use crate::hooks::{HookArgs, HookContext, HookEngine, HookReport};
use crate::intercept::{
    SharedInterceptor, StepAction, StepPlan, ToolContext, ToolOutcome, ToolPlan, TurnStopAction,
};
use crate::redact::{redact_run, redact_text};
use crate::subagent::{
    SUBAGENT_BRIDGE, SubagentBridge, SubagentBudgetUpdate, SubagentToolFactory, SubagentToolset,
};

const STEP_LIMIT_NOTICE: &str =
    "Step limit reached. Provide a final summary of what was done and what remains.";
const STEP_CONTINUE_NOTICE: &str = "Step budget reached. Continue now and finish the task: make \
    the concrete change, run the check, and report the result. Do not summarize or list next steps.";

const SUMMARY_INSTRUCTION: &str = "Summarize the transcript below as a compact state block. Use \
    exactly these headings, each followed by terse bullets: Goal, Completed progress, Modified \
    files, Key decisions, Open blockers. Do not add other headings, do not invent facts, and do \
    not call tools.";

const SUMMARY_MESSAGE_TOKEN_LIMIT: u64 = 2_000;

const COMPACTION_EMPTY_SKIP: &str = "no compactable range";
const COMPACTION_EMPTY_SUMMARY: &str = "compaction summary was empty";
const TOKEN_BUDGET_DECLINED: &str = "session token budget declined";
const SPEND_CAP_DECLINED: &str = "session spend cap declined";
const LOOP_DETECTED: &str = "loop detected: repeated identical tool failures";
const NO_APPROVAL_CHANNEL: &str = "no approval channel available; call declined";
const USER_DECLINED: &str = "declined by user";
const NOT_EXECUTED_STOPPED: &str = "not executed: turn stopped";
const EMPTY_STREAM_ERROR: &str =
    "provider returned an empty stream: no text, no tool calls, no usage";

const MAX_PROVIDER_ATTEMPTS: usize = 3;
const PROVIDER_BACKOFF_MS: u64 = 250;
const MAX_PRESSURE_COMPACTIONS_PER_TURN: usize = 2;
const MAX_RECOVERY_ATTEMPTS: usize = 1;
const NO_PROGRESS_FAIL_STEPS: usize = 12;
const NO_PROGRESS_NOTICE: &str = "No file change in the last few steps. Make a concrete edit, run a check, or end the turn with what you have.";

const STREAM_HEADER_TIMEOUT: Duration = Duration::from_secs(30);
const STREAM_READ_TIMEOUT: Duration = Duration::from_secs(120);

const MAX_SCOPED_INSTRUCTION_FILES: usize = 8;
const MAX_SCOPED_INSTRUCTION_BYTES: usize = 16 * 1024;
const MAX_TITLE_CHARS: usize = 80;

const OVERFLOW_MARKERS: [&str; 5] = [
    "context length",
    "context window",
    "maximum context",
    "too many tokens",
    "reduce the length",
];

const NON_TRANSIENT_ERROR_MARKERS: [&str; 7] = [
    "api key",
    "unauthorized",
    "401",
    "403",
    "invalid model",
    "not found",
    "permission denied",
];

const PLAN_USAGE_TERMINAL_CODES: [&str; 7] = [
    "subscription_sharing_usage_limit_exceeded",
    "subscription_sharing_user_not_eligible",
    "subscription_sharing_unsupported_capability",
    "subscription_sharing_route_not_supported",
    "subscription_sharing_invalid_user",
    "chatpass_v2_scope_not_authorized",
    "chatpass_v2_invalid_authorization_context",
];

mod compaction;
mod instructions;
mod tools;
mod turn;
mod worker;

#[cfg(test)]
mod tests;

use tools::FailureTracker;
pub use worker::spawn_worker;
pub(super) fn turn_deadline_exceeded(started: Instant, max_turn_secs: u64, now: Instant) -> bool {
    max_turn_secs != 0
        && now.saturating_duration_since(started) >= Duration::from_secs(max_turn_secs)
}

pub(super) fn remaining_turn_budget(started: Instant, max_turn_secs: u64) -> Option<Duration> {
    if max_turn_secs == 0 {
        return None;
    }
    Duration::from_secs(max_turn_secs).checked_sub(started.elapsed())
}

#[derive(Clone, Copy)]
pub(super) struct TurnClock {
    pub(super) started: Instant,
    pub(super) max_secs: u64,
}

impl TurnClock {
    pub(super) fn remaining(&self) -> Option<Duration> {
        remaining_turn_budget(self.started, self.max_secs)
    }

    pub(super) fn error(&self) -> TurnError {
        TurnError::budget(anyhow!("turn exceeded {}s", self.max_secs))
    }
}

pub(super) fn tool_budget_exceeded(max_secs: u64) -> ToolRun {
    ToolRun::from_output(ToolOutput::Fail(format!(
        "tool timed out after turn exceeded {max_secs}s"
    )))
}

pub(super) struct CollectedStream {
    end: StreamEnd,
    pub(super) text: String,
    pub(super) reasoning: String,
    pub(super) tool_calls: Vec<ToolCall>,
    text_item_id: Option<ItemId>,
    reasoning_item_id: Option<ItemId>,
}

pub(super) enum StreamFailure {
    Cancelled,
    Provider {
        error: anyhow::Error,
        items: Vec<ItemId>,
    },
    EmptyStream {
        detail: String,
        retryable: bool,
    },
}

pub(super) enum BudgetOutcome {
    Continue,
    Declined { detail: String },
    Cancelled,
}

pub type BoxedChatStream = Pin<Box<dyn Stream<Item = Result<ChatStreamEvent>> + Send>>;

#[async_trait]
pub trait ChatStreamer: Send + Sync {
    async fn stream_chat(
        &self,
        provider: Option<&str>,
        model: &str,
        request: ChatRequest,
        options: &ChatOptions,
    ) -> Result<BoxedChatStream>;
}

pub(super) fn start_text_item(event_tx: &mpsc::UnboundedSender<AgentEvent>) -> ItemId {
    let id = ItemId::new();
    let _ = event_tx.send(AgentEvent::ItemStarted {
        item: SessionItem::AgentMessage {
            id: id.clone(),
            text: String::new(),
        },
    });
    id
}

pub(super) fn discard_partial_items(
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
    items: &[ItemId],
) {
    for item_id in items {
        let _ = event_tx.send(AgentEvent::ItemDiscarded {
            item_id: item_id.clone(),
        });
    }
}

pub(super) fn start_reasoning_item(event_tx: &mpsc::UnboundedSender<AgentEvent>) -> ItemId {
    let id = ItemId::new();
    let _ = event_tx.send(AgentEvent::ItemStarted {
        item: SessionItem::Reasoning {
            id: id.clone(),
            summary: String::new(),
        },
    });
    id
}

pub(super) fn aborted(
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
    turn_id: &TurnId,
) -> (String, TurnEndReason) {
    let reason = TurnEndReason::Aborted {
        cause: AbortCause::User,
    };
    let _ = event_tx.send(AgentEvent::TurnCompleted {
        turn_id: turn_id.clone(),
        reason: reason.clone(),
    });
    (String::new(), reason)
}

pub(super) fn completed(
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
    turn_id: &TurnId,
    text: String,
) -> (String, TurnEndReason) {
    let reason = TurnEndReason::Completed;
    let _ = event_tx.send(AgentEvent::TurnCompleted {
        turn_id: turn_id.clone(),
        reason: reason.clone(),
    });
    (text, reason)
}

pub(super) fn failed(
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
    turn_id: &TurnId,
    error: String,
) -> (String, TurnEndReason) {
    let reason = TurnEndReason::Failed { error };
    let _ = event_tx.send(AgentEvent::TurnCompleted {
        turn_id: turn_id.clone(),
        reason: reason.clone(),
    });
    (String::new(), reason)
}

pub(super) fn tool_call_item(
    id: &ItemId,
    tool_call: &ToolCall,
    args: &str,
    effect: EffectClass,
    status: ItemStatus,
    output: Option<String>,
) -> SessionItem {
    SessionItem::ToolCall {
        id: id.clone(),
        call_id: CallId::from_raw(tool_call.call_id.clone()),
        name: tool_call.fn_name.clone(),
        args: args.to_string(),
        effect,
        status,
        output,
    }
}

const MEMORY_SECTION_MARKER: &str = "## Project memory";

fn split_memory_prompt(prompt: &str) -> (String, String) {
    match prompt.find(MEMORY_SECTION_MARKER) {
        Some(index) => (prompt[..index].to_string(), prompt[index..].to_string()),
        None => (prompt.to_string(), String::new()),
    }
}

pub(super) fn provider_of(model: &str) -> String {
    match model.split_once(':') {
        Some((provider, _)) if !provider.is_empty() => provider.to_string(),
        _ => "unknown".to_string(),
    }
}

#[derive(Debug)]
pub(super) struct TurnError {
    pub(super) code: ErrorCode,
    pub(super) source: anyhow::Error,
    pub(super) retryable: bool,
}

impl TurnError {
    pub(super) fn provider(source: anyhow::Error) -> Self {
        Self {
            code: ErrorCode::Provider,
            source,
            retryable: false,
        }
    }

    pub(super) fn transient_provider(source: anyhow::Error) -> Self {
        Self {
            code: ErrorCode::Provider,
            source,
            retryable: true,
        }
    }

    pub(super) fn budget(source: anyhow::Error) -> Self {
        Self {
            code: ErrorCode::Budget,
            source,
            retryable: false,
        }
    }

    pub(super) fn context_overflow(source: anyhow::Error) -> Self {
        Self {
            code: ErrorCode::ContextOverflow,
            source,
            retryable: false,
        }
    }

    pub(super) fn loop_detected(source: anyhow::Error) -> Self {
        Self {
            code: ErrorCode::LoopDetected,
            source,
            retryable: false,
        }
    }

    pub(super) fn config(source: anyhow::Error) -> Self {
        Self {
            code: ErrorCode::Config,
            source,
            retryable: false,
        }
    }

    pub(super) fn message(&self) -> String {
        format!("{:#}", self.source)
    }
}

impl std::fmt::Display for TurnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#}", self.source)
    }
}

impl std::error::Error for TurnError {}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct PendingCompaction {
    pub(super) focus: Option<String>,
    pub(super) from_turn_id: Option<TurnId>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ProjectionOverrides {
    pub(super) verbatim_window_turns: Option<usize>,
    pub(super) reasoning_pruning: Option<bool>,
    pub(super) clear_old_tool_results: Option<bool>,
    pub(super) pinned_turns: Option<Vec<TurnId>>,
}

#[derive(Debug, Clone)]
pub struct MultiTurnAgentConfig {
    pub max_cycles: usize,
    pub max_retries: usize,
}

impl Default for MultiTurnAgentConfig {
    fn default() -> Self {
        Self {
            max_cycles: 16,
            max_retries: 3,
        }
    }
}

pub(super) enum StepFailure {
    Cancelled,
    Error(TurnError),
}

struct ProjectionCache {
    event_count: usize,
    policy: ProjectionPolicy,
    messages: Vec<ChatMessage>,
}

pub struct MultiTurnAgent {
    pub(super) streamer: Arc<dyn ChatStreamer>,
    pub(super) model: String,
    pub(super) provider: Option<String>,
    pub(super) shared_model: Arc<std::sync::RwLock<String>>,
    pub(super) session_id: String,
    pub(super) chat_history: Vec<ChatMessage>,
    pub(super) tools: Vec<Box<dyn Toolset>>,
    pub(super) system_prompt: String,
    pub(super) config: MultiTurnAgentConfig,
    pub(super) cumulative_usage: Usage,
    pub(super) harness: Option<AgentHarness>,
    pub(super) system_prompt_generation: u64,
    pub(super) logged_system_prompt: Option<String>,
    pub(super) tool_schema_tokens: u64,
    pub(super) file_changes_this_step: usize,
    projection_cache: Option<ProjectionCache>,
    pub(super) pressure_wasted: bool,
    pub(super) pending_steers: VecDeque<String>,
    pub(super) pending_compact: Option<PendingCompaction>,
    pub(in crate::multi_turn_agent) failure_tracker: FailureTracker,
    pub(super) injected_instruction_dirs: HashSet<PathBuf>,
    pub(super) outside_grants: HashSet<PathBuf>,
    pub(super) sandbox_escape: Option<GrantScope>,
    pub(super) interceptors: Vec<SharedInterceptor>,
    pub(super) question_cancels: usize,
    pub(super) hooks: Arc<HookEngine>,
    pub(super) post_edit_diagnostics: bool,
    pub(super) session_started: bool,
    pub(super) projection_overrides: ProjectionOverrides,
    pub(super) task_type: meerai_common::settings::TaskType,
    pub(super) subagent_factory: Option<SubagentToolFactory>,
    pub(super) subagent_agents: Vec<meerai_common::definitions::AgentDefinition>,
    pub(super) active_subagents: HashMap<ItemId, CancellationToken>,
    pub(super) memory_capture: Option<crate::memory::MemoryCapture>,
    pub(super) loop_state: Option<meerai_skills::LoopHandle>,
    pub(super) loop_auto_continue: bool,
    pub(super) auto_approved_notified: bool,
}

impl std::fmt::Debug for MultiTurnAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultiTurnAgent")
            .field("model", &self.model)
            .field("session_id", &self.session_id)
            .field("history_len", &self.chat_history.len())
            .field("tools_len", &self.tools.len())
            .field("config", &self.config)
            .field("harnessed", &self.harness.is_some())
            .finish()
    }
}

impl MultiTurnAgent {
    pub fn with_streamer(
        streamer: Arc<dyn ChatStreamer>,
        model: impl Into<String>,
        tools: Vec<Box<dyn Toolset>>,
        system_prompt: impl Into<String>,
        config: MultiTurnAgentConfig,
    ) -> Self {
        let model = model.into();
        let shared_model = Arc::new(std::sync::RwLock::new(model.clone()));
        Self {
            streamer,
            model,
            provider: None,
            shared_model,
            session_id: generate_opencode_session_id(),
            chat_history: Vec::new(),
            tools,
            system_prompt: system_prompt.into(),
            config,
            cumulative_usage: Usage::default(),
            harness: None,
            system_prompt_generation: 0,
            logged_system_prompt: None,
            tool_schema_tokens: 0,
            file_changes_this_step: 0,
            projection_cache: None,
            pressure_wasted: false,
            pending_steers: VecDeque::new(),
            pending_compact: None,
            failure_tracker: FailureTracker::default(),
            injected_instruction_dirs: HashSet::new(),
            outside_grants: HashSet::new(),
            sandbox_escape: None,
            interceptors: vec![Arc::new(crate::checkpoint::CheckpointInterceptor)],
            question_cancels: 0,
            hooks: Arc::new(HookEngine::default()),
            post_edit_diagnostics: false,
            session_started: false,
            projection_overrides: ProjectionOverrides::default(),
            task_type: meerai_common::settings::TaskType::Default,
            subagent_factory: None,
            subagent_agents: Vec::new(),
            active_subagents: HashMap::new(),
            memory_capture: None,
            loop_state: None,
            loop_auto_continue: true,
            auto_approved_notified: false,
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn add_tool(&mut self, tool: impl Toolset + 'static) {
        self.tools.push(Box::new(tool));
    }

    pub fn add_tools(&mut self, tools: Vec<Box<dyn Toolset>>) {
        self.tools.extend(tools);
    }

    pub fn add_interceptor(&mut self, interceptor: impl crate::intercept::Interceptor + 'static) {
        self.interceptors.push(Arc::new(interceptor));
    }

    pub fn add_shared_interceptor(&mut self, interceptor: SharedInterceptor) {
        self.interceptors.push(interceptor);
    }

    pub fn interceptors(&self) -> &[SharedInterceptor] {
        &self.interceptors
    }

    pub fn set_hooks(&mut self, hooks: Arc<HookEngine>) {
        self.hooks = hooks;
    }

    pub fn set_loop_handle(&mut self, state: meerai_skills::LoopHandle) {
        self.loop_state = Some(state);
    }

    pub fn loop_handle(&self) -> Option<&meerai_skills::LoopHandle> {
        self.loop_state.as_ref()
    }

    pub fn set_loop_auto_continue(&mut self, auto: bool) {
        self.loop_auto_continue = auto;
    }

    pub fn loop_auto_continue(&self) -> bool {
        self.loop_auto_continue
    }

    pub fn emit_loop_updated(&self, event_tx: &mpsc::UnboundedSender<AgentEvent>) {
        if let Some(state) = &self.loop_state {
            let state = state.snapshot();
            if state.active() {
                let _ = event_tx.send(AgentEvent::LoopUpdated { state });
            }
        }
    }

    pub fn set_memory_capture(&mut self, capture: crate::memory::MemoryCapture) {
        self.memory_capture = Some(capture);
    }

    pub fn set_task_type(&mut self, task_type: meerai_common::settings::TaskType) {
        self.task_type = task_type;
    }

    pub fn set_subagent_support(
        &mut self,
        factory: SubagentToolFactory,
        custom_agents: Vec<meerai_common::definitions::AgentDefinition>,
    ) {
        self.subagent_factory = Some(factory);
        self.subagent_agents = custom_agents;
    }

    pub fn set_projection_overrides(
        &mut self,
        verbatim_window_turns: Option<usize>,
        reasoning_pruning: Option<bool>,
        clear_old_tool_results: Option<bool>,
        pin_turns: Option<Vec<TurnId>>,
    ) {
        if verbatim_window_turns.is_some() {
            self.projection_overrides.verbatim_window_turns = verbatim_window_turns;
        }
        if reasoning_pruning.is_some() {
            self.projection_overrides.reasoning_pruning = reasoning_pruning;
        }
        if clear_old_tool_results.is_some() {
            self.projection_overrides.clear_old_tool_results = clear_old_tool_results;
        }
        if pin_turns.is_some() {
            self.projection_overrides.pinned_turns = pin_turns;
        }
        self.projection_cache = None;
    }

    pub(crate) async fn apply_projection_overrides(
        &mut self,
        verbatim_window_turns: Option<usize>,
        reasoning_pruning: Option<bool>,
        clear_old_tool_results: Option<bool>,
        pin_turns: Option<Vec<TurnId>>,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) {
        let previous = self
            .projection_overrides
            .pinned_turns
            .clone()
            .unwrap_or_default();
        self.set_projection_overrides(
            verbatim_window_turns,
            reasoning_pruning,
            clear_old_tool_results,
            pin_turns,
        );
        let Some(next) = self.projection_overrides.pinned_turns.clone() else {
            return;
        };
        for turn in previous.iter().filter(|turn| !next.contains(turn)) {
            self.log_event(
                event_tx,
                SessionEvent::ContextTurnPinned {
                    turn_id: turn.clone(),
                    pinned: false,
                },
            )
            .await;
        }
        for turn in next.iter().filter(|turn| !previous.contains(turn)) {
            self.log_event(
                event_tx,
                SessionEvent::ContextTurnPinned {
                    turn_id: turn.clone(),
                    pinned: true,
                },
            )
            .await;
        }
    }

    pub(crate) async fn set_turn_pinned(
        &mut self,
        turn_id: TurnId,
        pinned: bool,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) {
        let mut pinned_turns = self
            .projection_overrides
            .pinned_turns
            .clone()
            .unwrap_or_default();
        if pinned {
            if !pinned_turns.contains(&turn_id) {
                pinned_turns.push(turn_id.clone());
            }
        } else {
            pinned_turns.retain(|turn| turn != &turn_id);
        }
        self.projection_overrides.pinned_turns = Some(pinned_turns);
        self.log_event(
            event_tx,
            SessionEvent::ContextTurnPinned { turn_id, pinned },
        )
        .await;
        self.emit_context_updated(event_tx);
    }

    pub fn cancel_subagent(&mut self, item_id: &ItemId) {
        if let Some(cancel) = self.active_subagents.get(item_id) {
            cancel.cancel();
        }
    }

    pub(crate) async fn spawn_manual_subagent(
        &mut self,
        spec: SubagentSpec,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cmd_rx: &mut mpsc::UnboundedReceiver<AgentCommand>,
        deferred: &mut VecDeque<AgentCommand>,
    ) {
        if spec.task.trim().is_empty() {
            let _ = event_tx.send(AgentEvent::Error {
                code: ErrorCode::Config,
                message: "subagent task must not be empty".to_string(),
            });
            return;
        }
        let Some(factory) = self.subagent_factory.clone() else {
            let _ = event_tx.send(AgentEvent::Error {
                code: ErrorCode::Config,
                message: "manual subagents need an attached subagent toolset".to_string(),
            });
            return;
        };
        let Some(harness) = self.harness.as_ref() else {
            let _ = event_tx.send(AgentEvent::Error {
                code: ErrorCode::Config,
                message: "manual subagents need an active session".to_string(),
            });
            return;
        };

        let item_id = ItemId::new();
        let cancel = CancellationToken::new();
        self.active_subagents
            .insert(item_id.clone(), cancel.clone());

        let (approval_tx, mut approval_rx) = mpsc::unbounded_channel();
        let (usage_tx, mut usage_rx) = mpsc::unbounded_channel();
        let (grant_tx, mut grant_rx) = mpsc::unbounded_channel();
        let (budget_tx, mut budget_rx) = mpsc::unbounded_channel();
        let bridge = SubagentBridge {
            event_tx: event_tx.clone(),
            approval_tx,
            usage_tx,
            grant_tx,
            budget_tx,
            cancel: cancel.clone(),
            turn_deadline: None,
            parent_session: Some(harness.session.clone()),
            parent_policy_mode: harness.policy.mode(),
            parent_policy_rules: harness.policy.rules().to_vec(),
            parent_policy_grants: harness.policy.grants(),
            workspace_root: harness.policy.workspace_root().to_path_buf(),
            parent_harness_config: harness.config.clone(),
            parent_used_tokens: self.cumulative_usage_tokens(),
            routing: harness.routing.clone(),
        };

        let toolset =
            SubagentToolset::new(self.streamer.clone(), self.shared_model.clone(), factory)
                .with_custom_agents(self.subagent_agents.clone())
                .with_hooks(self.hooks.clone());

        let mut args = serde_json::json!({
            "agent": spec.agent,
            "task": spec.task,
            "item_id": item_id.to_string(),
        });
        if let Some(name) = spec.name {
            args["name"] = serde_json::Value::String(name);
        }
        if let Some(model) = spec.model {
            args["model"] = serde_json::Value::String(model);
        }
        if let Some(mode) = spec.approval_mode {
            args["approval_mode"] = serde_json::Value::String(mode.label().to_string());
        }
        if let Some(tools) = spec.allowed_tools {
            args["allowed_tools"] = serde_json::json!(tools);
        }
        if let Some(steps) = spec.max_steps {
            args["max_steps"] = serde_json::json!(steps);
        }
        if let Some(tokens) = spec.max_tokens {
            args["max_tokens"] = serde_json::json!(tokens);
        }
        if let Some(cost) = spec.max_cost_usd {
            args["max_cost_usd"] = serde_json::json!(cost);
        }
        let args = args.to_string();

        let mut run = std::pin::pin!(
            SUBAGENT_BRIDGE.scope(bridge, toolset.invoke_structured("subagent-run", &args))
        );

        loop {
            tokio::select! {
                result = &mut run => {
                    if let Err(error) = result {
                        let _ = event_tx.send(AgentEvent::Error {
                            code: ErrorCode::Tool,
                            message: format!("manual subagent failed: {error:?}"),
                        });
                    }
                    break;
                }
                Some(request) = approval_rx.recv() => {
                    let request_id = request.request.request_id.clone();
                    let _ = event_tx.send(AgentEvent::ApprovalRequested {
                        request: request.request.clone(),
                    });
                    self.log_event(
                        event_tx,
                        SessionEvent::ApprovalRequested {
                            request_id: request_id.clone(),
                            kind: request.request.kind,
                            preview: request.request.preview.clone(),
                        },
                    )
                    .await;
                    let outcome = self
                        .await_decision(&request_id, event_tx, &cancel, Some(cmd_rx))
                        .await;
                    let _ = request.reply.send((outcome.decision, outcome.scope));
                }
                Some(usage) = usage_rx.recv() => {
                    self.cumulative_usage.add(usage);
                    let est_cost_usd = self.estimated_cost(self.cumulative_usage);
                    self.note_routing_cost(est_cost_usd);
                    self.record_usage_row(None, &usage, est_cost_usd).await;
                    let _ = event_tx.send(AgentEvent::UsageUpdated {
                        usage,
                        cumulative: self.cumulative_usage,
                        est_cost_usd,
                    });
                }
                Some(grant) = grant_rx.recv() => {
                    if let Some(harness) = self.harness.as_mut() {
                        harness.policy.record_grant(grant);
                    }
                }
                Some(update) = budget_rx.recv() => {
                    if let Some(harness) = self.harness.as_mut() {
                        match update {
                            SubagentBudgetUpdate::Unlimited => {
                                harness.config.max_session_tokens = None;
                            }
                            SubagentBudgetUpdate::Increase(delta) => {
                                if let Some(cap) = harness.config.max_session_tokens {
                                    harness.config.max_session_tokens =
                                        Some(cap.saturating_add(delta));
                                }
                            }
                        }
                    }
                }
                Some(command) = cmd_rx.recv() => {
                    match command {
                        AgentCommand::CancelSubagent { item_id: id } if id == item_id => {
                            cancel.cancel();
                        }
                        AgentCommand::Cancel | AgentCommand::Quit => cancel.cancel(),
                        other => deferred.push_back(other),
                    }
                }
            }
        }

        self.active_subagents.remove(&item_id);
    }

    pub fn set_post_edit_diagnostics(&mut self, enabled: bool) {
        self.post_edit_diagnostics = enabled;
    }

    pub fn hooks(&self) -> &HookEngine {
        &self.hooks
    }

    pub async fn start_session(&mut self, event_tx: &mpsc::UnboundedSender<AgentEvent>) {
        if self.session_started {
            return;
        }
        self.session_started = true;
        self.run_hook(
            HookEvent::SessionStart,
            HookArgs {
                turn_id: None,
                ..HookArgs::default()
            },
            event_tx,
        )
        .await;
        self.emit_loop_updated(event_tx);
    }

    pub async fn end_session(&mut self, event_tx: &mpsc::UnboundedSender<AgentEvent>) {
        if !self.session_started {
            return;
        }
        self.session_started = false;
        self.run_hook(
            HookEvent::SessionEnd,
            HookArgs {
                turn_id: None,
                ..HookArgs::default()
            },
            event_tx,
        )
        .await;
    }

    pub(super) fn run_before_step(&self, plan: &mut StepPlan) -> StepAction {
        for interceptor in &self.interceptors {
            if let StepAction::Stop { reason } = interceptor.before_step(plan) {
                return StepAction::Stop { reason };
            }
        }
        StepAction::Continue
    }

    pub(super) fn run_after_tool(&self, outcome: &ToolOutcome) -> Vec<String> {
        self.interceptors
            .iter()
            .filter_map(|interceptor| interceptor.after_tool(outcome))
            .collect()
    }

    pub(super) async fn run_before_tool(
        &self,
        plan: &ToolPlan<'_>,
        ctx: &ToolContext<'_>,
    ) -> Vec<SessionEvent> {
        let mut events = Vec::new();
        for interceptor in &self.interceptors {
            events.extend(interceptor.before_tool(plan, ctx).await);
        }
        events
    }

    pub(super) fn run_turn_stopping(&self, turn_id: &TurnId, step_index: usize) -> Option<String> {
        for interceptor in &self.interceptors {
            if let TurnStopAction::Continue { reason } =
                interceptor.turn_stopping(turn_id, step_index)
            {
                return Some(reason);
            }
        }
        None
    }

    async fn rollback_after_verify_failure(
        &self,
        turn_id: &TurnId,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) -> Option<String> {
        let harness = self.harness.as_ref()?;
        if !harness.config.rollback_on_verify_failure {
            return None;
        }
        if harness.policy.mode() != ApprovalMode::Auto {
            return Some(
                "Automatic rollback is off outside auto mode; use /rewind to restore the turn."
                    .to_string(),
            );
        }
        let store = harness.session.snapshot_store()?;
        let report = match store
            .restore(turn_id, harness.policy.workspace_root())
            .await
        {
            Ok(report) => report,
            Err(error) => {
                self.log_event(
                    event_tx,
                    SessionEvent::Error {
                        code: ErrorCode::Internal,
                        message: format!("rollback failed: {error:#}"),
                        retryable: false,
                    },
                )
                .await;
                return None;
            }
        };
        if report.restored.is_empty() && report.deleted.is_empty() {
            return None;
        }
        let mut changed = report.restored;
        changed.extend(report.deleted);
        let count = changed.len();
        self.log_event(
            event_tx,
            SessionEvent::RewindApplied {
                turn_id: turn_id.clone(),
                scope: RewindScope::Code,
                restored: changed,
                replaced: None,
            },
        )
        .await;
        Some(format!(
            "Verify failed. Rolled back {count} file(s) to the turn snapshot. Retry from the clean state."
        ))
    }

    pub(super) async fn run_hook(
        &self,
        event: HookEvent,
        payload: HookArgs<'_>,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) -> HookReport {
        self.run_hook_with_error(event, payload, event_tx).await
    }

    pub(super) async fn run_hook_with_error(
        &self,
        event: HookEvent,
        payload: HookArgs<'_>,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) -> HookReport {
        if self.hooks.is_empty() {
            return HookReport::default();
        }
        let Some(harness) = &self.harness else {
            return HookReport::default();
        };
        let ctx = HookContext {
            session_id: harness.session.id().as_str(),
            turn_id: payload.turn_id,
            tool_name: payload.tool_name,
            tool_input: payload.tool_input,
            tool_output: payload.tool_output,
            prompt: payload.prompt,
            error: payload.error,
            cwd: harness.policy.workspace_root(),
        };
        let report = self.hooks.run(event, &ctx).await;
        for firing in &report.firings {
            self.log_event(event_tx, firing.clone().into_session_event())
                .await;
            let _ = event_tx.send(AgentEvent::HookCompleted {
                event: firing.event.as_str().to_string(),
                decision: firing.decision.as_str().to_string(),
                reason: firing.reason.clone(),
            });
        }
        report
    }

    pub(super) fn note_routing_cost(&self, cost: Option<f64>) {
        if let Some(coordinator) = self
            .harness
            .as_ref()
            .and_then(|harness| harness.routing.clone())
            && let Some(cost) = cost
        {
            coordinator.note_cost(cost);
        }
    }

    pub(super) async fn record_usage_row(
        &self,
        turn_id: Option<&meerai_session::TurnId>,
        usage: &Usage,
        est_cost_usd: Option<f64>,
    ) {
        let Some(harness) = self.harness.as_ref() else {
            return;
        };
        let provider = crate::routing::provider_of(&self.model);
        let _ = harness
            .session
            .record_usage(turn_id, &provider, &self.model, None, usage, est_cost_usd)
            .await;
    }

    pub(super) fn estimated_cost(&self, usage: Usage) -> Option<f64> {
        let pricing = self.harness.as_ref()?.config.token_pricing?;
        let prompt = usage.prompt_tokens.unwrap_or(0).max(0) as f64;
        let completion = usage.completion_tokens.unwrap_or(0).max(0) as f64;
        Some(
            prompt / 1_000_000.0 * pricing.input_per_mtok
                + completion / 1_000_000.0 * pricing.output_per_mtok,
        )
    }

    pub fn streamer(&self) -> Arc<dyn ChatStreamer> {
        self.streamer.clone()
    }

    pub fn model_handle(&self) -> Arc<std::sync::RwLock<String>> {
        self.shared_model.clone()
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn set_model(&mut self, model: impl Into<String>) {
        self.model = model.into();
        *self
            .shared_model
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = self.model.clone();
    }

    pub fn set_provider(&mut self, provider: impl Into<String>) {
        self.provider = Some(provider.into());
    }

    pub fn set_context_window_tokens(&mut self, tokens: u64) {
        if let Some(harness) = &mut self.harness {
            harness.config.context_window_tokens = tokens;
        }
    }

    pub fn set_effort_levels(&mut self, levels: Option<Vec<EffortLevel>>) {
        if let Some(harness) = &mut self.harness {
            harness.config.effort_levels = levels;
        }
    }

    pub fn set_plan_usage(&mut self, plan_usage: bool) {
        if let Some(harness) = &mut self.harness {
            harness.config.plan_usage = plan_usage;
        }
    }

    pub(super) async fn persist_model(&self) {
        if let Some(harness) = &self.harness {
            let model = self
                .model
                .rsplit_once("::")
                .map(|(_, bare)| bare)
                .unwrap_or(self.model.as_str());
            let _ = harness.session.set_model(model.to_string()).await;
            if let Some(provider) = self.provider.as_deref() {
                let _ = harness.session.set_provider(provider.to_string()).await;
            }
        }
    }

    pub(super) fn effective_system_prompt(&self) -> String {
        crate::prompt::append_model_identity(self.system_prompt.clone(), &self.model)
    }

    pub fn set_permission_mode(&mut self, mode: ApprovalMode) {
        if let Some(harness) = &mut self.harness {
            harness.policy.set_mode(mode);
        }
    }

    pub fn history(&self) -> &[ChatMessage] {
        &self.chat_history
    }

    pub fn set_harness(&mut self, harness: AgentHarness) {
        let seeded = harness
            .session
            .events()
            .into_iter()
            .rev()
            .find_map(|logged| match logged.event {
                SessionEvent::UsageUpdated { cumulative, .. } => Some(cumulative),
                _ => None,
            });
        if let Some(usage) = seeded {
            self.cumulative_usage = usage;
        }
        self.harness = Some(harness);
        self.projection_cache = None;
    }

    pub fn harness(&self) -> Option<&AgentHarness> {
        self.harness.as_ref()
    }

    pub(crate) fn set_max_session_tokens(&mut self, max_tokens: Option<u64>) {
        if let Some(harness) = &mut self.harness {
            harness.config.max_session_tokens = max_tokens;
        }
    }

    pub(crate) fn cumulative_usage_tokens(&self) -> u64 {
        self.cumulative_usage
            .total_tokens
            .map(|tokens| tokens.max(0) as u64)
            .unwrap_or(0)
    }

    pub(super) fn new_session_title(&self, prompt: &str) -> Option<String> {
        let harness = self.harness.as_ref()?;
        if harness.session.meta().title.is_some() {
            return None;
        }
        let has_user_message = harness.session.events().iter().any(|logged| {
            matches!(
                &logged.event,
                SessionEvent::UserMessage {
                    source: InputSource::User,
                    ..
                }
            )
        });
        if has_user_message {
            return None;
        }
        instructions::session_title(prompt)
    }

    pub(super) fn reload_config(
        &mut self,
        rules: Option<Vec<PermissionRule>>,
        context: Option<ContextSettings>,
        effort_levels: Option<Vec<EffortLevel>>,
        system_prompt: String,
    ) {
        if let Some(harness) = self.harness.as_mut() {
            let mode = harness.policy.mode();
            let workspace = harness.policy.workspace_root().to_path_buf();
            let mut policy = PolicyEngine::new(mode).with_workspace_root(workspace);
            if let Some(rules) = rules {
                policy = policy.with_rules(rules);
            }
            harness.policy = policy;
            let token_pricing = harness.config.token_pricing;
            let plan_usage = harness.config.plan_usage;
            let context_window = harness.config.context_window_tokens;
            let routing = harness.config.routing.clone();
            harness.config = HarnessConfig::from_context_settings(context.as_ref());
            if effort_levels.is_some() {
                harness.config.effort_levels = effort_levels;
            }
            harness.config.token_pricing = token_pricing;
            harness.config.plan_usage = plan_usage;
            harness.config.routing = routing;
            if context
                .as_ref()
                .and_then(|context| context.context_window_tokens)
                .is_none()
            {
                harness.config.context_window_tokens = context_window;
            }
            if let Some(coordinator) = &harness.routing {
                coordinator.set_provider_limits(&harness.config.routing);
                coordinator.set_cap(
                    harness
                        .config
                        .routing
                        .route(&meerai_common::settings::TaskType::Default)
                        .and_then(|route| route.max_cost_usd),
                );
            }
        }
        self.post_edit_diagnostics = context
            .as_ref()
            .and_then(|context| context.post_edit_diagnostics)
            .unwrap_or(false);
        self.system_prompt = system_prompt;
        self.system_prompt_generation = self.system_prompt_generation.wrapping_add(1);
        self.logged_system_prompt = None;
        self.projection_cache = None;
    }

    pub async fn rewind(&mut self, turn_id: &TurnId, scope: RewindScope) -> Result<Vec<String>> {
        let Some(harness) = self.harness.as_ref() else {
            anyhow::bail!("no session to rewind");
        };
        let workspace = harness.policy.workspace_root().to_path_buf();
        let store = harness.session.snapshot_store();
        let mut restored = Vec::new();
        let mut restored_paths = Vec::new();

        if matches!(scope, RewindScope::Both | RewindScope::Code)
            && let Some(store) = store
        {
            let report = store.restore(turn_id, &workspace).await?;
            restored = report
                .restored
                .iter()
                .map(|path| path.display().to_string())
                .collect();
            restored_paths = report.restored;
        }

        if let Some(harness) = self.harness.as_ref() {
            harness
                .session
                .rewind(turn_id, scope, restored_paths)
                .await?;
        }

        if matches!(scope, RewindScope::Both | RewindScope::Conversation) {
            self.clear_rewind_state();
        }
        Ok(restored)
    }

    fn clear_rewind_state(&mut self) {
        self.chat_history.clear();
        self.pending_steers.clear();
        self.pending_compact = None;
        self.failure_tracker = FailureTracker::default();
        self.injected_instruction_dirs.clear();
        self.outside_grants.clear();
        self.sandbox_escape = None;
        self.question_cancels = 0;
        self.auto_approved_notified = false;
    }

    pub fn reset(&mut self) {
        self.chat_history.clear();
        self.session_id = generate_opencode_session_id();
        self.cumulative_usage = Usage::default();
        self.pending_steers.clear();
        self.pending_compact = None;
        self.failure_tracker = FailureTracker::default();
        self.injected_instruction_dirs.clear();
        self.outside_grants.clear();
        self.sandbox_escape = None;
        self.question_cancels = 0;
        self.session_started = false;
        self.projection_cache = None;
    }

    pub async fn prompt(&mut self, prompt: &str) -> Result<String> {
        let (tx, _rx) = mpsc::unbounded_channel();
        self.prompt_streaming(prompt, &tx, CancellationToken::new())
            .await
    }

    pub async fn prompt_streaming(
        &mut self,
        prompt: &str,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancel: CancellationToken,
    ) -> Result<String> {
        let (_, outcome) = self.run_turn(prompt, event_tx, cancel, None).await;
        outcome.map_err(|err| err.source)
    }

    pub async fn prompt_streaming_with_commands(
        &mut self,
        prompt: &str,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancel: CancellationToken,
        commands: &mut mpsc::UnboundedReceiver<AgentCommand>,
    ) -> Result<String> {
        let (_, outcome) = self
            .run_turn(prompt, event_tx, cancel, Some(commands))
            .await;
        outcome.map_err(|err| err.source)
    }

    pub(super) fn projection_policy(&self) -> ProjectionPolicy {
        match &self.harness {
            Some(harness) => ProjectionPolicy {
                strip_ephemera: true,
                prune_reasoning: self
                    .projection_overrides
                    .reasoning_pruning
                    .unwrap_or(harness.config.reasoning_pruning),
                verbatim_window_turns: self
                    .projection_overrides
                    .verbatim_window_turns
                    .unwrap_or(harness.config.verbatim_window_turns),
                clear_old_tool_results: self
                    .projection_overrides
                    .clear_old_tool_results
                    .unwrap_or(true),
                include_system_messages: false,
                pinned_turns: self
                    .projection_overrides
                    .pinned_turns
                    .clone()
                    .unwrap_or_default(),
            },
            None => ProjectionPolicy {
                strip_ephemera: true,
                prune_reasoning: self.projection_overrides.reasoning_pruning.unwrap_or(true),
                verbatim_window_turns: self.projection_overrides.verbatim_window_turns.unwrap_or(4),
                clear_old_tool_results: self
                    .projection_overrides
                    .clear_old_tool_results
                    .unwrap_or(true),
                include_system_messages: true,
                pinned_turns: self
                    .projection_overrides
                    .pinned_turns
                    .clone()
                    .unwrap_or_default(),
            },
        }
    }

    pub fn context_breakdown(&self) -> Option<ContextBreakdown> {
        let harness = self.harness.as_ref()?;
        let policy = self.projection_policy();
        let mut breakdown = context_breakdown(
            &harness.session.events(),
            &policy,
            harness.config.context_window_tokens,
            harness.config.high_watermark_pct,
        );
        let (prompt, memory) = split_memory_prompt(&self.effective_system_prompt());
        breakdown.system_prompt_tokens = estimate_tokens(&prompt);
        breakdown.memory_tokens = estimate_tokens(&memory);
        breakdown.tool_schemas_tokens = self.tool_schema_tokens;
        breakdown.recompute_total();
        Some(breakdown)
    }

    pub fn emit_context_updated(&self, event_tx: &mpsc::UnboundedSender<AgentEvent>) {
        if let Some(breakdown) = self.context_breakdown() {
            let _ = event_tx.send(AgentEvent::ContextUpdated { breakdown });
        }
    }

    pub(super) fn request_messages(&mut self) -> Vec<ChatMessage> {
        if self.harness.is_none() {
            return self.chat_history.clone();
        }
        let policy = self.projection_policy();
        let event_count = self
            .harness
            .as_ref()
            .map(|harness| harness.session.event_count())
            .unwrap_or(0);
        if let Some(cache) = &self.projection_cache
            && cache.event_count == event_count
            && cache.policy == policy
        {
            return cache.messages.clone();
        }
        let messages = self
            .harness
            .as_ref()
            .expect("checked above")
            .session
            .derive_messages(&policy);
        self.projection_cache = Some(ProjectionCache {
            event_count,
            policy,
            messages: messages.clone(),
        });
        messages
    }

    async fn sync_system_message(&mut self, event_tx: &mpsc::UnboundedSender<AgentEvent>) {
        if self.harness.is_none() {
            return;
        }

        let prompt = self.effective_system_prompt();
        let changed = match &self.logged_system_prompt {
            Some(previous) => previous != &prompt,
            None => true,
        };
        if !changed {
            return;
        }

        if self.logged_system_prompt.is_some() {
            self.system_prompt_generation += 1;
        }
        self.logged_system_prompt = Some(prompt.clone());

        if prompt.is_empty() {
            return;
        }

        self.log_event(
            event_tx,
            SessionEvent::SystemMessage {
                sections: vec![PromptSection {
                    label: "instructions".to_string(),
                    content: prompt,
                }],
                generation: self.system_prompt_generation,
            },
        )
        .await;
    }

    pub(super) async fn enforce_turn_deadline(
        &self,
        turn_started: Instant,
        max_turn_secs: u64,
        turn_id: &TurnId,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) -> Option<(String, TurnEndReason)> {
        if !turn_deadline_exceeded(turn_started, max_turn_secs, Instant::now()) {
            return None;
        }

        let message = format!("turn exceeded {max_turn_secs}s");
        let _ = event_tx.send(AgentEvent::Error {
            code: ErrorCode::Budget,
            message: message.clone(),
        });
        self.log_event(
            event_tx,
            SessionEvent::Error {
                code: ErrorCode::Budget,
                message: message.clone(),
                retryable: false,
            },
        )
        .await;
        Some(failed(event_tx, turn_id, message))
    }

    pub(super) async fn log_step_ended(
        &self,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        step_id: &StepId,
        finish: StepFinish,
    ) {
        self.log_event(
            event_tx,
            SessionEvent::StepEnded {
                step_id: step_id.clone(),
                finish,
            },
        )
        .await;
    }

    pub(super) async fn log_event(
        &self,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        event: SessionEvent,
    ) {
        let Some(harness) = &self.harness else {
            return;
        };
        if let Err(error) = harness.session.append(event).await {
            let _ = event_tx.send(AgentEvent::Error {
                code: ErrorCode::Internal,
                message: format!("failed to append session event: {error:#}"),
            });
        }
    }
}
