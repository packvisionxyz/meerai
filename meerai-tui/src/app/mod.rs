mod commands;
mod events;
mod input;
mod modals;
mod palette;
mod reload;
mod runs;
#[cfg(test)]
mod tests;

pub(crate) use commands::{MentionKind, mention_is_known, parse_mention_token};
pub use events::{session_usage_from_events, transcript_from_events, user_turn_map};
pub use input::{clamp_pos_to_bounds, normalize_selection_in_bounds};
pub(crate) use palette::{PaletteAction, filtered_commands};

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use meerai_agents::events::{ApprovalRequest, QuestionAnswer, QuestionPrompt, QuestionRequest};
use meerai_agents::{AgentCommand, ItemId, TurnId};
use meerai_client::providers::{
    KeyStatus, ProviderAuthKind, ProviderInfo, RuntimeProviderState, canonical_provider_id,
    check_key_status,
};
use meerai_client::secrets::SecretStore;
use meerai_common::definitions::{AgentDefinition, SkillDefinition};
use meerai_common::settings::{AppSettings, EffortLevel};
use meerai_core::text::truncate_single_line;
use meerai_host::{SessionRegistry, WebServerHandle, WebServerLauncher};
use meerai_policy::ApprovalMode;
pub use meerai_policy::{approval_mode_badge, approval_mode_label, parse_approval_mode};
use meerai_session::{LoggedEvent, SessionId, SessionSummary};
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use tokio::sync::mpsc;

use crate::completion::CompletionEngine;
use crate::diff::{FileDiffView, UnifiedLine};
use crate::markdown::MarkdownLine;
use crate::search::{SearchMatch, SearchMode, SearchPreview};
use crate::syntax::{Language, Token};
use crate::theme::{self, Palette};
use crate::workspace::{WorkspaceQuery, local_workspace};
use crate::{Backend, OpenedSession, SessionOpener};

const DOUBLE_CTRL_C_WINDOW: Duration = Duration::from_secs(2);
const NOTICE_TTL: Duration = Duration::from_secs(4);
const CHILD_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

