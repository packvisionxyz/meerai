mod commands;
mod events;
mod input;
mod modals;
#[cfg(test)]
mod tests;

pub(crate) use commands::{MentionKind, mention_is_known, parse_mention_token};
pub use events::{session_usage_from_events, transcript_from_events};
pub use input::{clamp_pos_to_bounds, normalize_selection_in_bounds};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use meerai_agents::events::ApprovalRequest;
use meerai_agents::{AgentCommand, ItemId, TurnId};
use meerai_common::definitions::{AgentDefinition, SkillDefinition};
use meerai_common::settings::AppSettings;
use meerai_policy::ApprovalMode;
pub use meerai_policy::{approval_mode_badge, approval_mode_label, parse_approval_mode};
use meerai_providers::providers::{
    KeyStatus, ProviderInfo, RuntimeProviderState, canonical_provider_id, check_key_status,
};
use meerai_providers::secrets::SecretStore;
use ratatui::layout::Rect;
use tokio::sync::mpsc;

use crate::completion::CompletionEngine;
use crate::diff::FileDiffView;
use crate::search::{SearchMatch, SearchMode, SearchPreview};

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

pub struct AppState {
    pub mode: InputMode,
    pub approval_mode: ApprovalMode,
    pub active_provider: String,
    pub model: String,
    pub workspace_root: PathBuf,
    pub input: String,
    pub cursor: usize,
    pub transcript: Vec<TranscriptEntry>,
    pub completion: CompletionEngine,
    pub modal: Option<ActiveModal>,
    pub runtime: Arc<RwLock<RuntimeProviderState>>,
    pub secrets: Arc<dyn SecretStore>,
    pub settings: AppSettings,
    pub config_dir: Option<PathBuf>,
    pub usage: UsageStats,
    pub metrics: TurnMetrics,
    pub is_running: bool,
    pub turn_id: Option<TurnId>,
    pub active_tool: Option<String>,
    pub spinner_tick: usize,
    pub scroll_offset: usize,
    pub auto_follow: bool,
    pub viewport_height: usize,
    pub total_lines: usize,
    pub show_sidebar: bool,
    pub terminal_width: u16,
    pub file_changes: Vec<FileChangeEntry>,
    pub mcp_servers: Vec<String>,
    pub registered_tools: Vec<String>,
    pub agents: Vec<AgentDefinition>,
    pub skills: Vec<SkillDefinition>,
    pub transcript_inner_area: Rect,
    pub sidebar_changes_inner_area: Rect,
    pub visible_file_change_rows: usize,
    pub thinking_hit_zones: Vec<ThinkingHitZone>,
    pub selection: Option<TextSelection>,
    pub screen_lines: Vec<Vec<String>>,
    pub last_copied_text: Option<String>,
    pub should_quit: bool,
    pub status_notice: Option<String>,
    pending_assistant_idx: Option<usize>,
    assistant_items: HashMap<ItemId, usize>,
    tool_items: HashMap<ItemId, String>,
    pending_g: bool,
    last_ctrl_c: Option<Instant>,
    cmd_tx: mpsc::UnboundedSender<AgentCommand>,
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

        Self {
            mode: InputMode::Insert,
            approval_mode: ApprovalMode::Ask,
            active_provider,
            model,
            workspace_root,
            input: String::new(),
            cursor: 0,
            transcript: Vec::new(),
            completion,
            modal,
            runtime,
            secrets,
            settings,
            config_dir,
            usage: UsageStats::default(),
            metrics: TurnMetrics::default(),
            is_running: false,
            turn_id: None,
            active_tool: None,
            spinner_tick: 0,
            scroll_offset: 0,
            auto_follow: true,
            viewport_height: 20,
            total_lines: 0,
            show_sidebar: true,
            terminal_width: 120,
            file_changes: Vec::new(),
            mcp_servers: Vec::new(),
            registered_tools: Vec::new(),
            agents: Vec::new(),
            skills: Vec::new(),
            transcript_inner_area: Rect::default(),
            sidebar_changes_inner_area: Rect::default(),
            visible_file_change_rows: 0,
            thinking_hit_zones: Vec::new(),
            selection: None,
            screen_lines: Vec::new(),
            last_copied_text: None,
            should_quit: false,
            status_notice: None,
            pending_assistant_idx: None,
            assistant_items: HashMap::new(),
            tool_items: HashMap::new(),
            pending_g: false,
            last_ctrl_c: None,
            cmd_tx,
        }
    }

    pub fn is_sidebar_visible(&self) -> bool {
        self.show_sidebar && self.terminal_width >= MIN_SIDEBAR_WIDTH
    }

    pub fn active_key_status(&self) -> KeyStatus {
        let provider = {
            let rt = self.runtime.read().unwrap();
            rt.active_provider_info()
        };
        check_key_status(&provider, self.secrets.as_ref())
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
            .map(|p| (p.clone(), check_key_status(p, self.secrets.as_ref())))
            .collect()
    }

    pub fn recent_models_for_active_provider(&self) -> Vec<String> {
        let mut models = self
            .settings
            .recent_models_for(&self.active_provider)
            .to_vec();
        let default_model = {
            let rt = self.runtime.read().unwrap();
            rt.catalog.default_model_for_provider(&self.active_provider)
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
        if canonical == self.active_provider && !self.model.trim().is_empty() {
            return Some(self.model.clone());
        }
        if let Some(saved) = self.settings.provider_model(&canonical) {
            return Some(saved.to_string());
        }
        let rt = self.runtime.read().unwrap();
        rt.catalog.default_model_for_provider(&canonical)
    }

    pub(super) fn sync_completion_models(&mut self) {
        let models = self.recent_models_for_active_provider();
        self.completion.sync_models(&models, &self.active_provider);
    }

    pub(super) fn resolve_model_target(&self, raw: &str) -> (String, String) {
        let trimmed = raw.trim();
        let rt = self.runtime.read().unwrap();
        if let Some((prefix, rest)) = trimmed.split_once('/')
            && self.active_provider != "openrouter"
            && rt.catalog.provider(prefix).is_some()
        {
            (canonical_provider_id(prefix), rest.trim().to_string())
        } else {
            (self.active_provider.clone(), trimmed.to_string())
        }
    }

    pub fn on_tick(&mut self) {
        if self.is_running {
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
            let _ = self.cmd_tx.send(AgentCommand::Quit);
        }
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

    pub(super) fn scroll_down(&mut self, lines: usize) {
        let max = self.max_scroll();
        let next = self.effective_scroll().saturating_add(lines).min(max);
        self.scroll_offset = next;
        self.auto_follow = next >= max;
    }

    pub(super) fn scroll_up(&mut self, lines: usize) {
        let current = self.effective_scroll();
        self.scroll_offset = current.saturating_sub(lines);
        self.auto_follow = false;
    }

    pub(super) fn scroll_half_page_down(&mut self) {
        let step = (self.viewport_height / 2).max(1);
        self.scroll_down(step);
    }

    pub(super) fn scroll_half_page_up(&mut self) {
        let step = (self.viewport_height / 2).max(1);
        self.scroll_up(step);
    }

    pub(super) fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
        self.auto_follow = false;
    }

    pub(super) fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.max_scroll();
        self.auto_follow = true;
    }
}
