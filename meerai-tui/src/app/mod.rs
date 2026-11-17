mod commands;
mod events;
mod input;
mod modals;
mod palette;
mod reload;
mod subagent_overlay;
#[cfg(test)]
mod tests;

pub(crate) use commands::{MentionKind, mention_is_known, parse_mention_token};
pub use events::{session_usage_from_events, transcript_from_events, user_turn_map};
pub use input::{clamp_pos_to_bounds, normalize_selection_in_bounds};
pub(crate) use palette::{PaletteAction, filtered_commands};

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use meerai_agents::events::{ApprovalRequest, QuestionAnswer, QuestionPrompt, QuestionRequest};
use meerai_agents::{AgentCommand, ItemId, TurnId};
use meerai_client::providers::{
    KeyStatus, ProviderInfo, RuntimeProviderState, canonical_provider_id, check_key_status,
};
use meerai_client::secrets::SecretStore;
use meerai_common::definitions::{AgentDefinition, SkillDefinition};
use meerai_common::settings::AppSettings;
use meerai_core::text::truncate_single_line;
use meerai_policy::ApprovalMode;
pub use meerai_policy::{approval_mode_badge, approval_mode_label, parse_approval_mode};
use meerai_session::{LoggedEvent, SessionId, SessionSummary};
use ratatui::layout::Rect;
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

