use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use meerai_agents::events::{ApprovalDecision, ApprovalRequest, GrantScope};
use meerai_agents::{
    AgentCommand, AgentEvent, ErrorCode, ItemDelta, ItemId, ItemStatus, SessionItem, TurnEndReason,
    TurnId,
};
use meerai_common::definitions::{AgentDefinition, DefinitionSource, SkillDefinition};
use meerai_common::providers::{
    KeyStatus, ProviderInfo, RuntimeProviderState, canonical_provider_id, check_key_status,
    format_auth_list,
};
use meerai_common::secrets::{SecretStore, auth_file_path};
use meerai_common::settings::{AppSettings, default_data_dir, save_settings_to};
use meerai_core::ObservationStatus;
use meerai_policy::ApprovalMode;
use meerai_session::{ContentBlock, InputSource, LoggedEvent, SessionEvent};
use ratatui::layout::Rect;
use tokio::sync::mpsc;

use crate::completion::{CompletionEngine, CompletionKind};
use crate::diff::{FileDiffView, build_file_diff};
use crate::search::{
    SearchMatch, SearchMode, SearchPreview, build_search_preview, discover_workspace_files,
    search_content, search_files,
};

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

    fn sync_completion_models(&mut self) {
        let models = self.recent_models_for_active_provider();
        self.completion.sync_models(&models, &self.active_provider);
    }

    fn resolve_model_target(&self, raw: &str) -> (String, String) {
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

    pub fn handle_terminal_event(&mut self, event: Event) -> bool {
        match event {
            Event::Key(key) => self.handle_key_event(key),
            Event::Paste(text) => {
                if let Some(modal) = &mut self.modal {
                    let clean = text.replace(['\r', '\n'], "");
                    match modal {
                        ActiveModal::ProviderSelect { filter, selected } => {
                            filter.push_str(&clean);
                            *selected = 0;
                        }
                        ActiveModal::ApiKeyInput {
                            api_key,
                            base_url,
                            editing_base_url,
                            error,
                            ..
                        } => {
                            if *editing_base_url {
                                base_url.push_str(&clean);
                            } else {
                                api_key.push_str(&clean);
                            }
                            *error = None;
                        }
                        ActiveModal::ModelInput {
                            input,
                            selected_recent,
                            error,
                        } => {
                            input.push_str(&clean);
                            *selected_recent = None;
                            *error = None;
                        }
                        ActiveModal::DiffView { .. } => {}
                        ActiveModal::Approval {
                            reason,
                            editing_reason,
                            ..
                        } => {
                            if *editing_reason {
                                reason.push_str(&clean);
                            }
                        }
                        ActiveModal::Search {
                            query,
                            mode,
                            workspace_files,
                            results,
                            selected,
                            preview,
                        } => {
                            query.push_str(&clean);
                            Self::refresh_search(
                                &self.workspace_root,
                                *mode,
                                workspace_files,
                                query,
                                results,
                                selected,
                                preview,
                            );
                        }
                    }
                    return true;
                }
                if self.mode == InputMode::Insert {
                    self.insert_str(&text);
                    self.update_completion();
                    return true;
                }
                false
            }
            Event::Mouse(mouse) => self.handle_mouse_event(mouse),
            Event::Resize(w, _) => {
                self.terminal_width = w;
                true
            }
            _ => false,
        }
    }

    pub fn handle_mouse_event(&mut self, mouse: MouseEvent) -> bool {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.modal.is_none() && self.click_file_change_at(mouse.column, mouse.row) {
                    self.selection = None;
                    return true;
                }
                let pos = ScreenPos {
                    col: mouse.column,
                    row: mouse.row,
                };
                self.selection = Some(TextSelection {
                    anchor: pos,
                    head: pos,
                    active: true,
                });
                if self.modal.is_none() {
                    let _ = self.toggle_thinking_at(mouse.column, mouse.row);
                }
                return true;
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(sel) = &mut self.selection {
                    sel.head = ScreenPos {
                        col: mouse.column,
                        row: mouse.row,
                    };
                    sel.active = true;
                    return true;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(mut sel) = self.selection.take() {
                    sel.head = ScreenPos {
                        col: mouse.column,
                        row: mouse.row,
                    };
                    sel.active = false;
                    if sel.anchor != sel.head {
                        let text = self.extract_selected_text(sel);
                        if !text.is_empty() {
                            self.copy_text_to_clipboard(&text);
                        }
                    }
                    return true;
                }
            }
            _ => {}
        }

        if let Some(ActiveModal::DiffView { diff, scroll, .. }) = &mut self.modal {
            let max_scroll = diff.rows.len().saturating_sub(1);
            return match mouse.kind {
                MouseEventKind::ScrollUp => {
                    *scroll = scroll.saturating_sub(3);
                    true
                }
                MouseEventKind::ScrollDown => {
                    *scroll = (*scroll + 3).min(max_scroll);
                    true
                }
                _ => false,
            };
        }
        if self.modal.is_some() {
            return false;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_up(3);
                true
            }
            MouseEventKind::ScrollDown => {
                self.scroll_down(3);
                true
            }
            _ => false,
        }
    }

    pub fn selection_bounds(&self, anchor: ScreenPos) -> Rect {
        let inner = self.transcript_inner_area;
        if self.modal.is_none()
            && inner.width > 0
            && inner.height > 0
            && anchor.col >= inner.x
            && anchor.col < inner.x + inner.width
            && anchor.row >= inner.y
            && anchor.row < inner.y + inner.height
        {
            inner
        } else {
            Rect::new(0, 0, self.terminal_width, self.screen_lines.len() as u16)
        }
    }

    pub fn extract_selected_text(&self, sel: TextSelection) -> String {
        if self.screen_lines.is_empty() {
            return String::new();
        }
        let bounds = self.selection_bounds(sel.anchor);
        let Some((start, end)) = normalize_selection_in_bounds(sel.anchor, sel.head, bounds) else {
            return String::new();
        };
        let max_screen_row = self.screen_lines.len().saturating_sub(1) as u16;
        let row_start = start.row.min(max_screen_row);
        let row_end = end.row.min(max_screen_row);
        if row_start > row_end {
            return String::new();
        }

        let min_col = bounds.x;
        let max_col = bounds.x + bounds.width - 1;
        let in_transcript =
            bounds == self.transcript_inner_area && self.transcript_inner_area.width > 0;
        let mut lines = Vec::new();
        for row in row_start..=row_end {
            let line_cells = &self.screen_lines[row as usize];
            if line_cells.is_empty() {
                lines.push(String::new());
                continue;
            }
            let c_start = (if row == row_start { start.col } else { min_col })
                .max(min_col)
                .min(max_col) as usize;
            let c_end = (if row == row_end { end.col } else { max_col })
                .max(min_col)
                .min(max_col) as usize;
            if c_start >= line_cells.len() || c_start > c_end {
                lines.push(String::new());
                continue;
            }
            let clamped_end = c_end.min(line_cells.len() - 1);
            let mut raw_line: String = line_cells[c_start..=clamped_end]
                .iter()
                .map(String::as_str)
                .collect();
            if in_transcript && c_start == min_col as usize {
                raw_line = raw_line
                    .strip_prefix("┃ ")
                    .map(str::to_string)
                    .unwrap_or(raw_line);
            }
            lines.push(raw_line.trim_end().to_string());
        }

        while lines.first().is_some_and(String::is_empty) {
            lines.remove(0);
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("\n")
    }

    pub fn copy_text_to_clipboard(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        write_system_clipboard(text);
        self.last_copied_text = Some(text.to_string());
        self.status_notice = Some(format!(
            "Copied {} chars to clipboard",
            text.chars().count()
        ));
    }

    pub fn click_file_change_at(&mut self, column: u16, row: u16) -> bool {
        if !self.is_sidebar_visible() {
            return false;
        }
        let area = self.sidebar_changes_inner_area;
        if area.width == 0
            || area.height == 0
            || column < area.x
            || column >= area.x + area.width
            || row < area.y
            || row >= area.y + area.height
        {
            return false;
        }
        let row_idx = (row - area.y) as usize;
        if row_idx >= self.visible_file_change_rows || row_idx >= self.file_changes.len() {
            return false;
        }
        self.open_diff_modal(row_idx);
        true
    }

    pub fn open_diff_modal(&mut self, file_index: usize) {
        let Some(entry) = self.file_changes.get(file_index) else {
            return;
        };
        let diff = build_file_diff(&self.workspace_root, &entry.path, &entry.status);
        self.modal = Some(ActiveModal::DiffView {
            file_index,
            diff,
            scroll: 0,
        });
    }

    pub fn open_search_modal(&mut self, mode: SearchMode, initial_query: &str) {
        let workspace_files = discover_workspace_files(&self.workspace_root);
        let query = initial_query.trim().to_string();
        let mut results = Vec::new();
        let mut selected = 0;
        let mut preview = None;
        Self::refresh_search(
            &self.workspace_root,
            mode,
            &workspace_files,
            &query,
            &mut results,
            &mut selected,
            &mut preview,
        );
        self.modal = Some(ActiveModal::Search {
            query,
            mode,
            workspace_files,
            results,
            selected,
            preview,
        });
    }

    fn refresh_search(
        workspace_root: &Path,
        mode: SearchMode,
        workspace_files: &[String],
        query: &str,
        results: &mut Vec<SearchMatch>,
        selected: &mut usize,
        preview: &mut Option<SearchPreview>,
    ) {
        *results = match mode {
            SearchMode::Files => search_files(workspace_files, query),
            SearchMode::Content => search_content(workspace_root, workspace_files, query),
        };
        *selected = 0;
        *preview = results
            .first()
            .map(|item| build_search_preview(workspace_root, item, 20));
    }

    pub fn toggle_thinking_at(&mut self, column: u16, row: u16) -> bool {
        let area = self.transcript_inner_area;
        if area.width == 0
            || area.height == 0
            || column < area.x
            || column >= area.x + area.width
            || row < area.y
            || row >= area.y + area.height
        {
            return false;
        }
        let virtual_line = self
            .effective_scroll()
            .saturating_add((row - area.y) as usize);
        let Some(entry_idx) = self
            .thinking_hit_zones
            .iter()
            .find(|z| virtual_line >= z.start_line && virtual_line < z.end_line)
            .map(|z| z.entry_idx)
        else {
            return false;
        };
        match self.transcript.get_mut(entry_idx) {
            Some(TranscriptEntry::Assistant {
                reasoning,
                reasoning_expanded,
                ..
            }) if !reasoning.is_empty() => {
                *reasoning_expanded = !*reasoning_expanded;
                true
            }
            Some(TranscriptEntry::Error { expanded, .. }) => {
                *expanded = !*expanded;
                true
            }
            _ => false,
        }
    }

    pub fn toggle_latest_thinking(&mut self) -> bool {
        for entry in self.transcript.iter_mut().rev() {
            match entry {
                TranscriptEntry::Assistant {
                    reasoning,
                    reasoning_expanded,
                    ..
                } if !reasoning.is_empty() => {
                    *reasoning_expanded = !*reasoning_expanded;
                    return true;
                }
                TranscriptEntry::Error { expanded, .. } => {
                    *expanded = !*expanded;
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            self.pending_g = false;
            self.handle_ctrl_c();
            return true;
        }

        self.last_ctrl_c = None;
        self.status_notice = None;

        if self.modal.is_some() {
            return self.handle_modal_key(key);
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('t') => {
                    return self.toggle_latest_thinking();
                }
                KeyCode::Char('b') => {
                    self.toggle_sidebar();
                    return true;
                }
                KeyCode::Char('f') => {
                    self.open_search_modal(SearchMode::Files, "");
                    return true;
                }
                _ => {}
            }
        }

        match self.mode {
            InputMode::Insert => self.handle_insert_key(key),
            InputMode::Normal => self.handle_normal_key(key),
        }
    }

    fn handle_ctrl_c(&mut self) {
        if self.is_running {
            let _ = self.cmd_tx.send(AgentCommand::Cancel);
            self.status_notice = Some("Cancelling turn...".to_string());
            self.last_ctrl_c = None;
            return;
        }

        if self
            .last_ctrl_c
            .is_some_and(|t| t.elapsed() < DOUBLE_CTRL_C_WINDOW)
        {
            self.quit();
        } else {
            self.last_ctrl_c = Some(Instant::now());
            self.status_notice = Some("Press Ctrl-c again to exit".to_string());
        }
    }

    fn handle_modal_key(&mut self, key: KeyEvent) -> bool {
        let Some(modal) = self.modal.take() else {
            return false;
        };
        let has_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let has_alt = key.modifiers.contains(KeyModifiers::ALT);

        match modal {
            ActiveModal::ProviderSelect {
                mut filter,
                mut selected,
            } => {
                let providers = self.filtered_providers(&filter);
                match key.code {
                    KeyCode::Esc => {
                        self.modal = None;
                    }
                    KeyCode::Up | KeyCode::Char('p') if key.code == KeyCode::Up || has_ctrl => {
                        if !providers.is_empty() {
                            selected = if selected == 0 {
                                providers.len() - 1
                            } else {
                                selected - 1
                            };
                        }
                        self.modal = Some(ActiveModal::ProviderSelect { filter, selected });
                    }
                    KeyCode::Down | KeyCode::Char('n') if key.code == KeyCode::Down || has_ctrl => {
                        if !providers.is_empty() {
                            selected = (selected + 1) % providers.len();
                        }
                        self.modal = Some(ActiveModal::ProviderSelect { filter, selected });
                    }
                    KeyCode::Backspace => {
                        filter.pop();
                        selected = 0;
                        self.modal = Some(ActiveModal::ProviderSelect { filter, selected });
                    }
                    KeyCode::Enter => {
                        if let Some((provider, status)) = providers.get(selected) {
                            if !provider.requires_api_key {
                                self.activate_provider_only(&provider.id);
                                self.open_model_input();
                            } else {
                                let saved_url = {
                                    let rt = self.runtime.read().unwrap();
                                    rt.base_urls.get(&provider.id).cloned().unwrap_or_default()
                                };
                                self.modal = Some(ActiveModal::ApiKeyInput {
                                    provider_id: provider.id.clone(),
                                    provider_name: provider.display_name.clone(),
                                    api_key: String::new(),
                                    base_url: saved_url,
                                    editing_base_url: false,
                                    existing_key_status: status.clone(),
                                    error: None,
                                });
                            }
                        } else {
                            self.modal = Some(ActiveModal::ProviderSelect { filter, selected });
                        }
                    }
                    KeyCode::Char(c) if !has_ctrl && !has_alt => {
                        filter.push(c);
                        selected = 0;
                        self.modal = Some(ActiveModal::ProviderSelect { filter, selected });
                    }
                    _ => {
                        self.modal = Some(ActiveModal::ProviderSelect { filter, selected });
                    }
                }
            }
            ActiveModal::ApiKeyInput {
                provider_id,
                provider_name,
                mut api_key,
                mut base_url,
                mut editing_base_url,
                mut existing_key_status,
                mut error,
            } => match key.code {
                KeyCode::Esc => {
                    let selected = {
                        let rt = self.runtime.read().unwrap();
                        rt.catalog
                            .providers()
                            .iter()
                            .position(|p| p.id == provider_id)
                            .unwrap_or(0)
                    };
                    self.modal = Some(ActiveModal::ProviderSelect {
                        filter: String::new(),
                        selected,
                    });
                }
                KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => {
                    editing_base_url = !editing_base_url;
                    self.modal = Some(ActiveModal::ApiKeyInput {
                        provider_id,
                        provider_name,
                        api_key,
                        base_url,
                        editing_base_url,
                        existing_key_status,
                        error,
                    });
                }
                KeyCode::Char('d') if has_ctrl => {
                    match self.secrets.delete_key(&provider_id) {
                        Ok(()) => {
                            api_key.clear();
                            let provider_info = {
                                let rt = self.runtime.read().unwrap();
                                rt.catalog.provider(&provider_id).cloned()
                            };
                            if let Some(info) = provider_info {
                                existing_key_status =
                                    check_key_status(&info, self.secrets.as_ref());
                            } else {
                                existing_key_status = KeyStatus::Missing;
                            }
                            error = None;
                            self.status_notice =
                                Some(format!("Removed stored key for {provider_id}"));
                        }
                        Err(err) => {
                            error = Some(format!("{err:#}"));
                        }
                    }
                    self.modal = Some(ActiveModal::ApiKeyInput {
                        provider_id,
                        provider_name,
                        api_key,
                        base_url,
                        editing_base_url,
                        existing_key_status,
                        error,
                    });
                }
                KeyCode::Backspace => {
                    if editing_base_url {
                        base_url.pop();
                    } else {
                        api_key.pop();
                    }
                    error = None;
                    self.modal = Some(ActiveModal::ApiKeyInput {
                        provider_id,
                        provider_name,
                        api_key,
                        base_url,
                        editing_base_url,
                        existing_key_status,
                        error,
                    });
                }
                KeyCode::Enter => {
                    let trimmed_key = api_key.trim();
                    if !trimmed_key.is_empty() {
                        if let Err(err) = self.secrets.set_key(&provider_id, trimmed_key) {
                            self.modal = Some(ActiveModal::ApiKeyInput {
                                provider_id,
                                provider_name,
                                api_key,
                                base_url,
                                editing_base_url,
                                existing_key_status,
                                error: Some(format!("{err:#}")),
                            });
                            return true;
                        }
                    } else if !existing_key_status.is_configured() {
                        self.modal = Some(ActiveModal::ApiKeyInput {
                            provider_id,
                            provider_name,
                            api_key,
                            base_url,
                            editing_base_url,
                            existing_key_status,
                            error: Some("API key is required".to_string()),
                        });
                        return true;
                    }

                    let clean_url = base_url.trim().to_string();
                    {
                        let mut rt = self.runtime.write().unwrap();
                        if clean_url.is_empty() {
                            rt.base_urls.remove(&provider_id);
                        } else {
                            rt.base_urls.insert(provider_id.clone(), clean_url.clone());
                        }
                    }
                    self.settings.set_provider_base_url(
                        &provider_id,
                        (!clean_url.is_empty()).then_some(clean_url),
                    );

                    self.activate_provider_only(&provider_id);
                    self.status_notice = Some(format!("Connected {provider_id}"));
                    self.open_model_input();
                }
                KeyCode::Char(c) if !has_ctrl && !has_alt => {
                    if editing_base_url {
                        base_url.push(c);
                    } else {
                        api_key.push(c);
                    }
                    error = None;
                    self.modal = Some(ActiveModal::ApiKeyInput {
                        provider_id,
                        provider_name,
                        api_key,
                        base_url,
                        editing_base_url,
                        existing_key_status,
                        error,
                    });
                }
                _ => {
                    self.modal = Some(ActiveModal::ApiKeyInput {
                        provider_id,
                        provider_name,
                        api_key,
                        base_url,
                        editing_base_url,
                        existing_key_status,
                        error,
                    });
                }
            },
            ActiveModal::ModelInput {
                mut input,
                mut selected_recent,
                mut error,
            } => {
                let recent = self.recent_models_for_active_provider();
                match key.code {
                    KeyCode::Esc => {
                        self.modal = None;
                    }
                    KeyCode::Char('u') if has_ctrl => {
                        input.clear();
                        selected_recent = None;
                        error = None;
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected_recent,
                            error,
                        });
                    }
                    KeyCode::Up | KeyCode::Char('p') if key.code == KeyCode::Up || has_ctrl => {
                        if !recent.is_empty() {
                            let next = match selected_recent {
                                None | Some(0) => recent.len() - 1,
                                Some(idx) => idx.saturating_sub(1),
                            };
                            input = recent[next].clone();
                            selected_recent = Some(next);
                            error = None;
                        }
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected_recent,
                            error,
                        });
                    }
                    KeyCode::Down | KeyCode::Char('n') if key.code == KeyCode::Down || has_ctrl => {
                        if !recent.is_empty() {
                            let next = match selected_recent {
                                None => 0,
                                Some(idx) => (idx + 1) % recent.len(),
                            };
                            input = recent[next].clone();
                            selected_recent = Some(next);
                            error = None;
                        }
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected_recent,
                            error,
                        });
                    }
                    KeyCode::Backspace => {
                        input.pop();
                        selected_recent = None;
                        error = None;
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected_recent,
                            error,
                        });
                    }
                    KeyCode::Enter => {
                        let candidate = input.trim();
                        if !candidate.is_empty() {
                            let (target_provider, target_model) =
                                self.resolve_model_target(candidate);
                            if target_model.is_empty() {
                                self.modal = Some(ActiveModal::ModelInput {
                                    input,
                                    selected_recent,
                                    error: Some("Model name is required".to_string()),
                                });
                            } else {
                                self.select_provider_and_model(&target_provider, &target_model);
                                self.modal = None;
                            }
                        } else if let Some(existing) =
                            self.current_or_default_model(&self.active_provider)
                        {
                            let provider = self.active_provider.clone();
                            self.select_provider_and_model(&provider, &existing);
                            self.modal = None;
                        } else {
                            self.modal = Some(ActiveModal::ModelInput {
                                input,
                                selected_recent,
                                error: Some("Model name is required".to_string()),
                            });
                        }
                    }
                    KeyCode::Char(c) if !has_ctrl && !has_alt => {
                        input.push(c);
                        selected_recent = None;
                        error = None;
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected_recent,
                            error,
                        });
                    }
                    _ => {
                        self.modal = Some(ActiveModal::ModelInput {
                            input,
                            selected_recent,
                            error,
                        });
                    }
                }
            }
            ActiveModal::DiffView {
                mut file_index,
                mut diff,
                mut scroll,
            } => {
                let max_scroll = diff.rows.len().saturating_sub(1);
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => {
                        self.modal = None;
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        scroll = (scroll + 1).min(max_scroll);
                        self.modal = Some(ActiveModal::DiffView {
                            file_index,
                            diff,
                            scroll,
                        });
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        scroll = scroll.saturating_sub(1);
                        self.modal = Some(ActiveModal::DiffView {
                            file_index,
                            diff,
                            scroll,
                        });
                    }
                    KeyCode::PageDown | KeyCode::Char('d')
                        if key.code == KeyCode::PageDown || has_ctrl =>
                    {
                        let step = (self.viewport_height / 2).max(5);
                        scroll = (scroll + step).min(max_scroll);
                        self.modal = Some(ActiveModal::DiffView {
                            file_index,
                            diff,
                            scroll,
                        });
                    }
                    KeyCode::PageUp | KeyCode::Char('u')
                        if key.code == KeyCode::PageUp || has_ctrl =>
                    {
                        let step = (self.viewport_height / 2).max(5);
                        scroll = scroll.saturating_sub(step);
                        self.modal = Some(ActiveModal::DiffView {
                            file_index,
                            diff,
                            scroll,
                        });
                    }
                    KeyCode::Home | KeyCode::Char('g') => {
                        scroll = 0;
                        self.modal = Some(ActiveModal::DiffView {
                            file_index,
                            diff,
                            scroll,
                        });
                    }
                    KeyCode::End | KeyCode::Char('G') => {
                        scroll = max_scroll;
                        self.modal = Some(ActiveModal::DiffView {
                            file_index,
                            diff,
                            scroll,
                        });
                    }
                    KeyCode::Tab | KeyCode::Char(']') => {
                        if !self.file_changes.is_empty() {
                            file_index = (file_index + 1) % self.file_changes.len();
                            let entry = &self.file_changes[file_index];
                            diff =
                                build_file_diff(&self.workspace_root, &entry.path, &entry.status);
                            scroll = 0;
                        }
                        self.modal = Some(ActiveModal::DiffView {
                            file_index,
                            diff,
                            scroll,
                        });
                    }
                    KeyCode::BackTab | KeyCode::Char('[') => {
                        if !self.file_changes.is_empty() {
                            file_index = if file_index == 0 {
                                self.file_changes.len() - 1
                            } else {
                                file_index - 1
                            };
                            let entry = &self.file_changes[file_index];
                            diff =
                                build_file_diff(&self.workspace_root, &entry.path, &entry.status);
                            scroll = 0;
                        }
                        self.modal = Some(ActiveModal::DiffView {
                            file_index,
                            diff,
                            scroll,
                        });
                    }
                    _ => {
                        self.modal = Some(ActiveModal::DiffView {
                            file_index,
                            diff,
                            scroll,
                        });
                    }
                }
            }
            ActiveModal::Search {
                mut query,
                mut mode,
                workspace_files,
                mut results,
                mut selected,
                mut preview,
            } => match key.code {
                KeyCode::Esc => {
                    self.modal = None;
                }
                KeyCode::Tab | KeyCode::BackTab => {
                    mode = mode.toggle();
                    Self::refresh_search(
                        &self.workspace_root,
                        mode,
                        &workspace_files,
                        &query,
                        &mut results,
                        &mut selected,
                        &mut preview,
                    );
                    self.modal = Some(ActiveModal::Search {
                        query,
                        mode,
                        workspace_files,
                        results,
                        selected,
                        preview,
                    });
                }
                KeyCode::Up | KeyCode::Char('p') if key.code == KeyCode::Up || has_ctrl => {
                    if !results.is_empty() {
                        selected = if selected == 0 {
                            results.len() - 1
                        } else {
                            selected - 1
                        };
                        preview = results
                            .get(selected)
                            .map(|item| build_search_preview(&self.workspace_root, item, 20));
                    }
                    self.modal = Some(ActiveModal::Search {
                        query,
                        mode,
                        workspace_files,
                        results,
                        selected,
                        preview,
                    });
                }
                KeyCode::Down | KeyCode::Char('n') if key.code == KeyCode::Down || has_ctrl => {
                    if !results.is_empty() {
                        selected = (selected + 1) % results.len();
                        preview = results
                            .get(selected)
                            .map(|item| build_search_preview(&self.workspace_root, item, 20));
                    }
                    self.modal = Some(ActiveModal::Search {
                        query,
                        mode,
                        workspace_files,
                        results,
                        selected,
                        preview,
                    });
                }
                KeyCode::PageDown | KeyCode::Char('d')
                    if key.code == KeyCode::PageDown || has_ctrl =>
                {
                    if let Some(prev) = preview.as_mut() {
                        let step = (self.viewport_height / 2).max(5);
                        let max_scroll = prev.lines.len().saturating_sub(6);
                        prev.scroll = prev.scroll.saturating_add(step).min(max_scroll);
                    }
                    self.modal = Some(ActiveModal::Search {
                        query,
                        mode,
                        workspace_files,
                        results,
                        selected,
                        preview,
                    });
                }
                KeyCode::PageUp => {
                    if let Some(prev) = preview.as_mut() {
                        let step = (self.viewport_height / 2).max(5);
                        prev.scroll = prev.scroll.saturating_sub(step);
                    }
                    self.modal = Some(ActiveModal::Search {
                        query,
                        mode,
                        workspace_files,
                        results,
                        selected,
                        preview,
                    });
                }
                KeyCode::Char('u') if has_ctrl => {
                    query.clear();
                    Self::refresh_search(
                        &self.workspace_root,
                        mode,
                        &workspace_files,
                        &query,
                        &mut results,
                        &mut selected,
                        &mut preview,
                    );
                    self.modal = Some(ActiveModal::Search {
                        query,
                        mode,
                        workspace_files,
                        results,
                        selected,
                        preview,
                    });
                }
                KeyCode::Backspace => {
                    query.pop();
                    Self::refresh_search(
                        &self.workspace_root,
                        mode,
                        &workspace_files,
                        &query,
                        &mut results,
                        &mut selected,
                        &mut preview,
                    );
                    self.modal = Some(ActiveModal::Search {
                        query,
                        mode,
                        workspace_files,
                        results,
                        selected,
                        preview,
                    });
                }
                KeyCode::Enter => {
                    if let Some(item) = results.get(selected) {
                        let snippet = match item.line {
                            Some(line) => format!("@{}#L{line} ", item.path),
                            None => format!("@{} ", item.path),
                        };
                        self.insert_str(&snippet);
                        self.mode = InputMode::Insert;
                        self.completion.reset();
                        self.modal = None;
                    } else {
                        self.modal = Some(ActiveModal::Search {
                            query,
                            mode,
                            workspace_files,
                            results,
                            selected,
                            preview,
                        });
                    }
                }
                KeyCode::Char(c) if !has_ctrl && !has_alt => {
                    query.push(c);
                    Self::refresh_search(
                        &self.workspace_root,
                        mode,
                        &workspace_files,
                        &query,
                        &mut results,
                        &mut selected,
                        &mut preview,
                    );
                    self.modal = Some(ActiveModal::Search {
                        query,
                        mode,
                        workspace_files,
                        results,
                        selected,
                        preview,
                    });
                }
                _ => {
                    self.modal = Some(ActiveModal::Search {
                        query,
                        mode,
                        workspace_files,
                        results,
                        selected,
                        preview,
                    });
                }
            },
            ActiveModal::Approval {
                queue,
                mut reason,
                mut editing_reason,
            } => {
                let request_id = queue.first().map(|request| request.request_id.clone());
                if editing_reason {
                    match key.code {
                        KeyCode::Esc => {
                            reason.clear();
                            editing_reason = false;
                        }
                        KeyCode::Enter => {
                            if let Some(request_id) = request_id {
                                let text = reason.trim().to_string();
                                let decision = if text.is_empty() {
                                    ApprovalDecision::Decline
                                } else {
                                    ApprovalDecision::DeclineWithReason(text)
                                };
                                let _ = self.cmd_tx.send(AgentCommand::Approve {
                                    request_id,
                                    decision,
                                    scope: None,
                                });
                            }
                            reason.clear();
                            editing_reason = false;
                        }
                        KeyCode::Backspace => {
                            reason.pop();
                        }
                        KeyCode::Char(c) if !has_ctrl && !has_alt => {
                            reason.push(c);
                        }
                        _ => {}
                    }
                } else {
                    let action = match key.code {
                        KeyCode::Char('y') | KeyCode::Enter => {
                            Some((ApprovalDecision::Accept, None))
                        }
                        KeyCode::Char('t') => {
                            Some((ApprovalDecision::Accept, Some(GrantScope::Turn)))
                        }
                        KeyCode::Char('a') => Some((ApprovalDecision::AcceptForSession, None)),
                        KeyCode::Char('n') => Some((ApprovalDecision::Decline, None)),
                        KeyCode::Char('r') => {
                            reason.clear();
                            editing_reason = true;
                            None
                        }
                        KeyCode::Esc => Some((ApprovalDecision::Cancel, None)),
                        _ => None,
                    };
                    if let Some((decision, scope)) = action
                        && let Some(request_id) = request_id
                    {
                        let _ = self.cmd_tx.send(AgentCommand::Approve {
                            request_id,
                            decision,
                            scope,
                        });
                    }
                }
                self.modal = Some(ActiveModal::Approval {
                    queue,
                    reason,
                    editing_reason,
                });
            }
        }

        true
    }

    fn toggle_sidebar(&mut self) {
        self.show_sidebar = !self.show_sidebar;
        if self.show_sidebar && self.terminal_width < MIN_SIDEBAR_WIDTH {
            self.status_notice = Some(format!(
                "Sidebar hidden until terminal width >= {MIN_SIDEBAR_WIDTH} cols"
            ));
        }
    }

    fn activate_provider_only(&mut self, provider_id: &str) {
        let canonical = canonical_provider_id(provider_id);
        let switching_provider = self.active_provider != canonical;
        self.active_provider = canonical.clone();
        self.settings.active_provider = Some(canonical.clone());

        let (spec, next_model) = {
            let mut rt = self.runtime.write().unwrap();
            rt.active_provider = canonical.clone();

            let candidate = self
                .settings
                .provider_model(&canonical)
                .map(str::to_string)
                .or_else(|| rt.catalog.default_model_for_provider(&canonical))
                .or_else(|| {
                    (!switching_provider && !self.model.trim().is_empty())
                        .then(|| self.model.clone())
                })
                .unwrap_or_default();

            let spec = (!candidate.is_empty())
                .then(|| rt.catalog.format_genai_spec(&canonical, &candidate));
            (spec, candidate)
        };

        self.model = next_model.clone();
        self.settings.active_model = (!next_model.is_empty()).then_some(next_model);
        self.sync_completion_models();
        self.persist_settings();
        if let Some(spec) = spec {
            let _ = self.cmd_tx.send(AgentCommand::SetModel(spec));
        }
    }

    fn select_provider_and_model(&mut self, provider_id: &str, model_id: &str) {
        let canonical = canonical_provider_id(provider_id);
        let clean_model = model_id.trim().to_string();
        self.active_provider = canonical.clone();
        self.model = clean_model.clone();
        self.settings
            .record_provider_model(&canonical, &clean_model);

        let spec = {
            let mut rt = self.runtime.write().unwrap();
            rt.active_provider = canonical.clone();
            rt.catalog.format_genai_spec(&canonical, &clean_model)
        };

        self.sync_completion_models();
        self.persist_settings();
        let _ = self.cmd_tx.send(AgentCommand::SetModel(spec));
        self.transcript.push(TranscriptEntry::System {
            text: format!("Active model: {canonical}/{clean_model}"),
        });
    }

    fn persist_settings(&mut self) {
        if let Some(dir) = &self.config_dir
            && let Err(err) = save_settings_to(dir, &self.settings)
        {
            self.status_notice = Some(format!("Failed to save settings: {err:#}"));
        }
    }

    fn open_provider_select(&mut self) {
        let selected = {
            let rt = self.runtime.read().unwrap();
            rt.catalog
                .providers()
                .iter()
                .position(|p| p.id == self.active_provider)
                .unwrap_or(0)
        };
        self.modal = Some(ActiveModal::ProviderSelect {
            filter: String::new(),
            selected,
        });
    }

    fn open_model_input(&mut self) {
        self.modal = Some(ActiveModal::ModelInput {
            input: String::new(),
            selected_recent: None,
            error: None,
        });
    }

    fn handle_insert_key(&mut self, key: KeyEvent) -> bool {
        let has_shift_or_alt = key.modifiers.contains(KeyModifiers::SHIFT)
            || key.modifiers.contains(KeyModifiers::ALT);
        let has_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        if self.completion.is_active() {
            match key.code {
                KeyCode::Up => {
                    self.completion.select_prev();
                    return true;
                }
                KeyCode::Down => {
                    self.completion.select_next();
                    return true;
                }
                KeyCode::Char('p') if has_ctrl => {
                    self.completion.select_prev();
                    return true;
                }
                KeyCode::Char('n') if has_ctrl => {
                    self.completion.select_next();
                    return true;
                }
                KeyCode::Tab => {
                    self.accept_completion();
                    return true;
                }
                KeyCode::Enter if !has_shift_or_alt && !has_ctrl => {
                    let is_slash = self.completion.kind() == Some(CompletionKind::SlashCommand);
                    if self.completion.selected_is_noop(&self.input) {
                        self.completion.reset();
                        self.submit_input();
                    } else {
                        self.accept_completion();
                        if is_slash && !self.input.ends_with(' ') {
                            self.submit_input();
                        }
                    }
                    return true;
                }
                KeyCode::Esc => {
                    self.completion.dismiss();
                    return true;
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Esc => {
                self.mode = InputMode::Normal;
            }
            KeyCode::Enter if has_shift_or_alt => {
                self.insert_char('\n');
                self.update_completion();
            }
            KeyCode::Enter if !has_ctrl => {
                self.submit_input();
            }
            KeyCode::Backspace => {
                self.backspace();
                self.update_completion();
            }
            KeyCode::Delete => {
                self.delete_forward();
                self.update_completion();
            }
            KeyCode::Left => {
                self.move_left();
                self.update_completion();
            }
            KeyCode::Right => {
                self.move_right();
                self.update_completion();
            }
            KeyCode::Home => {
                self.move_home();
                self.update_completion();
            }
            KeyCode::End => {
                self.move_end();
                self.update_completion();
            }
            KeyCode::Up => {
                self.move_up();
                self.update_completion();
            }
            KeyCode::Down => {
                self.move_down();
                self.update_completion();
            }
            KeyCode::Char(' ')
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if self.input == "/search" || self.input == "/grep" {
                    let mode = if self.input == "/grep" {
                        SearchMode::Content
                    } else {
                        SearchMode::Files
                    };
                    self.input.clear();
                    self.cursor = 0;
                    self.completion.reset();
                    self.open_search_modal(mode, "");
                    return true;
                }
                self.insert_char(' ');
                self.update_completion();
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert_char(c);
                self.update_completion();
            }
            _ => return false,
        }

        true
    }

    fn handle_normal_key(&mut self, key: KeyEvent) -> bool {
        let has_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        if key.code != KeyCode::Char('g') || has_ctrl {
            self.pending_g = false;
        }

        match key.code {
            KeyCode::Char('i') | KeyCode::Char('a') if !has_ctrl => {
                self.mode = InputMode::Insert;
            }
            KeyCode::Char('j') if !has_ctrl => {
                self.scroll_down(1);
            }
            KeyCode::Down => {
                self.scroll_down(1);
            }
            KeyCode::Char('k') if !has_ctrl => {
                self.scroll_up(1);
            }
            KeyCode::Up => {
                self.scroll_up(1);
            }
            KeyCode::Char('d') if has_ctrl => {
                self.scroll_half_page_down();
            }
            KeyCode::Char('u') if has_ctrl => {
                self.scroll_half_page_up();
            }
            KeyCode::Char('g') if !has_ctrl => {
                if self.pending_g {
                    self.pending_g = false;
                    self.scroll_to_top();
                } else {
                    self.pending_g = true;
                }
            }
            KeyCode::Char('G') => {
                self.scroll_to_bottom();
            }
            KeyCode::Char('t') if !has_ctrl => {
                return self.toggle_latest_thinking();
            }
            KeyCode::Char('y') if !has_ctrl => {
                let text = self
                    .transcript
                    .iter()
                    .rev()
                    .find_map(|entry| match entry {
                        TranscriptEntry::Assistant { text, .. } if !text.is_empty() => {
                            Some(text.clone())
                        }
                        _ => None,
                    })
                    .or_else(|| {
                        self.transcript.iter().rev().find_map(|entry| match entry {
                            TranscriptEntry::Error { text, .. }
                            | TranscriptEntry::System { text }
                                if !text.is_empty() =>
                            {
                                Some(text.clone())
                            }
                            _ => None,
                        })
                    });
                if let Some(content) = text {
                    self.copy_text_to_clipboard(&content);
                    return true;
                }
                return false;
            }
            KeyCode::Char('/') if !has_ctrl => {
                self.open_search_modal(SearchMode::Files, "");
            }
            KeyCode::Char('q') if !has_ctrl => {
                self.quit();
            }
            KeyCode::Esc => {
                if self.is_running {
                    let _ = self.cmd_tx.send(AgentCommand::Cancel);
                    self.status_notice = Some("Cancelling turn...".to_string());
                }
            }
            _ => return false,
        }

        true
    }

    pub fn handle_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::TurnStarted { turn_id, model } => {
                self.is_running = true;
                self.turn_id = Some(turn_id);
                self.usage = UsageStats::default();
                self.metrics.turn_started_at = Some(Instant::now());
                self.metrics.first_token_at = None;
                let display_model = model
                    .split_once("::")
                    .map(|(_, m)| m.to_string())
                    .unwrap_or(model);
                self.model = display_model;
                self.active_tool = None;
                self.pending_assistant_idx = None;
                self.assistant_items.clear();
                self.tool_items.clear();
            }
            AgentEvent::ItemStarted { item } => match item {
                SessionItem::AgentMessage { id, .. } => {
                    let idx = self.ensure_pending_assistant();
                    self.assistant_items.insert(id, idx);
                }
                SessionItem::Reasoning { id, summary } => {
                    let idx = self.ensure_pending_assistant();
                    self.assistant_items.insert(id, idx);
                    if !summary.is_empty()
                        && let Some(TranscriptEntry::Assistant { reasoning, .. }) =
                            self.transcript.get_mut(idx)
                    {
                        reasoning.push_str(&summary);
                    }
                }
                SessionItem::ToolCall {
                    id,
                    call_id,
                    name,
                    args,
                    status,
                    ..
                } => {
                    self.finalize_pending_assistant();
                    self.active_tool = Some(name.clone());
                    let call_id = call_id.to_string();
                    self.transcript
                        .push(TranscriptEntry::ToolCall(ToolCallEntry {
                            call_id: call_id.clone(),
                            name,
                            args,
                            status: map_item_status(status),
                            output: None,
                        }));
                    self.tool_items.insert(id, call_id);
                }
                SessionItem::Compaction { freed_tokens, .. } => {
                    self.transcript.push(TranscriptEntry::System {
                        text: format!("Compaction freed {freed_tokens} tokens"),
                    });
                }
                _ => {}
            },
            AgentEvent::ItemDelta { item_id, delta } => match delta {
                ItemDelta::AgentText(chunk) => {
                    self.metrics.record_first_token();
                    let idx = self.assistant_item_idx(item_id);
                    if let Some(TranscriptEntry::Assistant { text, .. }) =
                        self.transcript.get_mut(idx)
                    {
                        text.push_str(&chunk);
                    }
                }
                ItemDelta::ReasoningSummary(chunk) => {
                    self.metrics.record_first_token();
                    let idx = self.assistant_item_idx(item_id);
                    if let Some(TranscriptEntry::Assistant { reasoning, .. }) =
                        self.transcript.get_mut(idx)
                    {
                        reasoning.push_str(&chunk);
                    }
                }
                ItemDelta::StatusChanged(status) => {
                    if let Some(call_id) = self.tool_items.get(&item_id).cloned()
                        && let Some(entry) = self.find_tool_call_mut(&call_id)
                    {
                        entry.status = map_item_status(status);
                    }
                }
                ItemDelta::CommandOutput(_) | ItemDelta::PlanUpdated(_) => {}
            },
            AgentEvent::ItemCompleted { item } => match item {
                SessionItem::AgentMessage { id, text } => {
                    let idx = self
                        .assistant_items
                        .remove(&id)
                        .or(self.pending_assistant_idx);
                    if let Some(idx) = idx {
                        self.pending_assistant_idx = None;
                        if let Some(TranscriptEntry::Assistant {
                            text: existing,
                            streaming,
                            ..
                        }) = self.transcript.get_mut(idx)
                        {
                            *streaming = false;
                            if !text.is_empty() {
                                *existing = text;
                            }
                        }
                    } else if !text.is_empty() {
                        self.transcript.push(TranscriptEntry::Assistant {
                            text,
                            reasoning: String::new(),
                            streaming: false,
                            reasoning_expanded: false,
                        });
                    }
                }
                SessionItem::Reasoning { id, summary } => {
                    let idx = self
                        .assistant_items
                        .remove(&id)
                        .or(self.pending_assistant_idx);
                    if !summary.is_empty() {
                        if let Some(idx) = idx {
                            if let Some(TranscriptEntry::Assistant { reasoning, .. }) =
                                self.transcript.get_mut(idx)
                            {
                                *reasoning = summary;
                            }
                        } else {
                            self.transcript.push(TranscriptEntry::Assistant {
                                text: String::new(),
                                reasoning: summary,
                                streaming: false,
                                reasoning_expanded: false,
                            });
                        }
                    }
                }
                SessionItem::ToolCall {
                    id,
                    call_id,
                    name,
                    status,
                    output,
                    ..
                } => {
                    let call_id = self
                        .tool_items
                        .remove(&id)
                        .unwrap_or_else(|| call_id.to_string());
                    let status = map_completed_item_status(status);
                    if let Some(entry) = self.find_tool_call_mut(&call_id) {
                        entry.status = status;
                        if output.is_some() {
                            entry.output = output;
                        }
                    } else {
                        self.transcript
                            .push(TranscriptEntry::ToolCall(ToolCallEntry {
                                call_id,
                                name: name.clone(),
                                args: String::new(),
                                status,
                                output,
                            }));
                    }
                    if self.active_tool.as_deref() == Some(name.as_str()) {
                        self.active_tool = None;
                    }
                }
                SessionItem::Compaction { freed_tokens, .. } => {
                    self.transcript.push(TranscriptEntry::System {
                        text: format!("Compaction freed {freed_tokens} tokens"),
                    });
                }
                _ => {}
            },
            AgentEvent::ApprovalRequested { request } => match &mut self.modal {
                Some(ActiveModal::Approval { queue, .. }) => queue.push(request),
                _ => {
                    self.modal = Some(ActiveModal::Approval {
                        queue: vec![request],
                        reason: String::new(),
                        editing_reason: false,
                    });
                }
            },
            AgentEvent::ApprovalResolved { request_id } => {
                let mut close = false;
                if let Some(ActiveModal::Approval { queue, .. }) = &mut self.modal {
                    queue.retain(|request| request.request_id != request_id);
                    close = queue.is_empty();
                }
                if close {
                    self.modal = None;
                }
            }
            AgentEvent::UsageUpdated {
                usage, cumulative, ..
            } => {
                let session_prompt = cumulative.prompt_tokens.unwrap_or(0).max(0) as u64;
                let session_completion = cumulative.completion_tokens.unwrap_or(0).max(0) as u64;
                let session_total = cumulative
                    .total_tokens
                    .map(|t| t.max(0) as u64)
                    .unwrap_or(session_prompt + session_completion);
                self.metrics.session_prompt_tokens = session_prompt;
                self.metrics.session_completion_tokens = session_completion;
                self.metrics.session_total_tokens = session_total;
                self.metrics.update_speed(usage.completion_tokens);
                self.usage = UsageStats {
                    prompt_tokens: usage.prompt_tokens,
                    completion_tokens: usage.completion_tokens,
                    total_tokens: usage.total_tokens,
                };
            }
            AgentEvent::TurnCompleted { reason, .. } => {
                self.is_running = false;
                self.active_tool = None;
                self.turn_id = None;
                self.metrics.turn_count += 1;
                if let Some(ms) = self.metrics.elapsed_ms() {
                    self.metrics.last_turn_ms = Some(ms);
                }
                self.metrics.turn_started_at = None;
                match reason {
                    TurnEndReason::Completed => {
                        self.metrics.update_speed(self.usage.completion_tokens);
                        self.finalize_pending_assistant();
                    }
                    TurnEndReason::Aborted { .. } => {
                        self.finalize_pending_assistant();
                        self.transcript.push(TranscriptEntry::System {
                            text: "Turn cancelled.".to_string(),
                        });
                    }
                    TurnEndReason::Failed { error } => {
                        self.finalize_pending_assistant();
                        if !matches!(self.transcript.last(), Some(TranscriptEntry::Error { .. })) {
                            self.push_error(format!("turn failed: {error}"));
                        }
                    }
                    _ => {
                        self.finalize_pending_assistant();
                    }
                }
            }
            AgentEvent::Error { code, message } => {
                self.is_running = false;
                self.active_tool = None;
                if let Some(ms) = self.metrics.elapsed_ms() {
                    self.metrics.last_turn_ms = Some(ms);
                }
                self.metrics.turn_started_at = None;
                self.finalize_pending_assistant();
                let text = format!("{} error: {message}", error_code_label(code));
                self.push_error(text);
            }
        }
    }

    fn assistant_item_idx(&mut self, item_id: ItemId) -> usize {
        if let Some(idx) = self.assistant_items.get(&item_id) {
            return *idx;
        }
        let idx = self.ensure_pending_assistant();
        self.assistant_items.insert(item_id, idx);
        idx
    }

    fn push_error(&mut self, text: String) {
        let duplicate = matches!(
            self.transcript.last(),
            Some(TranscriptEntry::Error { text: existing, .. }) if *existing == text
        );
        if !duplicate {
            self.transcript.push(TranscriptEntry::Error {
                text,
                expanded: false,
            });
        }
    }

    fn ensure_pending_assistant(&mut self) -> usize {
        if let Some(idx) = self.pending_assistant_idx {
            return idx;
        }
        let idx = self.transcript.len();
        self.transcript.push(TranscriptEntry::Assistant {
            text: String::new(),
            reasoning: String::new(),
            streaming: true,
            reasoning_expanded: false,
        });
        self.pending_assistant_idx = Some(idx);
        idx
    }

    fn finalize_pending_assistant(&mut self) {
        if let Some(idx) = self.pending_assistant_idx.take()
            && let Some(TranscriptEntry::Assistant { streaming, .. }) = self.transcript.get_mut(idx)
        {
            *streaming = false;
        }
    }

    fn find_tool_call_mut(&mut self, call_id: &str) -> Option<&mut ToolCallEntry> {
        self.transcript
            .iter_mut()
            .rev()
            .find_map(|entry| match entry {
                TranscriptEntry::ToolCall(tc) if tc.call_id == call_id => Some(tc),
                _ => None,
            })
    }

    fn update_completion(&mut self) {
        if self.input.contains('@') {
            self.completion.set_definitions(&self.agents, &self.skills);
        }
        self.completion.update(&self.input, self.cursor);
    }

    fn accept_completion(&mut self) {
        if let Some((new_input, new_cursor)) = self.completion.accept(&self.input, self.cursor) {
            self.input = new_input;
            self.cursor = new_cursor;
            self.update_completion();
        }
    }

    fn submit_input(&mut self) {
        let trimmed = self.input.trim();
        if trimmed.is_empty() {
            return;
        }

        if trimmed.starts_with('/') {
            let owned = trimmed.to_string();
            self.input.clear();
            self.cursor = 0;
            self.completion.reset();
            self.handle_slash_command(&owned);
            return;
        }

        let plan = parse_mentions(&self.input, &self.agents, &self.skills);
        let remaining = plan
            .text
            .clone()
            .unwrap_or_else(|| self.input.trim_end().to_string());

        if remaining.is_empty() {
            self.apply_mentions(&plan);
            self.acknowledge_mentions(&plan);
            self.input.clear();
            self.cursor = 0;
            self.completion.reset();
            self.auto_follow = true;
            return;
        }

        if !self.is_running && self.model.trim().is_empty() {
            self.status_notice = Some("Set a model first (/model)".to_string());
            self.open_model_input();
            return;
        }

        self.apply_mentions(&plan);

        if self.is_running {
            self.input.clear();
            self.cursor = 0;
            self.completion.reset();
            self.auto_follow = true;
            self.transcript.push(TranscriptEntry::User {
                text: remaining.clone(),
            });
            let _ = self.cmd_tx.send(AgentCommand::Steer(remaining));
            return;
        }

        self.input.clear();
        self.cursor = 0;
        self.completion.reset();
        self.auto_follow = true;
        self.is_running = true;
        self.transcript.push(TranscriptEntry::User {
            text: remaining.clone(),
        });
        let _ = self.cmd_tx.send(AgentCommand::SubmitPrompt(remaining));
    }

    fn apply_mentions(&mut self, plan: &MentionPlan) {
        for index in &plan.skill_indices {
            let Some(skill) = self.skills.get(*index).cloned() else {
                continue;
            };
            self.send_skill(&skill);
        }
        for index in &plan.agent_indices {
            let Some(agent) = self.agents.get(*index).cloned() else {
                continue;
            };
            let model = self.agent_model_spec(&agent);
            self.send_agent(&agent, model);
        }
    }

    fn acknowledge_mentions(&mut self, plan: &MentionPlan) {
        let mut messages = Vec::new();
        for index in &plan.skill_indices {
            if let Some(skill) = self.skills.get(*index) {
                messages.push(format!(
                    "Applied skill {} ({}).",
                    skill.id,
                    definition_source_label(skill.source)
                ));
            }
        }
        for index in &plan.agent_indices {
            if let Some(agent) = self.agents.get(*index) {
                messages.push(format!(
                    "Applied agent {} ({}).",
                    agent.id,
                    definition_source_label(agent.source)
                ));
            }
        }
        for message in messages {
            self.transcript
                .push(TranscriptEntry::System { text: message });
        }
    }

    fn handle_slash_command(&mut self, command: &str) {
        let mut parts = command.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or_default();
        let arg = parts.next().map(str::trim).unwrap_or_default();

        match cmd {
            "/help" => {
                self.auto_follow = true;
                self.transcript.push(TranscriptEntry::System {
                    text: concat!(
                        "Commands: /auth, /connect, /provider, /models, /model [name], /agent <id> (or @agent/<id>), /skill <id> (or @skill/<id>), /mode [plan|ask|auto-edit|yolo], /compact, /search [query], /grep [query], /diff [path], /sidebar, /clear, /reset, /cancel, /quit, /help\n",
                        "Shortcuts: Ctrl-f search, Ctrl-t toggle thinking, Ctrl-b toggle sidebar, click thinking or changed file\n",
                        "Insert: Enter send, Shift/Alt+Enter newline, @ file, / command, Esc Normal\n",
                        "Normal: j/k/Up/Down scroll, / search, t toggle thinking, Ctrl-d/Ctrl-u half-page, gg/G top/bottom, i/a Insert, q quit"
                    )
                    .to_string(),
                });
            }
            "/auth" => {
                self.auto_follow = true;
                let data_dir = default_data_dir();
                let auth_path = data_dir.as_deref().map(auth_file_path);
                let runtime = self.runtime.read().unwrap();
                let text = format_auth_list(
                    &runtime.catalog,
                    self.secrets.as_ref(),
                    auth_path.as_deref(),
                    self.config_dir.as_deref(),
                );
                self.transcript.push(TranscriptEntry::System { text });
            }
            "/sidebar" => {
                self.toggle_sidebar();
            }
            "/search" | "/tv" | "/find" => {
                self.open_search_modal(SearchMode::Files, arg);
            }
            "/grep" => {
                self.open_search_modal(SearchMode::Content, arg);
            }
            "/diff" => {
                if self.file_changes.is_empty() {
                    self.status_notice = Some("Working tree is clean".to_string());
                } else {
                    let idx = if arg.is_empty() {
                        0
                    } else {
                        self.file_changes
                            .iter()
                            .position(|c| c.path.contains(arg))
                            .unwrap_or(0)
                    };
                    self.open_diff_modal(idx);
                }
            }
            "/connect" | "/provider" => {
                self.open_provider_select();
            }
            "/models" => {
                self.open_model_input();
            }
            "/agent" => {
                self.apply_agent(arg);
            }
            "/skill" => {
                self.apply_skill(arg);
            }
            "/model" => {
                self.auto_follow = true;
                if arg.is_empty() {
                    self.open_model_input();
                } else {
                    let (target_provider, target_model) = self.resolve_model_target(arg);
                    self.select_provider_and_model(&target_provider, &target_model);
                }
            }
            "/clear" => {
                self.transcript.clear();
                self.pending_assistant_idx = None;
                self.assistant_items.clear();
                self.tool_items.clear();
                self.scroll_offset = 0;
                self.total_lines = 0;
                self.auto_follow = true;
            }
            "/reset" => {
                self.transcript.clear();
                self.pending_assistant_idx = None;
                self.assistant_items.clear();
                self.tool_items.clear();
                self.turn_id = None;
                self.usage = UsageStats::default();
                self.metrics = TurnMetrics::default();
                self.scroll_offset = 0;
                self.total_lines = 0;
                self.auto_follow = true;
                self.is_running = false;
                self.active_tool = None;
                let _ = self.cmd_tx.send(AgentCommand::Reset);
                self.transcript.push(TranscriptEntry::System {
                    text: "Conversation reset.".to_string(),
                });
            }
            "/cancel" => {
                if self.is_running {
                    let _ = self.cmd_tx.send(AgentCommand::Cancel);
                    self.status_notice = Some("Cancelling turn...".to_string());
                } else {
                    self.transcript.push(TranscriptEntry::System {
                        text: "No active turn to cancel.".to_string(),
                    });
                }
            }
            "/compact" => {
                self.auto_follow = true;
                self.transcript.push(TranscriptEntry::System {
                    text: "Compaction requested.".to_string(),
                });
                let _ = self.cmd_tx.send(AgentCommand::Compact);
            }
            "/mode" => {
                self.auto_follow = true;
                if arg.is_empty() {
                    self.transcript.push(TranscriptEntry::System {
                        text: format!("Approval mode: {}", approval_mode_label(self.approval_mode)),
                    });
                } else {
                    match parse_approval_mode(arg) {
                        Some(mode) => {
                            self.approval_mode = mode;
                            let _ = self.cmd_tx.send(AgentCommand::SetPermissionMode(mode));
                            self.transcript.push(TranscriptEntry::System {
                                text: format!("Approval mode: {}", approval_mode_label(mode)),
                            });
                        }
                        None => {
                            self.transcript.push(TranscriptEntry::Error {
                                text: format!(
                                    "Invalid mode: {arg}. Expected: plan, ask, auto-edit, yolo"
                                ),
                                expanded: false,
                            });
                        }
                    }
                }
            }
            "/quit" | "/q" => {
                self.quit();
            }
            other => {
                self.auto_follow = true;
                self.transcript.push(TranscriptEntry::Error {
                    text: format!("Unknown command: {other}. Type /help for available commands."),
                    expanded: false,
                });
            }
        }
    }

    fn available_agent_ids(&self) -> String {
        if self.agents.is_empty() {
            return "(none)".to_string();
        }
        self.agents
            .iter()
            .map(|agent| agent.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn available_skill_ids(&self) -> String {
        if self.skills.is_empty() {
            return "(none)".to_string();
        }
        self.skills
            .iter()
            .map(|skill| skill.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn apply_agent(&mut self, id: &str) {
        self.auto_follow = true;
        let Some(agent) = self.agents.iter().find(|agent| agent.id == id).cloned() else {
            let available = self.available_agent_ids();
            let message = if id.is_empty() {
                format!("Usage: /agent <id>. Available agents: {available}")
            } else {
                format!("Unknown agent: {id}. Available agents: {available}")
            };
            self.transcript
                .push(TranscriptEntry::System { text: message });
            return;
        };
        let model = self.agent_model_spec(&agent);
        let source = definition_source_label(agent.source);
        self.send_agent(&agent, model);
        self.transcript.push(TranscriptEntry::System {
            text: format!("Applied agent {} ({source}).", agent.id),
        });
    }

    fn apply_skill(&mut self, id: &str) {
        self.auto_follow = true;
        let Some(skill) = self.skills.iter().find(|skill| skill.id == id).cloned() else {
            let available = self.available_skill_ids();
            let message = if id.is_empty() {
                format!("Usage: /skill <id>. Available skills: {available}")
            } else {
                format!("Unknown skill: {id}. Available skills: {available}")
            };
            self.transcript
                .push(TranscriptEntry::System { text: message });
            return;
        };
        let source = definition_source_label(skill.source);
        self.send_skill(&skill);
        self.transcript.push(TranscriptEntry::System {
            text: format!("Applied skill {} ({source}).", skill.id),
        });
    }

    fn send_agent(&mut self, agent: &AgentDefinition, model: Option<String>) {
        let _ = self.cmd_tx.send(AgentCommand::ApplyAgent {
            name: agent.id.clone(),
            instructions: agent.prompt.clone(),
            model,
        });
    }

    fn send_skill(&mut self, skill: &SkillDefinition) {
        let _ = self.cmd_tx.send(AgentCommand::ApplySkill {
            name: skill.id.clone(),
            instructions: skill.body.clone(),
        });
    }

    fn agent_model_spec(&mut self, agent: &AgentDefinition) -> Option<String> {
        let raw = agent
            .model
            .as_deref()
            .map(str::trim)
            .filter(|raw| !raw.is_empty())?;
        if raw.contains(':') {
            self.transcript.push(TranscriptEntry::System {
                text: format!(
                    "Agent {} model '{raw}' implies a provider change; it is not applied.",
                    agent.id
                ),
            });
            return None;
        }
        let spec = {
            let rt = self.runtime.read().unwrap();
            rt.catalog.format_genai_spec(&self.active_provider, raw)
        };
        self.model = raw.to_string();
        Some(spec)
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

    fn scroll_down(&mut self, lines: usize) {
        let max = self.max_scroll();
        let next = self.effective_scroll().saturating_add(lines).min(max);
        self.scroll_offset = next;
        self.auto_follow = next >= max;
    }

    fn scroll_up(&mut self, lines: usize) {
        let current = self.effective_scroll();
        self.scroll_offset = current.saturating_sub(lines);
        self.auto_follow = false;
    }

    fn scroll_half_page_down(&mut self) {
        let step = (self.viewport_height / 2).max(1);
        self.scroll_down(step);
    }

    fn scroll_half_page_up(&mut self) {
        let step = (self.viewport_height / 2).max(1);
        self.scroll_up(step);
    }

    fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
        self.auto_follow = false;
    }

    fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.max_scroll();
        self.auto_follow = true;
    }

    fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    fn insert_str(&mut self, text: &str) {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        self.input.insert_str(self.cursor, &normalized);
        self.cursor += normalized.len();
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = prev_char_boundary(&self.input, self.cursor);
        self.input.drain(prev..self.cursor);
        self.cursor = prev;
    }

    fn delete_forward(&mut self) {
        if self.cursor >= self.input.len() {
            return;
        }
        let next = next_char_boundary(&self.input, self.cursor);
        self.input.drain(self.cursor..next);
    }

    fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = prev_char_boundary(&self.input, self.cursor);
        }
    }

    fn move_right(&mut self) {
        if self.cursor < self.input.len() {
            self.cursor = next_char_boundary(&self.input, self.cursor);
        }
    }

    fn move_home(&mut self) {
        let before = &self.input[..self.cursor];
        self.cursor = before.rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    }

    fn move_end(&mut self) {
        let after = &self.input[self.cursor..];
        if let Some(rel) = after.find('\n') {
            self.cursor += rel;
        } else {
            self.cursor = self.input.len();
        }
    }

    fn move_up(&mut self) {
        let before = &self.input[..self.cursor];
        let Some(cur_line_break) = before.rfind('\n') else {
            return;
        };
        let col_chars = before[cur_line_break + 1..].chars().count();
        let prev_line_start = before[..cur_line_break]
            .rfind('\n')
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let prev_line = &self.input[prev_line_start..cur_line_break];
        self.cursor = prev_line_start + char_col_to_byte_idx(prev_line, col_chars);
    }

    fn move_down(&mut self) {
        let before = &self.input[..self.cursor];
        let cur_line_start = before.rfind('\n').map(|idx| idx + 1).unwrap_or(0);
        let col_chars = before[cur_line_start..].chars().count();
        let Some(rel_next_break) = self.input[self.cursor..].find('\n') else {
            return;
        };
        let next_line_start = self.cursor + rel_next_break + 1;
        let next_line_end = self.input[next_line_start..]
            .find('\n')
            .map(|rel| next_line_start + rel)
            .unwrap_or(self.input.len());
        let next_line = &self.input[next_line_start..next_line_end];
        self.cursor = next_line_start + char_col_to_byte_idx(next_line, col_chars);
    }
}

