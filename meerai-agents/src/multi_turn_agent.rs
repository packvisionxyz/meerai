use std::collections::{HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use futures::future::join_all;
use futures::{Stream, StreamExt};
use genai::chat::{
    ChatMessage, ChatOptions, ChatRequest, ChatRole, ChatStreamEvent, StreamEnd, Tool, ToolCall,
    ToolResponse,
};
use genai::{Client, Headers};
use meerai_core::{EffectClass, ObservationStatus, ToolObservation, ToolOutput, ToolRun, Toolset};
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

const USER_AGENT: &str = concat!("meerai/", env!("CARGO_PKG_VERSION"));

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

fn provider_backoff_after(failures: usize) -> Duration {
    let factor = 1u64 << failures.saturating_sub(1).min(8);
    Duration::from_millis(PROVIDER_BACKOFF_MS.saturating_mul(factor))
}

fn turn_deadline_exceeded(started: Instant, max_turn_secs: u64, now: Instant) -> bool {
    max_turn_secs != 0
        && now.saturating_duration_since(started) >= Duration::from_secs(max_turn_secs)
}

fn remaining_turn_budget(started: Instant, max_turn_secs: u64) -> Option<Duration> {
    if max_turn_secs == 0 {
        return None;
    }
    Duration::from_secs(max_turn_secs).checked_sub(started.elapsed())
}

#[derive(Clone, Copy)]
struct TurnClock {
    started: Instant,
    max_secs: u64,
}

impl TurnClock {
    fn remaining(&self) -> Option<Duration> {
        remaining_turn_budget(self.started, self.max_secs)
    }

    fn error(&self) -> TurnError {
        TurnError::budget(anyhow!("turn exceeded {}s", self.max_secs))
    }
}

fn tool_budget_exceeded(max_secs: u64) -> ToolRun {
    ToolRun::from_output(ToolOutput::Fail(format!(
        "tool timed out after turn exceeded {max_secs}s"
    )))
}

fn is_read_file_tool(name: &str) -> bool {
    let bare = name
        .strip_prefix("code-")
        .or_else(|| name.strip_prefix("code_"))
        .unwrap_or(name);
    bare == "read_file"
}

fn resolve_workspace_path(root: &Path, raw: &str) -> Option<PathBuf> {
    let candidate = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        root.join(raw)
    };
    let mut resolved = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !resolved.pop() {
                    return None;
                }
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    resolved.starts_with(root).then_some(resolved)
}

async fn nearest_agents_dir(root: &Path, resolved: &Path) -> Option<PathBuf> {
    let is_directory = tokio::fs::metadata(resolved)
        .await
        .is_ok_and(|meta| meta.is_dir());
    let mut current = if is_directory {
        Some(resolved)
    } else {
        resolved.parent()
    };
    while let Some(directory) = current {
        if directory == root || !directory.starts_with(root) {
            return None;
        }
        let candidate = directory.join("AGENTS.md");
        if tokio::fs::metadata(&candidate)
            .await
            .is_ok_and(|meta| meta.is_file())
        {
            return Some(directory.to_path_buf());
        }
        current = directory.parent();
    }
    None
}

fn session_title(prompt: &str) -> Option<String> {
    let line = prompt.lines().find(|line| !line.trim().is_empty())?;
    let mut title = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return None;
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        let end = title
            .char_indices()
            .nth(MAX_TITLE_CHARS)
            .map_or(title.len(), |(index, _)| index);
        title.truncate(end);
        title = title.trim_end().to_string();
    }
    Some(title)
}

fn capped_instructions(text: String) -> String {
    if text.len() <= MAX_SCOPED_INSTRUCTION_BYTES {
        return text;
    }
    let mut end = MAX_SCOPED_INSTRUCTION_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n\n[truncated]", &text[..end])
}

fn message_text(message: &ChatMessage) -> String {
    message.content.texts().join("\n")
}