pub fn ctrl_label(key: char) -> String {
    format!("ctrl+{}", key.to_ascii_lowercase())
}
pub const MIN_SIDEBAR_WIDTH: u16 = 110;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputMode {
    Insert,
    Normal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffListState {
    Closed,
    Open { selected: usize },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContextPanel {
    pub breakdown: Option<meerai_session::ContextBreakdown>,
    pub scroll: usize,
    pub selected_turn: usize,
    pub selected_blob: usize,
    pub focus_input: Option<String>,
    pub blob_key: Option<String>,
    pub blob_content: Option<String>,
    pub blob_scroll: usize,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SubagentSpawnPanel {
    pub name: String,
    pub agent: String,
    pub task: String,
    pub model: String,
    pub mode: Option<ApprovalMode>,
    pub steps: String,
    pub tokens: String,
    pub cost: String,
    pub tools: String,
    pub field: usize,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ActiveModal {
    ProviderSelect {
        filter: String,
        selected: usize,
    },
    AuthMethodSelect {
        provider_id: String,
        provider_name: String,
        selected: usize,
    },
    ProviderConnection {
        provider_id: String,
        provider_name: String,
        status: KeyStatus,
        selected: usize,
    },
    ChatGptLogin {
        provider_id: String,
        provider_name: String,
        url: Option<String>,
        input: String,
        error: Option<String>,
        busy: bool,
    },
    ApiKeyInput {
        provider_id: String,
        provider_name: String,
        api_key: String,
        base_url: String,
        editing_base_url: bool,
        existing_key_status: KeyStatus,
        error: Option<String>,
    },
    ModelInput {
        input: String,
        selected: Option<usize>,
        error: Option<String>,
    },
    DiffView {
        file_index: usize,
        diff: FileDiffView,
        scroll: usize,
        list: DiffListState,
    },
    Search {
        query: String,
        mode: SearchMode,
        workspace_files: Vec<String>,
        results: Vec<SearchMatch>,
        selected: usize,
        preview: Option<SearchPreview>,
    },
    Approval {
        queue: Vec<ApprovalRequest>,
        reason: String,
        editing_reason: bool,
    },
    Question {
        request: QuestionRequest,
        answers: Vec<Vec<usize>>,
        custom: Vec<String>,
        editing_custom: bool,
        current: usize,
        focus: usize,
    },
    Terminal {
        run_id: u64,
        command: String,
        output: Option<String>,
        running: bool,
        scroll: usize,
    },
    Runs {
        kind: RunKind,
        selected: usize,
    },
    RunDetail {
        run: RunTarget,
        selected: usize,
        scroll: usize,
    },
    RewindMenu {
        turn_id: TurnId,
        selected: usize,
    },
    SessionSelect {
        filter: String,
        selected: usize,
        sessions: Vec<SessionSummary>,
        loading: bool,
        error: Option<String>,
    },
    CommandPalette {
        filter: String,
        selected: usize,
    },
    UserMessageActions {
        entry_idx: usize,
        selected: usize,
    },
    ServeDialog {
        port_input: String,
        secret_input: String,
        editing_secret: bool,
        error: Option<String>,
        busy: bool,
    },
    QueuedEdit {
        index: usize,
        input: String,
        cursor: usize,
        error: Option<String>,
    },
    Context(ContextPanel),
    Goal {
        state: Option<meerai_session::GoalState>,
        scroll: usize,
    },
    Memory {
        entries: Vec<meerai_host::ProjectMemory>,
        query: String,
        selected: usize,
        loading: bool,
        error: Option<String>,
    },
    Routing {
        routes: Vec<meerai_host::ModelRoute>,
        selected: usize,
        loading: bool,
        error: Option<String>,
    },
    Packs {
        packs: Vec<meerai_host::InstalledPack>,
        definitions: meerai_host::HostDefinitionsDiagnostics,
        tab: PackTab,
        selected: usize,
        loading: bool,
        error: Option<String>,
    },
    SubagentSpawn(SubagentSpawnPanel),
    VaultReset {
        input: String,
        error: Option<String>,
    },
    Sampling {
        effort: Option<EffortLevel>,
        temperature: String,
        field: usize,
        error: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackTab {
    Packs,
    Definitions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryModalAction {
    Delete,
    TogglePin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackModalAction {
    ToggleEnabled,
    ToggleTrusted,
    Remove,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderConnectionAction {
    Next,
    Update,
    Remove,
}

impl ProviderConnectionAction {
    pub fn options(status: &KeyStatus) -> Vec<Self> {
        let mut options = vec![Self::Next, Self::Update];
        if matches!(status, KeyStatus::Stored | KeyStatus::OAuth { .. }) {
            options.push(Self::Remove);
        }
        options
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Next => "Continue to model select",
            Self::Update => "Update credential",
            Self::Remove => "Remove saved credential",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PendingSessionAction {
    RefreshSessions,
    OpenSessionId(SessionId),
    RestoreSessionId(SessionId),
    OpenSessionQuery(String),
    ForkSession {
        session_id: SessionId,
        turn: Option<TurnId>,
        draft: String,
    },
    DeleteSession {
        session_id: SessionId,
    },
    CreateNewTab,
    ReplaceLastTab,
    SyncContext,
    CreateAcpTab {
        preset: String,
    },
    ReadChildSession {
        tab_id: u64,
        item_id: String,
        session_id: SessionId,
    },
    RunUserShell {
        tab_id: u64,
        run_id: u64,
        command: String,
    },
    HostAction {
        tab_id: u64,
        action: meerai_host::HostAction,
    },
    LoadBlob {
        tab_id: u64,
        key: String,
    },
    LoadMemories {
        query: Option<String>,
    },
    LoadRoutes,
    LoadPacks,
    LoadDefinitions,
    MemoryAction {
        action: MemoryModalAction,
        id: String,
    },
    RouteAction {
        task_type: String,
    },
    PackAction {
        action: PackModalAction,
        id: String,
    },
    StartWebServer {
        port: Option<u16>,
        secret: Option<String>,
    },
    StopWebServer,
    CloseSession {
        session_id: SessionId,
    },
}

#[derive(Clone)]
pub struct RunningWebServer {
    pub port: u16,
    pub url: String,
    pub handle: Arc<dyn WebServerHandle>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabHitKind {
    Tab(usize),
    CloseTab(usize),
    NewTab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueuedHitKind {
    Edit(usize),
    Remove(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueuedHitZone {
    pub rect: Rect,
    pub kind: QueuedHitKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionOpenMode {
    ReplaceEmpty,
    NewTab,
    Background,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabDrag {
    pub from: usize,
    pub start_x: u16,
    pub current_x: u16,
    pub moved: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserAction {
    Rewind,
    Copy,
    Fork,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoverZone {
    pub entry_idx: usize,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TranscriptAnchor {
    pub entry_idx: usize,
    pub line: usize,
    pub number: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnchorRailMark {
    pub row: u16,
    pub line: usize,
    pub entry_idx: usize,
    pub number: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnchorRailLayout {
    pub marks: Vec<AnchorRailMark>,
    pub top_gap: bool,
    pub bottom_gap: bool,
    pub top_line: Option<usize>,
    pub bottom_line: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabHitZone {
    pub rect: Rect,
    pub kind: TabHitKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalClickAction {
    Accept,
    AcceptTurn,
    AcceptSession,
    Decline,
    DeclineReason,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalHitKind {
    ListItem(usize),
    ApiKeyField(bool),
    ServeField(bool),
    ApprovalAction(ApprovalClickAction),
    SearchModeToggle,
    EffortTier(EffortLevel),
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModalHitZone {
    pub rect: Rect,
    pub kind: ModalHitKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunKind {
    Agents,
    Shells,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusHitKind {
    Run(RunKind),
    Update,
    WebServer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarHitKind {
    DiagnoseHead,
    ChangesHead,
    ChangesMore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellRunStatus {
    Running,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellRun {
    pub id: u64,
    pub command: String,
    pub status: ShellRunStatus,
    pub output: Option<String>,
    pub started_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunTarget {
    Subagent(usize),
    Shell(u64),
}

impl RunTarget {
    pub fn kind(self) -> RunKind {
        match self {
            Self::Subagent(_) => RunKind::Agents,
            Self::Shell(_) => RunKind::Shells,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompletionHitZone {
    pub rect: Rect,
    pub index: usize,
}

pub use meerai_session::replay::ToolCallStatus;

pub(crate) fn is_delegation_tool(name: &str) -> bool {
    matches!(name, "subagent-run" | "acp-run")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallEntry {
    pub call_id: String,
    pub name: String,
    pub args: String,
    pub status: ToolCallStatus,
    pub output: Option<String>,
    pub expanded: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubagentEntry {
    pub id: String,
    pub name: Option<String>,
    pub agent: String,
    pub status: ToolCallStatus,
    pub model: Option<String>,
    pub approval_mode: String,
    pub steps_used: usize,
    pub max_steps: usize,
    pub usage: meerai_session::Usage,
    pub est_cost_usd: Option<f64>,
    pub active_tool: Option<String>,
    pub report: Option<String>,
    pub child_session_id: Option<String>,
    pub expanded: bool,
    pub nested: Option<Vec<TranscriptEntry>>,
    pub nested_refreshed_at: Option<Instant>,
    pub nested_loading: bool,
}

pub use meerai_host::diff::FileChangeEntry;

impl SubagentEntry {
    pub fn named(&self) -> Option<&str> {
        self.name.as_deref().filter(|name| *name != self.agent)
    }

    pub fn display_name(&self) -> &str {
        self.named().unwrap_or(&self.agent)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenPos {
    pub col: u16,
    pub row: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextSelection {
    pub anchor: ScreenPos,
    pub head: ScreenPos,
    pub active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocPos {
    pub line: usize,
    pub col: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocSelection {
    pub anchor: DocPos,
    pub head: DocPos,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptWrappedLine {
    pub text: String,
    pub gutter_len: usize,
    pub hang_len: usize,
    pub soft_wrapped_next: bool,
}

pub(crate) fn char_width(ch: char) -> usize {
    if ch == '\t' {
        1
    } else {
        unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0)
    }
}

pub(crate) fn display_width(text: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(text)
}

pub(crate) fn display_to_char(text: &str, display: usize) -> usize {
    let mut column = 0;
    for (index, ch) in text.chars().enumerate() {
        let width = char_width(ch);
        if column + width > display {
            return index;
        }
        column += width;
    }
    text.chars().count()
}

pub(crate) fn char_to_display(text: &str, char_index: usize) -> usize {
    text.chars().take(char_index).map(char_width).sum()
}

pub(crate) fn rank_model_matches(models: &[String], provider: &str, query: &str) -> Vec<String> {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return models.to_vec();
    }
    let provider_prefix = format!("{}/", provider.to_ascii_lowercase());
    let mut ranked: Vec<(u8, usize, usize, String)> = models
        .iter()
        .enumerate()
        .filter_map(|(index, model)| {
            let id = model.to_ascii_lowercase();
            let hit = id
                .find(&query)
                .map(|position| (position != 0, position))
                .or_else(|| {
                    format!("{provider_prefix}{id}")
                        .find(&query)
                        .map(|position| (true, position))
                });
            hit.map(|(is_infix, position)| (u8::from(is_infix), position, index, model.clone()))
        })
        .collect();
    ranked.sort();
    ranked.into_iter().map(|(_, _, _, model)| model).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThinkingHitZone {
    pub entry_idx: usize,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TranscriptEntry {
    User {
        text: String,
    },
    Assistant {
        text: String,
        reasoning: String,
        streaming: bool,
        reasoning_expanded: bool,
    },
    ToolCall(ToolCallEntry),
    Subagent(SubagentEntry),
    Question {
        request_id: String,
        questions: Vec<QuestionPrompt>,
        answers: Option<Vec<QuestionAnswer>>,
        resolved: bool,
    },
    System {
        text: String,
    },
    Error {
        text: String,
        expanded: bool,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UsageStats {
    pub prompt_tokens: Option<i32>,
    pub completion_tokens: Option<i32>,
    pub total_tokens: Option<i32>,
    pub cached_tokens: Option<i32>,
    pub cache_creation_tokens: Option<i32>,
}

#[derive(Clone, Debug, Default)]
pub struct TurnMetrics {
    pub turn_count: usize,
    pub session_prompt_tokens: u64,
    pub session_completion_tokens: u64,
    pub session_total_tokens: u64,
    pub session_cached_tokens: u64,
    pub session_cache_creation_tokens: u64,
    pub turn_started_at: Option<Instant>,
    pub first_token_at: Option<Instant>,
    pub last_ttft_ms: Option<u64>,
    pub last_turn_ms: Option<u64>,
    pub last_tokens_per_sec: Option<f64>,
}

impl TurnMetrics {
    pub fn record_first_token(&mut self) {
        if self.first_token_at.is_none() {
            let now = Instant::now();
            self.first_token_at = Some(now);
            if let Some(started) = self.turn_started_at {
                self.last_ttft_ms = Some(now.saturating_duration_since(started).as_millis() as u64);
            }
        }
    }

    pub fn update_speed(&mut self, completion_tokens: Option<i32>) {
        let Some(tokens) = completion_tokens.filter(|t| *t > 0) else {
            return;
        };
        let anchor = self.first_token_at.or(self.turn_started_at);
        if let Some(start) = anchor {
            let secs = start.elapsed().as_secs_f64();
            if secs >= 0.005 {
                self.last_tokens_per_sec = Some(tokens as f64 / secs);
            }
        }
    }

    pub fn elapsed_ms(&self) -> Option<u64> {
        self.turn_started_at
            .map(|start| start.elapsed().as_millis() as u64)
    }
}

pub struct SessionTab {
    pub tab_id: u64,
    pub session_id: SessionId,
    pub title: Option<String>,
    pub backend: Backend,
    pub active_provider: String,
    pub model: String,
    pub approval_mode: ApprovalMode,
    pub input: String,
    pub cursor: usize,
    pub transcript: Vec<TranscriptEntry>,
    pub transcript_version: u64,
    pub usage: UsageStats,
    pub metrics: TurnMetrics,
    pub context: Option<meerai_session::ContextBreakdown>,
    pub goal: Option<meerai_session::GoalState>,
    pub is_running: bool,
    pub queued: Vec<String>,
    pub shell_runs: Vec<ShellRun>,
    pub(super) next_shell_run_id: u64,
    pub user_turns: HashMap<usize, TurnId>,
    pub turn_activity: HashMap<usize, bool>,
    pub turn_id: Option<TurnId>,
    pub active_tool: Option<String>,
    pub scroll_offset: usize,
    pub auto_follow: bool,
    pub viewport_height: usize,
    pub total_lines: usize,
    pub has_unread: bool,
    pub pending_approvals: Vec<ApprovalRequest>,
    pub pending_questions: Vec<QuestionRequest>,
    pub(super) pending_assistant_idx: Option<usize>,
    pub(super) assistant_items: HashMap<ItemId, usize>,
    pub(super) tool_items: HashMap<ItemId, String>,
    pub(super) subagent_items: HashMap<ItemId, usize>,
    pub(super) cmd_tx: mpsc::UnboundedSender<AgentCommand>,
}

impl SessionTab {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tab_id: u64,
        session_id: SessionId,
        title: Option<String>,
        active_provider: impl Into<String>,
        model: impl Into<String>,
        approval_mode: ApprovalMode,
        cmd_tx: mpsc::UnboundedSender<AgentCommand>,
    ) -> Self {
        Self {
            tab_id,
            session_id,
            title: title.and_then(normalize_tab_title),
            backend: Backend::default(),
            active_provider: canonical_provider_id(&active_provider.into()),
            model: model.into(),
            approval_mode,
            input: String::new(),
            cursor: 0,
            transcript: Vec::new(),
            transcript_version: 0,
            usage: UsageStats::default(),
            metrics: TurnMetrics::default(),
            context: None,
            goal: None,
            is_running: false,
            queued: Vec::new(),
            shell_runs: Vec::new(),
            next_shell_run_id: 0,
            pending_questions: Vec::new(),
            user_turns: HashMap::new(),
            turn_activity: HashMap::new(),
            turn_id: None,
            active_tool: None,
            scroll_offset: 0,
            auto_follow: true,
            viewport_height: 20,
            total_lines: 0,
            has_unread: false,
            pending_approvals: Vec::new(),
            pending_assistant_idx: None,
            assistant_items: HashMap::new(),
            tool_items: HashMap::new(),
            subagent_items: HashMap::new(),
            cmd_tx,
        }
    }

    pub fn is_acp(&self) -> bool {
        matches!(self.backend, Backend::Acp { .. })
    }

    pub fn has_running_shell(&self) -> bool {
        self.shell_runs
            .iter()
            .any(|run| run.status == ShellRunStatus::Running)
    }

    pub fn acp_preset(&self) -> Option<&str> {
        match &self.backend {
            Backend::Acp { preset, .. } => Some(preset),
            Backend::Native => None,
        }
    }

    pub fn display_title(&self) -> String {
        if let Some(title) = self.title.as_deref().filter(|t| !t.trim().is_empty()) {
            return truncate_single_line(title, 24);
        }
        if let Some(first_user) = self.transcript.iter().find_map(|entry| match entry {
            TranscriptEntry::User { text } => normalize_tab_title(text.clone()),
            _ => None,
        }) {
            return truncate_single_line(&first_user, 24);
        }
        self.session_id.as_str().to_string()
    }

    pub fn seed_usage_from_events(&mut self, events: &[LoggedEvent]) {
        if let Some(usage) = session_usage_from_events(events) {
            let prompt = usage.prompt_tokens.unwrap_or(0).max(0) as u64;
            let completion = usage.completion_tokens.unwrap_or(0).max(0) as u64;
            self.metrics.session_prompt_tokens = prompt;
            self.metrics.session_completion_tokens = completion;
            self.metrics.session_total_tokens = usage
                .total_tokens
                .map(|total| total.max(0) as u64)
                .unwrap_or(prompt + completion);
            self.metrics.session_cached_tokens = usage.cached_tokens.unwrap_or(0).max(0) as u64;
        }
    }

    pub fn has_conversation(&self) -> bool {
        !self.transcript.is_empty() || self.is_running || !self.input.is_empty()
    }

    pub fn is_empty_session(&self) -> bool {
        !self.is_running
            && !self
                .transcript
                .iter()
                .any(|entry| matches!(entry, TranscriptEntry::User { .. }))
    }

    pub fn max_scroll(&self) -> usize {
        self.total_lines.saturating_sub(self.viewport_height)
    }

    pub fn effective_scroll(&self) -> usize {
        if self.auto_follow {
            self.max_scroll()
        } else {
            self.scroll_offset.min(self.max_scroll())
        }
    }
}

pub(super) fn normalize_tab_title(raw: String) -> Option<String> {
    let line = raw
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?
        .to_string();
    Some(truncate_single_line(&line, 80))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EntryState {
    User,
    Assistant {
        streaming: bool,
        reasoning: bool,
        reasoning_expanded: bool,
    },
    ToolCall {
        status: ToolCallStatus,
        expanded: bool,
    },
    Subagent {
        status: ToolCallStatus,
        expanded: bool,
        loading: bool,
        nested_entries: Option<usize>,
    },
    System,
    Question {
        resolved: bool,
        answered: bool,
    },
    Error {
        expanded: bool,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct EntryFingerprint {
    size: usize,
    state: EntryState,
}

impl EntryFingerprint {
    fn of(entry: &TranscriptEntry) -> Self {
        match entry {
            TranscriptEntry::User { text } => Self {
                size: text.len(),
                state: EntryState::User,
            },
            TranscriptEntry::Assistant {
                text,
                reasoning,
                streaming,
                reasoning_expanded,
            } => Self {
                size: text.len() + reasoning.len(),
                state: EntryState::Assistant {
                    streaming: *streaming,
                    reasoning: !reasoning.is_empty(),
                    reasoning_expanded: *reasoning_expanded,
                },
            },
            TranscriptEntry::ToolCall(call) => Self {
                size: call.name.len()
                    + call.args.len()
                    + call.output.as_ref().map_or(0, |output| output.len()),
                state: EntryState::ToolCall {
                    status: call.status,
                    expanded: call.expanded,
                },
            },
            TranscriptEntry::Subagent(subagent) => Self {
                size: subagent.display_name().len()
                    + subagent.agent.len()
                    + subagent.report.as_ref().map_or(0, |report| report.len()),
                state: EntryState::Subagent {
                    status: subagent.status,
                    expanded: subagent.expanded,
                    loading: subagent.nested_loading,
                    nested_entries: if subagent.expanded {
                        nested_size(&subagent.nested)
                    } else {
                        None
                    },
                },
            },
            TranscriptEntry::Question {
                questions,
                answers,
                resolved,
                ..
            } => Self {
                size: questions
                    .iter()
                    .map(|prompt| prompt.header.len() + prompt.question.len())
                    .sum::<usize>()
                    + answers.as_ref().map_or(0, |answers| {
                        answers
                            .iter()
                            .map(|answer| answer.selected.iter().map(String::len).sum::<usize>())
                            .sum()
                    }),
                state: EntryState::Question {
                    resolved: *resolved,
                    answered: answers.is_some(),
                },
            },
            TranscriptEntry::System { text } => Self {
                size: text.len(),
                state: EntryState::System,
            },
            TranscriptEntry::Error { text, expanded } => Self {
                size: text.len(),
                state: EntryState::Error {
                    expanded: *expanded,
                },
            },
        }
    }

    fn shows_spinner(&self) -> bool {
        matches!(
            self.state,
            EntryState::ToolCall {
                status: ToolCallStatus::Proposed | ToolCallStatus::Running,
                ..
            }
        )
    }
}

fn nested_size(nested: &Option<Vec<TranscriptEntry>>) -> Option<usize> {
    nested.as_ref().map(|entries| {
        entries.iter().fold(entries.len(), |acc, entry| {
            let fingerprint = EntryFingerprint::of(entry);
            acc.wrapping_mul(31)
                .wrapping_add(fingerprint.size)
                .wrapping_add(fingerprint.state.size_code())
        })
    })
}

impl EntryState {
    fn size_code(self) -> usize {
        match self {
            EntryState::User => 0,
            EntryState::Assistant { .. } => 1,
            EntryState::ToolCall { status, .. } => 2 + status_code(status),
            EntryState::Subagent { status, .. } => 9 + status_code(status),
            EntryState::System => 16,
            EntryState::Question { resolved, answered } => {
                18 + usize::from(resolved) * 2 + usize::from(answered)
            }
            EntryState::Error { .. } => 17,
        }
    }
}

fn status_code(status: ToolCallStatus) -> usize {
    match status {
        ToolCallStatus::Proposed => 0,
        ToolCallStatus::Running => 1,
        ToolCallStatus::Succeeded => 2,
        ToolCallStatus::Failed => 3,
        ToolCallStatus::Stopped => 4,
        ToolCallStatus::Cancelled => 5,
    }
}

pub(crate) struct TranscriptRenderKey {
    tab_id: u64,
    width: u16,
    version: u64,
    palette: Palette,
    provider: String,
    model: String,
    workspace: PathBuf,
    acp_preset: Option<String>,
    key_status: KeyStatus,
    spinner_tick: Option<usize>,
    entries: Vec<EntryFingerprint>,
    turn_activity: Vec<(usize, bool)>,
}

impl TranscriptRenderKey {
    pub(crate) fn capture(app: &AppState, width: u16) -> Self {
        let tab = app.tab();
        let entries: Vec<EntryFingerprint> =
            tab.transcript.iter().map(EntryFingerprint::of).collect();
        let spinner_tick = entries
            .iter()
            .any(EntryFingerprint::shows_spinner)
            .then_some(app.spinner_tick);
        let mut turn_activity: Vec<(usize, bool)> = tab
            .turn_activity
            .iter()
            .map(|(index, expanded)| (*index, *expanded))
            .collect();
        turn_activity.sort_unstable_by_key(|(index, _)| *index);
        Self {
            tab_id: tab.tab_id,
            width,
            version: tab.transcript_version,
            palette: theme::p(),
            provider: tab.active_provider.clone(),
            model: tab.model.clone(),
            workspace: app.workspace_root.clone(),
            acp_preset: tab.acp_preset().map(str::to_string),
            key_status: app.active_key_status(),
            spinner_tick,
            entries,
            turn_activity,
        }
    }

    fn matches(&self, app: &AppState, width: u16) -> bool {
        if self.width != width {
            return false;
        }
        let tab = app.tab();
        self.tab_id == tab.tab_id
            && self.version == tab.transcript_version
            && self.palette == theme::p()
            && self.provider == tab.active_provider
            && self.model == tab.model
            && self.workspace == app.workspace_root
            && self.acp_preset.as_deref() == tab.acp_preset()
            && self.key_status == app.active_key_status()
            && self.spinner_tick
                == self
                    .entries
                    .iter()
                    .any(EntryFingerprint::shows_spinner)
                    .then_some(app.spinner_tick)
            && self.entries.len() == tab.transcript.len()
            && self
                .entries
                .iter()
                .zip(tab.transcript.iter())
                .all(|(cached, entry)| *cached == EntryFingerprint::of(entry))
            && self.turn_activity.len() == tab.turn_activity.len()
            && self
                .turn_activity
                .iter()
                .all(|(index, expanded)| tab.turn_activity.get(index) == Some(expanded))
    }
}

struct MemoSlot<T> {
    key: u64,
    value: Rc<T>,
}

struct MemoTable<T> {
    slots: Vec<Option<MemoSlot<T>>>,
}

impl<T> Default for MemoTable<T> {
    fn default() -> Self {
        Self { slots: Vec::new() }
    }
}

impl<T> MemoTable<T> {
    fn begin(&mut self, len: usize) {
        self.slots.resize_with(len, || None);
        self.slots.truncate(len);
    }

    fn get(&mut self, index: usize, key: u64, compute: impl FnOnce() -> T) -> Rc<T> {
        if let Some(slot) = self.slots.get(index).and_then(Option::as_ref)
            && slot.key == key
        {
            return slot.value.clone();
        }
        let value = Rc::new(compute());
        if let Some(slot) = self.slots.get_mut(index) {
            *slot = Some(MemoSlot {
                key,
                value: value.clone(),
            });
        }
        value
    }
}

#[derive(Default)]
pub(crate) struct TranscriptBuildMemo {
    tab_id: Option<u64>,
    palette: Palette,
    markdown: MemoTable<Vec<MarkdownLine>>,
    edit_diff: MemoTable<Option<Vec<UnifiedLine>>>,
    tokens: MemoTable<Vec<Vec<Token>>>,
}

impl TranscriptBuildMemo {
    pub(crate) fn begin(&mut self, tab_id: u64, palette: Palette, len: usize) {
        if self.tab_id != Some(tab_id) || self.palette != palette {
            self.tab_id = Some(tab_id);
            self.palette = palette;
            self.markdown = MemoTable::default();
            self.edit_diff = MemoTable::default();
            self.tokens = MemoTable::default();
        }
        self.markdown.begin(len);
        self.edit_diff.begin(len);
        self.tokens.begin(len);
    }

    pub(crate) fn markdown(
        &mut self,
        index: usize,
        width: usize,
        text: &str,
    ) -> Rc<Vec<MarkdownLine>> {
        let key = memo_hash(None, &[text, &width.to_string()]);
        self.markdown
            .get(index, key, || crate::markdown::render(text, width))
    }

    pub(crate) fn edit_diff(
        &mut self,
        index: usize,
        call: &ToolCallEntry,
        compute: impl FnOnce() -> Option<Vec<UnifiedLine>>,
    ) -> Rc<Option<Vec<UnifiedLine>>> {
        let key = memo_hash(
            None,
            &[
                &call.name,
                &call.args,
                call.output.as_deref().unwrap_or_default(),
            ],
        );
        self.edit_diff.get(index, key, compute)
    }

    pub(crate) fn tokens(
        &mut self,
        index: usize,
        language: Option<Language>,
        source: &str,
        compute: impl FnOnce() -> Vec<Vec<Token>>,
    ) -> Rc<Vec<Vec<Token>>> {
        self.tokens
            .get(index, memo_hash(language, &[source]), compute)
    }
}

fn memo_hash(language: Option<Language>, parts: &[&str]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let seed = language.map_or(0, |language| language as u64 + 1);
    hash = hash.rotate_left(5) ^ seed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    for part in parts {
        for byte in part.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x1_0000_01b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x1_0000_01b3);
    }
    hash
}

#[derive(Default)]
pub(crate) struct TranscriptDocument {
    pub(crate) paragraph: Paragraph<'static>,
    pub(crate) line_count: usize,
    pub(crate) bands: Vec<(usize, usize)>,
    pub(crate) code_bands: Vec<(usize, usize)>,
    pub(crate) hover_text_zones: Vec<HoverZone>,
}

#[derive(Default)]
pub(crate) struct TranscriptRenderCache {
    key: Option<TranscriptRenderKey>,
    pub(crate) document: TranscriptDocument,
    memo: TranscriptBuildMemo,
    builds: u64,
}

impl TranscriptRenderCache {
    pub(crate) fn matches(&self, app: &AppState, width: u16) -> bool {
        self.key.as_ref().is_some_and(|key| key.matches(app, width))
    }

    pub(crate) fn clear(&mut self) {
        self.key = None;
        self.document = TranscriptDocument::default();
    }

    pub(crate) fn take_memo(&mut self) -> TranscriptBuildMemo {
        std::mem::take(&mut self.memo)
    }

    pub(crate) fn store(
        &mut self,
        key: TranscriptRenderKey,
        memo: TranscriptBuildMemo,
        document: TranscriptDocument,
    ) {
        self.key = Some(key);
        self.memo = memo;
        self.document = document;
        self.builds += 1;
    }

    pub(crate) fn builds(&self) -> u64 {
        self.builds
    }
}

#[derive(Default)]
pub(crate) struct SubagentViewCache {
    pub(crate) key: Option<(u64, usize, u64, u16)>,
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) doc_lines: Vec<TranscriptWrappedLine>,
    pub(crate) bands: Vec<(usize, usize)>,
    pub(crate) code_bands: Vec<(usize, usize)>,
    pub(crate) body: Rect,
    pub(crate) scroll: usize,
    pub(crate) selection: Option<DocSelection>,
    pub(crate) anchor: Option<DocPos>,
}

pub struct AppState {
    pub mode: InputMode,
    pub workspace_root: PathBuf,
    pub workspace: Arc<dyn WorkspaceQuery>,
    pub tabs: Vec<SessionTab>,
    pub active_tab: usize,
    pub completion: CompletionEngine,
    pub modal: Option<ActiveModal>,
    pub runtime: Arc<RwLock<RuntimeProviderState>>,
    pub secrets: Arc<dyn SecretStore>,
    pub settings: AppSettings,
    pub context_temperature: Option<f32>,
    pub context_effort: Option<EffortLevel>,
    pub explicit_context_window: Option<u64>,
    pub config_dir: Option<PathBuf>,
    pub pending_login_request: Option<String>,
    pub login_input: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    pub login_cancel: Option<tokio::sync::oneshot::Sender<()>>,
    pub spinner_tick: usize,
    pub show_sidebar: bool,
    pub sidebar_diagnose_open: bool,
    pub sidebar_diagnose_head_area: Rect,
    pub sidebar_changes_open: bool,
    pub sidebar_changes_head_area: Rect,
    pub terminal_width: u16,
    pub file_changes: Vec<FileChangeEntry>,
    pub mcp_servers: Vec<String>,
    pub registered_tools: Vec<String>,
    pub agents: Vec<AgentDefinition>,
    pub skills: Vec<SkillDefinition>,
    pub tab_hit_zones: Vec<TabHitZone>,
    pub hovered_tab: Option<TabHitKind>,
    pub transcript_inner_area: Rect,
    pub sidebar_changes_inner_area: Rect,
    pub visible_file_change_rows: usize,
    pub git_branch: Option<String>,
    pub sandbox_active: bool,
    pub sandbox_kind: String,
    pub sandbox_warning: Option<String>,
    pub file_change_scroll: usize,
    pub file_change_more_hit: Option<Rect>,
    pub hovered_file_change: Option<usize>,
    pub runs_hits: Vec<(Rect, RunKind)>,
    pub(super) run_detail_max_scroll: usize,
    pub thinking_hit_zones: Vec<ThinkingHitZone>,
    pub digest_hit_zones: Vec<ThinkingHitZone>,
    pub hover_zones: Vec<HoverZone>,
    pub hovered_entry: Option<usize>,
    pub hovered_digest: Option<usize>,
    pub transcript_anchors: Vec<TranscriptAnchor>,
    pub anchor_rail_area: Rect,
    pub transcript_area: Rect,
    pub anchor_rail_hover: Option<u16>,
    pub(super) anchor_rail_drag: bool,
    pub modal_hit_zones: Vec<ModalHitZone>,
    pub hovered_modal_hit: Option<ModalHitKind>,
    pub completion_hit_zones: Vec<CompletionHitZone>,
    pub hovered_completion: Option<usize>,
    pub update_available: Option<String>,
    pub launch_system: String,
    pub launch_definitions: String,
    pub(super) pending_reload: bool,
    pub(super) pending_quiet_reload: bool,
    pub(super) session_delete_confirm: Option<SessionId>,
    pub(crate) tab_persistence: bool,
    pub update_hit: Option<Rect>,
    pub web_server: Option<RunningWebServer>,
    pub web_server_hit: Option<Rect>,
    pub hovered_status: Option<StatusHitKind>,
    pub hovered_sidebar: Option<SidebarHitKind>,
    pub registry: Option<Arc<SessionRegistry>>,
    pub web_launcher: Option<Arc<dyn WebServerLauncher>>,
    pub(crate) tui_prompt_echoes: VecDeque<(SessionId, String)>,
    pub composer_block_area: Rect,
    pub composer_input_area: Rect,
    pub composer_scroll: usize,
    pub composer_selection: Option<(usize, usize)>,
    pub(super) composer_goal_col: Option<usize>,
    pub queued_editor_width: u16,
    pub(super) queued_editor_goal_col: Option<usize>,
    pub queued_band_area: Rect,
    pub queued_hit_zones: Vec<QueuedHitZone>,
    pub hovered_queued: Option<QueuedHitKind>,
    pub(super) composer_drag_anchor: Option<usize>,
    pub(super) tab_drag: Option<TabDrag>,
    pub(super) pending_toggle: Option<usize>,
    pub(super) pending_digest: Option<usize>,
    pub(super) last_click: Option<(Instant, u16, u16)>,
    pub(super) click_streak: u8,
    pub selection: Option<TextSelection>,
    pub doc_selection: Option<DocSelection>,
    pub last_transcript_anchor: Option<DocPos>,
    pub last_mouse_pos: Option<ScreenPos>,
    pub transcript_wrapped_lines: Vec<TranscriptWrappedLine>,
    pub(crate) transcript_cache: TranscriptRenderCache,
    pub(crate) subagent_view_cache: SubagentViewCache,
    pub screen_lines: Vec<Vec<String>>,
    pub last_copied_text: Option<String>,
    key_status_cache: RefCell<HashMap<String, KeyStatus>>,
    pub should_quit: bool,
    pub status_notice: Option<String>,
    notice_seen: Option<String>,
    notice_set_at: Option<Instant>,
    pub session_opener: Option<Arc<dyn SessionOpener>>,
    pub(super) pending_session_actions: Vec<PendingSessionAction>,
    closed_empty_session: Option<SessionId>,
    next_tab_id: u64,
    pending_g: bool,
    last_ctrl_c: Option<Instant>,
}

impl AppState {
    pub fn set_workspace_query(&mut self, workspace: Arc<dyn WorkspaceQuery>) {
        self.workspace = workspace.clone();
        self.completion.set_workspace(workspace);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        active_provider: impl Into<String>,
        model: impl Into<String>,
        workspace_root: PathBuf,
        cmd_tx: mpsc::UnboundedSender<AgentCommand>,
        runtime: Arc<RwLock<RuntimeProviderState>>,
        secrets: Arc<dyn SecretStore>,
        settings: AppSettings,
        config_dir: Option<PathBuf>,
    ) -> Self {
        let active_provider = canonical_provider_id(&active_provider.into());
        let model = model.into();
        let mut completion = CompletionEngine::new(workspace_root.clone());

        let missing_key_idx = {
            let mut rt = runtime.write().unwrap();
            rt.active_provider = active_provider.clone();
            let mut completion_models = settings.recent_models_for(&active_provider).to_vec();
            if let Some(default_model) = rt.catalog.default_model_for_provider(&active_provider)
                && !completion_models.contains(&default_model)
            {
                completion_models.push(default_model);
            }
            completion.sync_models(&completion_models, &active_provider);
            let provider_info = rt.active_provider_info();
            let status = check_key_status(&provider_info, secrets.as_ref());
            rt.plan_usage = matches!(&status, KeyStatus::OAuth { .. });
            if !status.is_configured() {
                rt.catalog
                    .providers()
                    .iter()
                    .position(|p| p.id == active_provider)
                    .or(Some(0))
            } else {
                None
            }
        };

        let modal = if let Some(selected) = missing_key_idx {
            Some(ActiveModal::ProviderSelect {
                filter: String::new(),
                selected,
            })
        } else if model.trim().is_empty() {
            Some(ActiveModal::ModelInput {
                input: String::new(),
                selected: None,
                error: None,
            })
        } else {
            None
        };

        let tab = SessionTab::new(
            1,
            SessionId::new(),
            None,
            active_provider,
            model,
            ApprovalMode::Ask,
            cmd_tx,
        );

        Self {
            mode: InputMode::Insert,
            workspace: local_workspace(workspace_root.clone()),
            workspace_root,
            tabs: vec![tab],
            active_tab: 0,
            completion,
            modal,
            runtime,
            secrets,
            settings,
            context_temperature: None,
            context_effort: None,
            explicit_context_window: None,
            config_dir,
            pending_login_request: None,
            login_input: None,
            login_cancel: None,
            spinner_tick: 0,
            show_sidebar: true,
            sidebar_diagnose_open: true,
            sidebar_diagnose_head_area: Rect::default(),
            sidebar_changes_open: true,
            sidebar_changes_head_area: Rect::default(),
            terminal_width: 120,
            file_changes: Vec::new(),
            mcp_servers: Vec::new(),
            registered_tools: Vec::new(),
            agents: Vec::new(),
            skills: Vec::new(),
            tab_hit_zones: Vec::new(),
            hovered_tab: None,
            transcript_inner_area: Rect::default(),
            sidebar_changes_inner_area: Rect::default(),
            visible_file_change_rows: 0,
            git_branch: None,
            sandbox_active: true,
            sandbox_kind: String::new(),
            sandbox_warning: None,
            file_change_scroll: 0,
            file_change_more_hit: None,
            runs_hits: Vec::new(),
            run_detail_max_scroll: 0,
            hovered_file_change: None,
            thinking_hit_zones: Vec::new(),
            digest_hit_zones: Vec::new(),
            hover_zones: Vec::new(),
            hovered_entry: None,
            hovered_digest: None,
            transcript_anchors: Vec::new(),
            anchor_rail_area: Rect::default(),
            transcript_area: Rect::default(),
            anchor_rail_hover: None,
            anchor_rail_drag: false,
            modal_hit_zones: Vec::new(),
            hovered_modal_hit: None,
            completion_hit_zones: Vec::new(),
            hovered_completion: None,
            update_available: None,
            launch_system: String::new(),
            launch_definitions: String::new(),
            pending_reload: false,
            pending_quiet_reload: false,
            session_delete_confirm: None,
            tab_persistence: false,
            update_hit: None,
            web_server: None,
            web_server_hit: None,
            hovered_status: None,
            hovered_sidebar: None,
            registry: None,
            web_launcher: None,
            tui_prompt_echoes: VecDeque::new(),
            composer_block_area: Rect::default(),
            composer_input_area: Rect::default(),
            composer_scroll: 0,
            composer_selection: None,
            composer_goal_col: None,
            queued_editor_width: 0,
            queued_editor_goal_col: None,
            queued_band_area: Rect::default(),
            queued_hit_zones: Vec::new(),
            hovered_queued: None,
            composer_drag_anchor: None,
            tab_drag: None,
            pending_toggle: None,
            pending_digest: None,
            last_click: None,
            click_streak: 0,
            selection: None,
            doc_selection: None,
            last_transcript_anchor: None,
            last_mouse_pos: None,
            transcript_wrapped_lines: Vec::new(),
            transcript_cache: TranscriptRenderCache::default(),
            subagent_view_cache: SubagentViewCache::default(),
            screen_lines: Vec::new(),
            last_copied_text: None,
            key_status_cache: RefCell::new(HashMap::new()),
            should_quit: false,
            status_notice: None,
            notice_seen: None,
            notice_set_at: None,
            session_opener: None,
            pending_session_actions: Vec::new(),
            closed_empty_session: None,
            next_tab_id: 2,
            pending_g: false,
            last_ctrl_c: None,
        }
    }

    pub fn tab(&self) -> &SessionTab {
        &self.tabs[self.active_tab]
    }

    pub fn tab_mut(&mut self) -> &mut SessionTab {
        &mut self.tabs[self.active_tab]
    }

    pub fn tabs(&self) -> &[SessionTab] {
        &self.tabs
    }

    pub fn active_tab_index(&self) -> usize {
        self.active_tab
    }

    pub fn transcript_builds(&self) -> u64 {
        self.transcript_cache.builds()
    }

    pub fn tab_index_for_session(&self, id: &SessionId) -> Option<usize> {
        self.tabs.iter().position(|tab| &tab.session_id == id)
    }

    pub fn tab_index_by_id(&self, tab_id: u64) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.tab_id == tab_id)
    }

    fn empty_tab_index(&self) -> Option<usize> {
        self.tabs.iter().position(SessionTab::is_empty_session)
    }

    pub(super) fn request_new_tab(&mut self) {
        if self.session_opener.is_none() {
            self.status_notice =
                Some("Session switching is not available in this context".to_string());
            return;
        }
        if let Some(index) = self.empty_tab_index() {
            if index == self.active_tab {
                self.status_notice = Some("This tab is already empty".to_string());
            } else {
                self.switch_to_tab(index);
                self.status_notice = Some(format!("Switched to empty tab {}", index + 1));
            }
            return;
        }
        self.status_notice = Some("Opening new session tab...".to_string());
        self.pending_session_actions
            .push(PendingSessionAction::CreateNewTab);
    }

    pub fn take_pending_session_actions(&mut self) -> Vec<PendingSessionAction> {
        std::mem::take(&mut self.pending_session_actions)
    }

    pub fn set_initial_session(
        &mut self,
        session_id: SessionId,
        title: Option<String>,
        backend: Backend,
        transcript: Vec<TranscriptEntry>,
        events: &[LoggedEvent],
        approval_mode: ApprovalMode,
    ) {
        let tab = self.tab_mut();
        tab.session_id = session_id;
        tab.title = title.and_then(normalize_tab_title);
        tab.backend = backend;
        tab.approval_mode = approval_mode;
        tab.transcript = transcript;
        tab.transcript_version = tab.transcript_version.wrapping_add(1);
        tab.user_turns = user_turn_map(&tab.transcript, events);
        tab.turn_activity.clear();
        tab.seed_usage_from_events(events);
        self.persist_tabs();
    }

    pub fn persist_tabs(&self) {
        if !self.tab_persistence {
            return;
        }
        let ids: Vec<SessionId> = self.tabs.iter().map(|tab| tab.session_id.clone()).collect();
        if let Ok(store) =
            meerai_common::settings::open_store_for_config_dir(self.config_dir.as_deref())
        {
            crate::tabs::save(
                &store,
                &ids,
                self.active_tab,
                &self.workspace_root,
                self.persistable_empty_session().as_ref(),
            );
        }
    }

    fn persistable_empty_session(&self) -> Option<SessionId> {
        self.tabs
            .iter()
            .find(|tab| tab.is_empty_session())
            .map(|tab| tab.session_id.clone())
            .or_else(|| self.closed_empty_session.clone())
    }

    fn reset_ephemeral_ui_state(&mut self) {
        self.completion.reset();
        self.thinking_hit_zones.clear();
        self.digest_hit_zones.clear();
        self.transcript_anchors.clear();
        self.anchor_rail_hover = None;
        self.anchor_rail_drag = false;
        self.selection = None;
        self.doc_selection = None;
        self.last_transcript_anchor = None;
        self.transcript_wrapped_lines.clear();
        self.transcript_cache.clear();
        self.subagent_view_cache = SubagentViewCache::default();
        self.queued_hit_zones.clear();
        self.hovered_queued = None;
        self.hovered_status = None;
        self.hovered_sidebar = None;
        self.file_change_scroll = 0;
        self.file_change_more_hit = None;
        self.pending_g = false;
    }

    pub fn should_replace_active_tab(&self) -> bool {
        self.tabs.len() == 1 && !self.tab().has_conversation()
    }

    fn sync_active_provider_runtime(&mut self) {
        if self.tab().is_acp() {
            return;
        }
        let provider = self.tab().active_provider.clone();
        if let Ok(mut rt) = self.runtime.write() {
            rt.active_provider = provider;
        }
    }

    pub fn switch_to_tab(&mut self, index: usize) -> bool {
        if index >= self.tabs.len() || index == self.active_tab {
            if let Some(tab) = self.tabs.get_mut(index) {
                tab.has_unread = false;
            }
            return false;
        }

        if let Some(ActiveModal::Approval { queue, .. }) = self.modal.take() {
            self.tabs[self.active_tab].pending_approvals = queue;
        }

        self.active_tab = index;
        self.tabs[index].has_unread = false;
        self.reset_ephemeral_ui_state();
        self.persist_tabs();

        self.sync_active_provider_runtime();
        self.sync_completion_models();
        self.open_pending_question();

        if self.modal.is_none() && !self.tabs[index].pending_approvals.is_empty() {
            let queue = std::mem::take(&mut self.tabs[index].pending_approvals);
            self.modal = Some(ActiveModal::Approval {
                queue,
                reason: String::new(),
                editing_reason: false,
            });
        }

        true
    }

    pub fn next_tab(&mut self) -> bool {
        if self.tabs.len() <= 1 {
            return false;
        }
        let next = (self.active_tab + 1) % self.tabs.len();
        self.switch_to_tab(next)
    }

    pub fn prev_tab(&mut self) -> bool {
        if self.tabs.len() <= 1 {
            return false;
        }
        let prev = if self.active_tab == 0 {
            self.tabs.len() - 1
        } else {
            self.active_tab - 1
        };
        self.switch_to_tab(prev)
    }

    pub fn move_tab(&mut self, from: usize, to: usize) -> bool {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return false;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active_tab = if self.active_tab == from {
            to
        } else if from < self.active_tab && to >= self.active_tab {
            self.active_tab - 1
        } else if from > self.active_tab && to <= self.active_tab {
            self.active_tab + 1
        } else {
            self.active_tab
        };
        self.persist_tabs();
        true
    }

    pub fn close_active_tab(&mut self) -> bool {
        self.close_tab(self.active_tab)
    }

    pub fn close_tab(&mut self, index: usize) -> bool {
        if self.tabs.len() <= 1 {
            if index >= self.tabs.len() {
                return false;
            }
            if self.session_opener.is_none() {
                self.status_notice = Some("Cannot close the last tab (use /quit to exit)".into());
                return false;
            }
            self.status_notice = Some("Closing the last tab and opening a new one...".into());
            self.pending_session_actions
                .push(PendingSessionAction::ReplaceLastTab);
            return true;
        }
        if index >= self.tabs.len() {
            return false;
        }

        if index == self.active_tab && matches!(self.modal, Some(ActiveModal::Approval { .. })) {
            self.modal = None;
        }

        let removed = self.tabs.remove(index);
        if removed.is_empty_session() {
            self.closed_empty_session = Some(removed.session_id.clone());
        }
        if self.registry.is_some() {
            self.pending_session_actions
                .push(PendingSessionAction::CloseSession {
                    session_id: removed.session_id,
                });
        } else {
            let _ = removed.cmd_tx.send(AgentCommand::Quit);
        }

        if index < self.active_tab {
            self.active_tab -= 1;
        } else if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }

        self.tabs[self.active_tab].has_unread = false;
        self.reset_ephemeral_ui_state();
        self.persist_tabs();

        self.sync_active_provider_runtime();
        self.sync_completion_models();
        self.open_pending_question();

        if self.modal.is_none() && !self.tabs[self.active_tab].pending_approvals.is_empty() {
            let queue = std::mem::take(&mut self.tabs[self.active_tab].pending_approvals);
            self.modal = Some(ActiveModal::Approval {
                queue,
                reason: String::new(),
                editing_reason: false,
            });
        }

        true
    }

    pub fn apply_opened_session(
        &mut self,
        opened: OpenedSession,
        mode: SessionOpenMode,
    ) -> Option<(u64, mpsc::UnboundedReceiver<meerai_agents::AgentEvent>)> {
        if let Some(existing_idx) = self.tab_index_for_session(&opened.session_id) {
            let _ = opened.cmd_tx.send(AgentCommand::Quit);
            self.switch_to_tab(existing_idx);
            self.status_notice = Some(format!("Switched to open tab {}", existing_idx + 1));
            return None;
        }
        if self.closed_empty_session.as_ref() == Some(&opened.session_id) {
            self.closed_empty_session = None;
        }

        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;

        let mut tab = SessionTab::new(
            tab_id,
            opened.session_id,
            opened.title,
            opened.provider,
            opened.model,
            opened.approval_mode,
            opened.cmd_tx,
        );
        tab.backend = opened.backend;
        tab.transcript = transcript_from_events(&opened.events);
        tab.user_turns = user_turn_map(&tab.transcript, &opened.events);
        tab.seed_usage_from_events(&opened.events);
        tab.goal = meerai_session::goal::last_goal(&opened.events);

        if mode == SessionOpenMode::ReplaceEmpty && self.should_replace_active_tab() {
            if matches!(self.modal, Some(ActiveModal::Approval { .. })) {
                self.modal = None;
            }
            let old = std::mem::replace(&mut self.tabs[0], tab);
            if old.is_empty_session() {
                self.closed_empty_session = Some(old.session_id.clone());
            }
            if self.registry.is_some() {
                self.pending_session_actions
                    .push(PendingSessionAction::CloseSession {
                        session_id: old.session_id,
                    });
            } else {
                let _ = old.cmd_tx.send(AgentCommand::Quit);
            }
            self.active_tab = 0;
            self.reset_ephemeral_ui_state();
            self.sync_active_provider_runtime();
            self.sync_completion_models();
            self.open_pending_question();
            self.persist_tabs();
        } else if mode == SessionOpenMode::Background {
            self.tabs.push(tab);
            self.persist_tabs();
        } else {
            self.tabs.push(tab);
            let new_idx = self.tabs.len() - 1;
            self.switch_to_tab(new_idx);
        }

        if mode != SessionOpenMode::Background {
            self.status_notice = Some(format!("Session {}", self.tab().session_id));
        }
        Some((tab_id, opened.event_rx))
    }

    pub fn apply_listed_sessions(&mut self, result: anyhow::Result<Vec<SessionSummary>>) {
        if let Some(ActiveModal::SessionSelect {
            sessions,
            loading,
            error,
            selected,
            filter,
        }) = &mut self.modal
        {
            *loading = false;
            match result {
                Ok(list) => {
                    *sessions = list;
                    *error = None;
                    let visible = filter_session_summaries(sessions, filter);
                    if visible.is_empty() {
                        *selected = 0;
                    } else {
                        *selected = (*selected).min(visible.len() - 1);
                    }
                }
                Err(err) => {
                    *error = Some(format!("{err:#}"));
                }
            }
        }
    }

    pub fn filtered_sessions<'a>(
        &self,
        sessions: &'a [SessionSummary],
        filter: &str,
    ) -> Vec<&'a SessionSummary> {
        filter_session_summaries(sessions, filter)
    }

    pub fn is_sidebar_visible(&self) -> bool {
        self.show_sidebar && self.terminal_width >= MIN_SIDEBAR_WIDTH
    }

    pub fn active_key_status(&self) -> KeyStatus {
        if self.tab().is_acp() {
            return KeyStatus::NotRequired;
        }
        let provider_id = &self.tab().active_provider;
        let provider = {
            let mut rt = self.runtime.write().unwrap();
            rt.active_provider = provider_id.clone();
            rt.active_provider_info()
        };
        self.cached_key_status(&provider)
    }

    pub fn invalidate_key_status(&mut self) {
        self.key_status_cache.get_mut().clear();
    }

    fn cached_key_status(&self, provider: &ProviderInfo) -> KeyStatus {
        if let Some(status) = self.key_status_cache.borrow().get(&provider.id) {
            return status.clone();
        }
        let status = check_key_status(provider, self.secrets.as_ref());
        self.key_status_cache
            .borrow_mut()
            .insert(provider.id.clone(), status.clone());
        status
    }

    pub fn active_provider_default_base_url(&self, provider_id: &str) -> Option<String> {
        let rt = self.runtime.read().unwrap();
        rt.catalog
            .provider(provider_id)
            .and_then(|p| p.default_base_url.clone())
    }

    pub fn filtered_providers(&self, filter: &str) -> Vec<(ProviderInfo, KeyStatus)> {
        let rt = self.runtime.read().unwrap();
        let needle = filter.trim().to_ascii_lowercase();
        rt.catalog
            .providers()
            .iter()
            .filter(|p| {
                needle.is_empty()
                    || p.id.to_ascii_lowercase().contains(&needle)
                    || p.display_name.to_ascii_lowercase().contains(&needle)
            })
            .map(|p| (p.clone(), self.cached_key_status(p)))
            .collect()
    }

    pub fn recent_models_for_active_provider(&self) -> Vec<String> {
        let provider = &self.tab().active_provider;
        let mut models = self.settings.recent_models_for(provider).to_vec();
        let (default_model, catalog_models) = {
            let rt = self.runtime.read().unwrap();
            (
                rt.catalog.default_model_for_provider(provider),
                rt.catalog
                    .models_for_provider(provider)
                    .into_iter()
                    .map(|model| model.id.clone())
                    .collect::<Vec<_>>(),
            )
        };
        for model in catalog_models {
            if !models.contains(&model) {
                models.push(model);
            }
        }
        if let Some(default_model) = default_model
            && !models.contains(&default_model)
        {
            models.push(default_model);
        }
        models
    }

    pub(crate) fn model_candidates(&self, query: &str) -> Vec<String> {
        let models = self.recent_models_for_active_provider();
        let provider = self.tab().active_provider.clone();
        rank_model_matches(&models, &provider, query)
    }

    pub fn current_or_default_model(&self, provider_id: &str) -> Option<String> {
        let canonical = canonical_provider_id(provider_id);
        let tab = self.tab();
        if canonical == tab.active_provider && !tab.model.trim().is_empty() {
            return Some(tab.model.clone());
        }
        if let Some(saved) = self.settings.provider_model(&canonical) {
            return Some(saved.to_string());
        }
        let rt = self.runtime.read().unwrap();
        rt.catalog.default_model_for_provider(&canonical)
    }

    pub(super) fn sync_completion_models(&mut self) {
        let models = self.recent_models_for_active_provider();
        let provider = self.tab().active_provider.clone();
        self.completion.sync_models(&models, &provider);
    }

    pub(super) fn model_context_window(&self, provider: &str, model: &str) -> Option<u64> {
        if self.explicit_context_window.is_some() {
            return None;
        }
        let model = model.rsplit("::").next().unwrap_or(model);
        let rt = self.runtime.read().unwrap();
        rt.catalog
            .model_info(provider, model)
            .and_then(|info| info.context_window)
    }

    pub(super) fn active_model_effort_levels(&self) -> Vec<EffortLevel> {
        let tab = self.tab();
        let provider = tab.active_provider.clone();
        let model = tab.model.clone();
        if model.trim().is_empty() {
            return EffortLevel::ALL.to_vec();
        }
        let allowed = {
            let rt = self.runtime.read().unwrap();
            rt.catalog.effort_levels_for(&provider, &model)
        };
        match allowed {
            Some(levels) if levels.is_empty() => EffortLevel::ALL.to_vec(),
            Some(levels) => levels,
            None => EffortLevel::ALL.to_vec(),
        }
    }
    pub(super) fn resolve_model_target(&self, raw: &str) -> (String, String) {
        let trimmed = raw.trim();
        let active_provider = &self.tab().active_provider;
        let rt = self.runtime.read().unwrap();
        if let Some((prefix, rest)) = trimmed.split_once('/')
            && active_provider != "openrouter"
            && rt.catalog.provider(prefix).is_some()
        {
            (canonical_provider_id(prefix), rest.trim().to_string())
        } else {
            (active_provider.clone(), trimmed.to_string())
        }
    }

    pub(super) fn latest_user_turn(&self) -> Option<TurnId> {
        self.tab()
            .user_turns
            .iter()
            .max_by_key(|(index, _)| *index)
            .map(|(_, turn_id)| turn_id.clone())
    }

    pub fn on_tick(&mut self) {
        self.open_pending_question();
        if self.tabs.iter().any(|tab| tab.is_running) {
            self.spinner_tick = self.spinner_tick.wrapping_add(1);
        }
        self.queue_child_session_refreshes();
        if self
            .last_ctrl_c
            .is_some_and(|t| t.elapsed() >= DOUBLE_CTRL_C_WINDOW)
        {
            self.last_ctrl_c = None;
            if self.status_notice.as_deref() == Some("Press ctrl+c again to exit") {
                self.status_notice = None;
            }
        }
        if self.status_notice != self.notice_seen {
            self.notice_seen = self.status_notice.clone();
            self.notice_set_at = self.status_notice.as_ref().map(|_| Instant::now());
        } else if self
            .notice_set_at
            .is_some_and(|t| t.elapsed() >= NOTICE_TTL)
        {
            self.status_notice = None;
            self.notice_seen = None;
            self.notice_set_at = None;
        }
    }

    pub fn quit(&mut self) {
        if !self.should_quit {
            self.should_quit = true;
            for tab in &self.tabs {
                let _ = tab.cmd_tx.send(AgentCommand::Quit);
            }
        }
    }

    pub fn max_scroll(&self) -> usize {
        self.tab().max_scroll()
    }

    pub fn effective_scroll(&self) -> usize {
        self.tab().effective_scroll()
    }

    pub(super) fn scroll_down(&mut self, lines: usize) {
        let max = self.max_scroll();
        let next = self.effective_scroll().saturating_add(lines).min(max);
        let tab = self.tab_mut();
        tab.scroll_offset = next;
        tab.auto_follow = next >= max;
    }

    pub(super) fn scroll_up(&mut self, lines: usize) {
        let current = self.effective_scroll();
        let tab = self.tab_mut();
        tab.scroll_offset = current.saturating_sub(lines);
        tab.auto_follow = false;
    }

    pub(super) fn scroll_half_page_down(&mut self) {
        let step = (self.tab().viewport_height / 2).max(1);
        self.scroll_down(step);
    }

    pub(super) fn scroll_half_page_up(&mut self) {
        let step = (self.tab().viewport_height / 2).max(1);
        self.scroll_up(step);
    }

    pub(super) fn scroll_to_top(&mut self) {
        let tab = self.tab_mut();
        tab.scroll_offset = 0;
        tab.auto_follow = false;
    }

    pub(super) fn scroll_to_bottom(&mut self) {
        let max = self.max_scroll();
        let tab = self.tab_mut();
        tab.scroll_offset = max;
        tab.auto_follow = true;
    }

    pub fn anchor_rail_visible(&self) -> bool {
        let tab = self.tab();
        self.transcript_anchors.len() >= 2 && tab.total_lines > tab.viewport_height
    }

    pub fn anchor_rail_number_width(&self) -> u16 {
        self.transcript_anchors.len().to_string().len().max(1) as u16
    }

    pub fn anchor_rail_layout(&self, area: Rect) -> AnchorRailLayout {
        let mut layout = AnchorRailLayout::default();
        let turns = self.transcript_anchors.len();
        if area.width == 0 || area.height == 0 || turns == 0 {
            return layout;
        }
        let rows = area.height as usize;
        let current = self.anchor_rail_current_index();
        let (first, last, top_gap, bottom_gap) = numbered_window(turns, current, rows);
        layout.top_gap = top_gap;
        layout.bottom_gap = bottom_gap;
        if top_gap {
            layout.top_line = self.transcript_anchors.first().map(|anchor| anchor.line);
        }
        if bottom_gap {
            layout.bottom_line = self.transcript_anchors.last().map(|anchor| anchor.line);
        }
        let mut row = area.y.saturating_add(u16::from(top_gap));
        for anchor in &self.transcript_anchors[first..last] {
            layout.marks.push(AnchorRailMark {
                row,
                line: anchor.line,
                entry_idx: anchor.entry_idx,
                number: anchor.number,
            });
            row = row.saturating_add(1);
        }
        layout
    }

    fn anchor_rail_current_index(&self) -> usize {
        let top = self.effective_scroll();
        self.transcript_anchors
            .iter()
            .rposition(|anchor| anchor.line <= top)
            .unwrap_or(0)
    }

    pub fn anchor_rail_current_number(&self) -> Option<usize> {
        self.transcript_anchors
            .get(self.anchor_rail_current_index())
            .map(|anchor| anchor.number)
    }

    pub fn anchor_rail_hover_row(&self, column: u16, row: u16) -> Option<u16> {
        let area = self.anchor_rail_area;
        if area.width == 0 || row < area.y || row >= area.y + area.height {
            return None;
        }
        (column >= area.x && column < area.x + area.width).then_some(row)
    }

    pub(super) fn jump_from_anchor_rail(&mut self, row: u16, allow_gaps: bool) {
        let area = self.anchor_rail_area;
        let layout = self.anchor_rail_layout(area);
        let line = layout
            .marks
            .iter()
            .find(|mark| mark.row == row)
            .map(|mark| mark.line)
            .or_else(|| {
                if !allow_gaps {
                    return None;
                }
                if layout.top_gap && row == area.y {
                    layout.top_line
                } else if layout.bottom_gap && row == area.y + area.height.saturating_sub(1) {
                    layout.bottom_line
                } else {
                    None
                }
            });
        let Some(line) = line else {
            return;
        };
        let target = line.min(self.max_scroll());
        let follow = target >= self.max_scroll();
        let tab = self.tab_mut();
        tab.scroll_offset = target;
        tab.auto_follow = follow;
    }
}

fn numbered_window(turns: usize, current: usize, rows: usize) -> (usize, usize, bool, bool) {
    if rows == 0 || turns == 0 {
        return (0, 0, false, false);
    }
    if turns <= rows {
        return (0, turns, false, false);
    }
    if rows <= 2 {
        let current = current.min(turns - 1);
        return (current, current + 1, false, false);
    }
    let mut first = current.saturating_sub(rows / 2);
    let mut last = (first + rows).min(turns);
    first = last.saturating_sub(rows);
    for _ in 0..2 {
        let slots = rows - usize::from(first > 0) - usize::from(last < turns);
        first = current.saturating_sub(slots / 2);
        last = (first + slots).min(turns);
        first = last.saturating_sub(slots);
    }
    (first, last, first > 0, last < turns)
}

pub(super) fn filter_session_summaries<'a>(
    sessions: &'a [SessionSummary],
    filter: &str,
) -> Vec<&'a SessionSummary> {
    let needle = filter.trim().to_ascii_lowercase();
    sessions
        .iter()
        .filter(|summary| summary.parent_session_id.is_none())
        .filter(|summary| {
            if needle.is_empty() {
                return true;
            }
            summary.id.as_str().to_ascii_lowercase().contains(&needle)
                || summary
                    .title
                    .as_deref()
                    .is_some_and(|title| title.to_ascii_lowercase().contains(&needle))
                || summary
                    .model
                    .as_deref()
                    .is_some_and(|model| model.to_ascii_lowercase().contains(&needle))
        })
        .collect()
}