pub fn parse_approval_mode(raw: &str) -> Option<ApprovalMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "plan" => Some(ApprovalMode::Plan),
        "ask" => Some(ApprovalMode::Ask),
        "auto-edit" | "auto_edit" => Some(ApprovalMode::AutoEdit),
        "yolo" => Some(ApprovalMode::Yolo),
        _ => None,
    }
}

pub fn approval_mode_label(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Plan => "plan",
        ApprovalMode::Ask => "ask",
        ApprovalMode::AutoEdit => "auto-edit",
        ApprovalMode::Yolo => "yolo",
    }
}

pub fn approval_mode_badge(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Plan => "PLAN",
        ApprovalMode::Ask => "ASK",
        ApprovalMode::AutoEdit => "EDIT",
        ApprovalMode::Yolo => "YOLO",
    }
}

fn definition_source_label(source: DefinitionSource) -> &'static str {
    match source {
        DefinitionSource::Agents => ".agents",
        DefinitionSource::Claude => ".claude",
    }
}

const MENTION_PREFIX_LEN: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MentionKind {
    Agent,
    Skill,
}

pub(crate) fn mention_is_known(
    kind: MentionKind,
    id: &str,
    agents: &[AgentDefinition],
    skills: &[SkillDefinition],
) -> bool {
    match kind {
        MentionKind::Agent => agents.iter().any(|agent| agent.id == id),
        MentionKind::Skill => skills.iter().any(|skill| skill.id == id),
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct MentionPlan {
    agent_indices: Vec<usize>,
    skill_indices: Vec<usize>,
    text: Option<String>,
}

fn parse_mentions(
    text: &str,
    agents: &[AgentDefinition],
    skills: &[SkillDefinition],
) -> MentionPlan {
    let mut agent_indices: Vec<usize> = Vec::new();
    let mut skill_indices: Vec<usize> = Vec::new();
    let mut cleaned = String::with_capacity(text.len());
    let mut changed = false;
    let mut index = 0;

    while index < text.len() {
        let Some(ch) = text[index..].chars().next() else {
            break;
        };
        if ch == '@'
            && let Some((kind, id, end)) = parse_mention_token(text, index)
        {
            let found = match kind {
                MentionKind::Agent => agents.iter().position(|agent| agent.id == id).map(|pos| {
                    if !agent_indices.contains(&pos) {
                        agent_indices.push(pos);
                    }
                }),
                MentionKind::Skill => skills.iter().position(|skill| skill.id == id).map(|pos| {
                    if !skill_indices.contains(&pos) {
                        skill_indices.push(pos);
                    }
                }),
            };
            if found.is_some() {
                changed = true;
                index = end;
                continue;
            }
        }
        cleaned.push(ch);
        index += ch.len_utf8();
    }

    MentionPlan {
        agent_indices,
        skill_indices,
        text: changed.then(|| collapse_whitespace(&cleaned)),
    }
}

pub(crate) fn parse_mention_token(text: &str, start: usize) -> Option<(MentionKind, &str, usize)> {
    let rest = text.get(start + 1..)?;
    let (kind, rest) = if let Some(rest) = rest.strip_prefix("agent/") {
        (MentionKind::Agent, rest)
    } else {
        let rest = rest.strip_prefix("skill/")?;
        (MentionKind::Skill, rest)
    };
    let id_len = rest
        .char_indices()
        .find(|(_, ch)| !is_mention_id_char(*ch))
        .map(|(idx, _)| idx)
        .unwrap_or(rest.len());
    if id_len == 0 {
        return None;
    }
    Some((
        kind,
        &rest[..id_len],
        start + 1 + MENTION_PREFIX_LEN + id_len,
    ))
}

fn is_mention_id_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')
}

fn collapse_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut pending_space = false;
    let mut pending_newline = false;
    for ch in text.chars() {
        if ch == '\n' {
            pending_newline = true;
            pending_space = false;
        } else if ch.is_whitespace() {
            pending_space = true;
        } else {
            if !result.is_empty() {
                if pending_newline {
                    result.push('\n');
                } else if pending_space {
                    result.push(' ');
                }
            }
            pending_newline = false;
            pending_space = false;
            result.push(ch);
        }
    }
    result
}

