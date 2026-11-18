use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::diff::{FileChangeEntry, UnifiedLine, tool_edit_diff};
use crate::session::{
    Backend, HostInfoSnapshot, HostReloadOutcome, OpenedSession, ResolvedAgentApplication,
    ResolvedSkillApplication,
};

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
    pub last_turn_ms: Option<u64>,
    pub tokens_per_sec: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
        agent: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        report: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        child_session_id: Option<String>,
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegistrySnapshot {
    pub event_id: u64,
    pub active_session_id: Option<SessionId>,
    pub sessions: Vec<LiveSessionSummary>,
    pub active_session: Option<LiveSessionDetail>,
    pub file_changes: Vec<FileChangeEntry>,
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
        host_info: HostInfoSnapshot,
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

impl SessionRegistry {
    pub fn new() -> Arc<Self> {
        let (broadcast_tx, _) = broadcast::channel(CAPACITY);
        let (agent_broadcast_tx, _) = broadcast::channel(CAPACITY);
        Arc::new(Self {
            inner: Arc::new(RegistryInner {
                state: Mutex::new(RegistryState::new()),
                broadcast_tx,
                agent_broadcast_tx,
            }),
        })
    }

    pub fn subscribe_agent_events(&self) -> broadcast::Receiver<(SessionId, AgentEvent)> {
        self.inner.agent_broadcast_tx.subscribe()
    }

    pub fn register_opened(&self, opened: OpenedSession) -> LiveSessionSummary {
        self.register_opened_with_activation(opened, false)
    }

    pub fn register_opened_active(&self, opened: OpenedSession) -> LiveSessionSummary {
        self.register_opened_with_activation(opened, true)
    }

    fn register_opened_with_activation(
        &self,
        opened: OpenedSession,
        activate: bool,
    ) -> LiveSessionSummary {
        let replay = replay_from_events(&opened.events);
        let turn_ids = collect_user_turn_ids(&opened.events);
        let mut turn_iter = turn_ids.into_iter();
        let cumulative = session_usage_from_events(&opened.events);
        let metrics = cumulative_metrics_from_usage(cumulative.as_ref());

        let session_id = opened.session_id;
        let summary = LiveSessionSummary {
            session_id: session_id.clone(),
            title: opened.title,
            backend: opened.backend,
            provider: opened.provider,
            model: opened.model,
            approval_mode: opened.approval_mode,
            status: SessionStatus::Idle,
            active_turn_id: None,
            active_tool: None,
            turn_started_at_ms: None,
            user_shell_running: false,
        };

        let mut state = self.inner.state.lock().unwrap();
        let entries = replay
            .into_iter()
            .map(|entry| replay_entry_to_live(entry, &mut turn_iter, &mut state))
            .collect();

        let record = SessionRecord {
            summary: summary.clone(),
            entries,
            streaming_item_id: None,
            streaming_text: String::new(),
            streaming_reasoning: String::new(),
            cmd_tx: opened.cmd_tx,
            pending_approvals: VecDeque::new(),
            pending_questions: VecDeque::new(),
            usage: UsageStatsSnapshot::default(),
            metrics,
            turn_started_instant: None,
            tool_items: HashMap::new(),
            recovery_pending: false,
        };

        if !state.sessions.contains_key(&session_id) {
            state.session_order.push(session_id.clone());
        }
        state.sessions.insert(session_id.clone(), record);
        let became_active = if activate || state.active_session_id.is_none() {
            let changed = state.active_session_id.as_ref() != Some(&session_id);
            state.active_session_id = Some(session_id.clone());
            changed
        } else {
            false
        };
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::SessionOpened {
                session: summary.clone(),
            },
        );
        if became_active && activate {
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::SessionActivated {
                    session_id: session_id.clone(),
                },
            );
        }

        let inner = Arc::clone(&self.inner);
        let event_rx = opened.event_rx;
        let spawned_id = session_id.clone();
        let handle = tokio::spawn(async move {
            run_pump(inner, spawned_id, event_rx).await;
        });
        state.pump_handles.insert(session_id, handle);

        summary
    }

    pub fn snapshot(&self) -> RegistrySnapshot {
        let state = self.inner.state.lock().unwrap();
        let sessions = state
            .session_order
            .iter()
            .filter_map(|id| state.sessions.get(id).map(|r| r.summary.clone()))
            .collect();
        let active_session = state
            .active_session_id
            .as_ref()
            .and_then(|id| state.sessions.get(id).map(SessionRecord::to_detail));
        RegistrySnapshot {
            event_id: state.current_seq,
            active_session_id: state.active_session_id.clone(),
            sessions,
            active_session,
            file_changes: state.file_changes.clone(),
        }
    }

    pub fn has_session(&self, id: &SessionId) -> bool {
        let state = self.inner.state.lock().unwrap();
        state.sessions.contains_key(id)
    }

    pub fn session_detail(&self, id: &SessionId) -> Option<LiveSessionDetail> {
        let state = self.inner.state.lock().unwrap();
        state.sessions.get(id).map(SessionRecord::to_detail)
    }

    pub fn session_summary(&self, id: &SessionId) -> Option<LiveSessionSummary> {
        let state = self.inner.state.lock().unwrap();
        state.sessions.get(id).map(|r| r.summary.clone())
    }

    pub fn cmd_tx(&self, id: &SessionId) -> Option<mpsc::UnboundedSender<AgentCommand>> {
        let state = self.inner.state.lock().unwrap();
        state.sessions.get(id).map(|r| r.cmd_tx.clone())
    }

    pub fn note_tui_user_prompt(&self, session_id: &SessionId, prompt: &str, steer: bool) {
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return;
        }
        let mut state = self.inner.state.lock().unwrap();
        if !state.sessions.contains_key(session_id) {
            return;
        }
        let entry_id = state.next_entry_id();
        let summary = {
            let record = state.sessions.get_mut(session_id).unwrap();
            if record.summary.title.is_none() {
                record.summary.title = normalize_title(&prompt);
            }
            if !steer {
                record.summary.status = SessionStatus::Running;
                record.summary.turn_started_at_ms = Some(now_millis());
                record.turn_started_instant = Some(Instant::now());
            }
            record.entries.push(LiveEntry::User {
                id: entry_id.clone(),
                turn_id: if steer {
                    record.summary.active_turn_id.clone()
                } else {
                    None
                },
                text: prompt.clone(),
            });
            record.summary.clone()
        };
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::UserPromptSubmitted {
                session_id: session_id.clone(),
                entry_id,
                text: prompt,
            },
        );
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::SessionUpdated { session: summary },
        );
    }

    pub fn note_tui_mode(&self, session_id: &SessionId, mode: ApprovalMode) {
        let mut state = self.inner.state.lock().unwrap();
        let Some(record) = state.sessions.get_mut(session_id) else {
            return;
        };
        if record.summary.approval_mode == mode {
            return;
        }
        record.summary.approval_mode = mode;
        let session = record.summary.clone();
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::SessionUpdated { session },
        );
    }

    pub fn note_tui_model(&self, session_id: &SessionId, provider: &str, model: &str) {
        let mut state = self.inner.state.lock().unwrap();
        let Some(record) = state.sessions.get_mut(session_id) else {
            return;
        };
        record.summary.provider = provider.to_string();
        record.summary.model = display_model(model);
        let session = record.summary.clone();
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::SessionUpdated { session },
        );
    }

    pub fn note_tui_reset(&self, session_id: &SessionId) {
        let mut state = self.inner.state.lock().unwrap();
        let Some(record) = state.sessions.get_mut(session_id) else {
            return;
        };
        record.entries.clear();
        record.streaming_item_id = None;
        record.streaming_text.clear();
        record.streaming_reasoning.clear();
        record.pending_approvals.clear();
        record.pending_questions.clear();
        record.usage = UsageStatsSnapshot::default();
        record.metrics = TurnMetricsSnapshot::default();
        record.summary.active_turn_id = None;
        record.summary.active_tool = None;
        record.summary.turn_started_at_ms = None;
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::SessionReset {
                session_id: session_id.clone(),
            },
        );
    }

    pub fn set_active_session(&self, session_id: &SessionId) -> Result<(), HostError> {
        let mut state = self.inner.state.lock().unwrap();
        if !state.sessions.contains_key(session_id) {
            return Err(HostError::SessionNotFound(session_id.clone()));
        }
        if state.active_session_id.as_ref() != Some(session_id) {
            state.active_session_id = Some(session_id.clone());
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::SessionActivated {
                    session_id: session_id.clone(),
                },
            );
        }
        Ok(())
    }

    pub async fn close_and_wait_session(&self, session_id: &SessionId) -> Result<(), HostError> {
        let handle = {
            let mut state = self.inner.state.lock().unwrap();
            let Some(record) = state.sessions.remove(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            state.session_order.retain(|id| id != session_id);
            let next_active = if state.active_session_id.as_ref() == Some(session_id) {
                state.active_session_id = state.session_order.first().cloned();
                state.active_session_id.clone()
            } else {
                state.active_session_id.clone()
            };
            let _ = record.cmd_tx.send(AgentCommand::Cancel);
            let _ = record.cmd_tx.send(AgentCommand::Quit);
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::SessionClosed {
                    session_id: session_id.clone(),
                    next_active_session_id: next_active,
                },
            );
            state.pump_handles.remove(session_id)
        };
        if let Some(handle) = handle {
            let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, handle).await;
        }
        Ok(())
    }

    pub fn subscribe(&self, after_seq: Option<u64>) -> SubscribeResult {
        let state = self.inner.state.lock().unwrap();
        let rx = self.inner.broadcast_tx.subscribe();
        let Some(after) = after_seq else {
            return SubscribeResult::Replay {
                events: Vec::new(),
                rx,
            };
        };
        if after >= state.current_seq {
            return SubscribeResult::Replay {
                events: Vec::new(),
                rx,
            };
        }
        let Some(oldest) = state.ring.front() else {
            return SubscribeResult::ResyncRequired {
                current_seq: state.current_seq,
                rx,
            };
        };
        if after < oldest.seq.saturating_sub(1) {
            return SubscribeResult::ResyncRequired {
                current_seq: state.current_seq,
                rx,
            };
        }
        let events = state
            .ring
            .iter()
            .filter(|event| event.seq > after)
            .cloned()
            .collect();
        SubscribeResult::Replay { events, rx }
    }

    pub fn submit_prompt(&self, session_id: &SessionId, prompt: String) -> Result<(), HostError> {
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return Err(HostError::EmptyPrompt);
        }

        let cmd_tx = {
            let mut state = self.inner.state.lock().unwrap();
            let Some(record) = state.sessions.get(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            if record.summary.status != SessionStatus::Idle || record.recovery_pending {
                return Err(HostError::Busy);
            }
            if record.cmd_tx.is_closed() {
                return Err(HostError::WorkerClosed);
            }

            let entry_id = state.next_entry_id();
            let (summary, cmd_tx) = {
                let record = state.sessions.get_mut(session_id).unwrap();
                record.summary.status = SessionStatus::Running;
                record.summary.turn_started_at_ms = Some(now_millis());
                record.turn_started_instant = Some(Instant::now());
                if record.summary.title.is_none() {
                    record.summary.title = normalize_title(&prompt);
                }
                record.entries.push(LiveEntry::User {
                    id: entry_id.clone(),
                    turn_id: None,
                    text: prompt.clone(),
                });
                (record.summary.clone(), record.cmd_tx.clone())
            };

            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::UserPromptSubmitted {
                    session_id: session_id.clone(),
                    entry_id,
                    text: prompt.clone(),
                },
            );
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::SessionUpdated { session: summary },
            );
            cmd_tx
        };

        cmd_tx
            .send(AgentCommand::SubmitPrompt(prompt))
            .map_err(|_| HostError::WorkerClosed)
    }

    pub fn steer_prompt(&self, session_id: &SessionId, prompt: String) -> Result<(), HostError> {
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return Err(HostError::EmptyPrompt);
        }

        let cmd_tx = {
            let mut state = self.inner.state.lock().unwrap();
            let Some(record) = state.sessions.get(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            if record.is_acp() {
                return Err(HostError::UnsupportedBackend(
                    "steering mid-turn is not supported for ACP sessions".to_string(),
                ));
            }
            if record.summary.status == SessionStatus::Idle {
                return Err(HostError::NotRunning);
            }
            if record.cmd_tx.is_closed() {
                return Err(HostError::WorkerClosed);
            }
            let entry_id = state.next_entry_id();
            let record = state.sessions.get_mut(session_id).unwrap();
            record.entries.push(LiveEntry::User {
                id: entry_id.clone(),
                turn_id: record.summary.active_turn_id.clone(),
                text: prompt.clone(),
            });
            let cmd_tx = record.cmd_tx.clone();
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::UserPromptSubmitted {
                    session_id: session_id.clone(),
                    entry_id,
                    text: prompt.clone(),
                },
            );
            cmd_tx
        };

        cmd_tx
            .send(AgentCommand::Steer(prompt))
            .map_err(|_| HostError::WorkerClosed)
    }

    pub fn cancel_turn(&self, session_id: &SessionId) -> Result<(), HostError> {
        let state = self.inner.state.lock().unwrap();
        let Some(record) = state.sessions.get(session_id) else {
            return Err(HostError::SessionNotFound(session_id.clone()));
        };
        if record.summary.status == SessionStatus::Idle {
            return Err(HostError::NotRunning);
        }
        record
            .cmd_tx
            .send(AgentCommand::Cancel)
            .map_err(|_| HostError::WorkerClosed)
    }

    pub fn compact(&self, session_id: &SessionId) -> Result<(), HostError> {
        let cmd_tx = {
            let state = self.inner.state.lock().unwrap();
            let Some(record) = state.sessions.get(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            if record.is_acp() {
                return Err(HostError::UnsupportedBackend(
                    "compaction is not supported for ACP sessions".to_string(),
                ));
            }
            if record.summary.status != SessionStatus::Idle {
                return Err(HostError::Busy);
            }
            record.cmd_tx.clone()
        };
        cmd_tx
            .send(AgentCommand::Compact)
            .map_err(|_| HostError::WorkerClosed)
    }

    pub fn reset(&self, session_id: &SessionId) -> Result<(), HostError> {
        let cmd_tx = {
            let mut state = self.inner.state.lock().unwrap();
            let Some(record) = state.sessions.get(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            if record.is_acp() {
                return Err(HostError::UnsupportedBackend(
                    "reset is not supported for ACP sessions".to_string(),
                ));
            }
            if record.summary.status != SessionStatus::Idle {
                return Err(HostError::Busy);
            }
            if record.cmd_tx.is_closed() {
                return Err(HostError::WorkerClosed);
            }
            let record = state.sessions.get_mut(session_id).unwrap();
            record.entries.clear();
            record.streaming_item_id = None;
            record.streaming_text.clear();
            record.streaming_reasoning.clear();
            record.pending_approvals.clear();
            record.pending_questions.clear();
            record.usage = UsageStatsSnapshot::default();
            record.metrics = TurnMetricsSnapshot::default();
            record.summary.active_turn_id = None;
            record.summary.active_tool = None;
            record.summary.turn_started_at_ms = None;
            let cmd_tx = record.cmd_tx.clone();
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::SessionReset {
                    session_id: session_id.clone(),
                },
            );
            cmd_tx
        };
        cmd_tx
            .send(AgentCommand::Reset)
            .map_err(|_| HostError::WorkerClosed)
    }

    pub fn rewind(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        scope: RewindScope,
    ) -> Result<Option<String>, HostError> {
        let (cmd_tx, restored_prompt) = {
            let state = self.inner.state.lock().unwrap();
            let Some(record) = state.sessions.get(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            if record.is_acp() {
                return Err(HostError::UnsupportedBackend(
                    "rewind is not supported for ACP sessions".to_string(),
                ));
            }
            if record.summary.status != SessionStatus::Idle {
                return Err(HostError::Busy);
            }
            let prompt = if matches!(scope, RewindScope::Both | RewindScope::Conversation) {
                record.entries.iter().find_map(|entry| match entry {
                    LiveEntry::User {
                        turn_id: Some(tid),
                        text,
                        ..
                    } if tid == &turn_id => Some(text.clone()),
                    _ => None,
                })
            } else {
                None
            };
            (record.cmd_tx.clone(), prompt)
        };
        cmd_tx
            .send(AgentCommand::Rewind { turn_id, scope })
            .map_err(|_| HostError::WorkerClosed)?;
        Ok(restored_prompt)
    }

    pub fn decide_approval(
        &self,
        session_id: &SessionId,
        request_id: RequestId,
        decision: ApprovalDecision,
        scope: Option<GrantScope>,
    ) -> Result<(), HostError> {
        let cmd_tx = {
            let mut state = self.inner.state.lock().unwrap();
            let Some(record) = state.sessions.get_mut(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            let Some(pos) = record
                .pending_approvals
                .iter()
                .position(|req| req.request_id == request_id)
            else {
                return Err(HostError::StaleRequest(request_id));
            };
            if record.cmd_tx.is_closed() {
                return Err(HostError::WorkerClosed);
            }
            record.pending_approvals.remove(pos);
            let prev = record.summary.clone();
            record.recompute_status();
            let updated = (record.summary != prev).then(|| record.summary.clone());
            let cmd_tx = record.cmd_tx.clone();
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::ApprovalResolved {
                    session_id: session_id.clone(),
                    request_id: request_id.clone(),
                },
            );
            if let Some(session) = updated {
                state.push_event(
                    &self.inner.broadcast_tx,
                    RegistryEventKind::SessionUpdated { session },
                );
            }
            cmd_tx
        };
        cmd_tx
            .send(AgentCommand::Approve {
                request_id,
                decision,
                scope,
            })
            .map_err(|_| HostError::WorkerClosed)
    }

    pub fn answer_question(
        &self,
        session_id: &SessionId,
        request_id: RequestId,
        answers: Option<Vec<QuestionAnswer>>,
    ) -> Result<(), HostError> {
        let cmd_tx =
            {
                let mut state = self.inner.state.lock().unwrap();
                let Some(record) = state.sessions.get_mut(session_id) else {
                    return Err(HostError::SessionNotFound(session_id.clone()));
                };
                let Some(pos) = record
                    .pending_questions
                    .iter()
                    .position(|req| req.request_id == request_id)
                else {
                    return Err(HostError::StaleRequest(request_id));
                };
                if record.cmd_tx.is_closed() {
                    return Err(HostError::WorkerClosed);
                }
                record.pending_questions.remove(pos);
                let req_id_str = request_id.to_string();
                let mut updated_entry = None;
                if let Some(LiveEntry::Question {
                    answers: slot,
                    resolved,
                    ..
                }) = record.entries.iter_mut().rev().find(
                    |entry| matches!(entry, LiveEntry::Question { id, .. } if *id == req_id_str),
                ) {
                    *slot = answers.clone();
                    *resolved = true;
                }
                if let Some(entry) = record.entries.iter().rev().find(
                    |entry| matches!(entry, LiveEntry::Question { id, .. } if *id == req_id_str),
                ) {
                    updated_entry = Some(entry.clone());
                }
                let prev = record.summary.clone();
                record.recompute_status();
                let updated_summary = (record.summary != prev).then(|| record.summary.clone());
                let cmd_tx = record.cmd_tx.clone();
                if let Some(entry) = updated_entry {
                    state.push_event(
                        &self.inner.broadcast_tx,
                        RegistryEventKind::EntryUpdated {
                            session_id: session_id.clone(),
                            entry,
                        },
                    );
                }
                state.push_event(
                    &self.inner.broadcast_tx,
                    RegistryEventKind::QuestionResolved {
                        session_id: session_id.clone(),
                        request_id: request_id.clone(),
                    },
                );
                if let Some(session) = updated_summary {
                    state.push_event(
                        &self.inner.broadcast_tx,
                        RegistryEventKind::SessionUpdated { session },
                    );
                }
                cmd_tx
            };
        cmd_tx
            .send(AgentCommand::AnswerQuestion {
                request_id,
                answers,
            })
            .map_err(|_| HostError::WorkerClosed)
    }

    pub fn set_model(
        &self,
        session_id: &SessionId,
        provider: String,
        display_model: String,
        command_model: String,
        context_window_tokens: Option<u64>,
    ) -> Result<(), HostError> {
        let cmd_tx = {
            let mut state = self.inner.state.lock().unwrap();
            let Some(record) = state.sessions.get_mut(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            if record.is_acp() {
                return Err(HostError::UnsupportedBackend(
                    "changing models is not supported for ACP sessions".to_string(),
                ));
            }
            if record.summary.status != SessionStatus::Idle {
                return Err(HostError::Busy);
            }
            record.summary.provider = provider;
            record.summary.model = display_model.clone();
            let summary = record.summary.clone();
            let cmd_tx = record.cmd_tx.clone();
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::SessionUpdated { session: summary },
            );
            cmd_tx
        };
        cmd_tx
            .send(AgentCommand::SetModel {
                model: command_model,
                context_window_tokens,
            })
            .map_err(|_| HostError::WorkerClosed)
    }

    pub fn set_approval_mode(
        &self,
        session_id: &SessionId,
        mode: ApprovalMode,
    ) -> Result<(), HostError> {
        let cmd_tx = {
            let mut state = self.inner.state.lock().unwrap();
            let Some(record) = state.sessions.get_mut(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            if record.is_acp() {
                return Err(HostError::UnsupportedBackend(
                    "changing approval mode is not supported for ACP sessions".to_string(),
                ));
            }
            record.summary.approval_mode = mode;
            let summary = record.summary.clone();
            let cmd_tx = record.cmd_tx.clone();
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::SessionUpdated { session: summary },
            );
            cmd_tx
        };
        cmd_tx
            .send(AgentCommand::SetPermissionMode(mode))
            .map_err(|_| HostError::WorkerClosed)
    }

    pub fn apply_agent(
        &self,
        session_id: &SessionId,
        app: ResolvedAgentApplication,
    ) -> Result<(), HostError> {
        let (cmd_tx, notice) = {
            let mut state = self.inner.state.lock().unwrap();
            let Some(record) = state.sessions.get(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            if record.is_acp() {
                return Err(HostError::UnsupportedBackend(
                    "agents are not supported for ACP sessions".to_string(),
                ));
            }
            if record.summary.status != SessionStatus::Idle {
                return Err(HostError::Busy);
            }
            let entry_id = state.next_entry_id();
            let notice = match &app.model {
                Some(model) => format!("Applied agent @{} (model: {model})", app.name),
                None => format!("Applied agent @{}", app.name),
            };
            let entry = LiveEntry::System {
                id: entry_id,
                text: notice.clone(),
            };
            let record = state.sessions.get_mut(session_id).unwrap();
            if let Some(model) = &app.model {
                record.summary.model = display_model(model);
            }
            record.entries.push(entry.clone());
            let summary = record.summary.clone();
            let cmd_tx = record.cmd_tx.clone();
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::EntryCompleted {
                    session_id: session_id.clone(),
                    entry,
                },
            );
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::SessionUpdated { session: summary },
            );
            (cmd_tx, notice)
        };
        let _ = notice;
        cmd_tx
            .send(AgentCommand::ApplyAgent {
                name: app.name,
                instructions: app.instructions,
                model: app.model,
                context_window_tokens: app.context_window_tokens,
            })
            .map_err(|_| HostError::WorkerClosed)
    }

    pub fn apply_skill(
        &self,
        session_id: &SessionId,
        app: ResolvedSkillApplication,
    ) -> Result<(), HostError> {
        let cmd_tx = {
            let mut state = self.inner.state.lock().unwrap();
            let Some(record) = state.sessions.get(session_id) else {
                return Err(HostError::SessionNotFound(session_id.clone()));
            };
            if record.is_acp() {
                return Err(HostError::UnsupportedBackend(
                    "skills are not supported for ACP sessions".to_string(),
                ));
            }
            if record.summary.status != SessionStatus::Idle {
                return Err(HostError::Busy);
            }
            let entry_id = state.next_entry_id();
            let entry = LiveEntry::System {
                id: entry_id,
                text: format!("Loaded skill @{}", app.name),
            };
            let record = state.sessions.get_mut(session_id).unwrap();
            record.entries.push(entry.clone());
            let cmd_tx = record.cmd_tx.clone();
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::EntryCompleted {
                    session_id: session_id.clone(),
                    entry,
                },
            );
            cmd_tx
        };
        cmd_tx
            .send(AgentCommand::ApplySkill {
                name: app.name,
                instructions: app.instructions,
            })
            .map_err(|_| HostError::WorkerClosed)
    }

    pub fn begin_user_shell(&self, session_id: &SessionId, command: &str) -> Result<(), HostError> {
        let mut state = self.inner.state.lock().unwrap();
        let Some(record) = state.sessions.get(session_id) else {
            return Err(HostError::SessionNotFound(session_id.clone()));
        };
        if record.summary.approval_mode == ApprovalMode::Plan {
            return Err(HostError::InvalidInput(
                "shell commands are disabled in plan mode".to_string(),
            ));
        }
        if record.summary.user_shell_running {
            return Err(HostError::Busy);
        }
        let entry_id = state.next_entry_id();
        let entry = LiveEntry::User {
            id: entry_id,
            turn_id: None,
            text: format!("!{command}"),
        };
        let record = state.sessions.get_mut(session_id).unwrap();
        record.summary.user_shell_running = true;
        record.entries.push(entry.clone());
        let summary = record.summary.clone();
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::EntryCompleted {
                session_id: session_id.clone(),
                entry,
            },
        );
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::SessionUpdated { session: summary },
        );
        Ok(())
    }

    pub fn finish_user_shell(&self, session_id: &SessionId, result: Result<String, String>) {
        let mut state = self.inner.state.lock().unwrap();
        if !state.sessions.contains_key(session_id) {
            return;
        }
        let entry_id = state.next_entry_id();
        let entry = match result {
            Ok(output) => LiveEntry::System {
                id: entry_id,
                text: output,
            },
            Err(err) => LiveEntry::Error {
                id: entry_id,
                message: format!("shell command failed: {err}"),
            },
        };
        let record = state.sessions.get_mut(session_id).unwrap();
        record.summary.user_shell_running = false;
        record.entries.push(entry.clone());
        let summary = record.summary.clone();
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::EntryCompleted {
                session_id: session_id.clone(),
                entry,
            },
        );
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::SessionUpdated { session: summary },
        );
    }

    pub fn append_system_entry(&self, session_id: &SessionId, text: String) {
        let mut state = self.inner.state.lock().unwrap();
        if !state.sessions.contains_key(session_id) {
            return;
        }
        let entry = LiveEntry::System {
            id: state.next_entry_id(),
            text,
        };
        state
            .sessions
            .get_mut(session_id)
            .unwrap()
            .entries
            .push(entry.clone());
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::EntryCompleted {
                session_id: session_id.clone(),
                entry,
            },
        );
    }

    pub fn emit_reload(&self, outcome: &HostReloadOutcome) {
        let mut state = self.inner.state.lock().unwrap();
        for record in state.sessions.values() {
            if !record.is_acp() {
                let _ = record.cmd_tx.send(AgentCommand::Reload {
                    rules: outcome.payload.rules.clone(),
                    context: outcome.payload.context.clone(),
                    system_prompt: outcome.payload.system_prompt.clone(),
                });
                let _ = record
                    .cmd_tx
                    .send(AgentCommand::SetHooks(outcome.payload.hooks.clone()));
            }
        }
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::Reloaded {
                host_info: outcome.host_info.clone(),
                warnings: outcome.warnings.clone(),
            },
        );
    }

    pub fn emit_file_changes(&self, changes: Vec<FileChangeEntry>) {
        let mut state = self.inner.state.lock().unwrap();
        if state.file_changes == changes {
            return;
        }
        state.file_changes = changes.clone();
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::FileChangesUpdated { changes },
        );
    }

    pub fn emit_providers_updated(&self) {
        let mut state = self.inner.state.lock().unwrap();
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::ProvidersUpdated,
        );
    }

    pub fn emit_chatgpt_login(&self, status: ChatGptLoginStatus) {
        let mut state = self.inner.state.lock().unwrap();
        state.push_event(
            &self.inner.broadcast_tx,
            RegistryEventKind::ChatGptLoginUpdated(status),
        );
    }

    pub async fn shutdown_all(&self) {
        let handles = {
            let mut state = self.inner.state.lock().unwrap();
            state.push_event(
                &self.inner.broadcast_tx,
                RegistryEventKind::HostDisconnected,
            );
            for record in state.sessions.values() {
                let _ = record.cmd_tx.send(AgentCommand::Cancel);
                let _ = record.cmd_tx.send(AgentCommand::Quit);
            }
            std::mem::take(&mut state.pump_handles)
        };

        let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, async {
            for handle in handles.into_values() {
                let _ = handle.await;
            }
        })
        .await;
    }
}

