mod commands;
mod events;
mod input;
mod modals;
mod palette;
#[cfg(test)]
mod tests;

pub(crate) use commands::{MentionKind, mention_is_known, parse_mention_token};
pub use events::{session_usage_from_events, transcript_from_events};
pub use input::{clamp_pos_to_bounds, normalize_selection_in_bounds};
pub(crate) use palette::{PaletteAction, filtered_commands};

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use meerai_agents::events::ApprovalRequest;
use meerai_agents::{AgentCommand, ItemId, TurnId};
use meerai_common::definitions::{AgentDefinition, SkillDefinition};
use meerai_common::settings::AppSettings;
use meerai_core::text::truncate_single_line;
use meerai_policy::ApprovalMode;
pub use meerai_policy::{approval_mode_badge, approval_mode_label, parse_approval_mode};
use meerai_providers::providers::{
    KeyStatus, ProviderInfo, RuntimeProviderState, canonical_provider_id, check_key_status,
};
use meerai_providers::secrets::SecretStore;
use meerai_session::{LoggedEvent, SessionId, SessionSummary};
use ratatui::layout::Rect;
use tokio::sync::mpsc;

use crate::completion::CompletionEngine;
use crate::diff::FileDiffView;
use crate::search::{SearchMatch, SearchMode, SearchPreview};
use crate::{OpenedSession, SessionOpener};