fn map_item_status(status: ItemStatus) -> ToolCallStatus {
    match status {
        ItemStatus::Proposed => ToolCallStatus::Proposed,
        ItemStatus::Pending | ItemStatus::InProgress => ToolCallStatus::Running,
        ItemStatus::Succeeded => ToolCallStatus::Succeeded,
        ItemStatus::Failed | ItemStatus::Denied | ItemStatus::Cancelled => ToolCallStatus::Failed,
        ItemStatus::Stopped => ToolCallStatus::Stopped,
    }
}

fn map_completed_item_status(status: ItemStatus) -> ToolCallStatus {
    match map_item_status(status) {
        ToolCallStatus::Proposed => ToolCallStatus::Running,
        other => other,
    }
}

pub fn transcript_from_events(events: &[LoggedEvent]) -> Vec<TranscriptEntry> {
    let mut transcript = Vec::new();
    for logged in events {
        match &logged.event {
            SessionEvent::UserMessage {
                source: InputSource::User | InputSource::Steering,
                content,
            } => {
                let text = join_text_blocks(content);
                if !text.is_empty() {
                    transcript.push(TranscriptEntry::User { text });
                }
            }
            SessionEvent::AssistantMessage { content, .. } => {
                let text = join_text_blocks(content);
                let reasoning = content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Reasoning(text) => Some(text.as_str()),
                        ContentBlock::Text(_) => None,
                    })
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                if !text.is_empty() || !reasoning.is_empty() {
                    transcript.push(TranscriptEntry::Assistant {
                        text,
                        reasoning,
                        streaming: false,
                        reasoning_expanded: false,
                    });
                }
            }
            SessionEvent::ToolCall {
                call_id,
                name,
                args,
            } => {
                transcript.push(TranscriptEntry::ToolCall(ToolCallEntry {
                    call_id: call_id.to_string(),
                    name: name.clone(),
                    args: args.clone(),
                    status: ToolCallStatus::Running,
                    output: None,
                }));
            }
            SessionEvent::ToolResult { call_id, output } => {
                let call_id = call_id.to_string();
                let status = observation_status(output.status);
                let rendered = output.render();
                let existing = transcript.iter_mut().rev().find_map(|entry| match entry {
                    TranscriptEntry::ToolCall(entry) if entry.call_id == call_id => Some(entry),
                    _ => None,
                });
                if let Some(entry) = existing {
                    entry.status = status;
                    if !rendered.is_empty() {
                        entry.output = Some(rendered);
                    }
                }
            }
            SessionEvent::CompactionSummary { .. } => {
                transcript.push(TranscriptEntry::System {
                    text: "Compacted history".to_string(),
                });
            }
            _ => {}
        }
    }
    for entry in &mut transcript {
        if let TranscriptEntry::ToolCall(entry) = entry
            && entry.status == ToolCallStatus::Running
        {
            entry.status = ToolCallStatus::Cancelled;
            entry.output = None;
        }
    }
    transcript
}