fn estimate_context_tokens(messages: &[ChatMessage]) -> u64 {
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

fn truncate_tokens(text: &str, max_tokens: u64) -> String {
    let limit = (max_tokens.saturating_mul(4)) as usize;
    if text.len() <= limit {
        return text.to_string();
    }

    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
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

#[derive(Default)]
struct FailureTracker {
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

struct CollectedStream {
    end: StreamEnd,
    text: String,
    reasoning: String,
    text_item_id: Option<ItemId>,
    reasoning_item_id: Option<ItemId>,
}

enum StreamFailure {
    Cancelled,
    Provider { error: anyhow::Error, partial: bool },
}

enum BudgetOutcome {
    Continue,
    Declined { detail: String },
    Cancelled,
}

fn generate_session_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = u64::from(std::process::id());
    format!("ses_{millis:012x}{pid:08x}{seq:08x}")
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

fn start_text_item(event_tx: &mpsc::UnboundedSender<AgentEvent>) -> ItemId {
    let id = ItemId::new();
    let _ = event_tx.send(AgentEvent::ItemStarted {
        item: SessionItem::AgentMessage {
            id: id.clone(),
            text: String::new(),
        },
    });
    id
}

fn start_reasoning_item(event_tx: &mpsc::UnboundedSender<AgentEvent>) -> ItemId {
    let id = ItemId::new();
    let _ = event_tx.send(AgentEvent::ItemStarted {
        item: SessionItem::Reasoning {
            id: id.clone(),
            summary: String::new(),
        },
    });
    id
}

fn aborted(
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

fn completed(
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

fn failed(
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

fn tool_call_item(
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

fn provider_of(model: &str) -> String {
    match model.split_once(':') {
        Some((provider, _)) if !provider.is_empty() => provider.to_string(),
        _ => "unknown".to_string(),
    }
}

fn permission_check(tool_name: &str, args: &str, effect: EffectClass) -> PermissionCheck {
    let parsed: Option<serde_json::Value> = serde_json::from_str(args).ok();
    let field = |key: &str| {
        parsed
            .as_ref()
            .and_then(|value| value.get(key))
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string()
    };

    let named = match tool_name
        .strip_prefix("code-")
        .or_else(|| tool_name.strip_prefix("code_"))
    {
        Some("read_file" | "list_dir" | "grep" | "glob") => {
            Some((PermissionAction::Read, field("path")))
        }
        Some("write_file" | "edit_file") => Some((PermissionAction::Edit, field("path"))),
        Some("shell") => Some((PermissionAction::Shell, field("command"))),
        _ => None,
    };

    let (action, resource) = named.unwrap_or_else(|| {
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
    });

    PermissionCheck {
        action,
        resource,
        effect,
        tool_name: tool_name.to_string(),
    }
}

#[derive(Debug)]
struct TurnError {
    code: ErrorCode,
    source: anyhow::Error,
    retryable: bool,
}

impl TurnError {
    fn provider(source: anyhow::Error) -> Self {
        Self {
            code: ErrorCode::Provider,
            source,
            retryable: false,
        }
    }

    fn transient_provider(source: anyhow::Error) -> Self {
        Self {
            code: ErrorCode::Provider,
            source,
            retryable: true,
        }
    }

    fn budget(source: anyhow::Error) -> Self {
        Self {
            code: ErrorCode::Budget,
            source,
            retryable: false,
        }
    }

    fn context_overflow(source: anyhow::Error) -> Self {
        Self {
            code: ErrorCode::ContextOverflow,
            source,
            retryable: false,
        }
    }

    fn loop_detected(source: anyhow::Error) -> Self {
        Self {
            code: ErrorCode::LoopDetected,
            source,
            retryable: false,
        }
    }

    fn message(&self) -> String {
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

enum Authorization {
    Allowed,
    Refused(String),
    Cancelled,
}

struct ApprovalOutcome {
    decision: ApprovalDecision,
    scope: Option<GrantScope>,
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

enum StepFailure {
    Cancelled,
    Error(TurnError),
}

pub struct MultiTurnAgent {
    streamer: Arc<dyn ChatStreamer>,
    model: String,
    session_id: String,
    chat_history: Vec<ChatMessage>,
    tools: Vec<Box<dyn Toolset>>,
    system_prompt: String,
    config: MultiTurnAgentConfig,
    cumulative_usage: Usage,
    harness: Option<AgentHarness>,
    system_prompt_generation: u64,
    logged_system_prompt: Option<String>,
    pending_steers: VecDeque<String>,
    pending_compact: bool,
    failure_tracker: FailureTracker,
    injected_instruction_dirs: HashSet<PathBuf>,
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
        Self {
            streamer,
            model: model.into(),
            session_id: generate_session_id(),
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

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn set_model(&mut self, model: impl Into<String>) {
        self.model = model.into();
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

    fn new_session_title(&self, prompt: &str) -> Option<String> {
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
        session_title(prompt)
    }

    pub fn reset(&mut self) {
        self.chat_history.clear();
        self.session_id = generate_session_id();
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

    async fn run_turn(
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
            .with_extra_headers(Headers::from([
                ("x-opencode-session", self.session_id.as_str()),
                ("user-agent", USER_AGENT),
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
                    est_cost_usd: None,
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

            let tool_calls: Vec<ToolCall> = end
                .captured_tool_calls()
                .map(|calls| calls.into_iter().cloned().collect())
                .unwrap_or_default();
            let core_calls: Vec<meerai_core::ToolCall> =
                tool_calls.iter().map(meerai_core::ToolCall::from).collect();

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

            let stop_reason = if tool_calls.is_empty() {
                None
            } else {
                match self
                    .execute_tool_calls(
                        &tool_calls,
                        turn_id,
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

    async fn execute_tool_calls(
        &mut self,
        tool_calls: &[ToolCall],
        turn_id: &TurnId,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
        mut inbox: Option<&mut mpsc::UnboundedReceiver<AgentCommand>>,
        clock: TurnClock,
    ) -> Result<Option<String>, TurnError> {
        let mut plans: Vec<CallPlan> = Vec::with_capacity(tool_calls.len());

        for tc in tool_calls {
            if cancel.is_cancelled() {
                return Ok(None);
            }

            let args_str = tc.fn_arguments.to_string();
            let effect = self
                .tools
                .iter()
                .find(|tool| tool.contain(&tc.fn_name))
                .map(|tool| tool.effect_of(&tc.fn_name))
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
                let check = permission_check(&tc.fn_name, &args_str, effect);
                self.authorize(
                    &check,
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

        let this: &Self = self;
        let mut halted = false;
        let mut cursor = 0;

        while cursor < plans.len() && !halted {
            if cancel.is_cancelled() {
                break;
            }

            if !plans[cursor].authorized() || plans[cursor].effect != EffectClass::Read {
                if plans[cursor].authorized() {
                    let plan = &plans[cursor];
                    let result = match clock.remaining() {
                        Some(remaining) => match tokio::time::timeout(
                            remaining,
                            this.run_tool(&plan.tool_call.fn_name, &plan.args, cancel),
                        )
                        .await
                        {
                            Ok(result) => result,
                            Err(_) => Ok(tool_budget_exceeded(clock.max_secs)),
                        },
                        None if clock.max_secs != 0 => Ok(tool_budget_exceeded(clock.max_secs)),
                        None => {
                            this.run_tool(&plan.tool_call.fn_name, &plan.args, cancel)
                                .await
                        }
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
                && plans[end].authorized()
                && plans[end].effect == EffectClass::Read
            {
                end += 1;
            }

            let reads = (cursor..end).map(|index| {
                let plan = &plans[index];
                async move {
                    let result = this
                        .run_tool(&plan.tool_call.fn_name, &plan.args, cancel)
                        .await;
                    (index, result)
                }
            });
            let results = match clock.remaining() {
                Some(remaining) => match tokio::time::timeout(remaining, join_all(reads)).await {
                    Ok(results) => results,
                    Err(_) => {
                        let timeout = tool_budget_exceeded(clock.max_secs);
                        for plan in plans.iter_mut().take(end).skip(cursor) {
                            plan.outcome = Some(CallOutcome::Run(timeout.clone()));
                        }
                        cursor = end;
                        continue;
                    }
                },
                None if clock.max_secs != 0 => {
                    let timeout = tool_budget_exceeded(clock.max_secs);
                    for plan in plans.iter_mut().take(end).skip(cursor) {
                        plan.outcome = Some(CallOutcome::Run(timeout.clone()));
                    }
                    cursor = end;
                    continue;
                }
                None => join_all(reads).await,
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
                            let text = redact_text(&format!("Tool error: {error:#}"));
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

            self.log_event(
                event_tx,
                SessionEvent::ToolResult {
                    call_id,
                    output: observation,
                },
            )
            .await;

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

    async fn authorize(
        &mut self,
        check: &PermissionCheck,
        item_id: &ItemId,
        turn_id: &TurnId,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
        inbox: Option<&mut mpsc::UnboundedReceiver<AgentCommand>>,
    ) -> Authorization {
        let Some(harness) = self.harness.as_ref() else {
            return Authorization::Allowed;
        };

        match harness.policy.decide(check) {
            Decision::Allow => Authorization::Allowed,
            Decision::Deny { reason } => Authorization::Refused(reason),
            Decision::Ask => {
                let has_channel = inbox.is_some();
                let request_id = RequestId::new();
                let request = ApprovalRequest {
                    request_id: request_id.clone(),
                    turn_id: turn_id.clone(),
                    kind: ApprovalKind::ToolCall,
                    preview: approval_preview_for(check),
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
                AgentCommand::Steer(text) => self.pending_steers.push_back(text),
                AgentCommand::SetPermissionMode(mode) => {
                    if let Some(harness) = &mut self.harness {
                        harness.policy.set_mode(mode);
                    }
                }
                AgentCommand::Compact => self.pending_compact = true,
                AgentCommand::ApplyAgent {
                    name,
                    instructions,
                    model,
                } => {
                    self.apply_agent_instructions(&name, &instructions, model, event_tx)
                        .await;
                }
                AgentCommand::ApplySkill { name, instructions } => {
                    self.apply_skill_instructions(&name, &instructions, event_tx)
                        .await;
                }
                AgentCommand::SubmitPrompt(_) | AgentCommand::SetModel(_) | AgentCommand::Reset => {
                }
            }
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
        self.log_event(
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

    async fn drain_inbox(
        &mut self,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        inbox: Option<&mut mpsc::UnboundedReceiver<AgentCommand>>,
    ) -> bool {
        let Some(receiver) = inbox else {
            return false;
        };

        while let Ok(command) = receiver.try_recv() {
            match command {
                AgentCommand::Cancel | AgentCommand::Quit => return true,
                AgentCommand::Steer(text) => self.pending_steers.push_back(text),
                AgentCommand::SetPermissionMode(mode) => {
                    if let Some(harness) = &mut self.harness {
                        harness.policy.set_mode(mode);
                    }
                }
                AgentCommand::Compact => self.pending_compact = true,
                AgentCommand::ApplyAgent {
                    name,
                    instructions,
                    model,
                } => {
                    self.apply_agent_instructions(&name, &instructions, model, event_tx)
                        .await;
                }
                AgentCommand::ApplySkill { name, instructions } => {
                    self.apply_skill_instructions(&name, &instructions, event_tx)
                        .await;
                }
                AgentCommand::Approve { .. }
                | AgentCommand::SubmitPrompt(_)
                | AgentCommand::SetModel(_)
                | AgentCommand::Reset => {}
            }
        }

        false
    }

    async fn apply_pending_steers(&mut self, event_tx: &mpsc::UnboundedSender<AgentEvent>) -> bool {
        if self.pending_steers.is_empty() {
            return false;
        }

        let steers: Vec<String> = self.pending_steers.drain(..).collect();
        for text in steers {
            self.log_event(
                event_tx,
                SessionEvent::UserMessage {
                    source: InputSource::Steering,
                    content: vec![ContentBlock::Text(text.clone())],
                },
            )
            .await;
            self.chat_history.push(ChatMessage::user(text));
        }
        true
    }

    async fn apply_agent_instructions(
        &mut self,
        name: &str,
        instructions: &str,
        model: Option<String>,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) {
        self.apply_instructions(
            format!("Agent instructions ({name}):\n\n{instructions}"),
            model,
            event_tx,
        )
        .await;
    }

    async fn apply_skill_instructions(
        &mut self,
        name: &str,
        instructions: &str,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) {
        self.apply_instructions(
            format!("Skill instructions ({name}):\n\n{instructions}"),
            None,
            event_tx,
        )
        .await;
    }

    async fn apply_instructions(
        &mut self,
        text: String,
        model: Option<String>,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) {
        self.log_event(
            event_tx,
            SessionEvent::UserMessage {
                source: InputSource::System,
                content: vec![ContentBlock::Text(text.clone())],
            },
        )
        .await;
        self.chat_history.push(ChatMessage::user(text));
        if let Some(model) = model {
            self.set_model(model);
        }
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
                            ChatStreamEvent::Start
                            | ChatStreamEvent::ThoughtSignatureChunk(_)
                            | ChatStreamEvent::ToolCallChunk(_)
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
            if !self.system_prompt.is_empty() {
                request = request.with_system(self.system_prompt.clone());
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

    async fn run_compaction(
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

    fn over_high_watermark(&self) -> bool {
        let Some(harness) = &self.harness else {
            return false;
        };
        let threshold = harness
            .config
            .context_window_tokens
            .saturating_mul(u64::from(harness.config.high_watermark_pct))
            / 100;
        let estimated = estimate_context_tokens(&self.request_messages())
            + estimate_tokens(&self.system_prompt);
        estimated > threshold
    }

    async fn enforce_token_budget(
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
        if is_read_file_tool(tool_name) {
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

    async fn run_tool(
        &self,
        name: &str,
        args: &str,
        cancel: &CancellationToken,
    ) -> Result<ToolRun> {
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.contain(name))
            .ok_or_else(|| anyhow!("Tool not found: {name}"))?;

        let timeout_ms = self
            .harness
            .as_ref()
            .map(|harness| harness.config.tool_timeout_ms);
        let max_attempts = self.config.max_retries.max(1);

        for attempt in 1..=max_attempts {
            if cancel.is_cancelled() {
                return Err(anyhow!("Tool execution cancelled"));
            }

            let invocation = tool.invoke_structured(name, args);
            let result = match timeout_ms {
                Some(ms) => match tokio::time::timeout(Duration::from_millis(ms), invocation).await
                {
                    Ok(result) => result,
                    Err(_) => {
                        return Ok(ToolRun::from_output(ToolOutput::Fail(format!(
                            "tool timed out after {ms}ms"
                        ))));
                    }
                },
                None => invocation.await,
            };

            match result {
                Ok(run) => return Ok(run),
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

    fn projection_policy(&self) -> ProjectionPolicy {
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

    fn request_messages(&self) -> Vec<ChatMessage> {
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

    async fn inject_scoped_instructions(
        &mut self,
        args: &str,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) {
        let Some(root) = self
            .harness
            .as_ref()
            .map(|harness| harness.session.meta().cwd)
        else {
            return;
        };
        if self.injected_instruction_dirs.len() >= MAX_SCOPED_INSTRUCTION_FILES {
            return;
        }

        let Some(raw) = serde_json::from_str::<serde_json::Value>(args)
            .ok()
            .and_then(|value| value.get("path")?.as_str().map(str::to_string))
        else {
            return;
        };
        let Some(resolved) = resolve_workspace_path(&root, &raw) else {
            return;
        };
        let Some(directory) = nearest_agents_dir(&root, &resolved).await else {
            return;
        };
        if self.injected_instruction_dirs.contains(&directory) {
            return;
        }
        let Ok(text) = tokio::fs::read_to_string(directory.join("AGENTS.md")).await else {
            return;
        };
        if text.trim().is_empty() {
            return;
        }

        let relative = directory
            .strip_prefix(&root)
            .unwrap_or(&directory)
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        let content = format!(
            "Project instructions from {relative}/AGENTS.md:\n\n{}",
            capped_instructions(text)
        );
        self.injected_instruction_dirs.insert(directory);
        self.log_event(
            event_tx,
            SessionEvent::UserMessage {
                source: InputSource::RuntimeContext,
                content: vec![ContentBlock::Text(content.clone())],
            },
        )
        .await;
        self.chat_history.push(ChatMessage::user(content));
    }

    async fn enforce_turn_deadline(
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

    async fn log_step_ended(
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

    async fn log_event(&self, event_tx: &mpsc::UnboundedSender<AgentEvent>, event: SessionEvent) {
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

pub fn spawn_worker(
    mut agent: MultiTurnAgent,
) -> (
    mpsc::UnboundedSender<AgentCommand>,
    mpsc::UnboundedReceiver<AgentEvent>,
) {
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        let mut queued: VecDeque<String> = VecDeque::new();

        'worker: while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                AgentCommand::SubmitPrompt(prompt) | AgentCommand::Steer(prompt) => {
                    queued.push_back(prompt);

                    while let Some(prompt) = queued.pop_front() {
                        let harnessed = agent.harness.is_some();
                        let cancel = CancellationToken::new();
                        let mut pending_quit = false;
                        let mut pending_reset = false;
                        let mut pending_model: Option<String> = None;
                        let (inbox_tx, mut inbox_rx) = mpsc::unbounded_channel();

                        {
                            let mut turn_fut = std::pin::pin!(agent.run_turn(
                                &prompt,
                                &event_tx,
                                cancel.clone(),
                                Some(&mut inbox_rx),
                            ));

                            loop {
                                tokio::select! {
                                    res = &mut turn_fut => {
                                        let (turn_id, outcome) = res;
                                        if let Err(err) = outcome {
                                            let message = err.message();
                                            let _ = event_tx.send(AgentEvent::Error {
                                                code: err.code,
                                                message: message.clone(),
                                            });
                                            let _ = event_tx.send(AgentEvent::TurnCompleted {
                                                turn_id,
                                                reason: TurnEndReason::Failed { error: message },
                                            });
                                        }
                                        break;
                                    }
                                    maybe_cmd = cmd_rx.recv() => {
                                        match maybe_cmd {
                                            Some(AgentCommand::Cancel) => cancel.cancel(),
                                            Some(AgentCommand::Quit) | None => {
                                                cancel.cancel();
                                                pending_quit = true;
                                            }
                                            Some(AgentCommand::Reset) => {
                                                cancel.cancel();
                                                pending_reset = true;
                                            }
                                            Some(AgentCommand::SetModel(model)) => {
                                                pending_model = Some(model);
                                            }
                                            Some(AgentCommand::SubmitPrompt(prompt)) => {
                                                queued.push_back(prompt);
                                            }
                                            Some(AgentCommand::Steer(text)) if !harnessed => {
                                                queued.push_back(text);
                                            }
                                            Some(
                                                command @ (AgentCommand::Steer(_)
                                                | AgentCommand::Approve { .. }
                                                | AgentCommand::SetPermissionMode(_)
                                                | AgentCommand::Compact
                                                | AgentCommand::ApplyAgent { .. }
                                                | AgentCommand::ApplySkill { .. }),
                                            ) => {
                                                let _ = inbox_tx.send(command);
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if let Some(model) = pending_model {
                            agent.set_model(model);
                        }
                        if pending_reset {
                            agent.reset();
                        }
                        if pending_quit {
                            break 'worker;
                        }
                    }
                }
                AgentCommand::SetModel(model) => agent.set_model(model),
                AgentCommand::SetPermissionMode(mode) => agent.set_permission_mode(mode),
                AgentCommand::Compact => {
                    let cancel = CancellationToken::new();
                    let _ = agent
                        .run_compaction(CompactionTrigger::Manual, &event_tx, &cancel, None)
                        .await;
                }
                AgentCommand::ApplyAgent {
                    name,
                    instructions,
                    model,
                } => {
                    agent
                        .apply_agent_instructions(&name, &instructions, model, &event_tx)
                        .await;
                }
                AgentCommand::ApplySkill { name, instructions } => {
                    agent
                        .apply_skill_instructions(&name, &instructions, &event_tx)
                        .await;
                }
                AgentCommand::Reset => agent.reset(),
                AgentCommand::Cancel => {}
                AgentCommand::Approve { .. } => {}
                AgentCommand::Quit => break,
            }
        }
    });

    (cmd_tx, event_rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::HarnessConfig;
    use futures::stream;
    use genai::chat::{MessageContent, StreamChunk, Usage};
    use meerai_core::{
        FileChangeKind, FileDiff, ObservationStatus, ToolDefinition, ToolError, async_trait,
    };
    use meerai_policy::{ActionSelector, ApprovalMode, Effect, PermissionRule, PolicyEngine};
    use meerai_session::{COMPACTED_MARKER, Session, SessionConfig, SessionId};
    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::{Barrier, Notify};

    enum ScriptedStream {
        Events(Vec<ChatStreamEvent>),
        Error(String),
        Stall,
    }

    struct FakeStreamer {
        turns: Mutex<VecDeque<ScriptedStream>>,
        requests: Mutex<Vec<ChatRequest>>,
        options: Mutex<Vec<ChatOptions>>,
        models: Mutex<Vec<String>>,
    }

    impl FakeStreamer {
        fn new(turns: Vec<Vec<ChatStreamEvent>>) -> Self {
            Self::with_script(turns.into_iter().map(ScriptedStream::Events).collect())
        }

        fn with_script(turns: Vec<ScriptedStream>) -> Self {
            Self {
                turns: Mutex::new(turns.into()),
                requests: Mutex::new(Vec::new()),
                options: Mutex::new(Vec::new()),
                models: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ChatStreamer for FakeStreamer {
        async fn stream_chat(
            &self,
            model: &str,
            request: ChatRequest,
            options: &ChatOptions,
        ) -> Result<BoxedChatStream> {
            self.models.lock().unwrap().push(model.to_string());
            self.requests.lock().unwrap().push(request);
            self.options.lock().unwrap().push(options.clone());
            let scripted = self
                .turns
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected extra LLM turn");
            match scripted {
                ScriptedStream::Events(events) => {
                    Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
                }
                ScriptedStream::Error(message) => Err(anyhow!(message)),
                ScriptedStream::Stall => Ok(Box::pin(
                    stream::once(async { Ok(ChatStreamEvent::Start) })
                        .chain(stream::pending::<Result<ChatStreamEvent>>()),
                )),
            }
        }
    }

    struct EchoToolset;

    #[async_trait]
    impl Toolset for EchoToolset {
        fn name(&self) -> String {
            "echo".to_string()
        }

        fn definition(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                r#type: "function".to_string(),
                name: "echo-say".to_string(),
                description: "Echoes text or fails when asked".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": { "text": { "type": "string" } }
                }),
            }]
        }

        fn contain(&self, fn_name: &str) -> bool {
            fn_name == "echo-say"
        }

        async fn invoke(&self, _fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
            let v: serde_json::Value = serde_json::from_str(args)?;
            let text = v["text"].as_str().unwrap_or_default().to_string();
            if text == "fail" {
                Ok(ToolOutput::Fail("soft failure".to_string()))
            } else if text == "stop" {
                Ok(ToolOutput::Stop("done now".to_string()))
            } else {
                Ok(ToolOutput::Text(format!("echoed:{text}")))
            }
        }
    }

    fn text_end(text: &str) -> StreamEnd {
        StreamEnd {
            captured_content: Some(MessageContent::from_text(text)),
            captured_usage: Some(Usage {
                prompt_tokens: Some(10),
                completion_tokens: Some(5),
                total_tokens: Some(15),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn tool_end(calls: Vec<ToolCall>) -> StreamEnd {
        StreamEnd {
            captured_content: Some(MessageContent::from_tool_calls(calls)),
            ..Default::default()
        }
    }

    fn memory_session() -> Session {
        Session::memory(SessionId::new(), PathBuf::from("/tmp/workspace"))
    }

    async fn persistent_session(root: &Path) -> Session {
        let config = SessionConfig {
            root: root.to_path_buf(),
            workspace: root.to_path_buf(),
            project_local: false,
        };
        Session::open_or_create(&config, SessionId::new(), PathBuf::from("/tmp/workspace"))
            .await
            .unwrap()
    }

    struct StaticToolset {
        toolset: String,
        tool: String,
        output: String,
        effect: EffectClass,
    }

    impl StaticToolset {
        fn new(toolset: &str, tool: &str, output: impl Into<String>, effect: EffectClass) -> Self {
            Self {
                toolset: toolset.to_string(),
                tool: tool.to_string(),
                output: output.into(),
                effect,
            }
        }
    }

    #[async_trait]
    impl Toolset for StaticToolset {
        fn name(&self) -> String {
            self.toolset.clone()
        }

        fn definition(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                r#type: "function".to_string(),
                name: self.tool.clone(),
                description: "Returns fixed output".to_string(),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
            }]
        }

        fn contain(&self, fn_name: &str) -> bool {
            fn_name == self.tool
        }

        fn effect_of(&self, _fn_name: &str) -> EffectClass {
            self.effect
        }

        async fn invoke(&self, _fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::Text(self.output.clone()))
        }
    }

    fn big_output(lines: usize) -> String {
        (0..lines)
            .map(|index| format!("row {index} with padded content for spilling\n"))
            .collect()
    }

    fn tool_end_counting(calls: Vec<ToolCall>, total_tokens: i32) -> StreamEnd {
        StreamEnd {
            captured_content: Some(MessageContent::from_tool_calls(calls)),
            captured_usage: Some(Usage {
                prompt_tokens: Some(total_tokens),
                total_tokens: Some(total_tokens),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn harness(session: &Session, policy: PolicyEngine, config: HarnessConfig) -> AgentHarness {
        AgentHarness {
            session: session.clone(),
            policy,
            config,
        }
    }

    fn yolo() -> PolicyEngine {
        PolicyEngine::new(ApprovalMode::Yolo)
    }

    fn harness_agent(
        streamer: Arc<dyn ChatStreamer>,
        tools: Vec<Box<dyn Toolset>>,
        system_prompt: &str,
        session: &Session,
        policy: PolicyEngine,
        config: HarnessConfig,
    ) -> MultiTurnAgent {
        let mut agent = MultiTurnAgent::with_streamer(
            streamer,
            "gpt-4o-mini",
            tools,
            system_prompt,
            MultiTurnAgentConfig::default(),
        );
        agent.set_harness(harness(session, policy, config));
        agent
    }

    fn any_rule(effect: Effect) -> PermissionRule {
        PermissionRule {
            action: ActionSelector::Any,
            resource: "*".to_string(),
            effect,
        }
    }

    fn session_events(session: &Session) -> Vec<SessionEvent> {
        session
            .events()
            .into_iter()
            .map(|logged| logged.event)
            .collect()
    }

    fn event_kind(event: &SessionEvent) -> &'static str {
        match event {
            SessionEvent::TurnStarted { .. } => "turn_started",
            SessionEvent::TurnEnded { .. } => "turn_ended",
            SessionEvent::StepStarted { .. } => "step_started",
            SessionEvent::StepEnded { .. } => "step_ended",
            SessionEvent::SystemMessage { .. } => "system_message",
            SessionEvent::UserMessage { .. } => "user_message",
            SessionEvent::AssistantMessage { .. } => "assistant_message",
            SessionEvent::AssistantAttempt { .. } => "assistant_attempt",
            SessionEvent::ToolCall { .. } => "tool_call",
            SessionEvent::ToolResult { .. } => "tool_result",
            SessionEvent::SpillApplied { .. } => "spill_applied",
            SessionEvent::CompactionStarted { .. } => "compaction_started",
            SessionEvent::CompactionSummary { .. } => "compaction_summary",
            SessionEvent::CompactionEnded { .. } => "compaction_ended",
            SessionEvent::RevertApplied { .. } => "revert_applied",
            SessionEvent::UsageUpdated { .. } => "usage_updated",
            SessionEvent::ApprovalRequested { .. } => "approval_requested",
            SessionEvent::ApprovalDecided { .. } => "approval_decided",
            SessionEvent::HookFired { .. } => "hook_fired",
            SessionEvent::CheckpointCreated { .. } => "checkpoint_created",
            SessionEvent::CircuitBreaker { .. } => "circuit_breaker",
            SessionEvent::Error { .. } => "error",
        }
    }

    fn tool_call(call_id: &str, text: &str) -> ToolCall {
        ToolCall {
            call_id: call_id.to_string(),
            fn_name: "echo-say".to_string(),
            fn_arguments: serde_json::json!({ "text": text }),
            thought_signatures: None,
        }
    }

    struct SlowToolset;

    #[async_trait]
    impl Toolset for SlowToolset {
        fn name(&self) -> String {
            "slow".to_string()
        }

        fn definition(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                r#type: "function".to_string(),
                name: "slow-run".to_string(),
                description: "Sleeps past any reasonable timeout".to_string(),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
            }]
        }

        fn contain(&self, fn_name: &str) -> bool {
            fn_name == "slow-run"
        }

        fn effect_of(&self, _fn_name: &str) -> EffectClass {
            EffectClass::Execute
        }

        async fn invoke(&self, _fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(ToolOutput::Text("finished".to_string()))
        }
    }

    struct DiffToolset;

    #[async_trait]
    impl Toolset for DiffToolset {
        fn name(&self) -> String {
            "diff".to_string()
        }

        fn definition(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                r#type: "function".to_string(),
                name: "diff-write".to_string(),
                description: "Writes a file and reports a unified diff".to_string(),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
            }]
        }

        fn contain(&self, fn_name: &str) -> bool {
            fn_name == "diff-write"
        }

        fn effect_of(&self, _fn_name: &str) -> EffectClass {
            EffectClass::Write
        }

        async fn invoke(&self, fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
            Ok(self.invoke_structured(fn_name, args).await?.output)
        }

        async fn invoke_structured(
            &self,
            _fn_name: &str,
            _args: &str,
        ) -> Result<ToolRun, ToolError> {
            Ok(ToolRun {
                output: ToolOutput::Text("wrote a.rs".to_string()),
                observation: ToolObservation::success("wrote a.rs"),
                file_changes: vec![FileDiff {
                    path: "a.rs".to_string(),
                    kind: FileChangeKind::Modified,
                    unified_diff: "@@ -1 +1 @@\n-old\n+new".to_string(),
                }],
            })
        }
    }

    const LEAKED_TOKEN: &str = "sk-live-abcdefghijklmnopqrstuvwxyz012345";

    struct LeakToolset;

    #[async_trait]
    impl Toolset for LeakToolset {
        fn name(&self) -> String {
            "leak".to_string()
        }

        fn definition(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                r#type: "function".to_string(),
                name: "leak-echo".to_string(),
                description: "Returns text that contains a secret".to_string(),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
            }]
        }

        fn contain(&self, fn_name: &str) -> bool {
            fn_name == "leak-echo"
        }

        async fn invoke(&self, _fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::Text(format!("OPENAI_API_KEY={LEAKED_TOKEN}")))
        }
    }

    struct FailingToolset;

    #[async_trait]
    impl Toolset for FailingToolset {
        fn name(&self) -> String {
            "failing".to_string()
        }

        fn definition(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                r#type: "function".to_string(),
                name: "failing-run".to_string(),
                description: "Always fails with a secret in the error message".to_string(),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
            }]
        }

        fn contain(&self, fn_name: &str) -> bool {
            fn_name == "failing-run"
        }

        async fn invoke(&self, _fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
            Err(ToolError::Unknown(anyhow!(
                "provider rejected key {LEAKED_TOKEN}"
            )))
        }
    }

    struct LeakDiffToolset;

    #[async_trait]
    impl Toolset for LeakDiffToolset {
        fn name(&self) -> String {
            "leak_diff".to_string()
        }

        fn definition(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                r#type: "function".to_string(),
                name: "leak-diff".to_string(),
                description: "Reports a diff that contains a secret".to_string(),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
            }]
        }

        fn contain(&self, fn_name: &str) -> bool {
            fn_name == "leak-diff"
        }

        fn effect_of(&self, _fn_name: &str) -> EffectClass {
            EffectClass::Write
        }

        async fn invoke(&self, fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
            Ok(self.invoke_structured(fn_name, args).await?.output)
        }

        async fn invoke_structured(
            &self,
            _fn_name: &str,
            _args: &str,
        ) -> Result<ToolRun, ToolError> {
            Ok(ToolRun {
                output: ToolOutput::Text("wrote a.rs".to_string()),
                observation: ToolObservation::success("wrote a.rs"),
                file_changes: vec![FileDiff {
                    path: "a.rs".to_string(),
                    kind: FileChangeKind::Modified,
                    unified_diff: format!("+token={LEAKED_TOKEN}"),
                }],
            })
        }
    }

    struct GatedStreamer {
        turns: Mutex<VecDeque<Vec<ChatStreamEvent>>>,
        requests: Mutex<Vec<ChatRequest>>,
        gate_index: usize,
        started: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[async_trait]
    impl ChatStreamer for GatedStreamer {
        async fn stream_chat(
            &self,
            _model: &str,
            request: ChatRequest,
            _options: &ChatOptions,
        ) -> Result<BoxedChatStream> {
            let index = {
                let mut requests = self.requests.lock().unwrap();
                requests.push(request);
                requests.len() - 1
            };
            if index == self.gate_index {
                self.started.notify_one();
                self.release.notified().await;
            }
            let events = self
                .turns
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected extra LLM turn");
            Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
        }
    }

    #[tokio::test]
    async fn streams_text_and_persists_history() {
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![
                ChatStreamEvent::Start,
                ChatStreamEvent::Chunk(StreamChunk {
                    content: "Hello ".to_string(),
                }),
                ChatStreamEvent::Chunk(StreamChunk {
                    content: "world!".to_string(),
                }),
                ChatStreamEvent::End(text_end("Hello world!")),
            ],
            vec![
                ChatStreamEvent::Chunk(StreamChunk {
                    content: "Second reply".to_string(),
                }),
                ChatStreamEvent::End(text_end("Second reply")),
            ],
        ]));

        let mut agent = MultiTurnAgent::with_streamer(
            streamer.clone(),
            "gemini-3.8-flash",
            vec![],
            "Be concise.",
            MultiTurnAgentConfig::default(),
        );

        let (tx, mut rx) = mpsc::unbounded_channel();
        let reply1 = agent
            .prompt_streaming("Hi", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(reply1, "Hello world!");

        let reply2 = agent
            .prompt_streaming("Follow up", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(reply2, "Second reply");
        assert_eq!(agent.history().len(), 4);

        let reqs = streamer.requests.lock().unwrap();
        assert_eq!(reqs[1].messages.len(), 3);

        let initial_session = agent.session_id().to_string();
        assert!(initial_session.starts_with("ses_"));
        let opts = streamer.options.lock().unwrap();
        assert_eq!(opts.len(), 2);
        for opt in opts.iter() {
            let headers = opt.extra_headers.as_ref().expect("extra_headers");
            let session_hdr = headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("x-opencode-session"))
                .map(|(_, v)| v.as_str());
            assert_eq!(session_hdr, Some(initial_session.as_str()));
            let ua_hdr = headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
                .map(|(_, v)| v.as_str());
            assert!(ua_hdr.is_some_and(|v| v.starts_with("meerai/")));
        }

        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }

        let mut deltas = Vec::new();
        let mut started_messages = 0;
        let mut completed_messages = 0;
        let mut turn_starts = 0;
        let mut turn_completions = 0;
        let mut usage_updates = Vec::new();
        for ev in &events {
            match ev {
                AgentEvent::ItemDelta {
                    delta: ItemDelta::AgentText(d),
                    ..
                } => deltas.push(d.clone()),
                AgentEvent::ItemStarted {
                    item: SessionItem::AgentMessage { text, .. },
                } => {
                    assert!(text.is_empty());
                    started_messages += 1;
                }
                AgentEvent::ItemCompleted {
                    item: SessionItem::AgentMessage { .. },
                } => completed_messages += 1,
                AgentEvent::TurnStarted { .. } => turn_starts += 1,
                AgentEvent::TurnCompleted {
                    reason: TurnEndReason::Completed,
                    ..
                } => turn_completions += 1,
                AgentEvent::UsageUpdated {
                    usage, cumulative, ..
                } => usage_updates.push((*usage, *cumulative)),
                _ => {}
            }
        }
        assert_eq!(deltas, vec!["Hello ", "world!", "Second reply"]);
        assert_eq!(started_messages, 2);
        assert_eq!(completed_messages, 2);
        assert_eq!(turn_starts, 2);
        assert_eq!(turn_completions, 2);
        assert_eq!(usage_updates.len(), 2);
        assert_eq!(usage_updates[0].0.total_tokens, Some(15));
        assert_eq!(usage_updates[0].1.total_tokens, Some(15));
        assert_eq!(usage_updates[1].0.total_tokens, Some(15));
        assert_eq!(usage_updates[1].1.total_tokens, Some(30));
        assert_eq!(usage_updates[1].1.prompt_tokens, Some(20));
        assert_eq!(usage_updates[1].1.completion_tokens, Some(10));

        agent.reset();
        assert!(agent.history().is_empty());
        assert_ne!(agent.session_id(), initial_session);
    }

    #[tokio::test]
    async fn executes_multiple_tool_calls_and_feeds_failures_back() {
        let calls = vec![
            ToolCall {
                call_id: "c1".to_string(),
                fn_name: "echo-say".to_string(),
                fn_arguments: serde_json::json!({"text": "ok"}),
                thought_signatures: None,
            },
            ToolCall {
                call_id: "c2".to_string(),
                fn_name: "echo-say".to_string(),
                fn_arguments: serde_json::json!({"text": "fail"}),
                thought_signatures: None,
            },
        ];

        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(calls))],
            vec![
                ChatStreamEvent::Chunk(StreamChunk {
                    content: "Handled both tools".to_string(),
                }),
                ChatStreamEvent::End(text_end("Handled both tools")),
            ],
        ]));

        let mut agent = MultiTurnAgent::with_streamer(
            streamer.clone(),
            "gpt-4o-mini",
            vec![Box::new(EchoToolset)],
            "",
            MultiTurnAgentConfig::default(),
        );

        let (tx, mut rx) = mpsc::unbounded_channel();
        let out = agent
            .prompt_streaming("Run tools", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out, "Handled both tools");

        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }

        let mut started = Vec::new();
        let mut running = Vec::new();
        let mut completed = Vec::new();
        for ev in &events {
            match ev {
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
                    started.push((call_id.clone(), name.clone(), args.clone(), *effect));
                    assert_eq!(*status, ItemStatus::Proposed);
                    assert!(output.is_none());
                }
                AgentEvent::ItemDelta {
                    delta: ItemDelta::StatusChanged(status),
                    ..
                } => running.push(*status),
                AgentEvent::ItemCompleted {
                    item:
                        SessionItem::ToolCall {
                            call_id,
                            status,
                            output,
                            ..
                        },
                } => completed.push((call_id.clone(), *status, output.clone())),
                _ => {}
            }
        }

        assert_eq!(started.len(), 2);
        assert_eq!(started[0].0, CallId::from_raw("c1"));
        assert_eq!(started[0].1, "echo-say");
        assert_eq!(started[0].2, "{\"text\":\"ok\"}");
        assert_eq!(started[0].3, EffectClass::Read);
        assert_eq!(started[1].0, CallId::from_raw("c2"));
        assert_eq!(
            running,
            vec![ItemStatus::InProgress, ItemStatus::InProgress]
        );
        assert_eq!(
            completed,
            vec![
                (
                    CallId::from_raw("c1"),
                    ItemStatus::Succeeded,
                    Some("echoed:ok".to_string())
                ),
                (
                    CallId::from_raw("c2"),
                    ItemStatus::Failed,
                    Some("soft failure".to_string())
                ),
            ]
        );

        assert!(events.iter().any(|ev| matches!(
            ev,
            AgentEvent::TurnCompleted {
                reason: TurnEndReason::Completed,
                ..
            }
        )));

        let reqs = streamer.requests.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[1].messages.len(), 4);
    }

    #[tokio::test]
    async fn stops_immediately_on_tool_output_stop() {
        let calls = vec![ToolCall {
            call_id: "c-stop".to_string(),
            fn_name: "echo-say".to_string(),
            fn_arguments: serde_json::json!({"text": "stop"}),
            thought_signatures: None,
        }];

        let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
            tool_end(calls),
        )]]));

        let mut agent = MultiTurnAgent::with_streamer(
            streamer,
            "gpt-4o-mini",
            vec![Box::new(EchoToolset)],
            "",
            MultiTurnAgentConfig::default(),
        );

        let (tx, mut rx) = mpsc::unbounded_channel();
        let out = agent
            .prompt_streaming("Stop please", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out, "done now");

        let mut stopped = None;
        let mut completed_turn = false;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                AgentEvent::ItemCompleted {
                    item: SessionItem::ToolCall { status, output, .. },
                } => stopped = Some((status, output)),
                AgentEvent::TurnCompleted {
                    reason: TurnEndReason::Completed,
                    ..
                } => completed_turn = true,
                _ => {}
            }
        }
        assert_eq!(
            stopped,
            Some((ItemStatus::Stopped, Some("done now".to_string())))
        );
        assert!(completed_turn);
    }

    #[tokio::test]
    async fn reports_budget_errors_as_failed_turns() {
        let calls = vec![ToolCall {
            call_id: "c1".to_string(),
            fn_name: "echo-say".to_string(),
            fn_arguments: serde_json::json!({"text": "ok"}),
            thought_signatures: None,
        }];

        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(calls.clone()))],
            vec![ChatStreamEvent::End(tool_end(calls))],
        ]));

        let agent = MultiTurnAgent::with_streamer(
            streamer,
            "gpt-4o-mini",
            vec![Box::new(EchoToolset)],
            "",
            MultiTurnAgentConfig {
                max_cycles: 2,
                max_retries: 1,
            },
        );

        let (cmd_tx, mut rx) = spawn_worker(agent);
        cmd_tx
            .send(AgentCommand::SubmitPrompt("first".to_string()))
            .unwrap();

        let mut saw_error = None;
        let mut saw_failed_turn = None;
        while let Some(ev) = rx.recv().await {
            match ev {
                AgentEvent::Error { code, message } => saw_error = Some((code, message)),
                AgentEvent::TurnCompleted {
                    reason: TurnEndReason::Failed { error },
                    ..
                } => {
                    saw_failed_turn = Some(error);
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(saw_error.map(|(code, _)| code), Some(ErrorCode::Budget));
        assert!(
            saw_failed_turn
                .as_deref()
                .is_some_and(|msg| msg.contains("Exceeded maximum number of cycles"))
        );

        let _ = cmd_tx.send(AgentCommand::Quit);
    }

    #[tokio::test]
    async fn queues_steering_as_next_prompt() {
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![
                ChatStreamEvent::Chunk(StreamChunk {
                    content: "one".to_string(),
                }),
                ChatStreamEvent::End(text_end("one")),
            ],
            vec![
                ChatStreamEvent::Chunk(StreamChunk {
                    content: "two".to_string(),
                }),
                ChatStreamEvent::End(text_end("two")),
            ],
        ]));

        let agent = MultiTurnAgent::with_streamer(
            streamer.clone(),
            "gpt-4o-mini",
            vec![],
            "",
            MultiTurnAgentConfig::default(),
        );

        let (cmd_tx, mut rx) = spawn_worker(agent);
        cmd_tx
            .send(AgentCommand::SubmitPrompt("first".to_string()))
            .unwrap();
        cmd_tx
            .send(AgentCommand::Steer("second".to_string()))
            .unwrap();

        let mut completions = 0;
        while let Some(ev) = rx.recv().await {
            if matches!(ev, AgentEvent::TurnCompleted { .. }) {
                completions += 1;
                if completions == 2 {
                    break;
                }
            }
        }
        assert_eq!(completions, 2);

        let reqs = streamer.requests.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        assert!(format!("{:?}", reqs[1].messages).contains("second"));

        let _ = cmd_tx.send(AgentCommand::Quit);
    }

    #[tokio::test]
    async fn harness_dual_writes_session_events() {
        let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
            tool_end(vec![tool_call("c1", "stop")]),
        )]]));
        let session = memory_session();
        let mut agent = MultiTurnAgent::with_streamer(
            streamer,
            "gemini::gemini-3.8-flash",
            vec![Box::new(EchoToolset)],
            "Be precise.",
            MultiTurnAgentConfig::default(),
        );
        agent.set_harness(harness(&session, yolo(), HarnessConfig::default()));

        let (tx, _rx) = mpsc::unbounded_channel();
        let out = agent
            .prompt_streaming("do it", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out, "done now");

        let events = session_events(&session);
        let kinds: Vec<&str> = events.iter().map(event_kind).collect();
        assert_eq!(
            kinds,
            vec![
                "turn_started",
                "user_message",
                "system_message",
                "step_started",
                "assistant_message",
                "tool_call",
                "tool_result",
                "usage_updated",
                "step_ended",
                "turn_ended",
            ]
        );

        match &events[2] {
            SessionEvent::SystemMessage {
                sections,
                generation,
            } => {
                assert_eq!(*generation, 0);
                assert_eq!(sections.len(), 1);
                assert_eq!(sections[0].label, "instructions");
                assert_eq!(sections[0].content, "Be precise.");
            }
            other => panic!("expected SystemMessage, got {other:?}"),
        }

        match &events[4] {
            SessionEvent::AssistantMessage {
                provider,
                model,
                tool_calls,
                ..
            } => {
                assert_eq!(provider, "gemini");
                assert_eq!(model, "gemini::gemini-3.8-flash");
                assert_eq!(tool_calls.len(), 1);
                assert_eq!(tool_calls[0].id, "c1");
                assert_eq!(tool_calls[0].name, "echo-say");
            }
            other => panic!("expected AssistantMessage, got {other:?}"),
        }

        match &events[6] {
            SessionEvent::ToolResult { call_id, output } => {
                assert_eq!(call_id.as_str(), "c1");
                assert_eq!(output.status, ObservationStatus::Stopped);
                assert_eq!(output.stdout, "done now");
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }

        match &events[9] {
            SessionEvent::TurnEnded { reason, .. } => {
                assert_eq!(reason, &TurnEndReason::Completed);
            }
            other => panic!("expected TurnEnded, got {other:?}"),
        }

        assert_eq!(agent.history().len(), 3);
    }

    #[tokio::test]
    async fn harness_builds_requests_from_projection() {
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![
                ChatStreamEvent::ReasoningChunk(StreamChunk {
                    content: "secret thoughts".to_string(),
                }),
                ChatStreamEvent::End(tool_end(vec![tool_call("c1", "ok")])),
            ],
            vec![
                ChatStreamEvent::ReasoningChunk(StreamChunk {
                    content: "more thoughts".to_string(),
                }),
                ChatStreamEvent::Chunk(StreamChunk {
                    content: "final".to_string(),
                }),
                ChatStreamEvent::End(text_end("final")),
            ],
        ]));

        let session = memory_session();
        let mut agent = MultiTurnAgent::with_streamer(
            streamer.clone(),
            "gpt-4o-mini",
            vec![Box::new(EchoToolset)],
            "Be precise.",
            MultiTurnAgentConfig::default(),
        );
        agent.set_harness(harness(&session, yolo(), HarnessConfig::default()));

        let (tx, _rx) = mpsc::unbounded_channel();
        let out = agent
            .prompt_streaming("read it", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out, "final");

        let reqs = streamer.requests.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[1].messages.len(), 3);
        assert_eq!(reqs[1].system.as_deref(), Some("Be precise."));
        let rendered = format!("{:?}", reqs[1].messages);
        assert!(!rendered.contains("secret thoughts"));
        assert!(!rendered.contains("more thoughts"));
        assert!(rendered.contains("echoed:ok"));

        let reasoning_logged = session_events(&session).iter().any(|event| {
            matches!(
                event,
                SessionEvent::AssistantMessage { content, .. }
                    if content.contains(&ContentBlock::Reasoning("secret thoughts".to_string()))
            )
        });
        assert!(reasoning_logged);
    }

    #[tokio::test]
    async fn policy_denies_tool_call_and_turn_continues() {
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(vec![tool_call("c1", "ok")]))],
            vec![ChatStreamEvent::End(text_end("recovered"))],
        ]));

        let session = memory_session();
        let policy = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![any_rule(Effect::Deny)]);
        let mut agent = MultiTurnAgent::with_streamer(
            streamer.clone(),
            "gpt-4o-mini",
            vec![Box::new(EchoToolset)],
            "",
            MultiTurnAgentConfig::default(),
        );
        agent.set_harness(harness(&session, policy, HarnessConfig::default()));

        let (tx, mut rx) = mpsc::unbounded_channel();
        let out = agent
            .prompt_streaming("go", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out, "recovered");

        let mut denied = None;
        while let Ok(event) = rx.try_recv() {
            if let AgentEvent::ItemCompleted {
                item: SessionItem::ToolCall { status, output, .. },
            } = event
            {
                denied = Some((status, output));
            }
        }
        let (status, output) = denied.expect("tool item completed");
        assert_eq!(status, ItemStatus::Denied);
        assert!(output.is_some_and(|text| text.contains("denied")));

        let events = session_events(&session);
        let refused = events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::ToolResult { output, .. }
                    if output.status == ObservationStatus::Refused
            )
        });
        assert!(refused);
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::TurnEnded {
                reason: TurnEndReason::Completed,
                ..
            }
        )));

        let reqs = streamer.requests.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        assert!(format!("{:?}", reqs[1].messages).contains("Refused: denied by permission rule"));
    }

    #[tokio::test]
    async fn approval_accept_executes_the_tool() {
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(vec![tool_call("c1", "ok")]))],
            vec![ChatStreamEvent::End(text_end("approved"))],
        ]));

        let session = memory_session();
        let policy = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![any_rule(Effect::Ask)]);
        let mut agent = MultiTurnAgent::with_streamer(
            streamer.clone(),
            "gpt-4o-mini",
            vec![Box::new(EchoToolset)],
            "",
            MultiTurnAgentConfig::default(),
        );
        agent.set_harness(harness(&session, policy, HarnessConfig::default()));

        let (cmd_tx, mut rx) = spawn_worker(agent);
        cmd_tx
            .send(AgentCommand::SubmitPrompt("go".to_string()))
            .unwrap();

        let mut requested = 0;
        let mut resolved = 0;
        let mut flow: Vec<String> = Vec::new();
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::ItemDelta {
                    delta: ItemDelta::StatusChanged(status),
                    ..
                } => flow.push(format!("{status:?}")),
                AgentEvent::ApprovalRequested { request } => {
                    assert_eq!(request.kind, ApprovalKind::ToolCall);
                    assert_eq!(request.effect, Some(EffectClass::Read));
                    requested += 1;
                    flow.push("requested".to_string());
                    cmd_tx
                        .send(AgentCommand::Approve {
                            request_id: request.request_id,
                            decision: ApprovalDecision::Accept,
                            scope: None,
                        })
                        .unwrap();
                }
                AgentEvent::ApprovalResolved { .. } => {
                    resolved += 1;
                    flow.push("resolved".to_string());
                }
                AgentEvent::TurnCompleted { .. } => break,
                _ => {}
            }
        }
        assert_eq!(requested, 1);
        assert_eq!(resolved, 1);
        assert_eq!(
            flow,
            vec![
                format!("{:?}", ItemStatus::Pending),
                "requested".to_string(),
                "resolved".to_string(),
                format!("{:?}", ItemStatus::InProgress),
            ],
            "pending before the request, in-progress only after the accept"
        );

        let events = session_events(&session);
        let decided_without_scope = events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::ApprovalDecided {
                    decision: ApprovalDecision::Accept,
                    scope: None,
                    ..
                }
            )
        });
        assert!(decided_without_scope);

        let reqs = streamer.requests.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        assert!(format!("{:?}", reqs[1].messages).contains("echoed:ok"));

        let _ = cmd_tx.send(AgentCommand::Quit);
    }

    #[tokio::test]
    async fn accept_for_session_records_grant_and_skips_reprompt() {
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(vec![
                tool_call("c1", "ok"),
                tool_call("c2", "ok"),
            ]))],
            vec![ChatStreamEvent::End(text_end("both"))],
        ]));

        let session = memory_session();
        let policy = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![any_rule(Effect::Ask)]);
        let mut agent = MultiTurnAgent::with_streamer(
            streamer.clone(),
            "gpt-4o-mini",
            vec![Box::new(EchoToolset)],
            "",
            MultiTurnAgentConfig::default(),
        );
        agent.set_harness(harness(&session, policy, HarnessConfig::default()));

        let (commands_tx, mut commands_rx) = mpsc::unbounded_channel();
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let driver = tokio::spawn(async move {
            let mut requested = 0;
            while let Some(event) = event_rx.recv().await {
                match event {
                    AgentEvent::ApprovalRequested { request } => {
                        requested += 1;
                        let _ = commands_tx.send(AgentCommand::Approve {
                            request_id: request.request_id,
                            decision: ApprovalDecision::AcceptForSession,
                            scope: None,
                        });
                    }
                    AgentEvent::TurnCompleted { .. } => break,
                    _ => {}
                }
            }
            requested
        });

        let out = agent
            .prompt_streaming_with_commands(
                "go",
                &event_tx,
                CancellationToken::new(),
                &mut commands_rx,
            )
            .await
            .unwrap();
        assert_eq!(out, "both");
        assert_eq!(driver.await.unwrap(), 1);
        assert_eq!(agent.harness().unwrap().policy.grants().len(), 1);

        let reqs = streamer.requests.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        let rendered = format!("{:?}", reqs[1].messages);
        assert_eq!(rendered.matches("echoed:ok").count(), 2);
    }

    #[tokio::test]
    async fn steering_is_applied_at_the_step_boundary() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let streamer = Arc::new(GatedStreamer {
            turns: Mutex::new(
                vec![
                    vec![
                        ChatStreamEvent::Chunk(StreamChunk {
                            content: "one".to_string(),
                        }),
                        ChatStreamEvent::End(text_end("one")),
                    ],
                    vec![
                        ChatStreamEvent::Chunk(StreamChunk {
                            content: "two".to_string(),
                        }),
                        ChatStreamEvent::End(text_end("two")),
                    ],
                ]
                .into(),
            ),
            requests: Mutex::new(Vec::new()),
            gate_index: 0,
            started: started.clone(),
            release: release.clone(),
        });

        let session = memory_session();
        let mut agent = MultiTurnAgent::with_streamer(
            streamer.clone(),
            "gpt-4o-mini",
            vec![],
            "",
            MultiTurnAgentConfig::default(),
        );
        agent.set_harness(harness(&session, yolo(), HarnessConfig::default()));

        let (cmd_tx, mut rx) = spawn_worker(agent);
        cmd_tx
            .send(AgentCommand::SubmitPrompt("first".to_string()))
            .unwrap();
        started.notified().await;
        cmd_tx
            .send(AgentCommand::Steer("also add tests".to_string()))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(25)).await;
        release.notify_one();

        loop {
            match rx.recv().await {
                Some(AgentEvent::TurnCompleted { .. }) => break,
                Some(_) => {}
                None => panic!("worker stopped mid-turn"),
            }
        }

        let reqs = streamer.requests.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        assert!(format!("{:?}", reqs[1].messages).contains("also add tests"));

        let steered = session_events(&session).iter().any(|event| {
            matches!(
                event,
                SessionEvent::UserMessage { source: InputSource::Steering, content }
                    if content == &vec![ContentBlock::Text("also add tests".to_string())]
            )
        });
        assert!(steered);

        let _ = cmd_tx.send(AgentCommand::Quit);
    }

    #[tokio::test]
    async fn apply_commands_arrive_at_the_next_step_boundary_without_cancelling() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let streamer = Arc::new(GatedStreamer {
            turns: Mutex::new(
                vec![
                    vec![ChatStreamEvent::End(tool_end(vec![tool_call("c1", "ok")]))],
                    vec![ChatStreamEvent::End(text_end("done"))],
                ]
                .into(),
            ),
            requests: Mutex::new(Vec::new()),
            gate_index: 0,
            started: started.clone(),
            release: release.clone(),
        });

        let session = memory_session();
        let agent = harness_agent(
            streamer.clone(),
            vec![Box::new(EchoToolset)],
            "",
            &session,
            yolo(),
            HarnessConfig::default(),
        );

        let (cmd_tx, mut rx) = spawn_worker(agent);
        cmd_tx
            .send(AgentCommand::SubmitPrompt("first".to_string()))
            .unwrap();
        started.notified().await;
        cmd_tx
            .send(AgentCommand::ApplySkill {
                name: "tester".to_string(),
                instructions: "Add tests.".to_string(),
            })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(25)).await;
        release.notify_one();

        let reason = loop {
            match rx.recv().await {
                Some(AgentEvent::TurnCompleted { reason, .. }) => break reason,
                Some(_) => {}
                None => panic!("worker stopped mid-turn"),
            }
        };
        assert_eq!(reason, TurnEndReason::Completed);

        let reqs = streamer.requests.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        assert!(
            !format!("{:?}", reqs[0].messages).contains("Skill instructions (tester):"),
            "the in-flight request must not see the skill"
        );
        assert!(format!("{:?}", reqs[1].messages).contains("Skill instructions (tester):"));
        assert!(format!("{:?}", reqs[1].messages).contains("Add tests."));

        let applied = session_events(&session)
            .into_iter()
            .filter(|event| {
                matches!(
                    event,
                    SessionEvent::UserMessage { source: InputSource::System, content }
                        if content
                            == &vec![ContentBlock::Text(
                                "Skill instructions (tester):\n\nAdd tests.".to_string()
                            )]
                )
            })
            .count();
        assert_eq!(applied, 1);

        let _ = cmd_tx.send(AgentCommand::Quit);
    }

    #[tokio::test]
    async fn idle_apply_agent_logs_instruction_and_switches_model() {
        let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
            text_end("done"),
        )]]));

        let session = memory_session();
        let agent = harness_agent(
            streamer.clone(),
            vec![],
            "",
            &session,
            yolo(),
            HarnessConfig::default(),
        );

        let (cmd_tx, mut rx) = spawn_worker(agent);
        cmd_tx
            .send(AgentCommand::ApplyAgent {
                name: "reviewer".to_string(),
                instructions: "Review every change.".to_string(),
                model: Some("gpt-5-mini".to_string()),
            })
            .unwrap();
        cmd_tx
            .send(AgentCommand::SubmitPrompt("hello".to_string()))
            .unwrap();

        loop {
            match rx.recv().await {
                Some(AgentEvent::TurnCompleted { .. }) => break,
                Some(_) => {}
                None => panic!("worker stopped mid-turn"),
            }
        }

        let logged = session_events(&session);
        let applied = logged
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    SessionEvent::UserMessage { source: InputSource::System, content }
                        if content
                            == &vec![ContentBlock::Text(
                                "Agent instructions (reviewer):\n\nReview every change.".to_string()
                            )]
                )
            })
            .count();
        assert_eq!(applied, 1);

        let models = streamer.models.lock().unwrap().clone();
        assert_eq!(models, vec!["gpt-5-mini".to_string()]);
        let reqs = streamer.requests.lock().unwrap();
        assert!(format!("{:?}", reqs[0].messages).contains("Agent instructions (reviewer):"));
        assert!(format!("{:?}", reqs[0].messages).contains("Review every change."));

        let _ = cmd_tx.send(AgentCommand::Quit);
    }

    #[tokio::test]
    async fn idle_apply_skill_logs_instruction_without_changing_model() {
        let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
            text_end("done"),
        )]]));

        let session = memory_session();
        let agent = harness_agent(
            streamer.clone(),
            vec![],
            "",
            &session,
            yolo(),
            HarnessConfig::default(),
        );

        let (cmd_tx, mut rx) = spawn_worker(agent);
        cmd_tx
            .send(AgentCommand::ApplySkill {
                name: "tester".to_string(),
                instructions: "Add tests.".to_string(),
            })
            .unwrap();
        cmd_tx
            .send(AgentCommand::SubmitPrompt("hello".to_string()))
            .unwrap();

        loop {
            match rx.recv().await {
                Some(AgentEvent::TurnCompleted { .. }) => break,
                Some(_) => {}
                None => panic!("worker stopped mid-turn"),
            }
        }

        let logged = session_events(&session);
        let applied = logged
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    SessionEvent::UserMessage { source: InputSource::System, content }
                        if content
                            == &vec![ContentBlock::Text(
                                "Skill instructions (tester):\n\nAdd tests.".to_string()
                            )]
                )
            })
            .count();
        assert_eq!(applied, 1);

        let models = streamer.models.lock().unwrap().clone();
        assert_eq!(models, vec!["gpt-4o-mini".to_string()]);
        let reqs = streamer.requests.lock().unwrap();
        assert!(format!("{:?}", reqs[0].messages).contains("Skill instructions (tester):"));
        assert!(format!("{:?}", reqs[0].messages).contains("Add tests."));

        let _ = cmd_tx.send(AgentCommand::Quit);
    }

    #[tokio::test]
    async fn apply_agent_without_harness_updates_history_and_model() {
        let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
            text_end("done"),
        )]]));

        let agent = MultiTurnAgent::with_streamer(
            streamer.clone(),
            "gpt-4o-mini",
            vec![],
            "",
            MultiTurnAgentConfig::default(),
        );

        let (cmd_tx, mut rx) = spawn_worker(agent);
        cmd_tx
            .send(AgentCommand::ApplyAgent {
                name: "reviewer".to_string(),
                instructions: "Review every change.".to_string(),
                model: Some("gpt-5-mini".to_string()),
            })
            .unwrap();
        cmd_tx
            .send(AgentCommand::SubmitPrompt("hello".to_string()))
            .unwrap();

        loop {
            match rx.recv().await {
                Some(AgentEvent::TurnCompleted { .. }) => break,
                Some(_) => {}
                None => panic!("worker stopped mid-turn"),
            }
        }

        let models = streamer.models.lock().unwrap().clone();
        assert_eq!(models, vec!["gpt-5-mini".to_string()]);
        let reqs = streamer.requests.lock().unwrap();
        assert!(format!("{:?}", reqs[0].messages).contains("Agent instructions (reviewer):"));
        assert!(format!("{:?}", reqs[0].messages).contains("Review every change."));

        let _ = cmd_tx.send(AgentCommand::Quit);
    }

    #[tokio::test]
    async fn tool_timeout_fails_the_call() {
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
                call_id: "c1".to_string(),
                fn_name: "slow-run".to_string(),
                fn_arguments: serde_json::json!({}),
                thought_signatures: None,
            }]))],
            vec![ChatStreamEvent::End(text_end("after timeout"))],
        ]));

        let session = memory_session();
        let config = HarnessConfig {
            tool_timeout_ms: 10,
            ..HarnessConfig::default()
        };
        let mut agent = MultiTurnAgent::with_streamer(
            streamer.clone(),
            "gpt-4o-mini",
            vec![Box::new(SlowToolset)],
            "",
            MultiTurnAgentConfig::default(),
        );
        agent.set_harness(harness(&session, yolo(), config));

        let (tx, _rx) = mpsc::unbounded_channel();
        let out = agent
            .prompt_streaming("run", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out, "after timeout");

        let observation = session_events(&session)
            .into_iter()
            .find_map(|event| match event {
                SessionEvent::ToolResult { output, .. } => Some(output),
                _ => None,
            })
            .expect("tool result logged");
        assert_eq!(observation.status, ObservationStatus::Error);
        assert!(observation.stderr.contains("tool timed out after 10ms"));

        let reqs = streamer.requests.lock().unwrap();
        assert!(format!("{:?}", reqs[1].messages).contains("tool timed out after 10ms"));
    }

    #[tokio::test]
    async fn file_changes_emit_items() {
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
                call_id: "c1".to_string(),
                fn_name: "diff-write".to_string(),
                fn_arguments: serde_json::json!({}),
                thought_signatures: None,
            }]))],
            vec![ChatStreamEvent::End(text_end("wrote"))],
        ]));

        let session = memory_session();
        let mut agent = MultiTurnAgent::with_streamer(
            streamer,
            "gpt-4o-mini",
            vec![Box::new(DiffToolset)],
            "",
            MultiTurnAgentConfig::default(),
        );
        agent.set_harness(harness(&session, yolo(), HarnessConfig::default()));

        let (tx, mut rx) = mpsc::unbounded_channel();
        let out = agent
            .prompt_streaming("write", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out, "wrote");

        let mut started_changes = 0;
        let mut completed_changes = Vec::new();
        while let Ok(event) = rx.try_recv() {
            match event {
                AgentEvent::ItemStarted {
                    item: SessionItem::FileChange { status, .. },
                } => {
                    assert_eq!(status, ItemStatus::InProgress);
                    started_changes += 1;
                }
                AgentEvent::ItemCompleted {
                    item:
                        SessionItem::FileChange {
                            changes, status, ..
                        },
                } => completed_changes.push((changes, status)),
                _ => {}
            }
        }
        assert_eq!(started_changes, 1);
        assert_eq!(completed_changes.len(), 1);
        assert_eq!(completed_changes[0].0.len(), 1);
        assert_eq!(completed_changes[0].0[0].path, "a.rs");
        assert_eq!(completed_changes[0].1, ItemStatus::Succeeded);
    }

    #[test]
    fn estimates_context_tokens_from_message_text() {
        assert_eq!(estimate_context_tokens(&[]), 0);
        assert_eq!(estimate_context_tokens(&[ChatMessage::user("abcdefgh")]), 2);
        assert_eq!(
            estimate_context_tokens(&[
                ChatMessage::user("abcdefgh"),
                ChatMessage::assistant("ijkl")
            ]),
            3
        );

        let with_system =
            estimate_context_tokens(&[ChatMessage::user("abcd")]) + estimate_tokens("efgh");
        assert_eq!(with_system, 2);

        assert_eq!(truncate_tokens("abcdefgh", 1), "abcd\n... [truncated]");
        assert_eq!(truncate_tokens("abc", 1), "abc");
    }

    #[tokio::test]
    async fn spill_stores_blob_and_retains_notice() {
        let root = std::env::temp_dir().join(format!("meerai-spill-{}", SessionId::new()));
        let session = persistent_session(&root).await;
        let output = big_output(300);
        let tool = StaticToolset::new("big", "big-out", output.clone(), EffectClass::Read);
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
                call_id: "c1".to_string(),
                fn_name: "big-out".to_string(),
                fn_arguments: serde_json::json!({}),
                thought_signatures: None,
            }]))],
            vec![ChatStreamEvent::End(text_end("done"))],
        ]));
        let config = HarnessConfig {
            spill_max_inline_tokens: 20,
            ..HarnessConfig::default()
        };
        let mut agent = harness_agent(
            streamer.clone(),
            vec![Box::new(tool)],
            "",
            &session,
            yolo(),
            config,
        );

        let (tx, mut rx) = mpsc::unbounded_channel();
        let out = agent
            .prompt_streaming("go", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out, "done");

        let mut completed = None;
        while let Ok(event) = rx.try_recv() {
            if let AgentEvent::ItemCompleted {
                item: SessionItem::ToolCall { output, .. },
            } = event
            {
                completed = output;
            }
        }
        let completed = completed.expect("tool item completed");
        assert!(completed.contains("(Omitted "));
        assert!(completed.contains("Full result stored at:"));

        let events = session_events(&session);
        let (bytes_omitted, blob_path) = events
            .iter()
            .find_map(|event| match event {
                SessionEvent::SpillApplied {
                    bytes_omitted,
                    blob_path,
                    ..
                } => Some((*bytes_omitted, blob_path.clone())),
                _ => None,
            })
            .expect("spill applied");
        assert!(bytes_omitted > 0);
        assert!(blob_path.exists());
        assert!(
            blob_path.starts_with(root.join(session.id().as_str()).join("blobs")),
            "blob written under the session dir"
        );

        let observation = events
            .iter()
            .find_map(|event| match event {
                SessionEvent::ToolResult { output, .. } => Some(output.clone()),
                _ => None,
            })
            .expect("tool result logged");
        assert!(observation.stdout.starts_with("row 0 with"));
        assert!(observation.stdout.contains("Full result stored at:"));

        let reqs = streamer.requests.lock().unwrap();
        assert!(format!("{:?}", reqs[1].messages).contains("Full result stored at:"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn read_file_output_is_not_spilled() {
        let root = std::env::temp_dir().join(format!("meerai-nospill-{}", SessionId::new()));
        let session = persistent_session(&root).await;
        let output = big_output(300);
        let tool = StaticToolset::new("code", "code-read_file", output.clone(), EffectClass::Read);
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
                call_id: "c1".to_string(),
                fn_name: "code-read_file".to_string(),
                fn_arguments: serde_json::json!({ "path": "a.rs" }),
                thought_signatures: None,
            }]))],
            vec![ChatStreamEvent::End(text_end("done"))],
        ]));
        let config = HarnessConfig {
            spill_max_inline_tokens: 20,
            ..HarnessConfig::default()
        };
        let mut agent = harness_agent(streamer, vec![Box::new(tool)], "", &session, yolo(), config);

        let (tx, mut rx) = mpsc::unbounded_channel();
        agent
            .prompt_streaming("go", &tx, CancellationToken::new())
            .await
            .unwrap();

        let spilled = session_events(&session)
            .iter()
            .any(|event| matches!(event, SessionEvent::SpillApplied { .. }));
        assert!(!spilled);

        let mut completed = None;
        while let Ok(event) = rx.try_recv() {
            if let AgentEvent::ItemCompleted {
                item: SessionItem::ToolCall { output, .. },
            } = event
            {
                completed = output;
            }
        }
        assert_eq!(completed, Some(output));

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn pressure_compaction_brackets_and_shrinks_history() {
        let reply = "x".repeat(800);
        let summary = "Goal: ship the test\nCompleted progress: first turn";
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(text_end(&reply))],
            vec![
                ChatStreamEvent::Chunk(StreamChunk {
                    content: summary.to_string(),
                }),
                ChatStreamEvent::End(text_end(summary)),
            ],
            vec![ChatStreamEvent::End(text_end("second reply"))],
        ]));

        let session = memory_session();
        let config = HarnessConfig {
            context_window_tokens: 80,
            high_watermark_pct: 50,
            verbatim_window_turns: 1,
            ..HarnessConfig::default()
        };
        let mut agent = harness_agent(streamer.clone(), vec![], "system", &session, yolo(), config);

        let (tx, mut rx) = mpsc::unbounded_channel();
        agent
            .prompt_streaming("first", &tx, CancellationToken::new())
            .await
            .unwrap();
        let out = agent
            .prompt_streaming("second", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out, "second reply");

        let events = session_events(&session);
        let kinds: Vec<&str> = events.iter().map(event_kind).collect();
        let start = kinds
            .iter()
            .position(|kind| *kind == "compaction_started")
            .expect("compaction bracketed");
        let summary_index = kinds[start..]
            .iter()
            .position(|kind| *kind == "compaction_summary")
            .map(|offset| start + offset)
            .expect("compaction summary logged");
        let ended_index = kinds[summary_index..]
            .iter()
            .position(|kind| *kind == "compaction_ended")
            .map(|offset| summary_index + offset)
            .expect("compaction ended logged");

        match &events[start] {
            SessionEvent::CompactionStarted {
                trigger: CompactionTrigger::Pressure,
                ..
            } => {}
            other => panic!("expected pressure trigger, got {other:?}"),
        }

        match &events[summary_index] {
            SessionEvent::CompactionSummary { summary, .. } => {
                assert!(summary.starts_with(COMPACTED_MARKER));
            }
            other => panic!("expected compaction summary, got {other:?}"),
        }

        match &events[ended_index] {
            SessionEvent::CompactionEnded {
                result: CompactionResult::Applied { freed_tokens },
                ..
            } => assert!(*freed_tokens > 0),
            other => panic!("expected applied compaction, got {other:?}"),
        }

        let mut items = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let AgentEvent::ItemCompleted {
                item:
                    SessionItem::Compaction {
                        trigger,
                        freed_tokens,
                        ..
                    },
            } = event
            {
                items.push((trigger, freed_tokens));
            }
        }
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0, CompactionTrigger::Pressure);
        assert!(items[0].1 > 0);

        let policy = ProjectionPolicy {
            strip_ephemera: true,
            prune_reasoning: true,
            verbatim_window_turns: 1,
            include_system_messages: false,
        };
        let derived = session.derive_messages(&policy);
        assert_eq!(derived.len(), 3);
        assert!(
            derived[0]
                .content
                .first_text()
                .is_some_and(|text| text.starts_with(COMPACTED_MARKER))
        );

        let reqs = streamer.requests.lock().unwrap();
        assert_eq!(reqs.len(), 3);
        assert_eq!(reqs[2].messages.len(), 2);
        assert!(
            reqs[1]
                .system
                .as_deref()
                .is_some_and(|system| system.contains("Goal"))
        );
    }

    #[tokio::test]
    async fn compaction_usage_counts_toward_cumulative() {
        let reply = "y".repeat(800);
        let summary = "Goal: account for summarizer spend";
        let summary_end = StreamEnd {
            captured_content: Some(MessageContent::from_text(summary)),
            captured_usage: Some(Usage {
                prompt_tokens: Some(40),
                completion_tokens: Some(10),
                total_tokens: Some(50),
                ..Default::default()
            }),
            ..Default::default()
        };
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(text_end(&reply))],
            vec![
                ChatStreamEvent::Chunk(StreamChunk {
                    content: summary.to_string(),
                }),
                ChatStreamEvent::End(summary_end),
            ],
            vec![ChatStreamEvent::End(text_end("second reply"))],
        ]));

        let session = memory_session();
        let config = HarnessConfig {
            context_window_tokens: 80,
            high_watermark_pct: 50,
            verbatim_window_turns: 1,
            ..HarnessConfig::default()
        };
        let mut agent = harness_agent(streamer.clone(), vec![], "system", &session, yolo(), config);

        let (tx, mut rx) = mpsc::unbounded_channel();
        agent
            .prompt_streaming("first", &tx, CancellationToken::new())
            .await
            .unwrap();
        agent
            .prompt_streaming("second", &tx, CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(agent.cumulative_usage.total_tokens, Some(80));

        let mut live = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let AgentEvent::UsageUpdated {
                usage, cumulative, ..
            } = event
            {
                live.push((usage.total_tokens, cumulative.total_tokens));
            }
        }
        assert_eq!(
            live,
            vec![
                (Some(15), Some(15)),
                (Some(50), Some(65)),
                (Some(15), Some(80)),
            ]
        );

        let mut logged = Vec::new();
        for event in session_events(&session) {
            if let SessionEvent::UsageUpdated { usage, cumulative } = event {
                logged.push((usage.total_tokens, cumulative.total_tokens));
            }
        }
        assert_eq!(
            logged,
            vec![
                (Some(15), Some(15)),
                (Some(50), Some(65)),
                (Some(15), Some(80)),
            ]
        );
    }

    #[tokio::test]
    async fn manual_compact_runs_at_step_boundary() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let streamer = Arc::new(GatedStreamer {
            turns: Mutex::new(
                vec![
                    vec![ChatStreamEvent::End(text_end("one"))],
                    vec![ChatStreamEvent::End(tool_end(vec![tool_call("c1", "ok")]))],
                    vec![
                        ChatStreamEvent::Chunk(StreamChunk {
                            content: "Goal: manual compaction".to_string(),
                        }),
                        ChatStreamEvent::End(text_end("Goal: manual compaction")),
                    ],
                    vec![ChatStreamEvent::End(text_end("two"))],
                ]
                .into(),
            ),
            requests: Mutex::new(Vec::new()),
            gate_index: 1,
            started: started.clone(),
            release: release.clone(),
        });

        let session = memory_session();
        let config = HarnessConfig {
            verbatim_window_turns: 1,
            ..HarnessConfig::default()
        };
        let agent = harness_agent(
            streamer.clone(),
            vec![Box::new(EchoToolset)],
            "",
            &session,
            yolo(),
            config,
        );

        let (cmd_tx, mut rx) = spawn_worker(agent);
        cmd_tx
            .send(AgentCommand::SubmitPrompt("first".to_string()))
            .unwrap();
        loop {
            if matches!(rx.recv().await, Some(AgentEvent::TurnCompleted { .. })) {
                break;
            }
        }

        cmd_tx
            .send(AgentCommand::SubmitPrompt("second".to_string()))
            .unwrap();
        started.notified().await;
        cmd_tx.send(AgentCommand::Compact).unwrap();
        tokio::time::sleep(Duration::from_millis(25)).await;
        release.notify_one();

        loop {
            if matches!(rx.recv().await, Some(AgentEvent::TurnCompleted { .. })) {
                break;
            }
        }

        let events = session_events(&session);
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::CompactionStarted {
                trigger: CompactionTrigger::Manual,
                ..
            }
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::CompactionEnded {
                result: CompactionResult::Applied { .. },
                ..
            }
        )));
        assert_eq!(streamer.requests.lock().unwrap().len(), 4);

        let _ = cmd_tx.send(AgentCommand::Quit);
    }

    #[tokio::test]
    async fn context_overflow_retries_once_after_compaction() {
        let summary = "Goal: retried after overflow";
        let streamer = Arc::new(FakeStreamer::with_script(vec![
            ScriptedStream::Events(vec![ChatStreamEvent::End(text_end("one"))]),
            ScriptedStream::Error("maximum context length exceeded".to_string()),
            ScriptedStream::Events(vec![
                ChatStreamEvent::Chunk(StreamChunk {
                    content: summary.to_string(),
                }),
                ChatStreamEvent::End(text_end(summary)),
            ]),
            ScriptedStream::Events(vec![ChatStreamEvent::End(text_end("two"))]),
        ]));

        let session = memory_session();
        let config = HarnessConfig {
            verbatim_window_turns: 1,
            ..HarnessConfig::default()
        };
        let mut agent = harness_agent(streamer.clone(), vec![], "", &session, yolo(), config);

        let (tx, _rx) = mpsc::unbounded_channel();
        agent
            .prompt_streaming("first", &tx, CancellationToken::new())
            .await
            .unwrap();
        let out = agent
            .prompt_streaming("second", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out, "two");

        let events = session_events(&session);
        let attempts: Vec<&SessionEvent> = events
            .iter()
            .filter(|event| matches!(event, SessionEvent::AssistantAttempt { .. }))
            .collect();
        assert_eq!(attempts.len(), 1);
        match attempts[0] {
            SessionEvent::AssistantAttempt { error, .. } => {
                assert!(error.contains("maximum context length"));
            }
            other => panic!("expected assistant attempt, got {other:?}"),
        }
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::CompactionStarted {
                trigger: CompactionTrigger::ContextOverflow,
                ..
            }
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::CompactionEnded {
                result: CompactionResult::Applied { .. },
                ..
            }
        )));

        let reqs = streamer.requests.lock().unwrap();
        assert_eq!(reqs.len(), 4);
        assert!(
            reqs[3].messages[0]
                .content
                .first_text()
                .is_some_and(|text| text.starts_with(COMPACTED_MARKER))
        );
    }

    #[tokio::test]
    async fn second_context_overflow_fails_the_turn() {
        let streamer = Arc::new(FakeStreamer::with_script(vec![
            ScriptedStream::Events(vec![ChatStreamEvent::End(text_end("one"))]),
            ScriptedStream::Error("maximum context length exceeded".to_string()),
            ScriptedStream::Events(vec![ChatStreamEvent::End(text_end("Goal: retried"))]),
            ScriptedStream::Error("maximum context length exceeded".to_string()),
        ]));

        let session = memory_session();
        let config = HarnessConfig {
            verbatim_window_turns: 1,
            ..HarnessConfig::default()
        };
        let agent = harness_agent(streamer.clone(), vec![], "", &session, yolo(), config);

        let (cmd_tx, mut rx) = spawn_worker(agent);
        cmd_tx
            .send(AgentCommand::SubmitPrompt("first".to_string()))
            .unwrap();
        loop {
            if matches!(rx.recv().await, Some(AgentEvent::TurnCompleted { .. })) {
                break;
            }
        }
        cmd_tx
            .send(AgentCommand::SubmitPrompt("second".to_string()))
            .unwrap();

        let mut code = None;
        let mut failure = None;
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::Error {
                    code: error_code, ..
                } => code = Some(error_code),
                AgentEvent::TurnCompleted {
                    reason: TurnEndReason::Failed { error },
                    ..
                } => {
                    failure = Some(error);
                    break;
                }
                AgentEvent::TurnCompleted { .. } => break,
                _ => {}
            }
        }
        assert_eq!(code, Some(ErrorCode::ContextOverflow));
        assert!(
            failure
                .as_deref()
                .is_some_and(|message| message.contains("maximum context length"))
        );

        let events = session_events(&session);
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::Error {
                code: ErrorCode::ContextOverflow,
                ..
            }
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::TurnEnded {
                reason: TurnEndReason::Failed { .. },
                ..
            }
        )));

        let _ = cmd_tx.send(AgentCommand::Quit);
    }

    #[tokio::test]
    async fn final_step_strips_tools_and_requests_summary() {
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(vec![tool_call("c1", "ok")]))],
            vec![ChatStreamEvent::End(text_end("wrapped up"))],
        ]));

        let session = memory_session();
        let config = HarnessConfig {
            max_steps_per_turn: 2,
            ..HarnessConfig::default()
        };
        let mut agent = harness_agent(
            streamer.clone(),
            vec![Box::new(EchoToolset)],
            "",
            &session,
            yolo(),
            config,
        );

        let (tx, _rx) = mpsc::unbounded_channel();
        let out = agent
            .prompt_streaming("go", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out, "wrapped up");

        let reqs = streamer.requests.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        assert!(reqs[0].tools.is_some());
        assert!(reqs[1].tools.is_none());
        assert!(format!("{:?}", reqs[1].messages).contains("Step limit reached."));

        let notice = session_events(&session).iter().any(|event| {
            matches!(
                event,
                SessionEvent::UserMessage { source: InputSource::RuntimeContext, content }
                    if content.iter().any(|block| matches!(
                        block,
                        ContentBlock::Text(text) if text.starts_with("Step limit reached.")
                    ))
            )
        });
        assert!(notice);
    }

    #[tokio::test]
    async fn token_cap_accept_raises_the_limit() {
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end_counting(
                vec![tool_call("c1", "ok")],
                15,
            ))],
            vec![ChatStreamEvent::End(text_end("budget raised"))],
        ]));

        let session = memory_session();
        let config = HarnessConfig {
            max_session_tokens: Some(15),
            ..HarnessConfig::default()
        };
        let mut agent = harness_agent(
            streamer.clone(),
            vec![Box::new(EchoToolset)],
            "",
            &session,
            yolo(),
            config,
        );

        let (commands_tx, mut commands_rx) = mpsc::unbounded_channel();
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let driver = tokio::spawn(async move {
            let mut spend_requests = 0;
            while let Some(event) = event_rx.recv().await {
                match event {
                    AgentEvent::ApprovalRequested { request } => {
                        if request.kind == ApprovalKind::Spend {
                            assert_eq!(request.preview.title, "Session token budget reached");
                            spend_requests += 1;
                        }
                        let _ = commands_tx.send(AgentCommand::Approve {
                            request_id: request.request_id,
                            decision: ApprovalDecision::Accept,
                            scope: None,
                        });
                    }
                    AgentEvent::TurnCompleted { .. } => break,
                    _ => {}
                }
            }
            spend_requests
        });

        let out = agent
            .prompt_streaming_with_commands(
                "go",
                &event_tx,
                CancellationToken::new(),
                &mut commands_rx,
            )
            .await
            .unwrap();
        assert_eq!(out, "budget raised");
        assert_eq!(driver.await.unwrap(), 1);
        assert_eq!(
            agent.harness().unwrap().config.max_session_tokens,
            Some(15 + 128_000)
        );
        assert_eq!(streamer.requests.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn token_cap_decline_fails_the_turn() {
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end_counting(
                vec![tool_call("c1", "ok")],
                15,
            ))],
            vec![ChatStreamEvent::End(text_end("unreachable"))],
        ]));

        let session = memory_session();
        let config = HarnessConfig {
            max_session_tokens: Some(15),
            ..HarnessConfig::default()
        };
        let mut agent = harness_agent(
            streamer.clone(),
            vec![Box::new(EchoToolset)],
            "",
            &session,
            yolo(),
            config,
        );

        let (commands_tx, mut commands_rx) = mpsc::unbounded_channel();
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let driver = tokio::spawn(async move {
            let mut errors = Vec::new();
            let mut outcome = None;
            while let Some(event) = event_rx.recv().await {
                match event {
                    AgentEvent::ApprovalRequested { request } => {
                        let _ = commands_tx.send(AgentCommand::Approve {
                            request_id: request.request_id,
                            decision: ApprovalDecision::Decline,
                            scope: None,
                        });
                    }
                    AgentEvent::Error { code, .. } => errors.push(code),
                    AgentEvent::TurnCompleted { reason, .. } => {
                        outcome = Some(reason);
                        break;
                    }
                    _ => {}
                }
            }
            (errors, outcome)
        });

        let _ = agent
            .prompt_streaming_with_commands(
                "go",
                &event_tx,
                CancellationToken::new(),
                &mut commands_rx,
            )
            .await;

        let (errors, outcome) = driver.await.unwrap();
        assert_eq!(errors, vec![ErrorCode::Budget]);
        assert_eq!(
            outcome,
            Some(TurnEndReason::Failed {
                error: TOKEN_BUDGET_DECLINED.to_string()
            })
        );
        assert_eq!(streamer.requests.lock().unwrap().len(), 1);

        let events = session_events(&session);
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::Error {
                code: ErrorCode::Budget,
                retryable: false,
                ..
            }
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::TurnEnded {
                reason: TurnEndReason::Failed { error },
                ..
            } if error == TOKEN_BUDGET_DECLINED
        )));
    }

    #[tokio::test]
    async fn repeated_failures_trigger_breaker_and_end_turn() {
        let step = |call_id: &str| {
            vec![ChatStreamEvent::End(tool_end(vec![tool_call(
                call_id, "fail",
            )]))]
        };
        let streamer = Arc::new(FakeStreamer::new(vec![
            step("c1"),
            step("c2"),
            step("c3"),
            step("c4"),
        ]));

        let session = memory_session();
        let agent = harness_agent(
            streamer.clone(),
            vec![Box::new(EchoToolset)],
            "",
            &session,
            yolo(),
            HarnessConfig::default(),
        );

        let (cmd_tx, mut rx) = spawn_worker(agent);
        cmd_tx
            .send(AgentCommand::SubmitPrompt("go".to_string()))
            .unwrap();

        let mut code = None;
        let mut failure = None;
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::Error {
                    code: error_code, ..
                } => code = Some(error_code),
                AgentEvent::TurnCompleted {
                    reason: TurnEndReason::Failed { error },
                    ..
                } => {
                    failure = Some(error);
                    break;
                }
                AgentEvent::TurnCompleted { .. } => break,
                _ => {}
            }
        }
        assert_eq!(code, Some(ErrorCode::LoopDetected));
        assert_eq!(failure.as_deref(), Some(LOOP_DETECTED));

        let events = session_events(&session);
        let breakers: Vec<&SessionEvent> = events
            .iter()
            .filter(|event| matches!(event, SessionEvent::CircuitBreaker { .. }))
            .collect();
        assert_eq!(breakers.len(), 2);
        match breakers[0] {
            SessionEvent::CircuitBreaker { kind, action } => {
                assert_eq!(*kind, BreakerKind::RepeatedToolFailure);
                assert_eq!(*action, BreakerAction::DiagnosticInjected);
            }
            other => panic!("expected circuit breaker, got {other:?}"),
        }
        match breakers[1] {
            SessionEvent::CircuitBreaker { action, .. } => {
                assert_eq!(*action, BreakerAction::TurnFailed);
            }
            other => panic!("expected circuit breaker, got {other:?}"),
        }

        let injected = events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::UserMessage { source: InputSource::RuntimeContext, content }
                    if content.iter().any(|block| matches!(
                        block,
                        ContentBlock::Text(text) if text.contains("failed identically 3 times")
                    ))
            )
        });
        assert!(injected);
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::TurnEnded {
                reason: TurnEndReason::Failed { error },
                ..
            } if error == LOOP_DETECTED
        )));

        let reqs = streamer.requests.lock().unwrap();
        assert_eq!(reqs.len(), 4);
        let next_request = format!("{:?}", reqs[3].messages);
        assert!(
            next_request.contains("failed identically 3 times"),
            "the breaker diagnostic must reach the next request: {next_request}"
        );

        let _ = cmd_tx.send(AgentCommand::Quit);
    }

    #[tokio::test]
    async fn missing_approval_channel_auto_declines() {
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(vec![tool_call("c1", "ok")]))],
            vec![ChatStreamEvent::End(text_end("after refusal"))],
        ]));

        let session = memory_session();
        let policy = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![any_rule(Effect::Ask)]);
        let mut agent = harness_agent(
            streamer.clone(),
            vec![Box::new(EchoToolset)],
            "",
            &session,
            policy,
            HarnessConfig::default(),
        );

        let (tx, mut rx) = mpsc::unbounded_channel();
        let out = agent
            .prompt_streaming("go", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out, "after refusal");

        let mut requested = 0;
        let mut resolved = 0;
        while let Ok(event) = rx.try_recv() {
            match event {
                AgentEvent::ApprovalRequested { .. } => requested += 1,
                AgentEvent::ApprovalResolved { .. } => resolved += 1,
                _ => {}
            }
        }
        assert_eq!(requested, 1);
        assert_eq!(resolved, 1);

        let decided = session_events(&session).iter().any(|event| {
            matches!(
                event,
                SessionEvent::ApprovalDecided {
                    decision: ApprovalDecision::Decline,
                    scope: None,
                    ..
                }
            )
        });
        assert!(decided);

        let reqs = streamer.requests.lock().unwrap();
        assert!(format!("{:?}", reqs[1].messages).contains(NO_APPROVAL_CHANNEL));
    }

    struct BarrierToolset {
        barrier: Arc<Barrier>,
    }

    #[async_trait]
    impl Toolset for BarrierToolset {
        fn name(&self) -> String {
            "barrier".to_string()
        }

        fn definition(&self) -> Vec<ToolDefinition> {
            ["barrier-a", "barrier-b"]
                .into_iter()
                .map(|name| ToolDefinition {
                    r#type: "function".to_string(),
                    name: name.to_string(),
                    description: "Waits for its sibling to run".to_string(),
                    parameters: serde_json::json!({ "type": "object", "properties": {} }),
                })
                .collect()
        }

        fn contain(&self, fn_name: &str) -> bool {
            fn_name == "barrier-a" || fn_name == "barrier-b"
        }

        fn effect_of(&self, _fn_name: &str) -> EffectClass {
            EffectClass::Read
        }

        async fn invoke(&self, fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
            tokio::time::timeout(Duration::from_secs(2), self.barrier.wait())
                .await
                .map_err(|_| anyhow!("barrier timed out; calls did not overlap"))?;
            Ok(ToolOutput::Text(format!("ran:{fn_name}")))
        }
    }

    struct StopBatchToolset {
        effect: EffectClass,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Toolset for StopBatchToolset {
        fn name(&self) -> String {
            "batch".to_string()
        }

        fn definition(&self) -> Vec<ToolDefinition> {
            ["batch-stop", "batch-after"]
                .into_iter()
                .map(|name| ToolDefinition {
                    r#type: "function".to_string(),
                    name: name.to_string(),
                    description: "Stops the batch or records a later call".to_string(),
                    parameters: serde_json::json!({ "type": "object", "properties": {} }),
                })
                .collect()
        }

        fn contain(&self, fn_name: &str) -> bool {
            fn_name == "batch-stop" || fn_name == "batch-after"
        }

        fn effect_of(&self, _fn_name: &str) -> EffectClass {
            self.effect
        }

        async fn invoke(&self, fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
            if fn_name == "batch-stop" {
                return Ok(ToolOutput::Stop("stopped the batch".to_string()));
            }
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput::Text("should not run".to_string()))
        }
    }

    struct OrderToolset {
        order: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl Toolset for OrderToolset {
        fn name(&self) -> String {
            "order".to_string()
        }

        fn definition(&self) -> Vec<ToolDefinition> {
            ["order-read", "order-write"]
                .into_iter()
                .map(|name| ToolDefinition {
                    r#type: "function".to_string(),
                    name: name.to_string(),
                    description: "Records its execution position".to_string(),
                    parameters: serde_json::json!({ "type": "object", "properties": {} }),
                })
                .collect()
        }

        fn contain(&self, fn_name: &str) -> bool {
            fn_name == "order-read" || fn_name == "order-write"
        }

        fn effect_of(&self, fn_name: &str) -> EffectClass {
            if fn_name == "order-read" {
                EffectClass::Read
            } else {
                EffectClass::Write
            }
        }

        async fn invoke(&self, fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
            let label = fn_name.strip_prefix("order-").unwrap_or(fn_name);
            self.order.lock().unwrap().push(label.to_string());
            Ok(ToolOutput::Text(format!("did:{label}")))
        }
    }

    fn bare_call(call_id: &str, fn_name: &str) -> ToolCall {
        ToolCall {
            call_id: call_id.to_string(),
            fn_name: fn_name.to_string(),
            fn_arguments: serde_json::json!({}),
            thought_signatures: None,
        }
    }

    #[test]
    fn turn_deadline_comparison_uses_elapsed_seconds() {
        let started = Instant::now();
        assert!(!turn_deadline_exceeded(started, 30, started));
        assert!(!turn_deadline_exceeded(
            started,
            30,
            started + Duration::from_secs(29)
        ));
        assert!(turn_deadline_exceeded(
            started,
            30,
            started + Duration::from_secs(30)
        ));
        assert!(!turn_deadline_exceeded(
            started,
            0,
            started + Duration::from_secs(3_600)
        ));

        assert_eq!(provider_backoff_after(1), Duration::from_millis(250));
        assert_eq!(provider_backoff_after(2), Duration::from_millis(500));
        assert_eq!(provider_backoff_after(3), Duration::from_millis(1_000));
    }

    #[test]
    fn remaining_turn_budget_handles_disabled_exhausted_and_remaining() {
        let started = Instant::now();
        assert_eq!(remaining_turn_budget(started, 0), None);
        assert_eq!(
            remaining_turn_budget(started - Duration::from_secs(120), 30),
            None
        );

        let left = remaining_turn_budget(started, 30).expect("budget remaining");
        assert!(left > Duration::from_secs(29));
        assert!(left <= Duration::from_secs(30));
    }

    #[tokio::test]
    async fn read_only_calls_run_concurrently_and_settle_in_call_order() {
        let barrier = Arc::new(Barrier::new(2));
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(vec![
                bare_call("c1", "barrier-a"),
                bare_call("c2", "barrier-b"),
            ]))],
            vec![ChatStreamEvent::End(text_end("both ran"))],
        ]));

        let session = memory_session();
        let mut agent = harness_agent(
            streamer.clone(),
            vec![Box::new(BarrierToolset {
                barrier: barrier.clone(),
            })],
            "",
            &session,
            yolo(),
            HarnessConfig::default(),
        );

        let (tx, mut rx) = mpsc::unbounded_channel();
        let out = agent
            .prompt_streaming("run both", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out, "both ran");

        let mut completed = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let AgentEvent::ItemCompleted {
                item:
                    SessionItem::ToolCall {
                        call_id, status, ..
                    },
            } = event
            {
                completed.push((call_id, status));
            }
        }
        assert_eq!(
            completed,
            vec![
                (CallId::from_raw("c1"), ItemStatus::Succeeded),
                (CallId::from_raw("c2"), ItemStatus::Succeeded),
            ]
        );

        let results: Vec<(String, String)> = session_events(&session)
            .into_iter()
            .filter_map(|event| match event {
                SessionEvent::ToolResult { call_id, output } => {
                    Some((call_id.as_str().to_string(), output.stdout))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            results,
            vec![
                ("c1".to_string(), "ran:barrier-a".to_string()),
                ("c2".to_string(), "ran:barrier-b".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn tool_batches_preserve_call_order_across_effect_classes() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(vec![
                bare_call("c1", "order-write"),
                bare_call("c2", "order-read"),
            ]))],
            vec![ChatStreamEvent::End(text_end("first done"))],
            vec![ChatStreamEvent::End(tool_end(vec![
                bare_call("c3", "order-read"),
                bare_call("c4", "order-write"),
                bare_call("c5", "order-read"),
            ]))],
            vec![ChatStreamEvent::End(text_end("second done"))],
        ]));

        let session = memory_session();
        let mut agent = harness_agent(
            streamer.clone(),
            vec![Box::new(OrderToolset {
                order: order.clone(),
            })],
            "",
            &session,
            yolo(),
            HarnessConfig::default(),
        );

        let (tx, _rx) = mpsc::unbounded_channel();
        agent
            .prompt_streaming("first", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(*order.lock().unwrap(), vec!["write", "read"]);

        agent
            .prompt_streaming("second", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            *order.lock().unwrap(),
            vec!["write", "read", "read", "write", "read"]
        );

        let results: Vec<(String, String)> = session_events(&session)
            .into_iter()
            .filter_map(|event| match event {
                SessionEvent::ToolResult { call_id, output } => {
                    Some((call_id.as_str().to_string(), output.stdout))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            results,
            vec![
                ("c1".to_string(), "did:write".to_string()),
                ("c2".to_string(), "did:read".to_string()),
                ("c3".to_string(), "did:read".to_string()),
                ("c4".to_string(), "did:write".to_string()),
                ("c5".to_string(), "did:read".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn stop_skips_the_rest_of_the_tool_batch() {
        let calls = Arc::new(AtomicUsize::new(0));
        let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
            tool_end(vec![
                bare_call("c1", "batch-stop"),
                bare_call("c2", "batch-after"),
            ]),
        )]]));

        let session = memory_session();
        let mut agent = harness_agent(
            streamer.clone(),
            vec![Box::new(StopBatchToolset {
                effect: EffectClass::Write,
                calls: calls.clone(),
            })],
            "",
            &session,
            yolo(),
            HarnessConfig::default(),
        );

        let (tx, mut rx) = mpsc::unbounded_channel();
        let out = agent
            .prompt_streaming("stop it", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out, "stopped the batch");
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let mut completed = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let AgentEvent::ItemCompleted {
                item:
                    SessionItem::ToolCall {
                        call_id, status, ..
                    },
            } = event
            {
                completed.push((call_id, status));
            }
        }
        assert_eq!(
            completed,
            vec![
                (CallId::from_raw("c1"), ItemStatus::Stopped),
                (CallId::from_raw("c2"), ItemStatus::Cancelled),
            ]
        );

        let results: Vec<(String, ObservationStatus, String)> = session_events(&session)
            .into_iter()
            .filter_map(|event| match event {
                SessionEvent::ToolResult { call_id, output } => {
                    Some((call_id.as_str().to_string(), output.status, output.stderr))
                }
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "c1");
        assert_eq!(results[0].1, ObservationStatus::Stopped);
        assert_eq!(results[1].0, "c2");
        assert_eq!(results[1].1, ObservationStatus::Error);
        assert!(results[1].2.contains(NOT_EXECUTED_STOPPED));

        let reqs = streamer.requests.lock().unwrap();
        assert_eq!(reqs.len(), 1);
        drop(reqs);
        assert_eq!(agent.history().len(), 4);
    }

    #[tokio::test]
    async fn executed_reads_keep_their_result_when_a_sibling_stops() {
        let calls = Arc::new(AtomicUsize::new(0));
        let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
            tool_end(vec![
                bare_call("c1", "batch-stop"),
                bare_call("c2", "batch-after"),
            ]),
        )]]));

        let session = memory_session();
        let mut agent = harness_agent(
            streamer.clone(),
            vec![Box::new(StopBatchToolset {
                effect: EffectClass::Read,
                calls: calls.clone(),
            })],
            "",
            &session,
            yolo(),
            HarnessConfig::default(),
        );

        let (tx, mut rx) = mpsc::unbounded_channel();
        let out = agent
            .prompt_streaming("stop it", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out, "stopped the batch");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let mut completed = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let AgentEvent::ItemCompleted {
                item:
                    SessionItem::ToolCall {
                        call_id, status, ..
                    },
            } = event
            {
                completed.push((call_id, status));
            }
        }
        assert_eq!(
            completed,
            vec![
                (CallId::from_raw("c1"), ItemStatus::Stopped),
                (CallId::from_raw("c2"), ItemStatus::Succeeded),
            ]
        );

        let results: Vec<(String, ObservationStatus, String)> = session_events(&session)
            .into_iter()
            .filter_map(|event| match event {
                SessionEvent::ToolResult { call_id, output } => {
                    Some((call_id.as_str().to_string(), output.status, output.stdout))
                }
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "c1");
        assert_eq!(results[0].1, ObservationStatus::Stopped);
        assert_eq!(results[1].0, "c2");
        assert_eq!(results[1].1, ObservationStatus::Success);
        assert_eq!(results[1].2, "should not run");
    }

    #[tokio::test]
    async fn stalled_stream_ends_the_turn_at_the_wall_clock_budget() {
        let streamer = Arc::new(FakeStreamer::with_script(vec![ScriptedStream::Stall]));

        let session = memory_session();
        let config = HarnessConfig {
            max_turn_secs: 1,
            ..HarnessConfig::default()
        };
        let mut agent = harness_agent(streamer.clone(), vec![], "", &session, yolo(), config);

        let (tx, _rx) = mpsc::unbounded_channel();
        let started = Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            agent.prompt_streaming("go", &tx, CancellationToken::new()),
        )
        .await
        .expect("stalled turn did not end within 5s");
        assert!(result.is_err());
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the wall clock must end the stalled stream near 1s"
        );

        let events = session_events(&session);
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::Error {
                code: ErrorCode::Budget,
                message,
                retryable: false,
            } if message == "turn exceeded 1s"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::TurnEnded {
                reason: TurnEndReason::Failed { .. },
                ..
            }
        )));
        assert_eq!(streamer.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn idle_set_permission_mode_applies_to_the_next_turn() {
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(vec![bare_call(
                "c1",
                "write-file",
            )]))],
            vec![ChatStreamEvent::End(text_end("first done"))],
            vec![ChatStreamEvent::End(tool_end(vec![bare_call(
                "c2",
                "write-file",
            )]))],
            vec![ChatStreamEvent::End(text_end("second done"))],
        ]));

        let session = memory_session();
        let policy = PolicyEngine::new(ApprovalMode::AutoEdit);
        let agent = harness_agent(
            streamer,
            vec![Box::new(StaticToolset::new(
                "write",
                "write-file",
                "wrote",
                EffectClass::Write,
            ))],
            "",
            &session,
            policy,
            HarnessConfig::default(),
        );

        let (cmd_tx, mut rx) = spawn_worker(agent);
        cmd_tx
            .send(AgentCommand::SubmitPrompt("first".to_string()))
            .unwrap();

        let mut statuses = Vec::new();
        loop {
            match rx.recv().await {
                Some(AgentEvent::ItemCompleted {
                    item: SessionItem::ToolCall { status, .. },
                }) => statuses.push(status),
                Some(AgentEvent::TurnCompleted { .. }) => break,
                Some(_) => {}
                None => panic!("worker stopped mid-turn"),
            }
        }
        assert_eq!(statuses, vec![ItemStatus::Succeeded]);

        tokio::time::sleep(Duration::from_millis(50)).await;
        cmd_tx
            .send(AgentCommand::SetPermissionMode(ApprovalMode::Ask))
            .unwrap();
        cmd_tx
            .send(AgentCommand::SubmitPrompt("second".to_string()))
            .unwrap();

        let mut prompts = 0;
        loop {
            match rx.recv().await {
                Some(AgentEvent::ApprovalRequested { request }) => {
                    prompts += 1;
                    cmd_tx
                        .send(AgentCommand::Approve {
                            request_id: request.request_id,
                            decision: ApprovalDecision::Decline,
                            scope: None,
                        })
                        .unwrap();
                }
                Some(AgentEvent::ItemCompleted {
                    item: SessionItem::ToolCall { status, .. },
                }) => statuses.push(status),
                Some(AgentEvent::TurnCompleted { .. }) => break,
                Some(_) => {}
                None => panic!("worker stopped mid-turn"),
            }
        }
        assert_eq!(prompts, 1, "Ask mode must prompt for a write tool");
        assert_eq!(
            statuses,
            vec![ItemStatus::Succeeded, ItemStatus::Denied],
            "idle SetPermissionMode must apply to the next turn"
        );

        let _ = cmd_tx.send(AgentCommand::Quit);
    }

    #[tokio::test]
    async fn turn_scope_grant_skips_reprompt_until_the_turn_ends() {
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(vec![
                tool_call("c1", "ok"),
                tool_call("c2", "ok"),
            ]))],
            vec![ChatStreamEvent::End(text_end("first"))],
            vec![ChatStreamEvent::End(tool_end(vec![tool_call("c3", "ok")]))],
            vec![ChatStreamEvent::End(text_end("second"))],
        ]));

        let session = memory_session();
        let policy = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![any_rule(Effect::Ask)]);
        let agent = harness_agent(
            streamer,
            vec![Box::new(EchoToolset)],
            "",
            &session,
            policy,
            HarnessConfig::default(),
        );

        let (cmd_tx, mut rx) = spawn_worker(agent);
        let mut prompts = 0;
        for prompt in ["first", "second"] {
            cmd_tx
                .send(AgentCommand::SubmitPrompt(prompt.to_string()))
                .unwrap();
            loop {
                match rx.recv().await {
                    Some(AgentEvent::ApprovalRequested { request }) => {
                        prompts += 1;
                        cmd_tx
                            .send(AgentCommand::Approve {
                                request_id: request.request_id,
                                decision: ApprovalDecision::Accept,
                                scope: Some(GrantScope::Turn),
                            })
                            .unwrap();
                    }
                    Some(AgentEvent::TurnCompleted { .. }) => break,
                    Some(_) => {}
                    None => panic!("worker stopped mid-turn"),
                }
            }
        }
        assert_eq!(prompts, 2);

        let scopes: Vec<Option<GrantScope>> = session_events(&session)
            .into_iter()
            .filter_map(|event| match event {
                SessionEvent::ApprovalDecided { scope, .. } => Some(scope),
                _ => None,
            })
            .collect();
        assert_eq!(scopes, vec![Some(GrantScope::Turn); 2]);

        let _ = cmd_tx.send(AgentCommand::Quit);
    }

    #[tokio::test]
    async fn idle_compact_appends_bracketed_events() {
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(text_end("first reply"))],
            vec![
                ChatStreamEvent::Chunk(StreamChunk {
                    content: "Goal: idle compaction".to_string(),
                }),
                ChatStreamEvent::End(text_end("Goal: idle compaction")),
            ],
        ]));

        let session = memory_session();
        let config = HarnessConfig {
            verbatim_window_turns: 0,
            ..HarnessConfig::default()
        };
        let agent = harness_agent(streamer.clone(), vec![], "", &session, yolo(), config);

        let (cmd_tx, mut rx) = spawn_worker(agent);
        cmd_tx
            .send(AgentCommand::SubmitPrompt("first".to_string()))
            .unwrap();
        loop {
            match rx.recv().await {
                Some(AgentEvent::TurnCompleted { .. }) => break,
                Some(_) => {}
                None => panic!("worker stopped mid-turn"),
            }
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
        cmd_tx.send(AgentCommand::Compact).unwrap();
        let compacted = tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(event) = rx.recv().await {
                if matches!(
                    event,
                    AgentEvent::ItemCompleted {
                        item: SessionItem::Compaction { .. },
                    }
                ) {
                    return true;
                }
            }
            false
        })
        .await
        .expect("idle compaction did not emit an item");
        assert!(compacted);

        let events = session_events(&session);
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::CompactionStarted {
                trigger: CompactionTrigger::Manual,
                ..
            }
        )));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, SessionEvent::CompactionSummary { .. }))
        );
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::CompactionEnded {
                result: CompactionResult::Applied { .. },
                ..
            }
        )));
        assert_eq!(streamer.requests.lock().unwrap().len(), 2);

        let _ = cmd_tx.send(AgentCommand::Quit);
    }

    #[tokio::test]
    async fn transient_provider_error_is_retried_and_logged() {
        let streamer = Arc::new(FakeStreamer::with_script(vec![
            ScriptedStream::Error("connection reset by peer".to_string()),
            ScriptedStream::Events(vec![ChatStreamEvent::End(text_end("recovered"))]),
        ]));

        let session = memory_session();
        let mut agent = harness_agent(
            streamer.clone(),
            vec![],
            "",
            &session,
            yolo(),
            HarnessConfig::default(),
        );

        let (tx, _rx) = mpsc::unbounded_channel();
        let out = agent
            .prompt_streaming("go", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out, "recovered");
        assert_eq!(streamer.requests.lock().unwrap().len(), 2);

        let attempts = session_events(&session)
            .into_iter()
            .filter(|event| matches!(event, SessionEvent::AssistantAttempt { .. }))
            .count();
        assert_eq!(attempts, 1);
    }

    #[tokio::test]
    async fn provider_auth_error_is_not_retried() {
        let streamer = Arc::new(FakeStreamer::with_script(vec![ScriptedStream::Error(
            "401 Unauthorized: invalid API key".to_string(),
        )]));

        let session = memory_session();
        let mut agent = harness_agent(
            streamer.clone(),
            vec![],
            "",
            &session,
            yolo(),
            HarnessConfig::default(),
        );

        let (tx, _rx) = mpsc::unbounded_channel();
        let error = agent
            .prompt_streaming("go", &tx, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("invalid API key"));
        assert_eq!(streamer.requests.lock().unwrap().len(), 1);

        let events = session_events(&session);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, SessionEvent::AssistantAttempt { .. }))
                .count(),
            1
        );
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::Error {
                code: ErrorCode::Provider,
                retryable: false,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn transient_provider_failure_marks_the_turn_retryable() {
        let streamer = Arc::new(FakeStreamer::with_script(vec![
            ScriptedStream::Error("network timeout".to_string()),
            ScriptedStream::Error("network timeout".to_string()),
            ScriptedStream::Error("network timeout".to_string()),
        ]));

        let session = memory_session();
        let mut agent = harness_agent(
            streamer.clone(),
            vec![],
            "",
            &session,
            yolo(),
            HarnessConfig::default(),
        );

        let (tx, _rx) = mpsc::unbounded_channel();
        let result = agent
            .prompt_streaming("go", &tx, CancellationToken::new())
            .await;
        assert!(result.is_err());
        assert_eq!(
            streamer.requests.lock().unwrap().len(),
            MAX_PROVIDER_ATTEMPTS
        );

        let events = session_events(&session);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, SessionEvent::AssistantAttempt { .. }))
                .count(),
            MAX_PROVIDER_ATTEMPTS
        );
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::Error {
                code: ErrorCode::Provider,
                retryable: true,
                ..
            }
        )));
    }

    fn path_call(call_id: &str, path: &str) -> ToolCall {
        ToolCall {
            call_id: call_id.to_string(),
            fn_name: "echo-say".to_string(),
            fn_arguments: serde_json::json!({ "path": path }),
            thought_signatures: None,
        }
    }

    async fn scoped_workspace(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("meerai-{tag}-{}", SessionId::new()));
        tokio::fs::create_dir_all(root.join("src")).await.unwrap();
        tokio::fs::write(root.join("src/AGENTS.md"), "SCOPED RULE: keep tests green")
            .await
            .unwrap();
        root
    }

    fn runtime_injections(events: &[SessionEvent]) -> Vec<(usize, String)> {
        events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| match event {
                SessionEvent::UserMessage {
                    source: InputSource::RuntimeContext,
                    content,
                } => content.iter().find_map(|block| match block {
                    ContentBlock::Text(text) if text.contains("Project instructions from") => {
                        Some((index, text.clone()))
                    }
                    _ => None,
                }),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn scoped_agents_md_reaches_the_next_request_once() {
        let root = scoped_workspace("scoped").await;
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(vec![path_call(
                "c1",
                "src/lib.rs",
            )]))],
            vec![ChatStreamEvent::End(tool_end(vec![path_call(
                "c2",
                "src/main.rs",
            )]))],
            vec![ChatStreamEvent::End(text_end("done"))],
        ]));

        let session = Session::memory(SessionId::new(), root.clone());
        let mut agent = MultiTurnAgent::with_streamer(
            streamer.clone(),
            "gpt-4o-mini",
            vec![Box::new(EchoToolset)],
            "",
            MultiTurnAgentConfig::default(),
        );
        agent.set_harness(harness(&session, yolo(), HarnessConfig::default()));

        let (tx, _rx) = mpsc::unbounded_channel();
        let out = agent
            .prompt_streaming("look around", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out, "done");

        let events = session_events(&session);
        let injections = runtime_injections(&events);
        assert_eq!(injections.len(), 1);
        assert!(
            injections[0]
                .1
                .starts_with("Project instructions from src/AGENTS.md:")
        );
        assert!(injections[0].1.contains("SCOPED RULE: keep tests green"));
        let result_index = events
            .iter()
            .position(|event| matches!(event, SessionEvent::ToolResult { .. }))
            .unwrap();
        assert!(injections[0].0 > result_index);

        let reqs = streamer.requests.lock().unwrap();
        assert_eq!(reqs.len(), 3);
        assert!(!format!("{:?}", reqs[0].messages).contains("SCOPED RULE"));
        assert!(format!("{:?}", reqs[1].messages).contains("SCOPED RULE"));
        assert!(format!("{:?}", reqs[2].messages).contains("SCOPED RULE"));
        drop(reqs);

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn directory_path_injects_its_own_agents_md() {
        let root = scoped_workspace("scoped-dir").await;
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(vec![path_call("c1", "src")]))],
            vec![ChatStreamEvent::End(text_end("done"))],
        ]));

        let session = Session::memory(SessionId::new(), root.clone());
        let mut agent = MultiTurnAgent::with_streamer(
            streamer.clone(),
            "gpt-4o-mini",
            vec![Box::new(EchoToolset)],
            "",
            MultiTurnAgentConfig::default(),
        );
        agent.set_harness(harness(&session, yolo(), HarnessConfig::default()));

        let (tx, _rx) = mpsc::unbounded_channel();
        let out = agent
            .prompt_streaming("list src", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out, "done");

        let injections = runtime_injections(&session_events(&session));
        assert_eq!(injections.len(), 1);
        assert!(
            injections[0]
                .1
                .starts_with("Project instructions from src/AGENTS.md:")
        );

        let reqs = streamer.requests.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        assert!(format!("{:?}", reqs[1].messages).contains("SCOPED RULE"));
        drop(reqs);

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn path_outside_workspace_injects_nothing() {
        let root = scoped_workspace("outside").await;
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(vec![path_call(
                "c1",
                "../outside/src/lib.rs",
            )]))],
            vec![ChatStreamEvent::End(text_end("done"))],
        ]));

        let session = Session::memory(SessionId::new(), root.clone());
        let mut agent = MultiTurnAgent::with_streamer(
            streamer.clone(),
            "gpt-4o-mini",
            vec![Box::new(EchoToolset)],
            "",
            MultiTurnAgentConfig::default(),
        );
        agent.set_harness(harness(&session, yolo(), HarnessConfig::default()));

        let (tx, _rx) = mpsc::unbounded_channel();
        let out = agent
            .prompt_streaming("escape", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out, "done");

        assert!(runtime_injections(&session_events(&session)).is_empty());
        let reqs = streamer.requests.lock().unwrap();
        assert!(!format!("{:?}", reqs[1].messages).contains("SCOPED RULE"));
        drop(reqs);

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn scoped_instructions_are_cleared_between_turns() {
        let root = scoped_workspace("scoped-turns").await;
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(vec![path_call(
                "c1",
                "src/lib.rs",
            )]))],
            vec![ChatStreamEvent::End(text_end("first"))],
            vec![ChatStreamEvent::End(tool_end(vec![path_call(
                "c2",
                "src/lib.rs",
            )]))],
            vec![ChatStreamEvent::End(text_end("second"))],
        ]));

        let session = Session::memory(SessionId::new(), root.clone());
        let mut agent = MultiTurnAgent::with_streamer(
            streamer.clone(),
            "gpt-4o-mini",
            vec![Box::new(EchoToolset)],
            "",
            MultiTurnAgentConfig::default(),
        );
        agent.set_harness(harness(&session, yolo(), HarnessConfig::default()));

        let (tx, _rx) = mpsc::unbounded_channel();
        let first = agent
            .prompt_streaming("one", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(first, "first");
        let second = agent
            .prompt_streaming("two", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(second, "second");

        assert_eq!(runtime_injections(&session_events(&session)).len(), 2);

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn first_prompt_sets_a_single_line_session_title() {
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(text_end("first reply"))],
            vec![ChatStreamEvent::End(text_end("second reply"))],
        ]));
        let session = memory_session();
        let mut agent = harness_agent(
            streamer,
            vec![],
            "",
            &session,
            yolo(),
            HarnessConfig::default(),
        );
        let prompt = format!("  {}   tail\nsecond line is ignored", "a".repeat(200));

        let (tx, _rx) = mpsc::unbounded_channel();
        agent
            .prompt_streaming(&prompt, &tx, CancellationToken::new())
            .await
            .unwrap();

        let title = session.meta().title.expect("first prompt sets a title");
        assert_eq!(title, "a".repeat(80));
        assert!(!title.contains('\n'));
        assert!(!title.contains("second line"));

        agent
            .prompt_streaming("a later prompt", &tx, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(session.meta().title.as_deref(), Some(title.as_str()));
    }

    #[tokio::test]
    async fn summarizer_model_is_used_for_compaction() {
        let reply = "x".repeat(800);
        let summary = "Goal: use the summarizer model";
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(text_end(&reply))],
            vec![ChatStreamEvent::End(text_end(summary))],
            vec![ChatStreamEvent::End(text_end("second reply"))],
        ]));

        let session = memory_session();
        let config = HarnessConfig {
            context_window_tokens: 80,
            high_watermark_pct: 50,
            verbatim_window_turns: 1,
            summarizer_model: Some("openrouter:tiny-summarizer".to_string()),
            ..HarnessConfig::default()
        };
        let mut agent = harness_agent(streamer.clone(), vec![], "system", &session, yolo(), config);

        let (tx, _rx) = mpsc::unbounded_channel();
        agent
            .prompt_streaming("first", &tx, CancellationToken::new())
            .await
            .unwrap();
        agent
            .prompt_streaming("second", &tx, CancellationToken::new())
            .await
            .unwrap();

        let models = streamer.models.lock().unwrap().clone();
        assert_eq!(
            models,
            vec![
                "gpt-4o-mini".to_string(),
                "openrouter:tiny-summarizer".to_string(),
                "gpt-4o-mini".to_string(),
            ]
        );
        assert!(session_events(&session).iter().any(|event| matches!(
            event,
            SessionEvent::CompactionEnded {
                result: CompactionResult::Applied { .. },
                ..
            }
        )));
    }

    #[tokio::test]
    async fn redacts_tool_output_in_session_log_and_items() {
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
                call_id: "c1".to_string(),
                fn_name: "leak-echo".to_string(),
                fn_arguments: serde_json::json!({}),
                thought_signatures: None,
            }]))],
            vec![ChatStreamEvent::End(text_end("done"))],
        ]));
        let session = memory_session();
        let mut agent = harness_agent(
            streamer.clone(),
            vec![Box::new(LeakToolset)],
            "",
            &session,
            yolo(),
            HarnessConfig::default(),
        );

        let (tx, mut rx) = mpsc::unbounded_channel();
        agent
            .prompt_streaming("go", &tx, CancellationToken::new())
            .await
            .unwrap();

        let logged = serde_json::to_string(&session_events(&session)).unwrap();
        assert!(logged.contains("[REDACTED:openai_key]"));
        assert!(!logged.contains(LEAKED_TOKEN));

        let mut outputs = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let AgentEvent::ItemCompleted {
                item: SessionItem::ToolCall { output, .. },
            } = event
                && let Some(output) = output
            {
                outputs.push(output);
            }
        }
        assert_eq!(outputs.len(), 1);
        assert!(outputs[0].contains("[REDACTED:openai_key]"));
        assert!(!outputs[0].contains(LEAKED_TOKEN));

        let reqs = streamer.requests.lock().unwrap();
        let model_view = format!("{:?}", reqs[1].messages);
        assert!(model_view.contains("[REDACTED:openai_key]"));
        assert!(!model_view.contains(LEAKED_TOKEN));
    }

    #[tokio::test]
    async fn redacts_file_change_diff_text() {
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
                call_id: "c1".to_string(),
                fn_name: "leak-diff".to_string(),
                fn_arguments: serde_json::json!({}),
                thought_signatures: None,
            }]))],
            vec![ChatStreamEvent::End(text_end("done"))],
        ]));
        let session = memory_session();
        let mut agent = harness_agent(
            streamer,
            vec![Box::new(LeakDiffToolset)],
            "",
            &session,
            yolo(),
            HarnessConfig::default(),
        );

        let (tx, mut rx) = mpsc::unbounded_channel();
        agent
            .prompt_streaming("write", &tx, CancellationToken::new())
            .await
            .unwrap();

        let mut changes = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let AgentEvent::ItemCompleted {
                item: SessionItem::FileChange { changes: diffs, .. },
            } = event
            {
                changes.extend(diffs);
            }
        }
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "a.rs");
        assert!(changes[0].unified_diff.contains("[REDACTED:openai_key]"));
        assert!(!changes[0].unified_diff.contains(LEAKED_TOKEN));
    }

    #[tokio::test]
    async fn redacts_failed_tool_dispatch_errors() {
        let streamer = Arc::new(FakeStreamer::new(vec![
            vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
                call_id: "c1".to_string(),
                fn_name: "failing-run".to_string(),
                fn_arguments: serde_json::json!({}),
                thought_signatures: None,
            }]))],
            vec![ChatStreamEvent::End(text_end("done"))],
        ]));
        let session = memory_session();
        let mut agent = harness_agent(
            streamer.clone(),
            vec![Box::new(FailingToolset)],
            "",
            &session,
            yolo(),
            HarnessConfig::default(),
        );

        let (tx, mut rx) = mpsc::unbounded_channel();
        agent
            .prompt_streaming("go", &tx, CancellationToken::new())
            .await
            .unwrap();

        let logged = serde_json::to_string(&session_events(&session)).unwrap();
        assert!(logged.contains("[REDACTED:openai_key]"));
        assert!(!logged.contains(LEAKED_TOKEN));

        let mut outputs = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let AgentEvent::ItemCompleted {
                item: SessionItem::ToolCall { output, .. },
            } = event
                && let Some(output) = output
            {
                outputs.push(output);
            }
        }
        assert_eq!(outputs.len(), 1);
        assert!(outputs[0].contains("[REDACTED:openai_key]"));
        assert!(!outputs[0].contains(LEAKED_TOKEN));

        let reqs = streamer.requests.lock().unwrap();
        let model_view = format!("{:?}", reqs[1].messages);
        assert!(model_view.contains("[REDACTED:openai_key]"));
        assert!(!model_view.contains(LEAKED_TOKEN));
    }

    #[tokio::test]
    async fn attaching_a_harness_seeds_cumulative_usage() {
        let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
            text_end("resumed reply"),
        )]]));
        let session = memory_session();
        let seeded: meerai_session::Usage = meerai_session::Usage {
            prompt_tokens: Some(100),
            completion_tokens: Some(40),
            total_tokens: Some(140),
            ..Default::default()
        };
        session
            .append(SessionEvent::UsageUpdated {
                usage: seeded,
                cumulative: seeded,
            })
            .await
            .unwrap();

        let mut agent = harness_agent(
            streamer,
            vec![],
            "",
            &session,
            yolo(),
            HarnessConfig::default(),
        );

        let (tx, mut rx) = mpsc::unbounded_channel();
        agent
            .prompt_streaming("continue", &tx, CancellationToken::new())
            .await
            .unwrap();

        let mut latest = None;
        while let Ok(event) = rx.try_recv() {
            if let AgentEvent::UsageUpdated { cumulative, .. } = event {
                latest = Some(cumulative);
            }
        }
        assert_eq!(latest.unwrap().prompt_tokens, Some(110));
        assert_eq!(latest.unwrap().completion_tokens, Some(45));
        assert_eq!(latest.unwrap().total_tokens, Some(155));

        let logged = session_events(&session)
            .into_iter()
            .filter_map(|event| match event {
                SessionEvent::UsageUpdated { cumulative, .. } => Some(cumulative),
                _ => None,
            })
            .next_back()
            .expect("usage logged");
        assert_eq!(logged.prompt_tokens, Some(110));
        assert_eq!(logged.completion_tokens, Some(45));
        assert_eq!(logged.total_tokens, Some(155));
    }
}