const DOUBLE_CTRL_C_WINDOW: Duration = Duration::from_secs(2);
pub const MIN_SIDEBAR_WIDTH: u16 = 110;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputMode {
    Insert,
    Normal,
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingSessionAction {
    RefreshSessions,
    OpenSessionId(SessionId),
    OpenSessionQuery(String),
    CreateNewTab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabHitKind {
    Tab(usize),
    NewTab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabHitZone {
    pub rect: Rect,
    pub kind: TabHitKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolCallStatus {
    Proposed,
    Running,
    Succeeded,
    Failed,
    Stopped,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallEntry {
    pub call_id: String,
    pub name: String,
    pub args: String,
    pub status: ToolCallStatus,
    pub output: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubagentEntry {
    pub id: String,
    pub agent: String,
    pub status: ToolCallStatus,
    pub report: Option<String>,
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
}

#[derive(Clone, Debug, Default)]
pub struct TurnMetrics {
    pub turn_count: usize,
    pub session_prompt_tokens: u64,
    pub session_completion_tokens: u64,
    pub session_total_tokens: u64,
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
    pub active_provider: String,
    pub model: String,
    pub approval_mode: ApprovalMode,
    pub input: String,
    pub cursor: usize,
    pub transcript: Vec<TranscriptEntry>,
    pub usage: UsageStats,
    pub metrics: TurnMetrics,
    pub is_running: bool,
    pub queued: Vec<String>,
    pub turn_id: Option<TurnId>,
    pub active_tool: Option<String>,
    pub scroll_offset: usize,
    pub auto_follow: bool,
    pub viewport_height: usize,
    pub total_lines: usize,
    pub has_unread: bool,
    pub pending_approvals: Vec<ApprovalRequest>,
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
            active_provider: canonical_provider_id(&active_provider.into()),
            model: model.into(),
            approval_mode,
            input: String::new(),
            cursor: 0,
            transcript: Vec::new(),
            usage: UsageStats::default(),
            metrics: TurnMetrics::default(),
            is_running: false,
            queued: Vec::new(),
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

pub struct AppState {
    pub mode: InputMode,
    pub workspace_root: PathBuf,
    pub tabs: Vec<SessionTab>,
    pub active_tab: usize,
    pub completion: CompletionEngine,
    pub modal: Option<ActiveModal>,
    pub runtime: Arc<RwLock<RuntimeProviderState>>,
    pub secrets: Arc<dyn SecretStore>,
    pub settings: AppSettings,
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
    pub transcript_inner_area: Rect,
    pub sidebar_changes_inner_area: Rect,
    pub visible_file_change_rows: usize,
    pub thinking_hit_zones: Vec<ThinkingHitZone>,
    pub selection: Option<TextSelection>,
    pub doc_selection: Option<DocSelection>,
    pub last_transcript_anchor: Option<DocPos>,
    pub last_mouse_pos: Option<ScreenPos>,
    pub transcript_wrapped_lines: Vec<TranscriptWrappedLine>,
    pub screen_lines: Vec<Vec<String>>,
    pub last_copied_text: Option<String>,
    key_status_cache: RefCell<HashMap<String, KeyStatus>>,
    pub should_quit: bool,
    pub status_notice: Option<String>,
    pub session_opener: Option<Arc<dyn SessionOpener>>,
    pub(super) pending_session_actions: Vec<PendingSessionAction>,
    next_tab_id: u64,
    pending_g: bool,
    last_ctrl_c: Option<Instant>,
}

impl AppState {
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
            workspace_root,
            tabs: vec![tab],
            active_tab: 0,
            completion,
            modal,
            runtime,
            secrets,
            settings,
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
            transcript_inner_area: Rect::default(),
            sidebar_changes_inner_area: Rect::default(),
            visible_file_change_rows: 0,
            thinking_hit_zones: Vec::new(),
            selection: None,
            doc_selection: None,
            last_transcript_anchor: None,
            last_mouse_pos: None,
            transcript_wrapped_lines: Vec::new(),
            screen_lines: Vec::new(),
            last_copied_text: None,
            key_status_cache: RefCell::new(HashMap::new()),
            should_quit: false,
            status_notice: None,
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
        transcript: Vec<TranscriptEntry>,
        events: &[LoggedEvent],
        approval_mode: ApprovalMode,
    ) {
        let tab = self.tab_mut();
        tab.session_id = session_id;
        tab.title = title.and_then(normalize_tab_title);
        tab.approval_mode = approval_mode;
        tab.transcript = transcript;
        tab.seed_usage_from_events(events);
    }

    fn reset_ephemeral_ui_state(&mut self) {
        self.completion.reset();
        self.thinking_hit_zones.clear();
        self.selection = None;
        self.doc_selection = None;
        self.last_transcript_anchor = None;
        self.transcript_wrapped_lines.clear();
        self.pending_g = false;
    }

    pub fn should_replace_active_tab(&self) -> bool {
        self.tabs.len() == 1 && !self.tab().has_conversation()
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

        let provider = self.tabs[index].active_provider.clone();
        if let Ok(mut rt) = self.runtime.write() {
            rt.active_provider = provider;
        }
        self.sync_completion_models();

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

        let provider = self.tabs[self.active_tab].active_provider.clone();
        if let Ok(mut rt) = self.runtime.write() {
            rt.active_provider = provider;
        }
        self.sync_completion_models();

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
        tab.transcript = opened.transcript;
        tab.seed_usage_from_events(&opened.events);

        if replace_empty_tab && self.should_replace_active_tab() {
            if matches!(self.modal, Some(ActiveModal::Approval { .. })) {
                self.modal = None;
            }
            let old = std::mem::replace(&mut self.tabs[0], tab);
            let _ = old.cmd_tx.send(AgentCommand::Quit);
            self.active_tab = 0;
            self.reset_ephemeral_ui_state();
            let provider = self.tabs[0].active_provider.clone();
            if let Ok(mut rt) = self.runtime.write() {
                rt.active_provider = provider;
            }
            self.sync_completion_models();
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

    pub fn on_tick(&mut self) {
        if self.tabs.iter().any(|tab| tab.is_running) {
            self.spinner_tick = self.spinner_tick.wrapping_add(1);
        }
        if self
            .last_ctrl_c
            .is_some_and(|t| t.elapsed() >= DOUBLE_CTRL_C_WINDOW)
        {
            self.last_ctrl_c = None;
            if self.status_notice.as_deref() == Some("Press Ctrl-c again to exit") {
                self.status_notice = None;
            }
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