#[derive(Clone, Debug, PartialEq)]
pub enum ActiveModal {
    ProviderSelect {
        filter: String,
        selected: usize,
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
        selected_recent: Option<usize>,
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
        command: String,
        output: Option<String>,
        running: bool,
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingSessionAction {
    RefreshSessions,
    OpenSessionId(SessionId),
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
        command: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabHitKind {
    Tab(usize),
    CloseTab(usize),
    NewTab,
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
    ApprovalAction(ApprovalClickAction),
    SearchModeToggle,
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModalHitZone {
    pub rect: Rect,
    pub kind: ModalHitKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubagentOverlay {
    pub selected: usize,
    pub entries: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompletionHitZone {
    pub rect: Rect,
    pub index: usize,
}

pub use meerai_session::replay::ToolCallStatus;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallEntry {
    pub call_id: String,
    pub name: String,
    pub args: String,
    pub status: ToolCallStatus,
    pub output: Option<String>,
    pub expanded: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubagentEntry {
    pub id: String,
    pub agent: String,
    pub status: ToolCallStatus,
    pub report: Option<String>,
    pub child_session_id: Option<String>,
    pub expanded: bool,
    pub nested: Option<Vec<TranscriptEntry>>,
    pub nested_refreshed_at: Option<Instant>,
    pub nested_loading: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChangeEntry {
    pub status: String,
    pub path: String,
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
    pub soft_wrapped_next: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThinkingHitZone {
    pub entry_idx: usize,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
}

#[derive(Clone, Debug, Default)]
pub struct TurnMetrics {
    pub turn_count: usize,
    pub session_prompt_tokens: u64,
    pub session_completion_tokens: u64,
    pub session_total_tokens: u64,
    pub session_cached_tokens: u64,
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
    pub is_running: bool,
    pub queued: Vec<String>,
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
            is_running: false,
            queued: Vec::new(),
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
                size: subagent.agent.len()
                    + subagent.report.as_ref().map_or(0, |report| report.len()),
                state: EntryState::Subagent {
                    status: subagent.status,
                    expanded: subagent.expanded,
                    loading: subagent.nested_loading,
                    nested_entries: nested_size(&subagent.nested),
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

    pub(crate) fn markdown(&mut self, index: usize, text: &str) -> Rc<Vec<MarkdownLine>> {
        self.markdown.get(index, memo_hash(None, &[text]), || {
            crate::markdown::render(text)
        })
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
    pub explicit_context_window: Option<u64>,
    pub config_dir: Option<PathBuf>,
    pub spinner_tick: usize,
    pub show_sidebar: bool,
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
    pub subagent_overlay: Option<SubagentOverlay>,
    pub subagent_overlay_zones: Vec<(Rect, usize)>,
    pub subagent_hit: Option<Rect>,
    pub user_shell_running: Option<String>,
    pub thinking_hit_zones: Vec<ThinkingHitZone>,
    pub digest_hit_zones: Vec<ThinkingHitZone>,
    pub hover_zones: Vec<HoverZone>,
    pub hovered_entry: Option<usize>,
    pub hovered_digest: Option<usize>,
    pub modal_hit_zones: Vec<ModalHitZone>,
    pub hovered_modal_hit: Option<ModalHitKind>,
    pub completion_hit_zones: Vec<CompletionHitZone>,
    pub hovered_completion: Option<usize>,
    pub update_available: Option<String>,
    pub launch_system: String,
    pub launch_definitions: String,
    pub(super) pending_reload: bool,
    pub(super) session_delete_confirm: Option<SessionId>,
    pub(crate) tab_persistence: bool,
    pub update_hit: Option<Rect>,
    pub composer_block_area: Rect,
    pub composer_input_area: Rect,
    pub composer_scroll: usize,
    pub composer_selection: Option<(usize, usize)>,
    pub(super) composer_drag_anchor: Option<usize>,
    pub(super) tab_drag: Option<TabDrag>,
    pub(super) pending_toggle: Option<usize>,
    pub(super) pending_digest: Option<usize>,
    pub selection: Option<TextSelection>,
    pub doc_selection: Option<DocSelection>,
    pub last_transcript_anchor: Option<DocPos>,
    pub last_mouse_pos: Option<ScreenPos>,
    pub transcript_wrapped_lines: Vec<TranscriptWrappedLine>,
    pub(crate) transcript_cache: TranscriptRenderCache,
    pub screen_lines: Vec<Vec<String>>,
    pub last_copied_text: Option<String>,
    key_status_cache: RefCell<HashMap<String, KeyStatus>>,
    pub should_quit: bool,
    pub status_notice: Option<String>,
    notice_seen: Option<String>,
    notice_set_at: Option<Instant>,
    pub session_opener: Option<Arc<dyn SessionOpener>>,
    pub(super) pending_session_actions: Vec<PendingSessionAction>,
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
            if matches!(status, KeyStatus::Missing) {
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
                selected_recent: None,
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
            explicit_context_window: None,
            config_dir,
            spinner_tick: 0,
            show_sidebar: true,
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
            subagent_overlay: None,
            subagent_overlay_zones: Vec::new(),
            subagent_hit: None,
            user_shell_running: None,
            hovered_file_change: None,
            thinking_hit_zones: Vec::new(),
            digest_hit_zones: Vec::new(),
            hover_zones: Vec::new(),
            hovered_entry: None,
            hovered_digest: None,
            modal_hit_zones: Vec::new(),
            hovered_modal_hit: None,
            completion_hit_zones: Vec::new(),
            hovered_completion: None,
            update_available: None,
            launch_system: String::new(),
            launch_definitions: String::new(),
            pending_reload: false,
            session_delete_confirm: None,
            tab_persistence: false,
            update_hit: None,
            composer_block_area: Rect::default(),
            composer_input_area: Rect::default(),
            composer_scroll: 0,
            composer_selection: None,
            composer_drag_anchor: None,
            tab_drag: None,
            pending_toggle: None,
            pending_digest: None,
            selection: None,
            doc_selection: None,
            last_transcript_anchor: None,
            last_mouse_pos: None,
            transcript_wrapped_lines: Vec::new(),
            transcript_cache: TranscriptRenderCache::default(),
            screen_lines: Vec::new(),
            last_copied_text: None,
            key_status_cache: RefCell::new(HashMap::new()),
            should_quit: false,
            status_notice: None,
            notice_seen: None,
            notice_set_at: None,
            session_opener: None,
            pending_session_actions: Vec::new(),
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
        crate::tabs::save(&ids, self.active_tab, &self.workspace_root);
    }

    fn reset_ephemeral_ui_state(&mut self) {
        self.completion.reset();
        self.thinking_hit_zones.clear();
        self.digest_hit_zones.clear();
        self.selection = None;
        self.doc_selection = None;
        self.last_transcript_anchor = None;
        self.transcript_wrapped_lines.clear();
        self.transcript_cache.clear();
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
        if self.tabs.len() <= 1 || index >= self.tabs.len() {
            if self.tabs.len() <= 1 {
                self.status_notice = Some("Cannot close the last tab (use /quit to exit)".into());
            }
            return false;
        }

        if index == self.active_tab && matches!(self.modal, Some(ActiveModal::Approval { .. })) {
            self.modal = None;
        }

        let removed = self.tabs.remove(index);
        let _ = removed.cmd_tx.send(AgentCommand::Quit);

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
        replace_empty_tab: bool,
    ) -> Option<(u64, mpsc::UnboundedReceiver<meerai_agents::AgentEvent>)> {
        if let Some(existing_idx) = self.tab_index_for_session(&opened.session_id) {
            let _ = opened.cmd_tx.send(AgentCommand::Quit);
            self.switch_to_tab(existing_idx);
            self.status_notice = Some(format!("Switched to open tab {}", existing_idx + 1));
            return None;
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
        tab.transcript = opened.transcript;
        tab.user_turns = user_turn_map(&tab.transcript, &opened.events);
        tab.seed_usage_from_events(&opened.events);

        if replace_empty_tab && self.should_replace_active_tab() {
            if matches!(self.modal, Some(ActiveModal::Approval { .. })) {
                self.modal = None;
            }
            let old = std::mem::replace(&mut self.tabs[0], tab);
            let _ = old.cmd_tx.send(AgentCommand::Quit);
            self.active_tab = 0;
            self.reset_ephemeral_ui_state();
            self.sync_active_provider_runtime();
            self.sync_completion_models();
            self.open_pending_question();
            self.persist_tabs();
        } else {
            self.tabs.push(tab);
            let new_idx = self.tabs.len() - 1;
            self.switch_to_tab(new_idx);
        }

        self.status_notice = Some(format!("Session {}", self.tab().session_id));
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
        let default_model = {
            let rt = self.runtime.read().unwrap();
            rt.catalog.default_model_for_provider(provider)
        };
        if let Some(default_model) = default_model
            && !models.contains(&default_model)
        {
            models.push(default_model);
        }
        models
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