pub fn session_usage_from_events(events: &[LoggedEvent]) -> Option<meerai_agents::Usage> {
    events.iter().rev().find_map(|logged| match &logged.event {
        SessionEvent::UsageUpdated { cumulative, .. } => Some(*cumulative),
        _ => None,
    })
}

fn join_text_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            ContentBlock::Reasoning(_) => None,
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn observation_status(status: ObservationStatus) -> ToolCallStatus {
    match status {
        ObservationStatus::Success => ToolCallStatus::Succeeded,
        ObservationStatus::Error | ObservationStatus::Refused => ToolCallStatus::Failed,
        ObservationStatus::Stopped => ToolCallStatus::Stopped,
    }
}

fn error_code_label(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::Provider => "provider",
        ErrorCode::Tool => "tool",
        ErrorCode::Config => "config",
        ErrorCode::ContextOverflow => "context overflow",
        ErrorCode::Budget => "budget",
        ErrorCode::LoopDetected => "loop detected",
        ErrorCode::Unsupported => "unsupported",
        ErrorCode::Internal => "internal",
    }
}

pub fn parse_git_status_porcelain(raw: &str) -> Vec<FileChangeEntry> {
    raw.lines()
        .filter_map(|line| {
            if line.len() < 4 {
                return None;
            }
            let status_part = line[..2].trim();
            let status = if status_part.is_empty() {
                "M".to_string()
            } else {
                status_part.to_string()
            };
            let path = line[3..].trim().trim_matches('"').to_string();
            if path.is_empty() {
                return None;
            }
            Some(FileChangeEntry { status, path })
        })
        .collect()
}

