use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use futures::future::join_all;
use futures::{Stream, StreamExt};
use genai::chat::{
    ChatMessage, ChatOptions, ChatRequest, ChatRole, ChatStreamEvent, StreamEnd, Tool, ToolCall,
    ToolResponse,
};
use genai::{Client, Headers};
use meerai_core::env::{MEERAI_USER_AGENT, generate_opencode_session_id, resolve_lexical_subpath};
use meerai_core::text::floor_char_boundary;
use meerai_core::{
    EffectClass, ObservationStatus, ToolObservation, ToolOutput, ToolPermissionSpec, ToolRun,
    Toolset,
};
use meerai_policy::{
    ApprovalMode, Decision, PermissionAction, PermissionCheck, approval_preview_for,
};
use meerai_session::{
    ApprovalPreview, BreakerAction, BreakerKind, CompactionId, CompactionResult, CompactionTrigger,
    ContentBlock, InputSource, ProjectionPolicy, PromptSection, SessionEvent, StepFinish, StepId,
    StreamDigest, Usage, apply_spill, derive_messages, estimate_tokens, has_open_compaction,
    plan_compaction_boundary, summary_message,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::events::{
    AbortCause, AgentCommand, AgentEvent, ApprovalDecision, ApprovalKind, ApprovalRequest, CallId,
    ErrorCode, GrantScope, ItemDelta, ItemId, ItemStatus, RequestId, SessionItem, TurnEndReason,
    TurnId, TurnTrigger,
};
use crate::harness::AgentHarness;
use crate::redact::{redact_run, redact_text};

const STEP_LIMIT_NOTICE: &str =
    "Step limit reached. Provide a final summary of what was done and what remains.";

const SUMMARY_INSTRUCTION: &str = "Summarize the transcript below as a compact state block. Use \
    exactly these headings, each followed by terse bullets: Goal, Completed progress, Modified \
    files, Key decisions, Open blockers. Do not add other headings and do not invent facts.";

const SUMMARY_MESSAGE_TOKEN_LIMIT: u64 = 2_000;

const COMPACTION_EMPTY_SKIP: &str = "no compactable range";
const COMPACTION_EMPTY_SUMMARY: &str = "compaction summary was empty";
const TOKEN_BUDGET_DECLINED: &str = "session token budget declined";
const LOOP_DETECTED: &str = "loop detected: repeated identical tool failures";
const NO_APPROVAL_CHANNEL: &str = "no approval channel available; call declined";
const USER_DECLINED: &str = "declined by user";
const NOT_EXECUTED_STOPPED: &str = "not executed: turn stopped";

const MAX_PROVIDER_ATTEMPTS: usize = 3;
const PROVIDER_BACKOFF_MS: u64 = 250;

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
    text_item_id: Option<ItemId>,
    reasoning_item_id: Option<ItemId>,
}

pub(super) enum StreamFailure {
    Cancelled,
    Provider { error: anyhow::Error, partial: bool },
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
        model: &str,
        request: ChatRequest,
        options: &ChatOptions,
    ) -> Result<BoxedChatStream>;
}

#[async_trait]
impl ChatStreamer for Client {
    async fn stream_chat(
        &self,
        model: &str,
        request: ChatRequest,
        options: &ChatOptions,
    ) -> Result<BoxedChatStream> {
        let response = self
            .exec_chat_stream(model, request, Some(options))
            .await
            .map_err(|err| anyhow!("LLM stream request failed: {err}"))?;
        let mapped = response
            .stream
            .map(|item| item.map_err(|err| anyhow!("LLM stream error: {err}")));
        Ok(Box::pin(mapped))
    }
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

pub struct MultiTurnAgent {
    pub(super) streamer: Arc<dyn ChatStreamer>,
    pub(super) model: String,
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
    pub(super) pending_steers: VecDeque<String>,
    pub(super) pending_compact: bool,
    pub(in crate::multi_turn_agent) failure_tracker: FailureTracker,
    pub(super) injected_instruction_dirs: HashSet<PathBuf>,
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
    pub fn new(
        client: Client,
        model: impl Into<String>,
        tools: Vec<Box<dyn Toolset>>,
        system_prompt: impl Into<String>,
    ) -> Self {
        Self::new_with_config(
            client,
            model,
            tools,
            system_prompt,
            MultiTurnAgentConfig::default(),
        )
    }

    pub fn new_with_config(
        client: Client,
        model: impl Into<String>,
        tools: Vec<Box<dyn Toolset>>,
        system_prompt: impl Into<String>,
        config: MultiTurnAgentConfig,
    ) -> Self {
        Self::with_streamer(Arc::new(client), model, tools, system_prompt, config)
    }

    pub fn new_without_tools(
        client: Client,
        model: impl Into<String>,
        system_prompt: impl Into<String>,
    ) -> Self {
        Self::new(client, model, vec![], system_prompt)
    }

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
            pending_steers: VecDeque::new(),
            pending_compact: false,
            failure_tracker: FailureTracker::default(),
            injected_instruction_dirs: HashSet::new(),
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

    pub fn reset(&mut self) {
        self.chat_history.clear();
        self.session_id = generate_opencode_session_id();
        self.cumulative_usage = Usage::default();
        self.pending_steers.clear();
        self.pending_compact = false;
        self.failure_tracker = FailureTracker::default();
        self.injected_instruction_dirs.clear();
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
                prune_reasoning: harness.config.reasoning_pruning,
                verbatim_window_turns: harness.config.verbatim_window_turns,
                include_system_messages: false,
            },
            None => ProjectionPolicy {
                strip_ephemera: true,
                prune_reasoning: true,
                verbatim_window_turns: 4,
                include_system_messages: true,
            },
        }
    }

    pub(super) fn request_messages(&self) -> Vec<ChatMessage> {
        let Some(harness) = &self.harness else {
            return self.chat_history.clone();
        };

        harness.session.derive_messages(&self.projection_policy())
    }

    async fn sync_system_message(&mut self, event_tx: &mpsc::UnboundedSender<AgentEvent>) {
        if self.harness.is_none() {
            return;
        }

        let changed = match &self.logged_system_prompt {
            Some(previous) => previous != &self.system_prompt,
            None => true,
        };
        if !changed {
            return;
        }

        if self.logged_system_prompt.is_some() {
            self.system_prompt_generation += 1;
        }
        self.logged_system_prompt = Some(self.system_prompt.clone());

        if self.system_prompt.is_empty() {
            return;
        }

        self.log_event(
            event_tx,
            SessionEvent::SystemMessage {
                sections: vec![PromptSection {
                    label: "instructions".to_string(),
                    content: self.system_prompt.clone(),
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