async fn run_pump(
    inner: Arc<RegistryInner>,
    session_id: SessionId,
    mut event_rx: mpsc::UnboundedReceiver<AgentEvent>,
) {
    while let Some(event) = event_rx.recv().await {
        {
            let mut state = inner.state.lock().unwrap();
            apply_agent_event(&mut state, &inner.broadcast_tx, &session_id, event.clone());
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
        AgentEvent::TurnStarted { turn_id, model } => {
            let (summary, updated_user) = {
                let record = state.sessions.get_mut(session_id).unwrap();
                record.recovery_pending = false;
                record.pending_approvals.clear();
                record.pending_questions.clear();
                record.summary.status = SessionStatus::Running;
                record.summary.active_turn_id = Some(turn_id.clone());
                record.summary.active_tool = None;
                record.summary.turn_started_at_ms = Some(now_millis());
                record.turn_started_instant = Some(Instant::now());
                record.summary.model = display_model(&model);
                record.streaming_item_id = None;
                record.streaming_text.clear();
                record.streaming_reasoning.clear();
                let mut user_entry = None;
                for entry in record.entries.iter_mut().rev() {
                    if let LiveEntry::User { turn_id: slot, .. } = entry {
                        if slot.is_none() {
                            *slot = Some(turn_id.clone());
                            user_entry = Some(entry.clone());
                        }
                        break;
                    }
                }
                (record.summary.clone(), user_entry)
            };
            if let Some(entry) = updated_user {
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::EntryUpdated {
                        session_id: session_id.clone(),
                        entry,
                    },
                );
            }
            state.push_event(
                broadcast_tx,
                RegistryEventKind::TurnStarted {
                    session_id: session_id.clone(),
                    turn_id,
                    model,
                },
            );
            state.push_event(
                broadcast_tx,
                RegistryEventKind::SessionUpdated { session: summary },
            );
        }
        AgentEvent::RecoveryStarted { attempt } => {
            let entry_id = state.next_entry_id();
            let entry = LiveEntry::System {
                id: entry_id,
                text: format!("Recovery attempt {attempt} after a failed turn."),
            };
            let record = state.sessions.get_mut(session_id).unwrap();
            record.recovery_pending = true;
            record.entries.push(entry.clone());
            state.push_event(
                broadcast_tx,
                RegistryEventKind::EntryCompleted {
                    session_id: session_id.clone(),
                    entry,
                },
            );
        }
        AgentEvent::ItemStarted { item } => match item {
            SessionItem::AgentMessage { id, .. } => {
                let record = state.sessions.get_mut(session_id).unwrap();
                record.streaming_item_id = Some(id.to_string());
            }
            SessionItem::Reasoning { id, summary } => {
                let record = state.sessions.get_mut(session_id).unwrap();
                record.streaming_item_id = Some(id.to_string());
                if !summary.is_empty() {
                    record.streaming_reasoning.push_str(&summary);
                }
            }
            SessionItem::ToolCall {
                id,
                call_id,
                name,
                args,
                status,
                output,
                ..
            } => {
                state.flush_streaming(session_id, broadcast_tx);
                let entry_id = call_id.to_string();
                let edit_diff = tool_edit_diff(&name, &args, output.as_deref());
                let entry = LiveEntry::ToolCall {
                    id: entry_id.clone(),
                    name: name.clone(),
                    args,
                    status: item_status_str(status).to_string(),
                    output,
                    edit_diff,
                };
                let (updated_summary, entry) = {
                    let record = state.sessions.get_mut(session_id).unwrap();
                    record.tool_items.insert(id, entry_id);
                    record.metrics.tool_calls += 1;
                    record.summary.active_tool = Some(name);
                    record.entries.push(entry.clone());
                    (record.summary.clone(), entry)
                };
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::EntryCompleted {
                        session_id: session_id.clone(),
                        entry,
                    },
                );
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::SessionUpdated {
                        session: updated_summary,
                    },
                );
            }
            SessionItem::SubagentRun {
                id,
                agent,
                status,
                report,
                child_session_id,
            } => {
                state.flush_streaming(session_id, broadcast_tx);
                let entry_id = id.to_string();
                let entry = LiveEntry::Subagent {
                    id: entry_id.clone(),
                    agent: agent.clone(),
                    status: item_status_str(status).to_string(),
                    report,
                    child_session_id,
                };
                let (updated_summary, entry) = {
                    let record = state.sessions.get_mut(session_id).unwrap();
                    record.tool_items.insert(id, entry_id);
                    record.summary.active_tool = Some(agent);
                    record.entries.push(entry.clone());
                    (record.summary.clone(), entry)
                };
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::EntryCompleted {
                        session_id: session_id.clone(),
                        entry,
                    },
                );
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::SessionUpdated {
                        session: updated_summary,
                    },
                );
            }
            _ => {}
        },
        AgentEvent::ItemDelta { item_id, delta } => match delta {
            ItemDelta::AgentText(chunk) => {
                let id_str = item_id.to_string();
                let record = state.sessions.get_mut(session_id).unwrap();
                record.streaming_item_id = Some(id_str.clone());
                record.streaming_text.push_str(&chunk);
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::TextDelta {
                        session_id: session_id.clone(),
                        item_id: id_str,
                        delta: chunk,
                    },
                );
            }
            ItemDelta::ReasoningSummary(chunk) => {
                let id_str = item_id.to_string();
                let record = state.sessions.get_mut(session_id).unwrap();
                record.streaming_item_id = Some(id_str.clone());
                record.streaming_reasoning.push_str(&chunk);
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::ReasoningDelta {
                        session_id: session_id.clone(),
                        item_id: id_str,
                        delta: chunk,
                    },
                );
            }
            ItemDelta::StatusChanged(status) => {
                let updated = {
                    let record = state.sessions.get_mut(session_id).unwrap();
                    if let Some(entry_id) = record.tool_items.get(&item_id).cloned()
                        && let Some(entry) =
                            record.entries.iter_mut().rev().find(|entry| match entry {
                                LiveEntry::ToolCall { id, .. } | LiveEntry::Subagent { id, .. } => {
                                    *id == entry_id
                                }
                                _ => false,
                            })
                    {
                        let status_text = item_status_str(status).to_string();
                        match entry {
                            LiveEntry::ToolCall { status: slot, .. } => *slot = status_text,
                            LiveEntry::Subagent { status: slot, .. } => *slot = status_text,
                            _ => {}
                        }
                        Some(entry.clone())
                    } else {
                        None
                    }
                };
                if let Some(entry) = updated {
                    state.push_event(
                        broadcast_tx,
                        RegistryEventKind::EntryUpdated {
                            session_id: session_id.clone(),
                            entry,
                        },
                    );
                }
            }
            ItemDelta::CommandOutput(_) | ItemDelta::PlanUpdated(_) => {}
        },
        AgentEvent::ItemCompleted { item } => match item {
            SessionItem::AgentMessage { id, text } => {
                let id_str = id.to_string();
                let entry = {
                    let record = state.sessions.get_mut(session_id).unwrap();
                    let final_text = if !text.is_empty() {
                        text
                    } else {
                        std::mem::take(&mut record.streaming_text)
                    };
                    record.streaming_text.clear();
                    if record.streaming_item_id.as_deref() == Some(id_str.as_str()) {
                        record.streaming_item_id = None;
                    }
                    if final_text.is_empty() {
                        None
                    } else {
                        let entry = LiveEntry::Assistant {
                            id: id_str,
                            text: final_text,
                            reasoning: String::new(),
                        };
                        record.entries.push(entry.clone());
                        Some(entry)
                    }
                };
                if let Some(entry) = entry {
                    state.push_event(
                        broadcast_tx,
                        RegistryEventKind::EntryCompleted {
                            session_id: session_id.clone(),
                            entry,
                        },
                    );
                }
            }
            SessionItem::Reasoning { id, summary } => {
                let id_str = id.to_string();
                let entry = {
                    let record = state.sessions.get_mut(session_id).unwrap();
                    let final_reasoning = if !summary.is_empty() {
                        summary
                    } else {
                        std::mem::take(&mut record.streaming_reasoning)
                    };
                    record.streaming_reasoning.clear();
                    if record.streaming_item_id.as_deref() == Some(id_str.as_str()) {
                        record.streaming_item_id = None;
                    }
                    if final_reasoning.is_empty() {
                        None
                    } else {
                        let entry = LiveEntry::Assistant {
                            id: id_str,
                            text: String::new(),
                            reasoning: final_reasoning,
                        };
                        record.entries.push(entry.clone());
                        Some(entry)
                    }
                };
                if let Some(entry) = entry {
                    state.push_event(
                        broadcast_tx,
                        RegistryEventKind::EntryCompleted {
                            session_id: session_id.clone(),
                            entry,
                        },
                    );
                }
            }
            SessionItem::ToolCall {
                id,
                call_id,
                name,
                args,
                status,
                output,
                ..
            } => {
                let (entry, updated_summary) = {
                    let record = state.sessions.get_mut(session_id).unwrap();
                    let entry = update_or_push_tool_entry(
                        record,
                        &id,
                        call_id.to_string(),
                        name.clone(),
                        args,
                        completed_item_status_str(status),
                        output,
                    );
                    let updated_summary =
                        if record.summary.active_tool.as_deref() == Some(name.as_str()) {
                            record.summary.active_tool = None;
                            Some(record.summary.clone())
                        } else {
                            None
                        };
                    (entry, updated_summary)
                };
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::EntryCompleted {
                        session_id: session_id.clone(),
                        entry,
                    },
                );
                if let Some(session) = updated_summary {
                    state.push_event(broadcast_tx, RegistryEventKind::SessionUpdated { session });
                }
            }
            SessionItem::SubagentRun {
                id,
                agent,
                status,
                report,
                child_session_id,
            } => {
                let (entry, updated_summary) = {
                    let record = state.sessions.get_mut(session_id).unwrap();
                    let entry = update_or_push_subagent_entry(
                        record,
                        &id,
                        agent.clone(),
                        completed_item_status_str(status),
                        report,
                        child_session_id,
                    );
                    let updated_summary =
                        if record.summary.active_tool.as_deref() == Some(agent.as_str()) {
                            record.summary.active_tool = None;
                            Some(record.summary.clone())
                        } else {
                            None
                        };
                    (entry, updated_summary)
                };
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::EntryCompleted {
                        session_id: session_id.clone(),
                        entry,
                    },
                );
                if let Some(session) = updated_summary {
                    state.push_event(broadcast_tx, RegistryEventKind::SessionUpdated { session });
                }
            }
            SessionItem::Compaction {
                id, freed_tokens, ..
            } => {
                let entry = LiveEntry::System {
                    id: id.to_string(),
                    text: format!("Compaction freed {freed_tokens} tokens"),
                };
                state
                    .sessions
                    .get_mut(session_id)
                    .unwrap()
                    .entries
                    .push(entry.clone());
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::EntryCompleted {
                        session_id: session_id.clone(),
                        entry,
                    },
                );
            }
            _ => {}
        },
        AgentEvent::ApprovalRequested { request } => {
            let req_clone = request.clone();
            update_pending_status(state, broadcast_tx, session_id, |record| {
                if !record
                    .pending_approvals
                    .iter()
                    .any(|existing| existing.request_id == request.request_id)
                {
                    record.pending_approvals.push_back(request);
                }
            });
            state.push_event(
                broadcast_tx,
                RegistryEventKind::ApprovalRequested {
                    session_id: session_id.clone(),
                    request: req_clone,
                },
            );
        }
        AgentEvent::ApprovalResolved { request_id } => {
            update_pending_status(state, broadcast_tx, session_id, |record| {
                record
                    .pending_approvals
                    .retain(|req| req.request_id != request_id);
            });
            state.push_event(
                broadcast_tx,
                RegistryEventKind::ApprovalResolved {
                    session_id: session_id.clone(),
                    request_id,
                },
            );
        }
        AgentEvent::QuestionAsked { request } => {
            let req_clone = request.clone();
            let q_entry = LiveEntry::Question {
                id: request.request_id.to_string(),
                questions: request.questions.clone(),
                answers: None,
                resolved: false,
            };
            {
                let record = state.sessions.get_mut(session_id).unwrap();
                record.entries.push(q_entry.clone());
            }
            state.push_event(
                broadcast_tx,
                RegistryEventKind::EntryCompleted {
                    session_id: session_id.clone(),
                    entry: q_entry,
                },
            );
            update_pending_status(state, broadcast_tx, session_id, |record| {
                if !record
                    .pending_questions
                    .iter()
                    .any(|existing| existing.request_id == request.request_id)
                {
                    record.pending_questions.push_back(request);
                }
            });
            state.push_event(
                broadcast_tx,
                RegistryEventKind::QuestionAsked {
                    session_id: session_id.clone(),
                    request: req_clone,
                },
            );
        }
        AgentEvent::QuestionResolved { request_id } => {
            update_pending_status(state, broadcast_tx, session_id, |record| {
                record
                    .pending_questions
                    .retain(|req| req.request_id != request_id);
            });
            state.push_event(
                broadcast_tx,
                RegistryEventKind::QuestionResolved {
                    session_id: session_id.clone(),
                    request_id,
                },
            );
        }
        AgentEvent::HookCompleted {
            event,
            decision,
            reason,
        } => {
            let mut text = format!("hook {event}: {decision}");
            if let Some(reason) = reason {
                text.push_str(" — ");
                text.extend(reason.chars().take(200));
            }
            let entry = LiveEntry::System {
                id: state.next_entry_id(),
                text,
            };
            state
                .sessions
                .get_mut(session_id)
                .unwrap()
                .entries
                .push(entry.clone());
            state.push_event(
                broadcast_tx,
                RegistryEventKind::EntryCompleted {
                    session_id: session_id.clone(),
                    entry,
                },
            );
        }
        AgentEvent::UsageUpdated {
            usage,
            cumulative,
            est_cost_usd,
        } => {
            let (usage_snap, metrics_snap) = {
                let record = state.sessions.get_mut(session_id).unwrap();
                let session_prompt = cumulative.prompt_tokens.unwrap_or(0).max(0) as u64;
                let session_completion = cumulative.completion_tokens.unwrap_or(0).max(0) as u64;
                let session_total = cumulative
                    .total_tokens
                    .map(|t| t.max(0) as u64)
                    .unwrap_or(session_prompt + session_completion);
                record.metrics.session_prompt_tokens = session_prompt;
                record.metrics.session_completion_tokens = session_completion;
                record.metrics.session_total_tokens = session_total;
                record.metrics.session_cached_tokens =
                    cumulative.cached_tokens.unwrap_or(0).max(0) as u64;
                if let (Some(started), Some(tokens)) =
                    (record.turn_started_instant, usage.completion_tokens)
                {
                    let secs = started.elapsed().as_secs_f64();
                    if secs > 0.05 && tokens > 0 {
                        record.metrics.tokens_per_sec = Some(tokens as f64 / secs);
                    }
                }
                record.usage = UsageStatsSnapshot {
                    prompt_tokens: usage.prompt_tokens,
                    completion_tokens: usage.completion_tokens,
                    total_tokens: usage.total_tokens,
                    cached_tokens: usage.cached_tokens,
                    est_cost_usd,
                };
                (record.usage.clone(), record.metrics.clone())
            };
            state.push_event(
                broadcast_tx,
                RegistryEventKind::UsageUpdated {
                    session_id: session_id.clone(),
                    usage: usage_snap,
                    metrics: metrics_snap,
                },
            );
        }
        AgentEvent::RewindCompleted {
            turn_id,
            scope,
            restored,
            error,
        } => {
            let (entries_after, restored_prompt) = {
                let record = state.sessions.get_mut(session_id).unwrap();
                if error.is_none()
                    && matches!(scope, RewindScope::Both | RewindScope::Conversation)
                    && let Some(idx) = record.entries.iter().position(|entry| {
                        matches!(
                            entry,
                            LiveEntry::User { turn_id: Some(tid), .. } if tid == &turn_id
                        )
                    })
                {
                    let prompt = match &record.entries[idx] {
                        LiveEntry::User { text, .. } => Some(text.clone()),
                        _ => None,
                    };
                    record.entries.truncate(idx);
                    record.summary.active_turn_id = None;
                    record.summary.active_tool = None;
                    (Some(record.entries.clone()), prompt)
                } else {
                    (None, None)
                }
            };
            state.push_event(
                broadcast_tx,
                RegistryEventKind::SessionRewound {
                    session_id: session_id.clone(),
                    turn_id,
                    scope,
                    restored,
                    error,
                    entries: entries_after,
                    restored_prompt,
                },
            );
        }
        AgentEvent::TurnCompleted { turn_id, reason } => {
            state.flush_streaming(session_id, broadcast_tx);
            let cancelled_or_failed = match &reason {
                TurnEndReason::Aborted { .. } => ToolCallStatus::Cancelled,
                _ => ToolCallStatus::Failed,
            };
            let updated_subagents: Vec<LiveEntry> = {
                let record = state.sessions.get_mut(session_id).unwrap();
                finalize_open_entries(record, cancelled_or_failed)
            };
            for entry in updated_subagents {
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::EntryUpdated {
                        session_id: session_id.clone(),
                        entry,
                    },
                );
            }
            let terminal_entry = match &reason {
                TurnEndReason::Aborted { .. } => Some(LiveEntry::System {
                    id: state.next_entry_id(),
                    text: "Turn cancelled.".to_string(),
                }),
                TurnEndReason::Failed { error } => {
                    let already_error = matches!(
                        state
                            .sessions
                            .get(session_id)
                            .and_then(|r| r.entries.last()),
                        Some(LiveEntry::Error { .. })
                    );
                    if !already_error {
                        Some(LiveEntry::Error {
                            id: state.next_entry_id(),
                            message: format!("turn failed: {error}"),
                        })
                    } else {
                        None
                    }
                }
                _ => None,
            };
            if let Some(entry) = terminal_entry {
                state
                    .sessions
                    .get_mut(session_id)
                    .unwrap()
                    .entries
                    .push(entry.clone());
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::EntryCompleted {
                        session_id: session_id.clone(),
                        entry,
                    },
                );
            }
            let updated = {
                let record = state.sessions.get_mut(session_id).unwrap();
                let prev = record.summary.clone();
                record.metrics.turn_count += 1;
                if let Some(started) = record.turn_started_instant.take() {
                    record.metrics.last_turn_ms = Some(started.elapsed().as_millis() as u64);
                }
                record.pending_approvals.clear();
                record.pending_questions.clear();
                record.summary.status = SessionStatus::Idle;
                record.summary.active_turn_id = None;
                record.summary.active_tool = None;
                record.summary.turn_started_at_ms = None;
                if record.summary != prev {
                    Some(record.summary.clone())
                } else {
                    None
                }
            };
            state.push_event(
                broadcast_tx,
                RegistryEventKind::TurnCompleted {
                    session_id: session_id.clone(),
                    turn_id,
                    reason,
                },
            );
            if let Some(session) = updated {
                state.push_event(broadcast_tx, RegistryEventKind::SessionUpdated { session });
            }
        }
        AgentEvent::Error { code, message } => {
            state.flush_streaming(session_id, broadcast_tx);
            let updated_subagents = {
                let record = state.sessions.get_mut(session_id).unwrap();
                finalize_open_entries(record, ToolCallStatus::Failed)
            };
            for entry in updated_subagents {
                state.push_event(
                    broadcast_tx,
                    RegistryEventKind::EntryUpdated {
                        session_id: session_id.clone(),
                        entry,
                    },
                );
            }
            let entry_id = state.next_entry_id();
            let entry = LiveEntry::Error {
                id: entry_id,
                message: message.clone(),
            };
            let updated = {
                let record = state.sessions.get_mut(session_id).unwrap();
                let prev = record.summary.clone();
                if let Some(started) = record.turn_started_instant.take() {
                    record.metrics.last_turn_ms = Some(started.elapsed().as_millis() as u64);
                }
                record.pending_approvals.clear();
                record.pending_questions.clear();
                record.summary.status = SessionStatus::Idle;
                record.summary.active_turn_id = None;
                record.summary.active_tool = None;
                record.summary.turn_started_at_ms = None;
                record.entries.push(entry.clone());
                if record.summary != prev {
                    Some(record.summary.clone())
                } else {
                    None
                }
            };
            state.push_event(
                broadcast_tx,
                RegistryEventKind::EntryCompleted {
                    session_id: session_id.clone(),
                    entry,
                },
            );
            state.push_event(
                broadcast_tx,
                RegistryEventKind::Error {
                    session_id: session_id.clone(),
                    code,
                    message,
                },
            );
            if let Some(session) = updated {
                state.push_event(broadcast_tx, RegistryEventKind::SessionUpdated { session });
            }
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
        let record = state.sessions.get_mut(session_id).unwrap();
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
            agent: slot_agent,
            status: slot_status,
            report: slot_report,
            child_session_id: slot_child,
            ..
        } = existing
        {
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
        agent,
        status: status.to_string(),
        report,
        child_session_id,
    };
    record.entries.push(entry.clone());
    entry
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
            agent: sub.agent,
            status: replay_status_str(sub.status).to_string(),
            report: sub.report,
            child_session_id: sub.child_session_id,
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
            registry.submit_prompt(&worker.session_id, "  fix bug\nmore details  ".to_string()),
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
    async fn rejects_empty_missing_closed_and_busy_including_recovery_pending() {
        let registry = SessionRegistry::new();
        let worker = open_test_session(&registry, "ses-guards", Some("Guards"), Vec::new());

        assert_eq!(
            registry.submit_prompt(&worker.session_id, "   \n ".to_string()),
            Err(HostError::EmptyPrompt)
        );
        assert_eq!(
            registry.submit_prompt(&SessionId::from_raw("missing"), "hi".to_string()),
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
            registry.submit_prompt(&worker.session_id, "second".to_string()),
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
            registry.submit_prompt(&worker.session_id, "blocked".to_string()),
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
            registry.submit_prompt(&worker.session_id, "during recovery".to_string()),
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
            registry.submit_prompt(&worker.session_id, "after close".to_string()),
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
}