fn prev_char_boundary(s: &str, idx: usize) -> usize {
    let mut pos = idx.min(s.len()).saturating_sub(1);
    while pos > 0 && !s.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

fn next_char_boundary(s: &str, idx: usize) -> usize {
    let mut pos = (idx + 1).min(s.len());
    while pos < s.len() && !s.is_char_boundary(pos) {
        pos += 1;
    }
    pos
}

fn char_col_to_byte_idx(line: &str, target_chars: usize) -> usize {
    line.char_indices()
        .nth(target_chars)
        .map(|(idx, _)| idx)
        .unwrap_or(line.len())
}

pub fn clamp_pos_to_bounds(pos: ScreenPos, bounds: Rect) -> Option<ScreenPos> {
    if bounds.width == 0 || bounds.height == 0 {
        return None;
    }
    let min_col = bounds.x;
    let max_col = bounds.x + bounds.width - 1;
    let min_row = bounds.y;
    let max_row = bounds.y + bounds.height - 1;
    if pos.row < min_row {
        Some(ScreenPos {
            col: min_col,
            row: min_row,
        })
    } else if pos.row > max_row {
        Some(ScreenPos {
            col: max_col,
            row: max_row,
        })
    } else {
        Some(ScreenPos {
            col: pos.col.clamp(min_col, max_col),
            row: pos.row,
        })
    }
}

pub fn normalize_selection_in_bounds(
    anchor: ScreenPos,
    head: ScreenPos,
    bounds: Rect,
) -> Option<(ScreenPos, ScreenPos)> {
    let a = clamp_pos_to_bounds(anchor, bounds)?;
    let b = clamp_pos_to_bounds(head, bounds)?;
    if (a.row, a.col) <= (b.row, b.col) {
        Some((a, b))
    } else {
        Some((b, a))
    }
}

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(not(test))]
fn write_system_clipboard(text: &str) {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let b64 = encode_base64(text.as_bytes());
    let mut stdout = std::io::stdout();
    let _ = write!(stdout, "\x1b]52;c;{b64}\x07");
    let _ = stdout.flush();

    let payload = text.as_bytes().to_vec();
    std::thread::spawn(move || {
        #[cfg(target_os = "macos")]
        let candidates: &[(&str, &[&str])] = &[("pbcopy", &[])];
        #[cfg(target_os = "windows")]
        let candidates: &[(&str, &[&str])] = &[("clip", &[])];
        #[cfg(all(unix, not(target_os = "macos")))]
        let candidates: &[(&str, &[&str])] = &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ];

        for (bin, args) in candidates {
            if let Ok(mut child) = Command::new(bin)
                .args(*args)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(&payload);
                }
                let _ = child.wait();
                break;
            }
        }
    });
}

#[cfg(test)]
fn write_system_clipboard(_text: &str) {}

#[cfg(test)]
mod tests {
    use super::*;
    use meerai_agents::events::{
        ApprovalDecision, ApprovalKind, ApprovalPreview, ApprovalRequest, GrantScope, RequestId,
    };
    use meerai_agents::{AbortCause, Usage};
    use meerai_common::providers::ProviderCatalog;
    use meerai_common::secrets::MemorySecretStore;
    use meerai_core::EffectClass;

    use crate::completion::CompletionItemKind;

    fn test_app() -> (
        AppState,
        mpsc::UnboundedReceiver<AgentCommand>,
        Arc<MemorySecretStore>,
    ) {
        let (tx, rx) = mpsc::unbounded_channel();
        let secrets = Arc::new(MemorySecretStore::new());
        secrets.set_key("gemini", "test-gemini-key").unwrap();
        let settings = AppSettings::default();
        let runtime = Arc::new(RwLock::new(RuntimeProviderState::from_settings(
            &settings,
            ProviderCatalog::new(),
        )));
        let app = AppState::new(
            "gemini",
            "gemini-3.8-flash",
            PathBuf::from("."),
            tx,
            runtime,
            secrets.clone(),
            settings,
            None,
        );
        (app, rx, secrets)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn key_mod(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn type_text(app: &mut AppState, text: &str) {
        for ch in text.chars() {
            app.handle_key_event(key(KeyCode::Char(ch)));
        }
    }

    #[test]
    fn opens_provider_select_on_startup_when_key_missing() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let secrets = Arc::new(MemorySecretStore::new());
        let settings = AppSettings::default();
        let catalog =
            ProviderCatalog::from_yaml("providers:\n  - id: custom-test-provider\n").unwrap();
        let runtime = Arc::new(RwLock::new(RuntimeProviderState::from_settings(
            &settings, catalog,
        )));
        let app = AppState::new(
            "custom-test-provider",
            "test-model",
            PathBuf::from("."),
            tx,
            runtime,
            secrets,
            settings,
            None,
        );
        assert!(matches!(
            app.modal,
            Some(ActiveModal::ProviderSelect { .. })
        ));
    }

    #[test]
    fn connect_flow_saves_key_deletes_and_selects_model() {
        let (mut app, mut rx, secrets) = test_app();

        type_text(&mut app, "/connect");
        app.handle_key_event(key(KeyCode::Enter));
        assert!(matches!(
            app.modal,
            Some(ActiveModal::ProviderSelect { .. })
        ));

        type_text(&mut app, "opencode-go");
        app.handle_key_event(key(KeyCode::Enter));
        assert!(matches!(
            app.modal,
            Some(ActiveModal::ApiKeyInput { ref provider_id, .. }) if provider_id == "opencode-go"
        ));

        app.handle_terminal_event(Event::Paste("oc-secret-key\n".to_string()));
        app.handle_key_event(key(KeyCode::Enter));
        assert_eq!(
            secrets.get_key("opencode-go").unwrap().as_deref(),
            Some("oc-secret-key")
        );
        assert!(matches!(app.modal, Some(ActiveModal::ModelInput { .. })));

        app.handle_key_event(key(KeyCode::Enter));
        assert!(matches!(
            app.modal,
            Some(ActiveModal::ModelInput { error: Some(_), .. })
        ));

        type_text(&mut app, "glm-5.3");
        app.handle_key_event(key(KeyCode::Enter));
        assert!(app.modal.is_none());
        assert_eq!(app.active_provider, "opencode-go");
        assert_eq!(app.model, "glm-5.3");
        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::SetModel("opencode_go::glm-5.3".to_string())
        );

        type_text(&mut app, "/provider");
        app.handle_key_event(key(KeyCode::Enter));
        type_text(&mut app, "opencode-go");
        app.handle_key_event(key(KeyCode::Enter));
        app.handle_key_event(key_mod(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert_eq!(secrets.get_key("opencode-go").unwrap(), None);
    }

    #[test]
    fn model_input_accepts_manual_and_recent_models() {
        let (mut app, mut rx, _secrets) = test_app();

        type_text(&mut app, "/models");
        app.handle_key_event(key(KeyCode::Enter));
        assert!(matches!(app.modal, Some(ActiveModal::ModelInput { .. })));

        type_text(&mut app, "custom-gemini-model");
        app.handle_key_event(key(KeyCode::Enter));
        assert!(app.modal.is_none());
        assert_eq!(app.model, "custom-gemini-model");
        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::SetModel("gemini::custom-gemini-model".to_string())
        );

        type_text(&mut app, "/models");
        app.handle_key_event(key(KeyCode::Enter));
        app.handle_key_event(key(KeyCode::Down));
        assert!(matches!(
            app.modal,
            Some(ActiveModal::ModelInput { ref input, .. }) if input == "custom-gemini-model"
        ));
    }

    #[test]
    fn opens_model_input_on_startup_when_model_empty_and_blocks_empty_submit() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let secrets = Arc::new(MemorySecretStore::new());
        secrets.set_key("openai", "test-openai-key").unwrap();
        let settings = AppSettings::default();
        let runtime = Arc::new(RwLock::new(RuntimeProviderState::from_settings(
            &settings,
            ProviderCatalog::new(),
        )));
        let mut app = AppState::new(
            "openai",
            "",
            PathBuf::from("."),
            tx,
            runtime,
            secrets,
            settings,
            None,
        );
        assert!(matches!(app.modal, Some(ActiveModal::ModelInput { .. })));

        app.handle_key_event(key(KeyCode::Esc));
        assert!(app.modal.is_none());

        type_text(&mut app, "hello");
        app.handle_key_event(key(KeyCode::Enter));
        assert!(!app.is_running);
        assert!(rx.try_recv().is_err());
        assert!(matches!(app.modal, Some(ActiveModal::ModelInput { .. })));
    }

