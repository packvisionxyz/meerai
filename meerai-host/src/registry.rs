use crate::diff::{FileChangeEntry, UnifiedLine, tool_edit_diff};
use crate::session::{
    Backend, HostInfoSnapshot, HostReloadOutcome, OpenedSession, ResolvedAgentApplication,
    ResolvedSkillApplication,
};
use meerai_agents::{
    AgentCommand, AgentEvent, ApprovalRequest, ErrorCode, ItemDelta, ItemId, ItemStatus,
    QuestionAnswer, QuestionPrompt, QuestionRequest, RewindScope, SessionItem, TurnEndReason,
    TurnId, Usage,
};
use meerai_core::env::now_millis;
use meerai_policy::ApprovalMode;
use meerai_session::replay::{
    ReplayEntry, ToolCallStatus, replay_from_events, session_usage_from_events,
};
use meerai_session::{
    ApprovalDecision, GrantScope, InputSource, LoggedEvent, RequestId, SessionEvent, SessionId,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

mod config;
mod events_input;
mod events_items;
mod events_meta;
mod events_turn;
mod input;
mod notify;
mod open;
mod shell;
mod tui;
mod turns;
pub const CAPACITY: usize = 4096;
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const TITLE_MAX_CHARS: usize = 80;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostError {
    SessionNotFound(SessionId),
    Busy,
    NotRunning,
    UnsupportedBackend(String),
    EmptyPrompt,
    StaleRequest(RequestId),
    InvalidInput(String),
    WorkerClosed,
    Poisoned,
}
impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SessionNotFound(id) => write!(f, "session not found: {id}"),
            Self::Busy => write!(f, "session is busy"),
            Self::NotRunning => write!(f, "session has no active turn"),
            Self::UnsupportedBackend(msg) => write!(f, "{msg}"),
            Self::EmptyPrompt => write!(f, "prompt must not be empty"),
            Self::StaleRequest(id) => write!(f, "stale or unknown request: {id}"),
            Self::InvalidInput(msg) => write!(f, "{msg}"),
            Self::WorkerClosed => write!(f, "session worker is closed"),
            Self::Poisoned => write!(f, "session registry lock was poisoned"),
        }
    }
}
impl std::error::Error for HostError {}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Idle,
    Running,
    WaitingApproval,
    WaitingQuestion,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageStatsSnapshot {
    pub prompt_tokens: Option<i32>,
    pub completion_tokens: Option<i32>,
    pub total_tokens: Option<i32>,
    pub cached_tokens: Option<i32>,
    pub cache_creation_tokens: Option<i32>,
    pub est_cost_usd: Option<f64>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TurnMetricsSnapshot {
    pub turn_count: usize,
    pub tool_calls: usize,
    pub session_prompt_tokens: u64,
    pub session_completion_tokens: u64,
    pub session_total_tokens: u64,
    pub session_cached_tokens: u64,
    pub session_cache_creation_tokens: u64,
    pub last_turn_ms: Option<u64>,
    pub tokens_per_sec: Option<f64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "data")]