    #[test]
    fn modal_switching_and_editing() {
        let (mut app, mut rx, _secrets) = test_app();
        assert_eq!(app.mode, InputMode::Insert);

        type_text(&mut app, "hello");
        app.handle_key_event(key_mod(KeyCode::Enter, KeyModifiers::SHIFT));
        type_text(&mut app, "world");
        assert_eq!(app.input, "hello\nworld");

        app.handle_key_event(key(KeyCode::Esc));
        assert_eq!(app.mode, InputMode::Normal);

        app.handle_key_event(key(KeyCode::Char('i')));
        assert_eq!(app.mode, InputMode::Insert);

        app.handle_key_event(key(KeyCode::Esc));
        assert_eq!(app.mode, InputMode::Normal);
        app.handle_key_event(key(KeyCode::Char('a')));
        assert_eq!(app.mode, InputMode::Insert);

        app.handle_terminal_event(Event::Paste("!\r\nmore".to_string()));
        assert_eq!(app.input, "hello\nworld!\nmore");

        app.handle_key_event(key(KeyCode::Enter));
        assert!(app.input.is_empty());
        assert!(app.is_running);
        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::SubmitPrompt("hello\nworld!\nmore".to_string())
        );
    }

    #[test]
    fn normal_mode_scrolling_and_quit_controls() {
        let (mut app, mut rx, _secrets) = test_app();
        app.viewport_height = 10;
        app.total_lines = 40;
        app.auto_follow = true;
        app.mode = InputMode::Normal;

        assert_eq!(app.effective_scroll(), 30);

        app.handle_key_event(key(KeyCode::Char('k')));
        assert!(!app.auto_follow);
        assert_eq!(app.effective_scroll(), 29);

        app.handle_key_event(key_mod(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(app.effective_scroll(), 24);

        app.handle_key_event(key_mod(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert_eq!(app.effective_scroll(), 29);

        app.handle_key_event(key(KeyCode::Char('g')));
        app.handle_key_event(key(KeyCode::Char('g')));
        assert_eq!(app.effective_scroll(), 0);
        assert!(!app.auto_follow);

        app.handle_key_event(key(KeyCode::Char('G')));
        assert_eq!(app.effective_scroll(), 30);
        assert!(app.auto_follow);

        app.is_running = true;
        app.handle_key_event(key(KeyCode::Esc));
        assert_eq!(rx.try_recv().unwrap(), AgentCommand::Cancel);

        app.handle_key_event(key(KeyCode::Char('q')));
        assert!(app.should_quit);
        assert_eq!(rx.try_recv().unwrap(), AgentCommand::Quit);
    }

    #[test]
    fn double_ctrl_c_exits_and_single_ctrl_c_cancels_when_running() {
        let (mut app, mut rx, _secrets) = test_app();
        app.is_running = true;
        app.handle_key_event(key_mod(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(rx.try_recv().unwrap(), AgentCommand::Cancel);
        assert!(!app.should_quit);

        app.is_running = false;
        app.handle_key_event(key_mod(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!app.should_quit);
        assert!(app.status_notice.is_some());

        app.handle_key_event(key_mod(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
        assert_eq!(rx.try_recv().unwrap(), AgentCommand::Quit);
    }

    #[test]
    fn completion_esc_dismisses_before_entering_normal_mode() {
        let (mut app, _rx, _secrets) = test_app();
        type_text(&mut app, "/");
        assert!(app.completion.is_active());

        app.handle_key_event(key_mod(KeyCode::Char('n'), KeyModifiers::CONTROL));
        assert_eq!(app.completion.selected(), 1);
        app.handle_key_event(key_mod(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert_eq!(app.completion.selected(), 0);

        app.handle_key_event(key(KeyCode::Esc));
        assert!(!app.completion.is_active());
        assert_eq!(app.mode, InputMode::Insert);

        app.handle_key_event(key(KeyCode::Esc));
        assert_eq!(app.mode, InputMode::Normal);
    }

    #[test]
    fn slash_command_handling() {
        let (mut app, mut rx, _secrets) = test_app();

        type_text(&mut app, "/help");
        app.handle_key_event(key(KeyCode::Enter));
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptEntry::System { text }) if text.contains("/connect")
        ));

        type_text(&mut app, "/model gemini-3.1-pro-preview");
        app.handle_key_event(key(KeyCode::Enter));
        assert_eq!(app.model, "gemini-3.1-pro-preview");
        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::SetModel("gemini::gemini-3.1-pro-preview".to_string())
        );

        type_text(&mut app, "/clear");
        app.handle_key_event(key(KeyCode::Enter));
        assert!(app.transcript.is_empty());

        app.is_running = true;
        type_text(&mut app, "/cancel");
        app.handle_key_event(key(KeyCode::Enter));
        assert_eq!(rx.try_recv().unwrap(), AgentCommand::Cancel);

        type_text(&mut app, "/reset");
        app.handle_key_event(key(KeyCode::Enter));
        assert_eq!(rx.try_recv().unwrap(), AgentCommand::Reset);
        assert!(!app.is_running);

        type_text(&mut app, "/quit");
        app.handle_key_event(key(KeyCode::Enter));
        assert!(app.should_quit);
        assert_eq!(rx.try_recv().unwrap(), AgentCommand::Quit);
    }

    fn sample_agent(
        id: &str,
        source: DefinitionSource,
        description: Option<&str>,
        model: Option<&str>,
        prompt: &str,
    ) -> AgentDefinition {
        AgentDefinition {
            id: id.to_string(),
            source,
            description: description.map(str::to_string),
            model: model.map(str::to_string),
            steps: None,
            mode: None,
            prompt: prompt.to_string(),
            path: PathBuf::from(format!(".agents/agents/{id}.md")),
        }
    }

    fn sample_skill(
        id: &str,
        source: DefinitionSource,
        description: Option<&str>,
        body: &str,
    ) -> SkillDefinition {
        SkillDefinition {
            id: id.to_string(),
            source,
            description: description.map(str::to_string),
            body: body.to_string(),
            path: PathBuf::from(format!(".agents/skills/{id}/SKILL.md")),
        }
    }

    fn submit(app: &mut AppState, command: &str) {
        type_text(app, command);
        if app.completion.is_active() {
            app.handle_key_event(key(KeyCode::Esc));
        }
        app.handle_key_event(key(KeyCode::Enter));
    }

    #[test]
    fn mention_parser_applies_skills_then_agent_and_cleans_text() {
        let agents = vec![sample_agent(
            "reviewer",
            DefinitionSource::Agents,
            None,
            None,
            "Review.",
        )];
        let skills = vec![
            sample_skill("explore", DefinitionSource::Agents, None, "Explore."),
            sample_skill("fix", DefinitionSource::Claude, None, "Fix."),
        ];

        let plan = parse_mentions(
            "please @skill/explore and @skill/fix then @agent/reviewer do it",
            &agents,
            &skills,
        );
        assert_eq!(plan.skill_indices, vec![0, 1]);
        assert_eq!(plan.agent_indices, vec![0]);
        assert_eq!(plan.text.as_deref(), Some("please and then do it"));

        let deduped = parse_mentions(
            "@agent/reviewer @agent/reviewer @skill/fix @skill/fix",
            &agents,
            &skills,
        );
        assert_eq!(deduped.agent_indices, vec![0]);
        assert_eq!(deduped.skill_indices, vec![1]);
        assert_eq!(deduped.text.as_deref(), Some(""));

        let unknown = parse_mentions("@agent/nope @skill/nope keep", &agents, &skills);
        assert!(unknown.agent_indices.is_empty());
        assert!(unknown.skill_indices.is_empty());
        assert_eq!(unknown.text, None);
    }

    #[test]
    fn submitting_mention_applies_agent_then_prompt() {
        let (mut app, mut rx, _secrets) = test_app();
        app.agents = vec![sample_agent(
            "reviewer",
            DefinitionSource::Agents,
            Some("Reviews changes"),
            Some("gemini-3.1-pro-preview"),
            "Review the diff.",
        )];

        submit(&mut app, "@agent/reviewer do the task");

        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::ApplyAgent {
                name: "reviewer".to_string(),
                instructions: "Review the diff.".to_string(),
                model: Some("gemini::gemini-3.1-pro-preview".to_string()),
            }
        );
        assert_eq!(app.model, "gemini-3.1-pro-preview");
        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::SubmitPrompt("do the task".to_string())
        );
        assert!(app.is_running);
        assert_eq!(
            app.transcript.last(),
            Some(&TranscriptEntry::User {
                text: "do the task".to_string()
            })
        );
    }

    #[test]
    fn mention_popup_arrows_and_tab_insert_without_submitting() {
        let (mut app, mut rx, _secrets) = test_app();
        app.agents = vec![sample_agent(
            "reviewer",
            DefinitionSource::Agents,
            Some("Reviews changes"),
            None,
            "Review.",
        )];
        app.skills = vec![sample_skill(
            "reviewer",
            DefinitionSource::Claude,
            Some("Review skill"),
            "Skill.",
        )];

        type_text(&mut app, "@reviewer");
        assert!(app.completion.is_active());
        let items = app.completion.items();
        assert_eq!(items[0].label, "reviewer");
        assert_eq!(items[0].item_kind, CompletionItemKind::Agent);
        assert_eq!(items[1].label, "reviewer");
        assert_eq!(items[1].item_kind, CompletionItemKind::Skill);

        app.handle_key_event(key(KeyCode::Down));
        assert_eq!(app.completion.selected(), 1);
        app.handle_key_event(key(KeyCode::Up));
        assert_eq!(app.completion.selected(), 0);
        app.handle_key_event(key(KeyCode::Down));
        app.handle_key_event(key(KeyCode::Tab));

        assert_eq!(app.input, "@skill/reviewer ");
        assert_eq!(app.mode, InputMode::Insert);
        assert!(!app.is_running);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn enter_accepts_mention_without_submitting() {
        let (mut app, mut rx, _secrets) = test_app();
        app.agents = vec![sample_agent(
            "reviewer",
            DefinitionSource::Agents,
            Some("Reviews changes"),
            None,
            "Review.",
        )];

        type_text(&mut app, "@reviewer");
        assert!(app.completion.is_active());
        app.handle_key_event(key(KeyCode::Enter));

        assert_eq!(app.input, "@agent/reviewer ");
        assert!(!app.is_running);
        assert!(rx.try_recv().is_err());

        app.handle_key_event(key(KeyCode::Enter));
        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::ApplyAgent {
                name: "reviewer".to_string(),
                instructions: "Review.".to_string(),
                model: None,
            }
        );
        assert!(rx.try_recv().is_err());
        assert!(!app.is_running);
        assert!(app.input.is_empty());
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptEntry::System { text })
                if text == "Applied agent reviewer (.agents)."
        ));
    }

    #[test]
    fn only_mentions_apply_without_starting_a_turn() {
        let (mut app, mut rx, _secrets) = test_app();
        app.skills = vec![
            sample_skill("explore", DefinitionSource::Agents, None, "Explore."),
            sample_skill("fix", DefinitionSource::Claude, None, "Fix."),
        ];
        app.agents = vec![sample_agent(
            "reviewer",
            DefinitionSource::Claude,
            None,
            None,
            "Review.",
        )];

        submit(&mut app, "@skill/explore @skill/fix @agent/reviewer");

        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::ApplySkill {
                name: "explore".to_string(),
                instructions: "Explore.".to_string(),
            }
        );
        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::ApplySkill {
                name: "fix".to_string(),
                instructions: "Fix.".to_string(),
            }
        );
        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::ApplyAgent {
                name: "reviewer".to_string(),
                instructions: "Review.".to_string(),
                model: None,
            }
        );
        assert!(rx.try_recv().is_err());
        assert!(!app.is_running);
        assert!(app.input.is_empty());
        assert_eq!(
            app.transcript,
            vec![
                TranscriptEntry::System {
                    text: "Applied skill explore (.agents).".to_string()
                },
                TranscriptEntry::System {
                    text: "Applied skill fix (.claude).".to_string()
                },
                TranscriptEntry::System {
                    text: "Applied agent reviewer (.claude).".to_string()
                },
            ]
        );
    }

    #[test]
    fn unknown_mention_stays_in_the_prompt() {
        let (mut app, mut rx, _secrets) = test_app();
        app.agents = vec![sample_agent(
            "reviewer",
            DefinitionSource::Agents,
            None,
            None,
            "Review.",
        )];

        submit(&mut app, "@agent/nope do it");

        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::SubmitPrompt("@agent/nope do it".to_string())
        );
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptEntry::User { text }) if text == "@agent/nope do it"
        ));
    }

    #[test]
    fn repeated_mention_applies_once() {
        let (mut app, mut rx, _secrets) = test_app();
        app.skills = vec![sample_skill(
            "explore",
            DefinitionSource::Agents,
            None,
            "Explore.",
        )];

        submit(&mut app, "@skill/explore @skill/explore go");

        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::ApplySkill {
                name: "explore".to_string(),
                instructions: "Explore.".to_string(),
            }
        );
        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::SubmitPrompt("go".to_string())
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn mention_agent_model_with_provider_prefix_is_ignored() {
        let (mut app, mut rx, _secrets) = test_app();
        app.agents = vec![sample_agent(
            "reviewer",
            DefinitionSource::Agents,
            None,
            Some("openrouter:deepseek-v4.1"),
            "Review.",
        )];

        submit(&mut app, "@agent/reviewer go");

        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::ApplyAgent {
                name: "reviewer".to_string(),
                instructions: "Review.".to_string(),
                model: None,
            }
        );
        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::SubmitPrompt("go".to_string())
        );
        assert_eq!(app.model, "gemini-3.8-flash");
        assert!(app.transcript.iter().any(|entry| matches!(
            entry,
            TranscriptEntry::System { text } if text.contains("implies a provider change")
        )));
    }

    #[test]
    fn list_commands_are_removed_and_agent_shortcuts_remain() {
        let (mut app, _rx, _secrets) = test_app();
        app.agents = vec![sample_agent(
            "reviewer",
            DefinitionSource::Agents,
            None,
            None,
            "Review.",
        )];

        submit(&mut app, "/agents");
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptEntry::Error { text, .. })
                if text.contains("Unknown command: /agents")
        ));

        app.transcript.clear();
        submit(&mut app, "/skills");
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptEntry::Error { text, .. })
                if text.contains("Unknown command: /skills")
        ));

        app.transcript.clear();
        submit(&mut app, "/help");
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptEntry::System { text })
                if text.contains("/agent <id>")
                    && text.contains("@agent/<id>")
                    && !text.contains("/agents")
                    && !text.contains("/skills")
        ));
    }

    #[test]
    fn agent_command_sends_instructions_and_syncs_model() {
        let (mut app, mut rx, _secrets) = test_app();
        app.agents = vec![sample_agent(
            "reviewer",
            DefinitionSource::Agents,
            Some("Reviews changes"),
            Some("gemini-3.1-pro-preview"),
            "Review the diff.",
        )];

        submit(&mut app, "/agent reviewer");
        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::ApplyAgent {
                name: "reviewer".to_string(),
                instructions: "Review the diff.".to_string(),
                model: Some("gemini::gemini-3.1-pro-preview".to_string()),
            }
        );
        assert_eq!(app.model, "gemini-3.1-pro-preview");
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptEntry::System { text })
                if text == "Applied agent reviewer (.agents)."
        ));
    }

    #[test]
    fn skill_command_sends_body() {
        let (mut app, mut rx, _secrets) = test_app();
        app.skills = vec![sample_skill(
            "explore",
            DefinitionSource::Claude,
            Some("Search"),
            "Read-only exploration.",
        )];

        submit(&mut app, "/skill explore");
        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::ApplySkill {
                name: "explore".to_string(),
                instructions: "Read-only exploration.".to_string(),
            }
        );
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptEntry::System { text })
                if text == "Applied skill explore (.claude)."
        ));
    }

    #[test]
    fn unknown_definition_ids_error_and_send_nothing() {
        let (mut app, mut rx, _secrets) = test_app();
        app.agents = vec![sample_agent(
            "reviewer",
            DefinitionSource::Agents,
            None,
            None,
            "Review.",
        )];
        app.skills = vec![sample_skill(
            "explore",
            DefinitionSource::Agents,
            None,
            "Explore.",
        )];

        submit(&mut app, "/agent missing");
        assert!(rx.try_recv().is_err());
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptEntry::System { text })
                if text.contains("Unknown agent: missing") && text.contains("reviewer")
        ));

        submit(&mut app, "/agent");
        assert!(rx.try_recv().is_err());
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptEntry::System { text })
                if text.contains("Usage: /agent <id>") && text.contains("reviewer")
        ));

        submit(&mut app, "/skill missing");
        assert!(rx.try_recv().is_err());
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptEntry::System { text })
                if text.contains("Unknown skill: missing") && text.contains("explore")
        ));

        submit(&mut app, "/skill");
        assert!(rx.try_recv().is_err());
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptEntry::System { text })
                if text.contains("Usage: /skill <id>") && text.contains("explore")
        ));
    }

    #[test]
    fn agent_model_with_provider_prefix_is_not_applied() {
        let (mut app, mut rx, _secrets) = test_app();
        app.agents = vec![sample_agent(
            "reviewer",
            DefinitionSource::Claude,
            None,
            Some("openrouter:deepseek-v4.1"),
            "Review.",
        )];

        submit(&mut app, "/agent reviewer");
        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::ApplyAgent {
                name: "reviewer".to_string(),
                instructions: "Review.".to_string(),
                model: None,
            }
        );
        assert_eq!(app.model, "gemini-3.8-flash");
        assert!(app.transcript.iter().any(|entry| matches!(
            entry,
            TranscriptEntry::System { text } if text.contains("implies a provider change")
        )));
    }

    #[test]
    fn agent_event_streaming_updates_transcript_and_status() {
        let (mut app, _rx, _secrets) = test_app();

        let turn_id = TurnId::from_raw("turn-1");
        app.handle_agent_event(AgentEvent::TurnStarted {
            turn_id: turn_id.clone(),
            model: "openai::gpt-4o-mini".to_string(),
        });
        assert!(app.is_running);
        assert_eq!(app.model, "gpt-4o-mini");
        assert_eq!(app.turn_id, Some(turn_id));

        let reasoning_id = ItemId::from_raw("item-reasoning");
        app.handle_agent_event(AgentEvent::ItemStarted {
            item: SessionItem::Reasoning {
                id: reasoning_id.clone(),
                summary: String::new(),
            },
        });
        app.handle_agent_event(AgentEvent::ItemDelta {
            item_id: reasoning_id,
            delta: ItemDelta::ReasoningSummary("thinking...".to_string()),
        });

        let message_id = ItemId::from_raw("item-message");
        app.handle_agent_event(AgentEvent::ItemStarted {
            item: SessionItem::AgentMessage {
                id: message_id.clone(),
                text: String::new(),
            },
        });
        app.handle_agent_event(AgentEvent::ItemDelta {
            item_id: message_id.clone(),
            delta: ItemDelta::AgentText("Hello ".to_string()),
        });
        app.handle_agent_event(AgentEvent::ItemDelta {
            item_id: message_id.clone(),
            delta: ItemDelta::AgentText("world!".to_string()),
        });

        assert_eq!(
            app.transcript,
            vec![TranscriptEntry::Assistant {
                text: "Hello world!".to_string(),
                reasoning: "thinking...".to_string(),
                streaming: true,
                reasoning_expanded: false,
            }]
        );

        assert!(app.toggle_latest_thinking());
        assert!(matches!(
            app.transcript.first(),
            Some(TranscriptEntry::Assistant {
                reasoning_expanded: true,
                ..
            })
        ));

        let tool_item_id = ItemId::from_raw("item-tool");
        let call_id = meerai_agents::CallId::from_raw("c1");
        app.handle_agent_event(AgentEvent::ItemStarted {
            item: SessionItem::ToolCall {
                id: tool_item_id.clone(),
                call_id: call_id.clone(),
                name: "read_file".to_string(),
                args: "{\"path\":\"Cargo.toml\"}".to_string(),
                effect: EffectClass::Read,
                status: ItemStatus::Proposed,
                output: None,
            },
        });
        assert_eq!(app.active_tool.as_deref(), Some("read_file"));

        app.handle_agent_event(AgentEvent::ItemDelta {
            item_id: tool_item_id.clone(),
            delta: ItemDelta::StatusChanged(ItemStatus::InProgress),
        });
        assert_eq!(
            app.transcript.last(),
            Some(&TranscriptEntry::ToolCall(ToolCallEntry {
                call_id: "c1".to_string(),
                name: "read_file".to_string(),
                args: "{\"path\":\"Cargo.toml\"}".to_string(),
                status: ToolCallStatus::Running,
                output: None,
            }))
        );

        app.handle_agent_event(AgentEvent::ItemCompleted {
            item: SessionItem::ToolCall {
                id: tool_item_id,
                call_id,
                name: "read_file".to_string(),
                args: "{\"path\":\"Cargo.toml\"}".to_string(),
                effect: EffectClass::Read,
                status: ItemStatus::Succeeded,
                output: Some("ok".to_string()),
            },
        });
        assert!(app.active_tool.is_none());
        assert_eq!(
            app.transcript.last(),
            Some(&TranscriptEntry::ToolCall(ToolCallEntry {
                call_id: "c1".to_string(),
                name: "read_file".to_string(),
                args: "{\"path\":\"Cargo.toml\"}".to_string(),
                status: ToolCallStatus::Succeeded,
                output: Some("ok".to_string()),
            }))
        );

        app.handle_agent_event(AgentEvent::UsageUpdated {
            usage: Usage {
                prompt_tokens: Some(120),
                completion_tokens: Some(30),
                total_tokens: Some(150),
                cached_tokens: None,
            },
            cumulative: Usage {
                prompt_tokens: Some(120),
                completion_tokens: Some(30),
                total_tokens: Some(150),
                cached_tokens: None,
            },
            est_cost_usd: None,
        });
        assert_eq!(app.usage.total_tokens, Some(150));
        assert_eq!(app.metrics.session_total_tokens, 150);

        app.handle_agent_event(AgentEvent::ItemCompleted {
            item: SessionItem::AgentMessage {
                id: ItemId::from_raw("item-done"),
                text: "Done".to_string(),
            },
        });
        app.handle_agent_event(AgentEvent::TurnCompleted {
            turn_id: TurnId::from_raw("turn-1"),
            reason: TurnEndReason::Completed,
        });
        assert!(!app.is_running);
        assert_eq!(app.metrics.turn_count, 1);
        assert_eq!(
            app.transcript.last(),
            Some(&TranscriptEntry::Assistant {
                text: "Done".to_string(),
                reasoning: String::new(),
                streaming: false,
                reasoning_expanded: false,
            })
        );

        app.handle_agent_event(AgentEvent::Error {
            code: ErrorCode::Provider,
            message: "boom".to_string(),
        });
        assert_eq!(
            app.transcript.last(),
            Some(&TranscriptEntry::Error {
                text: "provider error: boom".to_string(),
                expanded: false,
            })
        );
        assert!(app.toggle_latest_thinking());
        assert_eq!(
            app.transcript.last(),
            Some(&TranscriptEntry::Error {
                text: "provider error: boom".to_string(),
                expanded: true,
            })
        );
    }

    #[test]
    fn authoritative_agent_message_replaces_streamed_text() {
        let (mut app, _rx, _secrets) = test_app();
        app.handle_agent_event(AgentEvent::TurnStarted {
            turn_id: TurnId::from_raw("turn-authoritative"),
            model: "openai::gpt-4o-mini".to_string(),
        });

        let id = ItemId::from_raw("item-message-authoritative");
        app.handle_agent_event(AgentEvent::ItemStarted {
            item: SessionItem::AgentMessage {
                id: id.clone(),
                text: String::new(),
            },
        });
        app.handle_agent_event(AgentEvent::ItemDelta {
            item_id: id.clone(),
            delta: ItemDelta::AgentText("streamed draft".to_string()),
        });
        app.handle_agent_event(AgentEvent::ItemCompleted {
            item: SessionItem::AgentMessage {
                id: id.clone(),
                text: "final answer".to_string(),
            },
        });
        assert_eq!(
            app.transcript,
            vec![TranscriptEntry::Assistant {
                text: "final answer".to_string(),
                reasoning: String::new(),
                streaming: false,
                reasoning_expanded: false,
            }]
        );

        let empty_id = ItemId::from_raw("item-message-empty");
        app.handle_agent_event(AgentEvent::ItemStarted {
            item: SessionItem::AgentMessage {
                id: empty_id.clone(),
                text: String::new(),
            },
        });
        app.handle_agent_event(AgentEvent::ItemDelta {
            item_id: empty_id.clone(),
            delta: ItemDelta::AgentText("kept stream".to_string()),
        });
        app.handle_agent_event(AgentEvent::ItemCompleted {
            item: SessionItem::AgentMessage {
                id: empty_id,
                text: String::new(),
            },
        });
        assert_eq!(
            app.transcript.last(),
            Some(&TranscriptEntry::Assistant {
                text: "kept stream".to_string(),
                reasoning: String::new(),
                streaming: false,
                reasoning_expanded: false,
            })
        );
    }

    #[test]
    fn aborted_and_failed_turns_render_terminal_entries() {
        let (mut app, _rx, _secrets) = test_app();

        app.handle_agent_event(AgentEvent::TurnStarted {
            turn_id: TurnId::from_raw("turn-abort"),
            model: "openai::gpt-4o-mini".to_string(),
        });
        app.handle_agent_event(AgentEvent::TurnCompleted {
            turn_id: TurnId::from_raw("turn-abort"),
            reason: TurnEndReason::Aborted {
                cause: AbortCause::User,
            },
        });
        assert!(!app.is_running);
        assert_eq!(
            app.transcript.last(),
            Some(&TranscriptEntry::System {
                text: "Turn cancelled.".to_string(),
            })
        );

        app.handle_agent_event(AgentEvent::TurnStarted {
            turn_id: TurnId::from_raw("turn-failed"),
            model: "openai::gpt-4o-mini".to_string(),
        });
        app.handle_agent_event(AgentEvent::Error {
            code: ErrorCode::Budget,
            message: "Exceeded maximum number of cycles (16)".to_string(),
        });
        app.handle_agent_event(AgentEvent::TurnCompleted {
            turn_id: TurnId::from_raw("turn-failed"),
            reason: TurnEndReason::Failed {
                error: "Exceeded maximum number of cycles (16)".to_string(),
            },
        });
        assert!(!app.is_running);
        assert_eq!(app.metrics.turn_count, 2);
        assert_eq!(app.transcript.len(), 2);
        assert_eq!(
            app.transcript.last(),
            Some(&TranscriptEntry::Error {
                text: "budget error: Exceeded maximum number of cycles (16)".to_string(),
                expanded: false,
            })
        );
    }

    #[test]
    fn compaction_items_render_system_entries() {
        let (mut app, _rx, _secrets) = test_app();
        app.handle_agent_event(AgentEvent::ItemCompleted {
            item: SessionItem::Compaction {
                id: ItemId::from_raw("item-compact"),
                trigger: meerai_agents::events::CompactionTrigger::Manual,
                freed_tokens: 2048,
            },
        });
        assert_eq!(
            app.transcript.last(),
            Some(&TranscriptEntry::System {
                text: "Compaction freed 2048 tokens".to_string(),
            })
        );
    }

    #[test]
    fn approval_request_opens_overlay_and_accept_resolves() {
        let (mut app, mut rx, _secrets) = test_app();
        app.handle_agent_event(AgentEvent::ApprovalRequested {
            request: approval_request("req-1", "src/main.rs"),
        });
        assert!(matches!(
            app.modal,
            Some(ActiveModal::Approval { ref queue, .. }) if queue.len() == 1
        ));

        app.handle_key_event(key(KeyCode::Char('y')));
        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::Approve {
                request_id: RequestId::from_raw("req-1"),
                decision: ApprovalDecision::Accept,
                scope: None,
            }
        );

        app.handle_agent_event(AgentEvent::ApprovalResolved {
            request_id: RequestId::from_raw("req-1"),
        });
        assert!(app.modal.is_none());
    }

    #[test]
    fn approval_turn_scope_key_sends_turn_grant() {
        let (mut app, mut rx, _secrets) = test_app();
        app.handle_agent_event(AgentEvent::ApprovalRequested {
            request: approval_request("req-turn", "src/main.rs"),
        });

        app.handle_key_event(key(KeyCode::Char('t')));
        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::Approve {
                request_id: RequestId::from_raw("req-turn"),
                decision: ApprovalDecision::Accept,
                scope: Some(GrantScope::Turn),
            }
        );
    }

    #[test]
    fn transcript_from_events_replays_known_entries_and_skips_runtime_events() {
        use meerai_core::ToolObservation;
        use meerai_session::{
            CallId, CompactionId, ContentBlock, InputSource, LoggedEvent, Seq, SeqRange,
            SessionEvent,
        };

        let events = vec![
            LoggedEvent {
                seq: Seq(1),
                event: SessionEvent::SystemMessage {
                    sections: Vec::new(),
                    generation: 0,
                },
            },
            LoggedEvent {
                seq: Seq(2),
                event: SessionEvent::UserMessage {
                    source: InputSource::RuntimeContext,
                    content: vec![ContentBlock::Text("runtime context".to_string())],
                },
            },
            LoggedEvent {
                seq: Seq(3),
                event: SessionEvent::UserMessage {
                    source: InputSource::User,
                    content: vec![ContentBlock::Text("hello".to_string())],
                },
            },
            LoggedEvent {
                seq: Seq(4),
                event: SessionEvent::AssistantMessage {
                    provider: "gemini".to_string(),
                    model: "gemini-3.8-flash".to_string(),
                    content: vec![
                        ContentBlock::Reasoning("thinking".to_string()),
                        ContentBlock::Text("answer".to_string()),
                    ],
                    tool_calls: Vec::new(),
                    usage: Usage::default(),
                    stream_digest: meerai_session::StreamDigest::default(),
                },
            },
            LoggedEvent {
                seq: Seq(5),
                event: SessionEvent::ToolCall {
                    call_id: CallId::from_raw("call-1"),
                    name: "code-read_file".to_string(),
                    args: "{\"path\":\"src/lib.rs\"}".to_string(),
                },
            },
            LoggedEvent {
                seq: Seq(6),
                event: SessionEvent::ToolResult {
                    call_id: CallId::from_raw("call-1"),
                    output: ToolObservation::success("file contents"),
                },
            },
            LoggedEvent {
                seq: Seq(7),
                event: SessionEvent::ToolCall {
                    call_id: CallId::from_raw("call-2"),
                    name: "code-shell".to_string(),
                    args: "{}".to_string(),
                },
            },
            LoggedEvent {
                seq: Seq(8),
                event: SessionEvent::CompactionSummary {
                    compaction_id: CompactionId::from_raw("cmp-1"),
                    replaced: SeqRange {
                        start: Seq(1),
                        end_inclusive: Seq(4),
                    },
                    summary: "summary".to_string(),
                },
            },
            LoggedEvent {
                seq: Seq(9),
                event: SessionEvent::StepStarted {
                    step_id: meerai_session::StepId::from_raw("step-1"),
                    turn_id: meerai_session::TurnId::from_raw("turn-1"),
                },
            },
        ];

        assert_eq!(
            transcript_from_events(&events),
            vec![
                TranscriptEntry::User {
                    text: "hello".to_string(),
                },
                TranscriptEntry::Assistant {
                    text: "answer".to_string(),
                    reasoning: "thinking".to_string(),
                    streaming: false,
                    reasoning_expanded: false,
                },
                TranscriptEntry::ToolCall(ToolCallEntry {
                    call_id: "call-1".to_string(),
                    name: "code-read_file".to_string(),
                    args: "{\"path\":\"src/lib.rs\"}".to_string(),
                    status: ToolCallStatus::Succeeded,
                    output: Some("file contents".to_string()),
                }),
                TranscriptEntry::ToolCall(ToolCallEntry {
                    call_id: "call-2".to_string(),
                    name: "code-shell".to_string(),
                    args: "{}".to_string(),
                    status: ToolCallStatus::Cancelled,
                    output: None,
                }),
                TranscriptEntry::System {
                    text: "Compacted history".to_string(),
                },
            ]
        );
    }

    #[test]
    fn transcript_from_events_marks_unmatched_tool_calls_cancelled() {
        use meerai_session::{CallId, LoggedEvent, Seq, SessionEvent};

        let events = vec![LoggedEvent {
            seq: Seq(1),
            event: SessionEvent::ToolCall {
                call_id: CallId::from_raw("call-crashed"),
                name: "code-shell".to_string(),
                args: "{}".to_string(),
            },
        }];

        assert_eq!(
            transcript_from_events(&events),
            vec![TranscriptEntry::ToolCall(ToolCallEntry {
                call_id: "call-crashed".to_string(),
                name: "code-shell".to_string(),
                args: "{}".to_string(),
                status: ToolCallStatus::Cancelled,
                output: None,
            })]
        );
    }

    #[test]
    fn session_usage_from_events_returns_last_cumulative_usage() {
        use meerai_session::{LoggedEvent, Seq, SessionEvent};

        let events = vec![
            LoggedEvent {
                seq: Seq(1),
                event: SessionEvent::UsageUpdated {
                    usage: Usage::default(),
                    cumulative: Usage {
                        prompt_tokens: Some(10),
                        completion_tokens: Some(5),
                        total_tokens: Some(15),
                        cached_tokens: None,
                    },
                },
            },
            LoggedEvent {
                seq: Seq(2),
                event: SessionEvent::UsageUpdated {
                    usage: Usage::default(),
                    cumulative: Usage {
                        prompt_tokens: Some(20),
                        completion_tokens: Some(7),
                        total_tokens: Some(27),
                        cached_tokens: None,
                    },
                },
            },
        ];

        assert_eq!(
            session_usage_from_events(&events),
            Some(Usage {
                prompt_tokens: Some(20),
                completion_tokens: Some(7),
                total_tokens: Some(27),
                cached_tokens: None,
            })
        );
        assert_eq!(session_usage_from_events(&[]), None);
    }

    #[test]
    fn approval_queue_keeps_order_and_closes_after_all_resolved() {
        let (mut app, mut rx, _secrets) = test_app();
        app.handle_agent_event(AgentEvent::ApprovalRequested {
            request: approval_request("req-1", "src/a.rs"),
        });
        app.handle_agent_event(AgentEvent::ApprovalRequested {
            request: approval_request("req-2", "src/b.rs"),
        });
        let Some(ActiveModal::Approval { queue, .. }) = &app.modal else {
            panic!("expected approval overlay");
        };
        assert_eq!(queue.len(), 2);
        assert_eq!(queue[0].request_id, RequestId::from_raw("req-1"));
        assert_eq!(queue[0].preview.title, "edit: src/a.rs");

        app.handle_key_event(key(KeyCode::Char('a')));
        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::Approve {
                request_id: RequestId::from_raw("req-1"),
                decision: ApprovalDecision::AcceptForSession,
                scope: None,
            }
        );

        app.handle_agent_event(AgentEvent::ApprovalResolved {
            request_id: RequestId::from_raw("req-1"),
        });
        assert!(matches!(
            app.modal,
            Some(ActiveModal::Approval { ref queue, .. }) if queue.len() == 1
        ));

        app.handle_agent_event(AgentEvent::ApprovalResolved {
            request_id: RequestId::from_raw("req-2"),
        });
        assert!(app.modal.is_none());
    }

    #[test]
    fn approval_decline_with_reason_and_cancel_keys() {
        let (mut app, mut rx, _secrets) = test_app();
        app.handle_agent_event(AgentEvent::ApprovalRequested {
            request: approval_request("req-1", "src/main.rs"),
        });

        app.handle_key_event(key(KeyCode::Char('r')));
        assert!(matches!(
            app.modal,
            Some(ActiveModal::Approval {
                editing_reason: true,
                ..
            })
        ));
        type_text(&mut app, "risky change");
        app.handle_key_event(key(KeyCode::Enter));
        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::Approve {
                request_id: RequestId::from_raw("req-1"),
                decision: ApprovalDecision::DeclineWithReason("risky change".to_string()),
                scope: None,
            }
        );

        app.handle_agent_event(AgentEvent::ApprovalResolved {
            request_id: RequestId::from_raw("req-1"),
        });
        app.handle_agent_event(AgentEvent::ApprovalRequested {
            request: approval_request("req-2", "src/lib.rs"),
        });
        app.handle_key_event(key(KeyCode::Esc));
        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::Approve {
                request_id: RequestId::from_raw("req-2"),
                decision: ApprovalDecision::Cancel,
                scope: None,
            }
        );
    }

    #[test]
    fn compact_and_mode_commands_emit_agent_commands() {
        let (mut app, mut rx, _secrets) = test_app();

        type_text(&mut app, "/compact");
        app.handle_key_event(key(KeyCode::Enter));
        assert_eq!(rx.try_recv().unwrap(), AgentCommand::Compact);
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptEntry::System { text }) if text.contains("Compaction requested")
        ));

        type_text(&mut app, "/mode ");
        app.handle_key_event(key(KeyCode::Enter));
        assert!(rx.try_recv().is_err());
        assert_eq!(app.approval_mode, ApprovalMode::Ask);
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptEntry::System { text }) if text == "Approval mode: ask"
        ));

        type_text(&mut app, "/mode auto-edit");
        app.handle_key_event(key(KeyCode::Enter));
        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::SetPermissionMode(ApprovalMode::AutoEdit)
        );
        assert_eq!(app.approval_mode, ApprovalMode::AutoEdit);

        type_text(&mut app, "/mode nonsense");
        app.handle_key_event(key(KeyCode::Enter));
        assert!(rx.try_recv().is_err());
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptEntry::Error { text, .. }) if text.contains("Invalid mode")
        ));
    }

    #[test]
    fn steer_while_running_sends_steer_and_keeps_running() {
        let (mut app, mut rx, _secrets) = test_app();
        app.is_running = true;
        app.turn_id = Some(TurnId::from_raw("turn-steer"));

        type_text(&mut app, "focus on tests");
        app.handle_key_event(key(KeyCode::Enter));

        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::Steer("focus on tests".to_string())
        );
        assert!(app.is_running);
        assert_eq!(app.turn_id, Some(TurnId::from_raw("turn-steer")));
        assert!(app.input.is_empty());
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptEntry::User { text }) if text == "focus on tests"
        ));
    }

    fn approval_request(id: &str, path: &str) -> ApprovalRequest {
        ApprovalRequest {
            request_id: RequestId::from_raw(id),
            turn_id: TurnId::from_raw("turn-approval"),
            kind: ApprovalKind::ToolCall,
            preview: ApprovalPreview {
                title: format!("edit: {path}"),
                detail: format!("tool: code-write_file\neffect: write\nresource: {path}"),
            },
            effect: Some(EffectClass::Write),
        }
    }

    #[test]
    fn parses_git_status_and_handles_mouse_thinking_toggle() {
        let parsed = parse_git_status_porcelain(" M meerai-tui/src/ui.rs\n?? notes.md\nA  a.txt\n");
        assert_eq!(
            parsed,
            vec![
                FileChangeEntry {
                    status: "M".to_string(),
                    path: "meerai-tui/src/ui.rs".to_string(),
                },
                FileChangeEntry {
                    status: "??".to_string(),
                    path: "notes.md".to_string(),
                },
                FileChangeEntry {
                    status: "A".to_string(),
                    path: "a.txt".to_string(),
                },
            ]
        );

        let (mut app, _rx, _secrets) = test_app();
        app.transcript.push(TranscriptEntry::Assistant {
            text: "Hello!".to_string(),
            reasoning: "line 1\nline 2".to_string(),
            streaming: false,
            reasoning_expanded: false,
        });
        app.transcript_inner_area = Rect::new(1, 1, 60, 10);
        app.thinking_hit_zones = vec![ThinkingHitZone {
            entry_idx: 0,
            start_line: 0,
            end_line: 1,
        }];

        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 1,
            modifiers: KeyModifiers::NONE,
        };
        assert!(app.handle_mouse_event(click));
        assert!(matches!(
            app.transcript.first(),
            Some(TranscriptEntry::Assistant {
                reasoning_expanded: true,
                ..
            })
        ));
    }

    #[test]
    fn clicking_file_change_opens_diff_modal_and_search_attaches_file() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "meerai_app_diff_search_{}_{}",
            std::process::id(),
            nanos
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/demo.rs"),
            "fn demo() {\n    let target = 1;\n}\n",
        )
        .unwrap();

        let (tx, _rx) = mpsc::unbounded_channel();
        let secrets = Arc::new(MemorySecretStore::new());
        secrets.set_key("gemini", "test-key").unwrap();
        let settings = AppSettings::default();
        let runtime = Arc::new(RwLock::new(RuntimeProviderState::from_settings(
            &settings,
            ProviderCatalog::new(),
        )));
        let mut app = AppState::new(
            "gemini",
            "gemini-3.8-flash",
            root.clone(),
            tx,
            runtime,
            secrets,
            settings,
            None,
        );

        app.handle_terminal_event(Event::Resize(90, 30));
        assert!(!app.is_sidebar_visible());
        app.handle_key_event(key_mod(KeyCode::Char('b'), KeyModifiers::CONTROL));
        app.handle_key_event(key_mod(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert!(app.status_notice.is_some());

        app.handle_terminal_event(Event::Resize(120, 30));
        assert!(app.is_sidebar_visible());

        app.file_changes.push(FileChangeEntry {
            status: "??".to_string(),
            path: "src/demo.rs".to_string(),
        });
        app.sidebar_changes_inner_area = Rect::new(85, 16, 32, 10);
        app.visible_file_change_rows = 1;

        let file_click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 90,
            row: 16,
            modifiers: KeyModifiers::NONE,
        };
        assert!(app.handle_mouse_event(file_click));
        assert!(matches!(
            app.modal,
            Some(ActiveModal::DiffView {
                file_index: 0,
                ref diff,
                ..
            }) if diff.path == "src/demo.rs" && diff.additions == 3
        ));

        app.handle_key_event(key(KeyCode::Esc));
        assert!(app.modal.is_none());

        app.handle_key_event(key_mod(KeyCode::Char('f'), KeyModifiers::CONTROL));
        assert!(matches!(
            app.modal,
            Some(ActiveModal::Search {
                mode: SearchMode::Files,
                ..
            })
        ));

        app.handle_key_event(key(KeyCode::Tab));
        assert!(matches!(
            app.modal,
            Some(ActiveModal::Search {
                mode: SearchMode::Content,
                ..
            })
        ));
        type_text(&mut app, "target");
        assert!(matches!(
            app.modal,
            Some(ActiveModal::Search { ref results, .. }) if results.len() == 1 && results[0].line == Some(2)
        ));

        app.handle_key_event(key(KeyCode::Enter));
        assert!(app.modal.is_none());
        assert_eq!(app.input, "@src/demo.rs#L2 ");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn detached_search_opens_immediately_from_slash_space_and_normal_mode() {
        let (mut app, _rx, _secrets) = test_app();

        type_text(&mut app, "/se");
        app.handle_key_event(key(KeyCode::Enter));
        assert!(matches!(
            app.modal,
            Some(ActiveModal::Search {
                mode: SearchMode::Files,
                ..
            })
        ));
        assert!(app.input.is_empty());

        app.handle_key_event(key(KeyCode::Esc));
        assert!(app.modal.is_none());

        type_text(&mut app, "/grep ");
        assert!(matches!(
            app.modal,
            Some(ActiveModal::Search {
                mode: SearchMode::Content,
                ..
            })
        ));
        assert!(app.input.is_empty());

        app.handle_key_event(key(KeyCode::Esc));
        type_text(&mut app, "draft prompt");
        app.handle_key_event(key(KeyCode::Esc));
        assert_eq!(app.mode, InputMode::Normal);

        app.handle_key_event(key(KeyCode::Char('/')));
        assert!(matches!(
            app.modal,
            Some(ActiveModal::Search {
                mode: SearchMode::Files,
                ..
            })
        ));
        assert_eq!(app.input, "draft prompt");
    }

    #[test]
    fn slash_auth_lists_configured_credential_sources() {
        let (mut app, _rx, secrets) = test_app();
        secrets.set_key("openai", "sk-stored-12345678").unwrap();
        let before_len = app.transcript.len();

        type_text(&mut app, "/auth");
        app.handle_key_event(key(KeyCode::Enter));

        assert_eq!(app.transcript.len(), before_len + 1);
        let Some(TranscriptEntry::System { text }) = app.transcript.last() else {
            panic!("expected system transcript entry for /auth");
        };
        assert!(text.contains("Credentials ("));
        assert!(text.contains("openai"));
        assert!(text.contains("sk-s...5678"));
    }

    #[test]
    fn drag_selection_auto_copies_within_pane_and_normal_yank_works() {
        let (mut app, _rx, _secrets) = test_app();
        app.terminal_width = 30;
        app.transcript_inner_area = Rect::new(1, 1, 20, 4);
        let mk_row = |s: &str| -> Vec<String> {
            let mut row: Vec<String> = s.chars().map(|c| c.to_string()).collect();
            row.resize(30, " ".to_string());
            row
        };
        app.screen_lines = vec![
            mk_row("┌────────────────────┐┌──────┐"),
            mk_row("│┃ user prompt       ││side1 │"),
            mk_row("│  assistant reply   ││side2 │"),
            mk_row("└────────────────────┘└──────┘"),
        ];

        assert!(app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        }));
        assert!(app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 25,
            row: 2,
            modifiers: KeyModifiers::NONE,
        }));
        assert!(matches!(
            app.selection,
            Some(TextSelection { active: true, .. })
        ));

        assert!(app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 25,
            row: 2,
            modifiers: KeyModifiers::NONE,
        }));
        assert!(app.selection.is_none());
        let copied = app.last_copied_text.as_deref().unwrap_or("");
        assert_eq!(copied, "user prompt\n  assistant reply");
        assert!(!copied.contains("side1"));
        assert!(!copied.contains("│"));
        assert!(
            app.status_notice
                .as_deref()
                .is_some_and(|n| n.contains("Copied"))
        );

        app.transcript.push(TranscriptEntry::Assistant {
            text: "Latest assistant answer".to_string(),
            reasoning: String::new(),
            streaming: false,
            reasoning_expanded: false,
        });
        app.mode = InputMode::Normal;
        assert!(app.handle_key_event(key(KeyCode::Char('y'))));
        assert_eq!(
            app.last_copied_text.as_deref(),
            Some("Latest assistant answer")
        );

        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"f"), "Zg==");
    }
}