pub enum LiveEntry {
    User {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<TurnId>,
        text: String,
    },
    Assistant {
        id: String,
        text: String,
        reasoning: String,
    },
    System {
        id: String,
        text: String,
    },
    Error {
        id: String,
        message: String,
    },
    ToolCall {
        id: String,
        name: String,
        #[serde(default)]
        args: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edit_diff: Option<Vec<UnifiedLine>>,
    },
    Subagent {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        agent: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        report: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        child_session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default)]
        approval_mode: String,
        #[serde(default)]
        steps_used: usize,
        #[serde(default)]
        max_steps: usize,
        #[serde(default)]
        usage: Usage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        est_cost_usd: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        active_tool: Option<String>,
    },
    Question {
        id: String,
        questions: Vec<QuestionPrompt>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answers: Option<Vec<QuestionAnswer>>,
        resolved: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveShellRun {
    pub id: String,
    pub command: String,
    pub started_at_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveSessionSummary {
    pub session_id: SessionId,
    pub title: Option<String>,
    pub backend: Backend,
    pub provider: String,
    pub model: String,
    pub approval_mode: ApprovalMode,
    pub status: SessionStatus,
    pub active_turn_id: Option<TurnId>,
    pub active_tool: Option<String>,
    pub turn_started_at_ms: Option<u64>,
    pub user_shell_running: bool,
    #[serde(default)]
    pub transient: bool,
    #[serde(default)]
    pub shell_runs: Vec<LiveShellRun>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiveSessionDetail {
    pub summary: LiveSessionSummary,
    pub entries: Vec<LiveEntry>,
    pub streaming_item_id: Option<String>,
    pub streaming_text: String,
    pub streaming_reasoning: String,
    pub pending_approvals: Vec<ApprovalRequest>,
    pub pending_questions: Vec<QuestionRequest>,
    pub usage: UsageStatsSnapshot,
    pub metrics: TurnMetricsSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<meerai_session::ContextBreakdown>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_state: Option<meerai_session::LoopState>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegistrySnapshot {
    pub event_id: u64,
    pub active_session_id: Option<SessionId>,
    pub sessions: Vec<LiveSessionSummary>,
    pub active_session: Option<LiveSessionDetail>,
    pub file_changes: Vec<FileChangeEntry>,
    pub poisoned: bool,
}

impl RegistrySnapshot {
    pub fn poisoned() -> Self {
        Self {
            event_id: 0,
            active_session_id: None,
            sessions: Vec::new(),
            active_session: None,
            file_changes: Vec::new(),
            poisoned: true,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "data")]
pub enum ChatGptLoginStatus {
    Started {
        attempt_id: String,
        provider_id: String,
        url: String,
    },
    Completed {
        attempt_id: String,
        provider_id: String,
        account: Option<String>,
    },
    Failed {
        attempt_id: String,
        provider_id: String,
        message: String,
    },
    Cancelled {
        attempt_id: String,
        provider_id: String,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "data")]
pub enum RegistryEventKind {
    SessionOpened {
        session: LiveSessionSummary,
    },
    SessionUpdated {
        session: LiveSessionSummary,
    },
    ContextUpdated {
        session_id: SessionId,
        breakdown: meerai_session::ContextBreakdown,
    },
    LoopUpdated {
        session_id: SessionId,
        state: meerai_session::LoopState,
    },
    SessionActivated {
        session_id: SessionId,
    },
    SessionClosed {
        session_id: SessionId,
        next_active_session_id: Option<SessionId>,
    },
    SessionReset {
        session_id: SessionId,
    },
    SessionRewound {
        session_id: SessionId,
        turn_id: TurnId,
        scope: RewindScope,
        restored: Vec<String>,
        error: Option<String>,
        entries: Option<Vec<LiveEntry>>,
        restored_prompt: Option<String>,
    },
    TurnStarted {
        session_id: SessionId,
        turn_id: TurnId,
        model: String,
    },
    UserPromptSubmitted {
        session_id: SessionId,
        entry_id: String,
        text: String,
    },
    TextDelta {
        session_id: SessionId,
        item_id: String,
        delta: String,
    },
    ReasoningDelta {
        session_id: SessionId,
        item_id: String,
        delta: String,
    },
    StreamDiscarded {
        session_id: SessionId,
    },
    EntryCompleted {
        session_id: SessionId,
        entry: LiveEntry,
    },
    EntryUpdated {
        session_id: SessionId,
        entry: LiveEntry,
    },
    ApprovalRequested {
        session_id: SessionId,
        request: ApprovalRequest,
    },
    ApprovalResolved {
        session_id: SessionId,
        request_id: RequestId,
    },
    QuestionAsked {
        session_id: SessionId,
        request: QuestionRequest,
    },
    QuestionResolved {
        session_id: SessionId,
        request_id: RequestId,
    },
    UsageUpdated {
        session_id: SessionId,
        usage: UsageStatsSnapshot,
        metrics: TurnMetricsSnapshot,
    },
    TurnCompleted {
        session_id: SessionId,
        turn_id: TurnId,
        reason: TurnEndReason,
    },
    Error {
        session_id: SessionId,
        code: ErrorCode,
        message: String,
    },
    FileChangesUpdated {
        changes: Vec<FileChangeEntry>,
    },
    Reloaded {
        host_info: Box<HostInfoSnapshot>,
        warnings: Vec<String>,
    },
    ProvidersUpdated,
    ChatGptLoginUpdated(ChatGptLoginStatus),
    HostDisconnected,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegistryEvent {
    pub seq: u64,
    pub kind: RegistryEventKind,
}
#[derive(Debug)]
pub enum SubscribeResult {
    Replay {
        events: Vec<RegistryEvent>,
        rx: broadcast::Receiver<RegistryEvent>,
    },
    ResyncRequired {
        current_seq: u64,
        rx: broadcast::Receiver<RegistryEvent>,
    },
}
struct SessionRecord {
    summary: LiveSessionSummary,
    entries: Vec<LiveEntry>,
    streaming_item_id: Option<String>,
    streaming_text: String,
    streaming_reasoning: String,
    cmd_tx: mpsc::UnboundedSender<AgentCommand>,
    pending_approvals: VecDeque<ApprovalRequest>,
    pending_questions: VecDeque<QuestionRequest>,
    usage: UsageStatsSnapshot,
    metrics: TurnMetricsSnapshot,
    context: Option<meerai_session::ContextBreakdown>,
    loop_state: Option<meerai_session::LoopState>,
    turn_started_instant: Option<Instant>,
    tool_items: HashMap<ItemId, String>,
    recovery_pending: bool,
}
impl SessionRecord {
    fn to_detail(&self) -> LiveSessionDetail {
        LiveSessionDetail {
            summary: self.summary.clone(),
            entries: self.entries.clone(),
            streaming_item_id: self.streaming_item_id.clone(),
            streaming_text: self.streaming_text.clone(),
            streaming_reasoning: self.streaming_reasoning.clone(),
            pending_approvals: self.pending_approvals.iter().cloned().collect(),
            pending_questions: self.pending_questions.iter().cloned().collect(),
            usage: self.usage.clone(),
            metrics: self.metrics.clone(),
            context: self.context.clone(),
            loop_state: self.loop_state.clone(),
        }
    }
    fn recompute_status(&mut self) {
        self.summary.status = if !self.pending_approvals.is_empty() {
            SessionStatus::WaitingApproval
        } else if !self.pending_questions.is_empty() {
            SessionStatus::WaitingQuestion
        } else if self.summary.active_turn_id.is_some() {
            SessionStatus::Running
        } else {
            SessionStatus::Idle
        };
    }
    fn is_acp(&self) -> bool {
        matches!(self.summary.backend, Backend::Acp { .. })
    }
}
struct RegistryState {
    active_session_id: Option<SessionId>,
    session_order: Vec<SessionId>,
    sessions: HashMap<SessionId, SessionRecord>,
    file_changes: Vec<FileChangeEntry>,
    current_seq: u64,
    ring: VecDeque<RegistryEvent>,
    next_entry_seq: u64,
    pump_handles: HashMap<SessionId, JoinHandle<()>>,
}
impl RegistryState {
    fn new() -> Self {
        Self {
            active_session_id: None,
            session_order: Vec::new(),
            sessions: HashMap::new(),
            file_changes: Vec::new(),
            current_seq: 0,
            ring: VecDeque::with_capacity(CAPACITY),
            next_entry_seq: 0,
            pump_handles: HashMap::new(),
        }
    }
    fn next_entry_id(&mut self) -> String {
        self.next_entry_seq += 1;
        format!("entry-{}", self.next_entry_seq)
    }
    fn push_event(
        &mut self,
        broadcast_tx: &broadcast::Sender<RegistryEvent>,
        kind: RegistryEventKind,
    ) {
        self.current_seq += 1;
        let event = RegistryEvent {
            seq: self.current_seq,
            kind,
        };
        if self.ring.len() == CAPACITY {
            self.ring.pop_front();
        }
        self.ring.push_back(event.clone());
        let _ = broadcast_tx.send(event);
    }
    fn flush_streaming(
        &mut self,
        session_id: &SessionId,
        broadcast_tx: &broadcast::Sender<RegistryEvent>,
    ) {
        let entry_id = self.next_entry_id();
        let completed = {
            let Some(record) = self.sessions.get_mut(session_id) else {
                return;
            };
            if record.streaming_text.is_empty() && record.streaming_reasoning.is_empty() {
                record.streaming_item_id = None;
                return;
            }
            let id = record.streaming_item_id.take().unwrap_or(entry_id);
            let text = std::mem::take(&mut record.streaming_text);
            let reasoning = std::mem::take(&mut record.streaming_reasoning);
            let entry = LiveEntry::Assistant {
                id,
                text,
                reasoning,
            };
            record.entries.push(entry.clone());
            entry
        };
        self.push_event(
            broadcast_tx,
            RegistryEventKind::EntryCompleted {
                session_id: session_id.clone(),
                entry: completed,
            },
        );
    }
}
struct RegistryInner {
    state: Mutex<RegistryState>,
    broadcast_tx: broadcast::Sender<RegistryEvent>,
    agent_broadcast_tx: broadcast::Sender<(SessionId, AgentEvent)>,
}
pub struct SessionRegistry {
    inner: Arc<RegistryInner>,
}
async fn run_pump(
    inner: Arc<RegistryInner>,
    session_id: SessionId,
    mut event_rx: mpsc::UnboundedReceiver<AgentEvent>,
) {
    while let Some(event) = event_rx.recv().await {
        match inner.state.lock() {
            Ok(mut state) => {
                apply_agent_event(&mut state, &inner.broadcast_tx, &session_id, event.clone());
            }
            Err(_) => break,
        }
        let _ = inner.agent_broadcast_tx.send((session_id.clone(), event));
    }
}
fn apply_agent_event(
    state: &mut RegistryState,
    broadcast_tx: &broadcast::Sender<RegistryEvent>,
    session_id: &SessionId,
    event: AgentEvent,
) {
    if !state.sessions.contains_key(session_id) {
        return;
    }
    match event {
        AgentEvent::TurnStarted { .. }
        | AgentEvent::RecoveryStarted { .. }
        | AgentEvent::TurnCompleted { .. }
        | AgentEvent::Error { .. } => {
            state.apply_turn_event(broadcast_tx, session_id, event);
        }
        AgentEvent::ItemStarted { .. }
        | AgentEvent::ItemDelta { .. }
        | AgentEvent::ItemCompleted { .. }
        | AgentEvent::ItemDiscarded { .. } => {
            state.apply_items_event(broadcast_tx, session_id, event);
        }
        AgentEvent::ApprovalRequested { .. }
        | AgentEvent::ApprovalResolved { .. }
        | AgentEvent::QuestionAsked { .. }
        | AgentEvent::QuestionResolved { .. } => {
            state.apply_input_event(broadcast_tx, session_id, event);
        }
        AgentEvent::ContextUpdated { .. }
        | AgentEvent::LoopUpdated { .. }
        | AgentEvent::LoopContinued { .. }
        | AgentEvent::HookCompleted { .. }
        | AgentEvent::UsageUpdated { .. }
        | AgentEvent::RewindCompleted { .. } => {
            state.apply_meta_event(broadcast_tx, session_id, event);
        }
    }
}
fn finalize_open_entries(record: &mut SessionRecord, fallback: ToolCallStatus) -> Vec<LiveEntry> {
    let fallback_str = replay_status_str(fallback).to_string();
    let mut updated = Vec::new();
    for entry in &mut record.entries {
        match entry {
            LiveEntry::Subagent { status, .. }
                if matches!(status.as_str(), "proposed" | "running") =>
            {
                *status = fallback_str.clone();
                updated.push(entry.clone());
            }
            _ => {}
        }
    }
    updated
}
fn update_pending_status<F>(
    state: &mut RegistryState,
    broadcast_tx: &broadcast::Sender<RegistryEvent>,
    session_id: &SessionId,
    mutate: F,
) where
    F: FnOnce(&mut SessionRecord),
{
    let updated = {
        let Some(record) = state.sessions.get_mut(session_id) else {
            return;
        };
        let prev_status = record.summary.status;
        mutate(record);
        record.recompute_status();
        if record.summary.status != prev_status {
            Some(record.summary.clone())
        } else {
            None
        }
    };
    if let Some(session) = updated {
        state.push_event(broadcast_tx, RegistryEventKind::SessionUpdated { session });
    }
}
fn update_or_push_tool_entry(
    record: &mut SessionRecord,
    item_id: &ItemId,
    fallback_id: String,
    name: String,
    args: String,
    status: &str,
    output: Option<String>,
) -> LiveEntry {
    let entry_id = record.tool_items.remove(item_id).unwrap_or(fallback_id);
    if let Some(existing) = record
        .entries
        .iter_mut()
        .rev()
        .find(|entry| matches!(entry, LiveEntry::ToolCall { id, .. } if *id == entry_id))
    {
        if let LiveEntry::ToolCall {
            name: slot_name,
            args: slot_args,
            status: slot_status,
            output: slot_output,
            edit_diff: slot_diff,
            ..
        } = existing
        {
            *slot_name = name.clone();
            if !args.is_empty() {
                *slot_args = args.clone();
            }
            *slot_status = status.to_string();
            if output.is_some() {
                *slot_output = output;
            }
            *slot_diff = tool_edit_diff(slot_name, slot_args, slot_output.as_deref());
        }
        return existing.clone();
    }
    let edit_diff = tool_edit_diff(&name, &args, output.as_deref());
    let entry = LiveEntry::ToolCall {
        id: entry_id,
        name,
        args,
        status: status.to_string(),
        output,
        edit_diff,
    };
    record.entries.push(entry.clone());
    entry
}
fn update_or_push_subagent_entry(
    record: &mut SessionRecord,
    item_id: &ItemId,
    name: Option<String>,
    agent: String,
    status: &str,
    report: Option<String>,
    child_session_id: Option<String>,
) -> LiveEntry {
    let fallback_id = item_id.to_string();
    let entry_id = record.tool_items.remove(item_id).unwrap_or(fallback_id);
    if let Some(existing) = record
        .entries
        .iter_mut()
        .rev()
        .find(|entry| matches!(entry, LiveEntry::Subagent { id, .. } if *id == entry_id))
    {
        if let LiveEntry::Subagent {
            name: slot_name,
            agent: slot_agent,
            status: slot_status,
            report: slot_report,
            child_session_id: slot_child,
            ..
        } = existing
        {
            if name.is_some() {
                *slot_name = name;
            }
            *slot_agent = agent;
            *slot_status = status.to_string();
            if report.is_some() {
                *slot_report = report;
            }
            if child_session_id.is_some() {
                *slot_child = child_session_id;
            }
        }
        return existing.clone();
    }
    let entry = LiveEntry::Subagent {
        id: entry_id,
        name,
        agent,
        status: status.to_string(),
        report,
        child_session_id,
        model: None,
        approval_mode: ApprovalMode::Ask.label().to_string(),
        steps_used: 0,
        max_steps: 0,
        usage: Usage::default(),
        est_cost_usd: None,
        active_tool: None,
    };
    record.entries.push(entry.clone());
    entry
}
fn last_loop_from_events(events: &[LoggedEvent]) -> Option<meerai_session::LoopState> {
    meerai_session::last_loop(events)
}
fn collect_user_turn_ids(events: &[LoggedEvent]) -> Vec<Option<TurnId>> {
    let shadowed: Vec<_> = events
        .iter()
        .filter_map(|logged| match &logged.event {
            SessionEvent::RewindApplied {
                replaced: Some(replaced),
                ..
            } => Some(*replaced),
            _ => None,
        })
        .collect();
    let mut turns = Vec::new();
    let mut current: Option<TurnId> = None;
    for logged in events {
        if shadowed.iter().any(|range| range.contains(logged.seq)) {
            continue;
        }
        match &logged.event {
            SessionEvent::TurnStarted { turn_id, .. } => current = Some(turn_id.clone()),
            SessionEvent::UserMessage {
                source: InputSource::User | InputSource::Steering,
                ..
            } => turns.push(current.clone()),
            _ => {}
        }
    }
    turns
}
fn cumulative_metrics_from_usage(usage: Option<&Usage>) -> TurnMetricsSnapshot {
    let Some(u) = usage else {
        return TurnMetricsSnapshot::default();
    };
    let prompt = u.prompt_tokens.unwrap_or(0).max(0) as u64;
    let completion = u.completion_tokens.unwrap_or(0).max(0) as u64;
    let total = u
        .total_tokens
        .map(|t| t.max(0) as u64)
        .unwrap_or(prompt + completion);
    let cached = u.cached_tokens.unwrap_or(0).max(0) as u64;
    TurnMetricsSnapshot {
        turn_count: 0,
        tool_calls: 0,
        session_prompt_tokens: prompt,
        session_completion_tokens: completion,
        session_total_tokens: total,
        session_cached_tokens: cached,
        session_cache_creation_tokens: u.cache_creation_tokens.unwrap_or(0).max(0) as u64,
        last_turn_ms: None,
        tokens_per_sec: None,
    }
}
fn replay_entry_to_live(
    entry: ReplayEntry,
    turn_iter: &mut impl Iterator<Item = Option<TurnId>>,
    state: &mut RegistryState,
) -> LiveEntry {
    match entry {
        ReplayEntry::User { text } => LiveEntry::User {
            id: state.next_entry_id(),
            turn_id: turn_iter.next().flatten(),
            text,
        },
        ReplayEntry::Assistant { text, reasoning } => LiveEntry::Assistant {
            id: state.next_entry_id(),
            text,
            reasoning,
        },
        ReplayEntry::ToolCall(tc) => {
            let edit_diff = tool_edit_diff(&tc.name, &tc.args, tc.output.as_deref());
            LiveEntry::ToolCall {
                id: tc.call_id,
                name: tc.name,
                args: tc.args,
                status: replay_status_str(tc.status).to_string(),
                output: tc.output,
                edit_diff,
            }
        }
        ReplayEntry::Subagent(sub) => LiveEntry::Subagent {
            id: sub.id,
            name: sub.name,
            agent: sub.agent,
            status: replay_status_str(sub.status).to_string(),
            report: sub.report,
            child_session_id: sub.child_session_id,
            model: None,
            approval_mode: ApprovalMode::Ask.label().to_string(),
            steps_used: 0,
            max_steps: 0,
            usage: Usage::default(),
            est_cost_usd: None,
            active_tool: None,
        },
        ReplayEntry::Question(q) => LiveEntry::Question {
            id: q.request_id,
            questions: q.questions,
            answers: q.answers,
            resolved: q.resolved,
        },
        ReplayEntry::System { text } => LiveEntry::System {
            id: state.next_entry_id(),
            text,
        },
    }
}
fn replay_status_str(status: ToolCallStatus) -> &'static str {
    match status {
        ToolCallStatus::Proposed => "proposed",
        ToolCallStatus::Running => "running",
        ToolCallStatus::Succeeded => "succeeded",
        ToolCallStatus::Failed => "failed",
        ToolCallStatus::Stopped => "stopped",
        ToolCallStatus::Cancelled => "cancelled",
    }
}
fn item_status_str(status: ItemStatus) -> &'static str {
    match status {
        ItemStatus::Proposed => "proposed",
        ItemStatus::Pending | ItemStatus::InProgress => "running",
        ItemStatus::Succeeded => "succeeded",
        ItemStatus::Failed | ItemStatus::Denied | ItemStatus::Cancelled => "failed",
        ItemStatus::Stopped => "stopped",
    }
}
fn completed_item_status_str(status: ItemStatus) -> &'static str {
    match item_status_str(status) {
        "proposed" => "running",
        other => other,
    }
}
fn display_model(model: &str) -> String {
    model
        .split_once("::")
        .map(|(_, m)| m.to_string())
        .unwrap_or_else(|| model.to_string())
}
fn display_text(display: Option<String>, prompt: &str) -> String {
    display
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| prompt.to_string())
}
fn normalize_title(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|line| !line.is_empty())?;
    let single: String = line
        .chars()
        .map(|ch| if ch == '\n' || ch == '\r' { ' ' } else { ch })
        .take(TITLE_MAX_CHARS + 1)
        .collect();
    if single.chars().count() > TITLE_MAX_CHARS {
        let prefix: String = single
            .chars()
            .take(TITLE_MAX_CHARS.saturating_sub(3))
            .collect();
        Some(format!("{prefix}..."))
    } else {
        Some(single)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use meerai_agents::ApprovalRequest;
    use meerai_session::{
        ApprovalKind, ApprovalPreview, ContentBlock, InputSource, LoggedEvent, Seq, SessionEvent,
    };
    struct TestWorker {
        session_id: SessionId,
        cmd_rx: mpsc::UnboundedReceiver<AgentCommand>,
        event_tx: mpsc::UnboundedSender<AgentEvent>,
    }
    fn open_test_session(
        registry: &SessionRegistry,
        id: &str,
        title: Option<&str>,
        events: Vec<LoggedEvent>,
    ) -> TestWorker {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let session_id = SessionId::from_raw(id);
        registry.register_opened(OpenedSession {
            session_id: session_id.clone(),
            title: title.map(str::to_string),
            backend: Backend::Native,
            provider: "gemini".to_string(),
            model: "gemini-3.8-flash".to_string(),
            approval_mode: ApprovalMode::Ask,
            transient: false,
            events,
            cmd_tx,
            event_rx,
        });
        TestWorker {
            session_id,
            cmd_rx,
            event_tx,
        }
    }
    async fn wait_until<F>(mut predicate: F)
    where
        F: FnMut() -> bool,
    {
        for _ in 0..100 {
            if predicate() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("condition not satisfied before timeout");
    }
    #[test]
    fn cumulative_metrics_carry_cache_read_and_write_tokens() {
        let usage = Usage {
            prompt_tokens: Some(200),
            completion_tokens: Some(20),
            total_tokens: Some(220),
            cached_tokens: Some(150),
            cache_creation_tokens: Some(40),
        };
        let metrics = cumulative_metrics_from_usage(Some(&usage));
        assert_eq!(metrics.session_prompt_tokens, 200);
        assert_eq!(metrics.session_cached_tokens, 150);
        assert_eq!(metrics.session_cache_creation_tokens, 40);
    }
    #[tokio::test]
    async fn submit_prompt_keeps_display_text_and_sends_the_clean_prompt() {
        let registry = SessionRegistry::new();
        let mut worker = open_test_session(&registry, "ses-display", None, Vec::new());
        registry
            .submit_prompt(
                &worker.session_id,
                "review this".to_string(),
                Some("@skill/orchestrate review this".to_string()),
            )
            .unwrap();
        assert_eq!(
            worker.cmd_rx.recv().await,
            Some(AgentCommand::SubmitPrompt("review this".to_string()))
        );
        let snap = registry.snapshot();
        assert_eq!(snap.sessions[0].title.as_deref(), Some("review this"));
        assert!(
            snap.active_session
                .as_ref()
                .unwrap()
                .entries
                .iter()
                .any(|entry| matches!(
                    entry,
                    LiveEntry::User { text, .. } if text == "@skill/orchestrate review this"
                ))
        );
    }
    #[tokio::test]
    async fn streams_deltas_completes_turn_and_updates_snapshot() {
        let registry = SessionRegistry::new();
        let mut worker = open_test_session(
            &registry,
            "ses-stream",
            None,
            vec![LoggedEvent {
                seq: Seq(1),
                event: SessionEvent::UserMessage {
                    source: InputSource::User,
                    content: vec![ContentBlock::Text("prior question".to_string())],
                },
            }],
        );
        let snap = registry.snapshot();
        assert_eq!(snap.sessions.len(), 1);
        assert_eq!(snap.active_session_id, Some(worker.session_id.clone()));
        assert_eq!(
            snap.active_session.as_ref().unwrap().entries,
            vec![LiveEntry::User {
                id: "entry-1".to_string(),
                turn_id: None,
                text: "prior question".to_string(),
            }]
        );
        let SubscribeResult::Replay {
            events: initial_replay,
            mut rx,
        } = registry.subscribe(None)
        else {
            panic!("expected Replay");
        };
        assert!(initial_replay.is_empty());
        assert_eq!(
            registry.submit_prompt(
                &worker.session_id,
                "  fix bug\nmore details  ".to_string(),
                None
            ),
            Ok(())
        );
        assert_eq!(
            worker.cmd_rx.recv().await,
            Some(AgentCommand::SubmitPrompt(
                "fix bug\nmore details".to_string()
            ))
        );
        let running_snap = registry.snapshot();
        assert_eq!(running_snap.sessions[0].title.as_deref(), Some("fix bug"));
        assert_eq!(running_snap.sessions[0].status, SessionStatus::Running);
        let turn_id = TurnId::from_raw("turn-1");
        let reasoning_id = ItemId::from_raw("item-r1");
        let message_id = ItemId::from_raw("item-m1");
        worker
            .event_tx
            .send(AgentEvent::TurnStarted {
                turn_id: turn_id.clone(),
                model: "google::gemini-3.8-flash".to_string(),
            })
            .unwrap();
        worker
            .event_tx
            .send(AgentEvent::ItemStarted {
                item: SessionItem::Reasoning {
                    id: reasoning_id.clone(),
                    summary: String::new(),
                },
            })
            .unwrap();
        worker
            .event_tx
            .send(AgentEvent::ItemDelta {
                item_id: reasoning_id.clone(),
                delta: ItemDelta::ReasoningSummary("thinking".to_string()),
            })
            .unwrap();
        worker
            .event_tx
            .send(AgentEvent::ItemCompleted {
                item: SessionItem::Reasoning {
                    id: reasoning_id,
                    summary: "thinking".to_string(),
                },
            })
            .unwrap();
        worker
            .event_tx
            .send(AgentEvent::ItemStarted {
                item: SessionItem::AgentMessage {
                    id: message_id.clone(),
                    text: String::new(),
                },
            })
            .unwrap();
        worker
            .event_tx
            .send(AgentEvent::ItemDelta {
                item_id: message_id.clone(),
                delta: ItemDelta::AgentText("hello ".to_string()),
            })
            .unwrap();
        worker
            .event_tx
            .send(AgentEvent::ItemDelta {
                item_id: message_id.clone(),
                delta: ItemDelta::AgentText("world".to_string()),
            })
            .unwrap();
        wait_until(|| {
            registry
                .session_detail(&worker.session_id)
                .is_some_and(|d| d.streaming_text == "hello world")
        })
        .await;
        worker
            .event_tx
            .send(AgentEvent::ItemCompleted {
                item: SessionItem::AgentMessage {
                    id: message_id,
                    text: "hello world".to_string(),
                },
            })
            .unwrap();
        worker
            .event_tx
            .send(AgentEvent::TurnCompleted {
                turn_id: turn_id.clone(),
                reason: TurnEndReason::Completed,
            })
            .unwrap();
        wait_until(|| {
            registry
                .session_detail(&worker.session_id)
                .is_some_and(|d| {
                    d.summary.status == SessionStatus::Idle && d.metrics.turn_count >= 1
                })
        })
        .await;
        let final_snap = registry.snapshot();
        let active = final_snap.active_session.unwrap();
        assert_eq!(active.summary.status, SessionStatus::Idle);
        assert_eq!(active.summary.active_turn_id, None);
        assert_eq!(active.streaming_item_id, None);
        assert!(active.streaming_text.is_empty());
        assert!(active.streaming_reasoning.is_empty());
        assert_eq!(
            active.entries,
            vec![
                LiveEntry::User {
                    id: "entry-1".to_string(),
                    turn_id: None,
                    text: "prior question".to_string(),
                },
                LiveEntry::User {
                    id: "entry-2".to_string(),
                    turn_id: Some(turn_id),
                    text: "fix bug\nmore details".to_string(),
                },
                LiveEntry::Assistant {
                    id: "item-r1".to_string(),
                    text: String::new(),
                    reasoning: "thinking".to_string(),
                },
                LiveEntry::Assistant {
                    id: "item-m1".to_string(),
                    text: "hello world".to_string(),
                    reasoning: String::new(),
                },
            ]
        );
        let mut received = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            received.push(ev);
        }
        assert!(received.iter().any(|ev| matches!(
            &ev.kind,
            RegistryEventKind::TextDelta { delta, .. } if delta == "hello "
        )));
        assert!(
            received
                .iter()
                .any(|ev| matches!(&ev.kind, RegistryEventKind::TurnCompleted { .. }))
        );
    }
    #[tokio::test]
    async fn discarding_streaming_output_clears_the_buffer_and_notifies_clients() {
        let registry = SessionRegistry::new();
        let worker = open_test_session(&registry, "ses-discard", None, Vec::new());
        let SubscribeResult::Replay { mut rx, .. } = registry.subscribe(None) else {
            panic!("expected Replay");
        };
        let turn_id = TurnId::from_raw("turn-1");
        let message_id = ItemId::from_raw("item-m1");
        worker
            .event_tx
            .send(AgentEvent::TurnStarted {
                turn_id,
                model: "openai::m".to_string(),
            })
            .unwrap();
        worker
            .event_tx
            .send(AgentEvent::ItemStarted {
                item: SessionItem::AgentMessage {
                    id: message_id.clone(),
                    text: String::new(),
                },
            })
            .unwrap();
        worker
            .event_tx
            .send(AgentEvent::ItemDelta {
                item_id: message_id.clone(),
                delta: ItemDelta::AgentText("partial".to_string()),
            })
            .unwrap();
        wait_until(|| {
            registry
                .session_detail(&worker.session_id)
                .is_some_and(|detail| detail.streaming_text == "partial")
        })
        .await;
        worker
            .event_tx
            .send(AgentEvent::ItemDiscarded {
                item_id: message_id,
            })
            .unwrap();
        wait_until(|| {
            registry
                .session_detail(&worker.session_id)
                .is_some_and(|detail| {
                    detail.streaming_item_id.is_none() && detail.streaming_text.is_empty()
                })
        })
        .await;
        let mut received = Vec::new();
        while let Ok(event) = rx.try_recv() {
            received.push(event);
        }
        assert!(
            received
                .iter()
                .any(|event| matches!(&event.kind, RegistryEventKind::StreamDiscarded { .. }))
        );
        let snapshot = registry.snapshot();
        let active = snapshot.active_session.unwrap();
        assert!(
            !active
                .entries
                .iter()
                .any(|entry| matches!(entry, LiveEntry::Assistant { .. }))
        );
    }
    #[tokio::test]
    async fn rejects_empty_missing_closed_and_busy_including_recovery_pending() {
        let registry = SessionRegistry::new();
        let worker = open_test_session(&registry, "ses-guards", Some("Guards"), Vec::new());
        assert_eq!(
            registry.submit_prompt(&worker.session_id, "   \n ".to_string(), None),
            Err(HostError::EmptyPrompt)
        );
        assert_eq!(
            registry.submit_prompt(&SessionId::from_raw("missing"), "hi".to_string(), None),
            Err(HostError::SessionNotFound(SessionId::from_raw("missing")))
        );
        let turn_id = TurnId::from_raw("turn-busy");
        worker
            .event_tx
            .send(AgentEvent::TurnStarted {
                turn_id: turn_id.clone(),
                model: "gemini-3.8-flash".to_string(),
            })
            .unwrap();
        wait_until(|| registry.snapshot().sessions[0].status == SessionStatus::Running).await;
        assert_eq!(
            registry.submit_prompt(&worker.session_id, "second".to_string(), None),
            Err(HostError::Busy)
        );
        let approval_id = RequestId::from_raw("apr-1");
        worker
            .event_tx
            .send(AgentEvent::ApprovalRequested {
                request: ApprovalRequest {
                    request_id: approval_id.clone(),
                    turn_id: turn_id.clone(),
                    kind: ApprovalKind::ToolCall,
                    preview: ApprovalPreview {
                        title: "ls".to_string(),
                        detail: "detail".to_string(),
                    },
                    effect: None,
                },
            })
            .unwrap();
        wait_until(|| registry.snapshot().sessions[0].status == SessionStatus::WaitingApproval)
            .await;
        assert_eq!(
            registry.submit_prompt(&worker.session_id, "blocked".to_string(), None),
            Err(HostError::Busy)
        );
        worker
            .event_tx
            .send(AgentEvent::ApprovalResolved {
                request_id: approval_id,
            })
            .unwrap();
        worker
            .event_tx
            .send(AgentEvent::TurnCompleted {
                turn_id: turn_id.clone(),
                reason: TurnEndReason::Failed {
                    error: "transient".to_string(),
                },
            })
            .unwrap();
        worker
            .event_tx
            .send(AgentEvent::RecoveryStarted { attempt: 1 })
            .unwrap();
        wait_until(|| {
            registry
                .session_detail(&worker.session_id)
                .is_some_and(|d| {
                    d.entries
                        .iter()
                        .any(|e| matches!(e, LiveEntry::System { .. }))
                })
        })
        .await;
        assert_eq!(
            registry.submit_prompt(&worker.session_id, "during recovery".to_string(), None),
            Err(HostError::Busy)
        );
        let turn2 = TurnId::from_raw("turn-recover");
        worker
            .event_tx
            .send(AgentEvent::TurnStarted {
                turn_id: turn2.clone(),
                model: "gemini-3.8-flash".to_string(),
            })
            .unwrap();
        worker
            .event_tx
            .send(AgentEvent::TurnCompleted {
                turn_id: turn2,
                reason: TurnEndReason::Completed,
            })
            .unwrap();
        wait_until(|| {
            registry
                .session_detail(&worker.session_id)
                .is_some_and(|d| {
                    d.summary.status == SessionStatus::Idle && d.metrics.turn_count >= 2
                })
        })
        .await;
        drop(worker.cmd_rx);
        assert_eq!(
            registry.submit_prompt(&worker.session_id, "after close".to_string(), None),
            Err(HostError::WorkerClosed)
        );
    }
    #[tokio::test]
    async fn approval_and_question_reject_stale_requests_and_update_entries() {
        let registry = SessionRegistry::new();
        let mut worker = open_test_session(&registry, "ses-req", Some("Req"), Vec::new());
        let turn_id = TurnId::from_raw("turn-req");
        worker
            .event_tx
            .send(AgentEvent::TurnStarted {
                turn_id: turn_id.clone(),
                model: "gemini-3.8-flash".to_string(),
            })
            .unwrap();
        let req_id = RequestId::from_raw("apr-live");
        worker
            .event_tx
            .send(AgentEvent::ApprovalRequested {
                request: ApprovalRequest {
                    request_id: req_id.clone(),
                    turn_id: turn_id.clone(),
                    kind: ApprovalKind::ToolCall,
                    preview: ApprovalPreview {
                        title: "ls".to_string(),
                        detail: "detail".to_string(),
                    },
                    effect: None,
                },
            })
            .unwrap();
        wait_until(|| {
            registry
                .session_detail(&worker.session_id)
                .is_some_and(|d| d.pending_approvals.len() == 1)
        })
        .await;
        assert_eq!(
            registry.decide_approval(
                &worker.session_id,
                RequestId::from_raw("apr-stale"),
                ApprovalDecision::Accept,
                Some(GrantScope::Turn),
            ),
            Err(HostError::StaleRequest(RequestId::from_raw("apr-stale")))
        );
        assert_eq!(
            registry.decide_approval(
                &worker.session_id,
                req_id.clone(),
                ApprovalDecision::AcceptForSession,
                Some(GrantScope::Session),
            ),
            Ok(())
        );
        assert_eq!(
            worker.cmd_rx.recv().await,
            Some(AgentCommand::Approve {
                request_id: req_id,
                decision: ApprovalDecision::AcceptForSession,
                scope: Some(GrantScope::Session),
            })
        );
    }
    #[tokio::test]
    async fn subscribe_replays_in_range_and_signals_resync_on_overflow() {
        let registry = SessionRegistry::new();
        let worker = open_test_session(&registry, "ses-ring", None, Vec::new());
        let item_id = ItemId::from_raw("item-stream");
        for _ in 0..10 {
            worker
                .event_tx
                .send(AgentEvent::ItemDelta {
                    item_id: item_id.clone(),
                    delta: ItemDelta::AgentText("a".to_string()),
                })
                .unwrap();
        }
        wait_until(|| registry.snapshot().event_id >= 11).await;
        let SubscribeResult::Replay { events, .. } = registry.subscribe(Some(5)) else {
            panic!("expected Replay for in-range seq");
        };
        assert_eq!(events.first().unwrap().seq, 6);
        assert_eq!(events.last().unwrap().seq, 11);
        let SubscribeResult::Replay { events: empty, .. } = registry.subscribe(Some(11)) else {
            panic!("expected empty Replay at head");
        };
        assert!(empty.is_empty());
        for _ in 0..(CAPACITY + 50) {
            worker
                .event_tx
                .send(AgentEvent::ItemDelta {
                    item_id: item_id.clone(),
                    delta: ItemDelta::AgentText("x".to_string()),
                })
                .unwrap();
        }
        let target_seq = 11 + CAPACITY as u64 + 50;
        wait_until(|| registry.snapshot().event_id >= target_seq).await;
        match registry.subscribe(Some(1)) {
            SubscribeResult::ResyncRequired { current_seq, .. } => {
                assert!(current_seq >= target_seq);
            }
            SubscribeResult::Replay { .. } => panic!("expected ResyncRequired after overflow"),
        }
        let oldest_kept_prev = target_seq - CAPACITY as u64;
        match registry.subscribe(Some(oldest_kept_prev)) {
            SubscribeResult::Replay { events, .. } => {
                assert_eq!(events.len(), CAPACITY);
                assert_eq!(events.first().unwrap().seq, oldest_kept_prev + 1);
                assert_eq!(events.last().unwrap().seq, target_seq);
            }
            SubscribeResult::ResyncRequired { .. } => {
                panic!("expected Replay at exact oldest_seq - 1 boundary")
            }
        }
    }
    #[tokio::test]
    async fn shutdown_all_emits_host_disconnected_and_cancels_workers() {
        let registry = SessionRegistry::new();
        let mut worker = open_test_session(&registry, "ses-shut", Some("Shut"), Vec::new());
        let SubscribeResult::Replay { mut rx, .. } = registry.subscribe(None) else {
            panic!("expected Replay");
        };
        let handler = tokio::spawn(async move {
            let mut cmds = Vec::new();
            while let Some(cmd) = worker.cmd_rx.recv().await {
                let quit = matches!(cmd, AgentCommand::Quit);
                cmds.push(cmd);
                if quit {
                    break;
                }
            }
            drop(worker.event_tx);
            cmds
        });
        registry.shutdown_all().await;
        let cmds = handler.await.unwrap();
        assert_eq!(cmds, vec![AgentCommand::Cancel, AgentCommand::Quit]);
        let mut disconnected = false;
        while let Ok(ev) = rx.try_recv() {
            if matches!(ev.kind, RegistryEventKind::HostDisconnected) {
                disconnected = true;
            }
        }
        assert!(disconnected);
    }
    #[tokio::test]
    async fn tracks_multiple_user_shell_runs() {
        let registry = SessionRegistry::new();
        let worker = open_test_session(&registry, "ses-shells", Some("Shells"), Vec::new());
        let first = registry
            .begin_user_shell(&worker.session_id, "sleep 1", Some("run-1".to_string()))
            .unwrap();
        let second = registry
            .begin_user_shell(&worker.session_id, "sleep 2", None)
            .unwrap();
        assert_ne!(first, second);
        let summary = registry.session_summary(&worker.session_id).unwrap();
        assert_eq!(summary.shell_runs.len(), 2);
        assert!(summary.user_shell_running);
        assert!(
            summary
                .shell_runs
                .iter()
                .any(|run| run.command == "sleep 2")
        );
        registry.finish_user_shell(&worker.session_id, &first, Ok("done".to_string()));
        let summary = registry.session_summary(&worker.session_id).unwrap();
        assert_eq!(summary.shell_runs.len(), 1);
        assert_eq!(summary.shell_runs[0].id, second);
        registry.finish_user_shell(&worker.session_id, &second, Err("boom".to_string()));
        let summary = registry.session_summary(&worker.session_id).unwrap();
        assert!(summary.shell_runs.is_empty());
        assert!(!summary.user_shell_running);
    }
}
