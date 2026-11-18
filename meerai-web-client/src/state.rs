use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::text::{TextBuffer, UiEffects};
use meerai_web_protocol::{
    ApprovalModeDto, ApprovalRequestDto, BackendKind, ChatGptLoginStateDto, ContextSettingsDto,
    FileChangeDto, FileDiffResponse, HostInfoDto, ModelItemDto, ProviderItemDto, ProvidersResponse,
    QuestionRequestDto, ReloadResponse, RewindScopeDto, SandboxSettingsDto, SessionDetailDto,
    SessionHistoryResponse, SessionSettingsDto, SessionStatus, SessionSummaryDto, SnapshotResponse,
    StoredSessionDto, ThemeOverrideDto, TranscriptEntryDto, TurnMetricsDto, UpdateSettingsRequest,
    UsageDto, ViewEvent, WorkspacePreviewResponse, WorkspaceSearchResponse,
};

pub const SIDEBAR_BREAKPOINT_WIDTH: f32 = 900.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Reconnecting,
    Disconnected,
    Resyncing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventEffect {
    None,
    Resync,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlashCommandSpec {
    pub cmd: &'static str,
    pub args: &'static str,
    pub desc: &'static str,
    pub takes_arg: bool,
}

pub const SLASH_COMMANDS: &[SlashCommandSpec] = &[
    SlashCommandSpec {
        cmd: "/help",
        args: "",
        desc: "Show commands and browser shortcuts",
        takes_arg: false,
    },
    SlashCommandSpec {
        cmd: "/sessions",
        args: "[id]",
        desc: "Browse, switch, fork, or delete sessions",
        takes_arg: true,
    },
    SlashCommandSpec {
        cmd: "/new",
        args: "",
        desc: "Start a new native session",
        takes_arg: false,
    },
    SlashCommandSpec {
        cmd: "/fork",
        args: "",
        desc: "Fork the current session from a turn",
        takes_arg: false,
    },
    SlashCommandSpec {
        cmd: "/acp",
        args: "[preset]",
        desc: "Start an ACP session (e.g. agy)",
        takes_arg: true,
    },
    SlashCommandSpec {
        cmd: "/close",
        args: "",
        desc: "Close and shut down the active session",
        takes_arg: false,
    },
    SlashCommandSpec {
        cmd: "/auth",
        args: "",
        desc: "Manage provider credentials",
        takes_arg: false,
    },
    SlashCommandSpec {
        cmd: "/connect",
        args: "",
        desc: "Connect a provider",
        takes_arg: false,
    },
    SlashCommandSpec {
        cmd: "/provider",
        args: "",
        desc: "Switch or configure provider",
        takes_arg: false,
    },
    SlashCommandSpec {
        cmd: "/models",
        args: "[name]",
        desc: "Choose a model",
        takes_arg: true,
    },
    SlashCommandSpec {
        cmd: "/model",
        args: "[name]",
        desc: "Switch active model",
        takes_arg: true,
    },
    SlashCommandSpec {
        cmd: "/agent",
        args: "<id>",
        desc: "Apply an agent definition to this session",
        takes_arg: true,
    },
    SlashCommandSpec {
        cmd: "/agents",
        args: "",
        desc: "Pick an @agent mention",
        takes_arg: false,
    },
    SlashCommandSpec {
        cmd: "/skill",
        args: "<id>",
        desc: "Apply a skill definition to this session",
        takes_arg: true,
    },
    SlashCommandSpec {
        cmd: "/skills",
        args: "",
        desc: "Pick a @skill mention",
        takes_arg: false,
    },
    SlashCommandSpec {
        cmd: "/search",
        args: "[query]",
        desc: "Search workspace files or content",
        takes_arg: true,
    },
    SlashCommandSpec {
        cmd: "/tv",
        args: "[query]",
        desc: "Search workspace files or content",
        takes_arg: true,
    },
    SlashCommandSpec {
        cmd: "/find",
        args: "[query]",
        desc: "Find workspace files by path",
        takes_arg: true,
    },
    SlashCommandSpec {
        cmd: "/grep",
        args: "[query]",
        desc: "Search workspace file contents",
        takes_arg: true,
    },
    SlashCommandSpec {
        cmd: "/diff",
        args: "[path]",
        desc: "Inspect workspace git diffs",
        takes_arg: true,
    },
    SlashCommandSpec {
        cmd: "/sidebar",
        args: "",
        desc: "Toggle the right details sidebar",
        takes_arg: false,
    },
    SlashCommandSpec {
        cmd: "/clear",
        args: "",
        desc: "Reset conversation history",
        takes_arg: false,
    },
    SlashCommandSpec {
        cmd: "/reset",
        args: "",
        desc: "Reset conversation history",
        takes_arg: false,
    },
    SlashCommandSpec {
        cmd: "/cancel",
        args: "",
        desc: "Interrupt the active turn",
        takes_arg: false,
    },
    SlashCommandSpec {
        cmd: "/compact",
        args: "",
        desc: "Compact conversation history",
        takes_arg: false,
    },
    SlashCommandSpec {
        cmd: "/mode",
        args: "[plan|ask|auto]",
        desc: "Inspect or set approval mode",
        takes_arg: true,
    },
    SlashCommandSpec {
        cmd: "/rewind",
        args: "[turn]",
        desc: "Rewind files and/or conversation to a turn",
        takes_arg: true,
    },
    SlashCommandSpec {
        cmd: "/reload",
        args: "",
        desc: "Reload settings, theme, hooks, and definitions",
        takes_arg: false,
    },
    SlashCommandSpec {
        cmd: "/settings",
        args: "",
        desc: "Configure theme, context, sessions, and sandbox",
        takes_arg: false,
    },
    SlashCommandSpec {
        cmd: "/update",
        args: "",
        desc: "Show version and update instructions",
        takes_arg: false,
    },
    SlashCommandSpec {
        cmd: "/quit",
        args: "",
        desc: "Session close info",
        takes_arg: false,
    },
    SlashCommandSpec {
        cmd: "/serve",
        args: "",
        desc: "Show active web server status note",
        takes_arg: false,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    Command,
    Model,
    Mention,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MentionCategory {
    Agent,
    Skill,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub insert: String,
    pub detail: String,
    pub category: Option<MentionCategory>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompletionState {
    pub active: bool,
    pub kind: Option<CompletionKind>,
    pub replace_start: usize,
    pub replace_end: usize,
    pub query: String,
    pub items: Vec<CompletionItem>,
    pub selected: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsSection {
    Connections,
    Session,
    Theme,
    Context,
    Sessions,
    Sandbox,
    Permissions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaletteActionSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
}

pub const PALETTE_ACTIONS: &[PaletteActionSpec] = &[
    PaletteActionSpec {
        key: "copy-last",
        label: "Copy last reply",
        hint: "Y",
    },
    PaletteActionSpec {
        key: "next-tab",
        label: "Next session tab",
        hint: "Alt+Right",
    },
    PaletteActionSpec {
        key: "prev-tab",
        label: "Previous session tab",
        hint: "Alt+Left",
    },
    PaletteActionSpec {
        key: "move-tab-left",
        label: "Move tab left",
        hint: "Alt+Shift+Left",
    },
    PaletteActionSpec {
        key: "move-tab-right",
        label: "Move tab right",
        hint: "Alt+Shift+Right",
    },
    PaletteActionSpec {
        key: "cycle-mode",
        label: "Cycle approval mode",
        hint: "Alt+M",
    },
];

pub fn filtered_palette_actions(filter: &str) -> Vec<&'static PaletteActionSpec> {
    let q = filter.trim().to_ascii_lowercase();
    PALETTE_ACTIONS
        .iter()
        .filter(|a| {
            q.is_empty()
                || a.label.to_ascii_lowercase().contains(&q)
                || a.hint.to_ascii_lowercase().contains(&q)
        })
        .collect()
}

pub fn copy_last_reply(state: &AppState, tx: &mut UiEffects) -> bool {
    let Some(tab) = state.active_tab() else {
        return false;
    };
    if let Some(TranscriptEntryDto::Assistant { text, .. }) = tab
        .entries
        .iter()
        .rev()
        .find(|e| matches!(e, TranscriptEntryDto::Assistant { .. }))
    {
        if text.trim().is_empty() {
            return false;
        }
        tx.copy(text.clone());
        return true;
    }
    false
}

pub fn execute_palette_action(state: &mut AppState, tx: &mut UiEffects, key: &str) {
    match key {
        "copy-last" => {
            if copy_last_reply(state, tx) {
                state.status_notice = Some("Copied the latest reply".to_owned());
            } else {
                state.status_notice = Some("No assistant reply to copy".to_owned());
            }
        }
        "next-tab" => cycle_tab(state, 1),
        "prev-tab" => cycle_tab(state, -1),
        "move-tab-left" => reorder_active_tab(state, -1),
        "move-tab-right" => reorder_active_tab(state, 1),
        "cycle-mode" => cycle_approval_mode(state),
        _ => {}
    }
}

#[derive(Clone, Debug)]
pub struct SettingsFormState {
    pub section: SettingsSection,
    pub reset_theme: bool,
    pub theme_fields: BTreeMap<&'static str, TextBuffer>,
    pub inputs: BTreeMap<&'static str, TextBuffer>,
    pub reasoning_pruning: bool,
    pub prompt_caching: bool,
    pub rollback_on_verify_failure: bool,
    pub post_edit_diagnostics: bool,
    pub require_for_scripts: bool,
    pub require_for_shell: bool,
    pub strict_env: bool,
    pub status: Option<String>,
    pub busy: bool,
}

impl SettingsFormState {
    pub fn from_host(info: &HostInfoDto) -> Self {
        let s = &info.settings;
        let mut theme_fields = BTreeMap::new();
        let raw = &s.raw_theme;
        let pairs: [(&'static str, &Option<String>); 17] = [
            ("background", &raw.background),
            ("surface", &raw.surface),
            ("surface_high", &raw.surface_high),
            ("text", &raw.text),
            ("muted", &raw.muted),
            ("faint", &raw.faint),
            ("accent", &raw.accent),
            ("interactive", &raw.interactive),
            ("border", &raw.border),
            ("error", &raw.error),
            ("warning", &raw.warning),
            ("success", &raw.success),
            ("info", &raw.info),
            ("diff_added", &raw.diff_added),
            ("diff_removed", &raw.diff_removed),
            ("diff_context", &raw.diff_context),
            ("code_function", &raw.code_function),
        ];
        for (k, v) in pairs {
            theme_fields.insert(k, TextBuffer::new(v.clone().unwrap_or_default()));
        }
        let c = &s.context;
        let opt_u64 = |v: Option<u64>| v.map(|n| n.to_string()).unwrap_or_default();
        let opt_usize = |v: Option<usize>| v.map(|n| n.to_string()).unwrap_or_default();
        let mut inputs = BTreeMap::new();
        for (k, val) in [
            ("context_window", opt_u64(c.context_window_tokens)),
            (
                "high_watermark_pct",
                c.high_watermark_pct
                    .map(|n| n.to_string())
                    .unwrap_or_default(),
            ),
            ("spill_max_inline", opt_u64(c.spill_max_inline_tokens)),
            ("verbatim_window_turns", opt_usize(c.verbatim_window_turns)),
            ("max_steps_per_turn", opt_usize(c.max_steps_per_turn)),
            ("tool_timeout_ms", opt_u64(c.tool_timeout_ms)),
            ("max_session_tokens", opt_u64(c.max_session_tokens)),
            ("max_turn_secs", opt_u64(c.max_turn_secs)),
            (
                "recovery_model",
                c.recovery_model.clone().unwrap_or_default(),
            ),
            (
                "summarizer_model",
                c.summarizer_model.clone().unwrap_or_default(),
            ),
            (
                "temperature",
                c.temperature.map(|f| format!("{f}")).unwrap_or_default(),
            ),
            ("auto_prune_days", opt_u64(s.session.auto_prune_days)),
            ("session_keep", opt_usize(s.session.keep)),
        ] {
            inputs.insert(k, TextBuffer::new(val));
        }
        Self {
            section: SettingsSection::Session,
            reset_theme: false,
            theme_fields,
            inputs,
            reasoning_pruning: c.reasoning_pruning.unwrap_or(true),
            prompt_caching: c.prompt_caching.unwrap_or(true),
            rollback_on_verify_failure: c.rollback_on_verify_failure.unwrap_or(false),
            post_edit_diagnostics: c.post_edit_diagnostics.unwrap_or(false),
            require_for_scripts: s.sandbox.require_for_scripts.unwrap_or(false),
            require_for_shell: s.sandbox.require_for_shell.unwrap_or(false),
            strict_env: s.sandbox.strict_env.unwrap_or(false),
            status: None,
            busy: false,
        }
    }

    pub fn field_text(&self, key: &str) -> &str {
        self.inputs.get(key).map(|e| e.text.as_str()).unwrap_or("")
    }

    pub fn to_request(&self) -> Result<UpdateSettingsRequest, String> {
        let parse_u64 = |label: &str, key: &str| -> Result<Option<u64>, String> {
            let t = self.field_text(key).trim();
            if t.is_empty() {
                Ok(None)
            } else {
                t.parse::<u64>()
                    .map(Some)
                    .map_err(|_| format!("Invalid {label}: expected positive integer"))
            }
        };
        let parse_usize = |label: &str, key: &str| -> Result<Option<usize>, String> {
            let t = self.field_text(key).trim();
            if t.is_empty() {
                Ok(None)
            } else {
                t.parse::<usize>()
                    .map(Some)
                    .map_err(|_| format!("Invalid {label}: expected integer"))
            }
        };
        let parse_pct = |key: &str| -> Result<Option<u8>, String> {
            let t = self.field_text(key).trim();
            if t.is_empty() {
                Ok(None)
            } else {
                let v = t
                    .parse::<u8>()
                    .map_err(|_| "Invalid high watermark %".to_owned())?;
                if v == 0 || v > 100 {
                    return Err("High watermark % must be 1..=100".to_owned());
                }
                Ok(Some(v))
            }
        };
        let parse_temp = |key: &str| -> Result<Option<f32>, String> {
            let t = self.field_text(key).trim();
            if t.is_empty() {
                Ok(None)
            } else {
                let v = t
                    .parse::<f32>()
                    .map_err(|_| "Invalid temperature (0.0..=2.0)".to_owned())?;
                if !(0.0..=2.0).contains(&v) {
                    return Err("Temperature must be between 0.0 and 2.0".to_owned());
                }
                Ok(Some(v))
            }
        };

        let opt_hex = |key: &str| -> Result<Option<String>, String> {
            let v = self
                .theme_fields
                .get(key)
                .map(|e| e.text.trim().to_owned())
                .unwrap_or_default();
            if v.is_empty() {
                return Ok(None);
            }
            if !(v.starts_with('#')
                && (v.len() == 7 || v.len() == 4)
                && v[1..].chars().all(|c| c.is_ascii_hexdigit()))
            {
                return Err(format!("Invalid hex color for {key}: {v}"));
            }
            Ok(Some(v))
        };

        let theme = if self.reset_theme {
            None
        } else {
            Some(ThemeOverrideDto {
                background: opt_hex("background")?,
                surface: opt_hex("surface")?,
                surface_high: opt_hex("surface_high")?,
                text: opt_hex("text")?,
                muted: opt_hex("muted")?,
                faint: opt_hex("faint")?,
                accent: opt_hex("accent")?,
                interactive: opt_hex("interactive")?,
                border: opt_hex("border")?,
                error: opt_hex("error")?,
                warning: opt_hex("warning")?,
                success: opt_hex("success")?,
                info: opt_hex("info")?,
                diff_added: opt_hex("diff_added")?,
                diff_removed: opt_hex("diff_removed")?,
                diff_context: opt_hex("diff_context")?,
                code_function: opt_hex("code_function")?,
            })
        };

        let opt_str = |key: &str| {
            let t = self.field_text(key).trim();
            (!t.is_empty()).then(|| t.to_owned())
        };

        Ok(UpdateSettingsRequest {
            reset_theme: self.reset_theme,
            theme,
            context: Some(ContextSettingsDto {
                context_window_tokens: parse_u64("context window", "context_window")?,
                high_watermark_pct: parse_pct("high_watermark_pct")?,
                spill_max_inline_tokens: parse_u64("spill threshold", "spill_max_inline")?,
                verbatim_window_turns: parse_usize("verbatim turns", "verbatim_window_turns")?,
                reasoning_pruning: Some(self.reasoning_pruning),
                prompt_caching: Some(self.prompt_caching),
                rollback_on_verify_failure: Some(self.rollback_on_verify_failure),
                recovery_model: opt_str("recovery_model"),
                max_steps_per_turn: parse_usize("max steps/turn", "max_steps_per_turn")?,
                tool_timeout_ms: parse_u64("tool timeout ms", "tool_timeout_ms")?,
                max_session_tokens: parse_u64("max session tokens", "max_session_tokens")?,
                max_turn_secs: parse_u64("max turn secs", "max_turn_secs")?,
                summarizer_model: opt_str("summarizer_model"),
                post_edit_diagnostics: Some(self.post_edit_diagnostics),
                temperature: parse_temp("temperature")?,
            }),
            session: Some(SessionSettingsDto {
                auto_prune_days: parse_u64("auto prune days", "auto_prune_days")?,
                keep: parse_usize("keep count", "session_keep")?,
            }),
            sandbox: Some(SandboxSettingsDto {
                require_for_scripts: Some(self.require_for_scripts),
                require_for_shell: Some(self.require_for_shell),
                strict_env: Some(self.strict_env),
            }),
        })
    }
}

#[derive(Clone, Debug)]
pub struct QuestionDraft {
    pub answers: Vec<Vec<String>>,
    pub custom_inputs: Vec<TextBuffer>,
    pub step: usize,
    pub focused_option: usize,
}

impl QuestionDraft {
    pub fn from_request(req: &QuestionRequestDto) -> Self {
        let len = req.questions.len();
        Self {
            answers: vec![Vec::new(); len],
            custom_inputs: vec![TextBuffer::new(""); len],
            step: 0,
            focused_option: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ActiveModal {
    ProviderSelect {
        filter: TextBuffer,
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
        status_badge: String,
        configured: bool,
        base_url: Option<String>,
        selected: usize,
    },
    ChatGptLogin {
        provider_id: String,
        provider_name: String,
        attempt_id: Option<String>,
        url: Option<String>,
        paste: TextBuffer,
        state: Option<ChatGptLoginStateDto>,
        error: Option<String>,
        busy: bool,
    },
    ApiKeyInput {
        provider_id: String,
        provider_name: String,
        status_badge: String,
        api_key: TextBuffer,
        base_url: TextBuffer,
        error: Option<String>,
        busy: bool,
    },
    ModelInput {
        filter: TextBuffer,
        provider_id: Option<String>,
        selected: usize,
    },
    DiffView {
        selected_path: Option<String>,
        unified: bool,
        diff: Option<FileDiffResponse>,
        loading: bool,
        error: Option<String>,
    },
    Search {
        mode: meerai_web_protocol::SearchModeDto,
        query: TextBuffer,
        results: Option<WorkspaceSearchResponse>,
        selected: usize,
        preview: Option<WorkspacePreviewResponse>,
        loading: bool,
    },
    Approval {
        session_id: String,
        request: ApprovalRequestDto,
        reason: TextBuffer,
        showing_reason: bool,
    },
    Question {
        session_id: String,
        request: QuestionRequestDto,
        draft: QuestionDraft,
    },
    Terminal {
        command: TextBuffer,
        output: Option<String>,
        running: bool,
        error: Option<String>,
    },
    RewindMenu {
        turns: Vec<(String, String)>,
        selected_turn: usize,
        scope: RewindScopeDto,
    },
    SessionSelect {
        filter: TextBuffer,
        history: Vec<StoredSessionDto>,
        selected: usize,
        loading: bool,
        confirm_delete: Option<String>,
    },
    CommandPalette {
        filter: TextBuffer,
        selected: usize,
    },
    UserMessageActions {
        entry_id: String,
        turn_id: Option<String>,
        text: String,
        selected: usize,
    },
    Settings(SettingsFormState),
    Help,
    SubagentOverlay {
        entry_id: String,
        agent: String,
        status: String,
        child_session_id: Option<String>,
        entries: Vec<TranscriptEntryDto>,
        usage: Option<UsageDto>,
        loading: bool,
        error: Option<String>,
    },
}

#[derive(Clone, Debug, Default)]
pub struct SessionTabState {
    pub entries: Vec<TranscriptEntryDto>,
    pub streaming_item_id: Option<String>,
    pub streaming_text: String,
    pub streaming_reasoning: String,
    pub pending_approvals: Vec<ApprovalRequestDto>,
    pub pending_questions: Vec<QuestionRequestDto>,
    pub usage: UsageDto,
    pub metrics: TurnMetricsDto,
    pub pending_queue: VecDeque<String>,
    pub draft_text: String,
    pub expanded_digests: HashSet<String>,
    pub expanded_tools: HashSet<String>,
    pub expanded_reasoning: HashSet<String>,
    pub expanded_errors: HashSet<String>,
    pub expanded_subagents: HashSet<String>,
    pub child_sessions: HashMap<String, Vec<TranscriptEntryDto>>,
    pub child_loading: HashSet<String>,
    pub unread: bool,
    pub local_notices: Vec<(String, String)>,
}

pub struct AppState {
    pub active_session_id: Option<String>,
    pub sessions: Vec<SessionSummaryDto>,
    pub tab_order: Vec<String>,
    pub hidden_session_ids: HashSet<String>,
    pub session_tabs: HashMap<String, SessionTabState>,
    pub host_info: HostInfoDto,
    pub file_changes: Vec<FileChangeDto>,
    pub providers: Option<ProvidersResponse>,
    pub workspace_files: Vec<String>,
    pub last_event_id: u64,
    pub connection: ConnectionState,
    pub draft: TextBuffer,
    pub prompt_history: Vec<String>,
    pub prompt_history_idx: Option<usize>,
    pub saved_draft_before_history: String,
    pub completion: CompletionState,
    pub sidebar_open: bool,
    pub modal: Option<ActiveModal>,
    pub modal_fallback: TextBuffer,
    pub sending: bool,
    pub error_banner: Option<String>,
    pub status_notice: Option<String>,
    pub last_title_sent: String,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            active_session_id: None,
            sessions: Vec::new(),
            tab_order: Vec::new(),
            hidden_session_ids: HashSet::new(),
            session_tabs: HashMap::new(),
            host_info: HostInfoDto::default(),
            file_changes: Vec::new(),
            providers: None,
            workspace_files: Vec::new(),
            last_event_id: 0,
            connection: ConnectionState::Connecting,
            draft: TextBuffer::new(""),
            prompt_history: Vec::new(),
            prompt_history_idx: None,
            saved_draft_before_history: String::new(),
            completion: CompletionState::default(),
            sidebar_open: true,
            modal: None,
            modal_fallback: TextBuffer::new(""),
            sending: false,
            error_banner: None,
            status_notice: None,
            last_title_sent: String::new(),
        }
    }
}

impl AppState {
    pub fn visible_sessions(&self) -> Vec<&SessionSummaryDto> {
        let mut out = Vec::new();
        for id in &self.tab_order {
            if self.hidden_session_ids.contains(id) {
                continue;
            }
            if let Some(s) = self.sessions.iter().find(|s| &s.id == id) {
                out.push(s);
            }
        }
        for s in &self.sessions {
            if !self.hidden_session_ids.contains(&s.id) && !self.tab_order.contains(&s.id) {
                out.push(s);
            }
        }
        out
    }

    pub fn active_session(&self) -> Option<&SessionSummaryDto> {
        let id = self.active_session_id.as_deref()?;
        self.sessions.iter().find(|s| s.id == id)
    }

    pub fn active_tab(&self) -> Option<&SessionTabState> {
        let id = self.active_session_id.as_deref()?;
        self.session_tabs.get(id)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut SessionTabState> {
        let id = self.active_session_id.clone()?;
        Some(self.session_tabs.entry(id).or_default())
    }

    pub fn is_busy(&self) -> bool {
        self.sending
            || self
                .active_session()
                .is_some_and(|s| s.status != SessionStatus::Idle || s.user_shell_running)
    }

    pub fn window_title(&self) -> String {
        let conn = match self.connection {
            ConnectionState::Disconnected => " [Disconnected]",
            ConnectionState::Reconnecting | ConnectionState::Resyncing => " [Reconnecting]",
            _ => "",
        };
        let Some(session) = self.active_session() else {
            return format!("meerai{conn}");
        };
        let title = session
            .title
            .as_deref()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or(&session.id);
        let status = match session.status {
            SessionStatus::WaitingApproval => " [Approval]",
            SessionStatus::WaitingQuestion => " [Question]",
            SessionStatus::Running => " [Running]",
            SessionStatus::Idle => "",
        };
        format!("M | {title}{status}{conn}")
    }

    pub fn resume_command_hint(&self) -> Option<String> {
        self.active_session_id
            .as_deref()
            .map(|id| format!("meerai --resume {id}"))
    }

    pub fn previous_turn_for_entry(&self, entry_id: &str) -> Option<String> {
        let tab = self.active_tab()?;
        let pos = tab
            .entries
            .iter()
            .position(|e| entry_id_of(e) == entry_id)?;
        tab.entries[..pos].iter().rev().find_map(|e| match e {
            TranscriptEntryDto::User {
                turn_id: Some(turn),
                ..
            } => Some(turn.clone()),
            _ => None,
        })
    }

    pub fn user_turns(&self) -> Vec<(String, String)> {
        let Some(tab) = self.active_tab() else {
            return Vec::new();
        };
        tab.entries
            .iter()
            .filter_map(|e| match e {
                TranscriptEntryDto::User {
                    turn_id: Some(turn_id),
                    text,
                    ..
                } => Some((turn_id.clone(), text.clone())),
                _ => None,
            })
            .collect()
    }
}

pub fn apply_snapshot(state: &mut AppState, snapshot: SnapshotResponse) {
    state.connection = ConnectionState::Connected;
    state.last_event_id = snapshot.event_id;
    state.error_banner = None;
    state.file_changes = snapshot.file_changes;
    if let Some(info) = snapshot.host_info {
        state.host_info = info;
    }

    let previous_active = state.active_session_id.clone();
    state.sessions = snapshot.sessions;
    sync_tab_order(state);

    let active_id = snapshot
        .active_session_id
        .or_else(|| previous_active.filter(|id| state.sessions.iter().any(|s| &s.id == id)))
        .or_else(|| state.sessions.first().map(|s| s.id.clone()));

    state.active_session_id = active_id.clone();
    if let Some(detail) = snapshot.active_session {
        let detail_id = detail.summary.id.clone();
        state.hidden_session_ids.remove(&detail_id);
        apply_active_detail(state, detail);
        if state.active_session_id.is_none() {
            state.active_session_id = Some(detail_id);
        }
    } else if let Some(id) = &active_id {
        let _ = state.session_tabs.entry(id.clone()).or_default();
    }
}

pub fn apply_active_detail(state: &mut AppState, detail: SessionDetailDto) {
    let id = detail.summary.id.clone();
    upsert_session(state, detail.summary);
    let tab = state.session_tabs.entry(id.clone()).or_default();
    tab.entries = detail.entries;
    tab.streaming_item_id = detail.streaming_item_id;
    tab.streaming_text = detail.streaming_text;
    tab.streaming_reasoning = detail.streaming_reasoning;
    tab.pending_approvals = detail.pending_approvals;
    tab.pending_questions = detail.pending_questions;
    tab.usage = detail.usage;
    tab.metrics = detail.metrics;
    tab.unread = false;
    sync_pending_modal_for_active(state);
}

pub fn apply_created_session(state: &mut AppState, session: SessionSummaryDto) {
    let id = session.id.clone();
    state.hidden_session_ids.remove(&id);
    upsert_session(state, session);
    switch_to_session(state, &id);
    let tab = state.session_tabs.entry(id).or_default();
    tab.entries.clear();
    clear_streaming_tab(tab);
    state.error_banner = None;
}

pub fn apply_view_event(state: &mut AppState, event: ViewEvent) -> EventEffect {
    match event {
        ViewEvent::SessionOpened { session } => {
            let id = session.id.clone();
            state.hidden_session_ids.remove(&id);
            upsert_session(state, session);
            if state.active_session_id.is_none() {
                state.active_session_id = Some(id);
            }
            EventEffect::None
        }
        ViewEvent::SessionUpdated { session } => {
            let id = session.id.clone();
            let became_idle = session.status == SessionStatus::Idle && !session.user_shell_running;
            upsert_session(state, session);
            if became_idle && Some(id.as_str()) == state.active_session_id.as_deref() {
                drain_next_queued_if_idle(state);
            }
            EventEffect::None
        }
        ViewEvent::SessionActivated { session_id } => {
            state.hidden_session_ids.remove(&session_id);
            switch_to_session(state, &session_id);
            EventEffect::None
        }
        ViewEvent::SessionClosed {
            session_id,
            next_active_session_id,
        } => {
            state.sessions.retain(|s| s.id != session_id);
            state.tab_order.retain(|id| id != &session_id);
            state.session_tabs.remove(&session_id);
            state.hidden_session_ids.remove(&session_id);
            if state.active_session_id.as_deref() == Some(session_id.as_str()) {
                state.active_session_id = next_active_session_id
                    .filter(|id| state.sessions.iter().any(|s| &s.id == id))
                    .or_else(|| state.visible_sessions().first().map(|s| s.id.clone()));
                sync_pending_modal_for_active(state);
            }
            EventEffect::None
        }
        ViewEvent::SessionReset { session_id } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            tab.entries.clear();
            clear_streaming_tab(tab);
            tab.pending_approvals.clear();
            tab.pending_questions.clear();
            if state.active_session_id.as_deref() == Some(session_id.as_str()) {
                sync_pending_modal_for_active(state);
            }
            EventEffect::None
        }
        ViewEvent::SessionRewound {
            session_id,
            turn_id,
            scope,
            restored,
            error,
            entries,
            restored_prompt,
        } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            if let Some(new_entries) = entries {
                tab.entries = new_entries;
            }
            clear_streaming_tab(tab);
            let msg = if let Some(err) = error {
                format!("Rewind error: {err}")
            } else {
                format!(
                    "Rewound ({scope:?}) to {turn_id}; restored {} file(s).",
                    restored.len()
                )
            };
            push_local_system(tab, msg);
            if state.active_session_id.as_deref() == Some(session_id.as_str())
                && let Some(prompt) = restored_prompt
            {
                state.draft.replace_text(prompt);
            }
            EventEffect::None
        }
        ViewEvent::TurnStarted {
            session_id,
            turn_id,
            model,
        } => {
            if let Some(session) = state.sessions.iter_mut().find(|s| s.id == session_id) {
                session.status = SessionStatus::Running;
                session.active_turn_id = Some(turn_id);
                if !model.is_empty() {
                    session.model = model;
                }
            }
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            clear_streaming_tab(tab);
            mark_unread_if_bg(state, &session_id);
            EventEffect::None
        }
        ViewEvent::UserPromptSubmitted {
            session_id,
            entry_id,
            text,
        } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            if !tab.entries.iter().any(|e| entry_id_of(e) == entry_id) {
                tab.entries.push(TranscriptEntryDto::User {
                    id: entry_id,
                    turn_id: None,
                    text,
                });
            }
            EventEffect::None
        }
        ViewEvent::TextDelta {
            session_id,
            item_id,
            delta,
        } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            if tab.streaming_item_id.as_deref() != Some(item_id.as_str()) {
                tab.streaming_item_id = Some(item_id);
                tab.streaming_text.clear();
                tab.streaming_reasoning.clear();
            }
            tab.streaming_text.push_str(&delta);
            mark_unread_if_bg(state, &session_id);
            EventEffect::None
        }
        ViewEvent::ReasoningDelta {
            session_id,
            item_id,
            delta,
        } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            if tab.streaming_item_id.as_deref() != Some(item_id.as_str()) {
                tab.streaming_item_id = Some(item_id);
                tab.streaming_text.clear();
                tab.streaming_reasoning.clear();
            }
            tab.streaming_reasoning.push_str(&delta);
            mark_unread_if_bg(state, &session_id);
            EventEffect::None
        }
        ViewEvent::EntryCompleted { session_id, entry } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            if matches!(entry, TranscriptEntryDto::Assistant { .. }) {
                clear_streaming_tab(tab);
            }
            upsert_entry(&mut tab.entries, entry);
            mark_unread_if_bg(state, &session_id);
            EventEffect::None
        }
        ViewEvent::EntryUpdated { session_id, entry } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            upsert_entry(&mut tab.entries, entry);
            EventEffect::None
        }
        ViewEvent::ApprovalRequested {
            session_id,
            request,
        } => {
            if let Some(session) = state.sessions.iter_mut().find(|s| s.id == session_id) {
                session.status = SessionStatus::WaitingApproval;
            }
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            if !tab
                .pending_approvals
                .iter()
                .any(|a| a.request_id == request.request_id)
            {
                tab.pending_approvals.push(request);
            }
            mark_unread_if_bg(state, &session_id);
            if state.active_session_id.as_deref() == Some(session_id.as_str()) {
                sync_pending_modal_for_active(state);
            }
            EventEffect::None
        }
        ViewEvent::ApprovalResolved {
            session_id,
            request_id,
        } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            tab.pending_approvals.retain(|a| a.request_id != request_id);
            if matches!(
                &state.modal,
                Some(ActiveModal::Approval { session_id: sid, request, .. })
                if sid == &session_id && request.request_id == request_id
            ) {
                state.modal = None;
            }
            if state.active_session_id.as_deref() == Some(session_id.as_str()) {
                sync_pending_modal_for_active(state);
            }
            EventEffect::None
        }
        ViewEvent::QuestionAsked {
            session_id,
            request,
        } => {
            if let Some(session) = state.sessions.iter_mut().find(|s| s.id == session_id) {
                session.status = SessionStatus::WaitingQuestion;
            }
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            if !tab
                .pending_questions
                .iter()
                .any(|q| q.request_id == request.request_id)
            {
                tab.pending_questions.push(request);
            }
            mark_unread_if_bg(state, &session_id);
            if state.active_session_id.as_deref() == Some(session_id.as_str()) {
                sync_pending_modal_for_active(state);
            }
            EventEffect::None
        }
        ViewEvent::QuestionResolved {
            session_id,
            request_id,
        } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            tab.pending_questions.retain(|q| q.request_id != request_id);
            if matches!(
                &state.modal,
                Some(ActiveModal::Question { session_id: sid, request, .. })
                if sid == &session_id && request.request_id == request_id
            ) {
                state.modal = None;
            }
            if state.active_session_id.as_deref() == Some(session_id.as_str()) {
                sync_pending_modal_for_active(state);
            }
            EventEffect::None
        }
        ViewEvent::UsageUpdated {
            session_id,
            usage,
            metrics,
        } => {
            let tab = state.session_tabs.entry(session_id).or_default();
            tab.usage = usage;
            tab.metrics = metrics;
            EventEffect::None
        }
        ViewEvent::TurnCompleted {
            session_id,
            turn_id: _,
            reason: _,
        } => {
            if let Some(session) = state.sessions.iter_mut().find(|s| s.id == session_id) {
                session.status = SessionStatus::Idle;
                session.active_turn_id = None;
                session.active_tool = None;
            }
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            clear_streaming_tab(tab);
            mark_unread_if_bg(state, &session_id);
            if state.active_session_id.as_deref() == Some(session_id.as_str()) {
                drain_next_queued_if_idle(state);
            }
            EventEffect::None
        }
        ViewEvent::Error {
            session_id,
            code,
            message,
        } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            clear_streaming_tab(tab);
            if state.active_session_id.as_deref() == Some(session_id.as_str()) {
                state.error_banner = Some(format!("{code}: {message}"));
            }
            EventEffect::None
        }
        ViewEvent::FileChangesUpdated { changes } => {
            state.file_changes = changes;
            EventEffect::None
        }
        ViewEvent::Reloaded {
            host_info,
            warnings,
        } => {
            state.host_info = host_info;
            if !warnings.is_empty() {
                state.status_notice = Some(format!("Reload warnings: {}", warnings.join("; ")));
            } else {
                state.status_notice = Some("Reloaded configuration".to_owned());
            }
            if let Some(ActiveModal::Settings(form)) = &mut state.modal {
                *form = SettingsFormState::from_host(&state.host_info);
                form.status = state.status_notice.clone();
            }
            EventEffect::None
        }
        ViewEvent::ProvidersUpdated => {
            if let Some(t) = crate::transport::current() {
                t.fetch_providers();
            }
            EventEffect::None
        }
        ViewEvent::ChatGptLoginUpdated {
            attempt_id,
            provider_id,
            state: login_state,
            url,
            account,
            message,
        } => {
            if let Some(ActiveModal::ChatGptLogin {
                provider_id: pid,
                attempt_id: att,
                url: modal_url,
                state: st,
                error,
                busy,
                ..
            }) = &mut state.modal
                && pid == &provider_id
            {
                if att.is_none() || att.as_deref() == Some(attempt_id.as_str()) {
                    *att = Some(attempt_id);
                    if let Some(u) = url {
                        *modal_url = Some(u);
                    }
                    *st = Some(login_state);
                    match login_state {
                        ChatGptLoginStateDto::Started => {
                            *busy = false;
                            *error = None;
                        }
                        ChatGptLoginStateDto::Completed => {
                            let acct = account.unwrap_or_else(|| "signed in".to_owned());
                            state.status_notice = Some(format!("Connected {provider_id} ({acct})"));
                            state.modal = None;
                            if let Some(t) = crate::transport::current() {
                                t.fetch_providers();
                            }
                        }
                        ChatGptLoginStateDto::Failed => {
                            *busy = false;
                            *error = message;
                        }
                        ChatGptLoginStateDto::Cancelled => {
                            state.modal = None;
                        }
                    }
                }
            }
            EventEffect::None
        }
        ViewEvent::ResyncRequired => {
            state.connection = ConnectionState::Resyncing;
            EventEffect::Resync
        }
        ViewEvent::HostDisconnected => {
            state.connection = ConnectionState::Disconnected;
            EventEffect::None
        }
    }
}

pub fn apply_history_response(state: &mut AppState, history: SessionHistoryResponse) {
    if let Some(ActiveModal::SessionSelect {
        history: slot,
        loading,
        ..
    }) = &mut state.modal
    {
        *slot = history.sessions;
        *loading = false;
    }
}

pub fn apply_providers_response(state: &mut AppState, providers: ProvidersResponse) {
    state.providers = Some(providers);
    update_completion(state);
}

pub fn apply_reload_response(state: &mut AppState, resp: ReloadResponse) {
    state.host_info = resp.host_info;
    state.status_notice = if resp.warnings.is_empty() {
        Some("Reloaded configuration".to_owned())
    } else {
        Some(format!(
            "Reloaded with warnings: {}",
            resp.warnings.join("; ")
        ))
    };
    if let Some(ActiveModal::Settings(form)) = &mut state.modal {
        *form = SettingsFormState::from_host(&state.host_info);
        form.status = state.status_notice.clone();
    }
}

pub fn apply_shell_output(state: &mut AppState, _session_id: &str, result: Result<String, String>) {
    if let Err(err) = &result {
        state.error_banner = Some(err.clone());
    }
    if let Some(ActiveModal::Terminal {
        output,
        running,
        error,
        ..
    }) = &mut state.modal
    {
        *running = false;
        match result {
            Ok(out) => {
                *output = Some(out);
                *error = None;
            }
            Err(err) => {
                *error = Some(err);
            }
        }
    }
}

pub fn sync_pending_modal_for_active(state: &mut AppState) {
    if state.modal.is_some() {
        return;
    }
    let Some(sid) = state.active_session_id.clone() else {
        return;
    };
    let Some(tab) = state.session_tabs.get(&sid) else {
        return;
    };
    if let Some(req) = tab.pending_approvals.first().cloned() {
        state.modal = Some(ActiveModal::Approval {
            session_id: sid,
            request: req,
            reason: TextBuffer::new(""),
            showing_reason: false,
        });
    } else if let Some(req) = tab.pending_questions.first().cloned() {
        let draft = QuestionDraft::from_request(&req);
        state.modal = Some(ActiveModal::Question {
            session_id: sid,
            request: req,
            draft,
        });
    }
}

pub fn switch_to_session(state: &mut AppState, session_id: &str) {
    if !state.sessions.iter().any(|s| s.id == session_id) {
        return;
    }
    state.hidden_session_ids.remove(session_id);
    if let Some(prev) = state.active_session_id.clone()
        && prev != session_id
    {
        let prev_draft = state.draft.text.clone();
        state.session_tabs.entry(prev).or_default().draft_text = prev_draft;
    }
    state.active_session_id = Some(session_id.to_owned());
    let next_draft = state
        .session_tabs
        .entry(session_id.to_owned())
        .or_default()
        .draft_text
        .clone();
    state.draft.replace_text(next_draft);
    if let Some(tab) = state.session_tabs.get_mut(session_id) {
        tab.unread = false;
    }
    state.error_banner = None;
    update_completion(state);
    sync_pending_modal_for_active(state);
}

pub fn select_tab_index(state: &mut AppState, idx: usize) {
    let visible_ids: Vec<String> = state
        .visible_sessions()
        .into_iter()
        .map(|s| s.id.clone())
        .collect();
    if let Some(id) = visible_ids.get(idx).cloned() {
        switch_to_session(state, &id);
        if let Some(t) = crate::transport::current() {
            t.activate_session(id);
        }
    }
}

pub fn cycle_tab(state: &mut AppState, delta: isize) {
    let visible_ids: Vec<String> = state
        .visible_sessions()
        .into_iter()
        .map(|s| s.id.clone())
        .collect();
    if visible_ids.len() < 2 {
        return;
    }
    let cur = state
        .active_session_id
        .as_deref()
        .and_then(|id| visible_ids.iter().position(|x| x == id))
        .unwrap_or(0);
    let len = visible_ids.len() as isize;
    let next = ((cur as isize + delta).rem_euclid(len)) as usize;
    select_tab_index(state, next);
}

pub fn reorder_active_tab(state: &mut AppState, delta: isize) {
    sync_tab_order(state);
    let Some(active) = state.active_session_id.as_deref() else {
        return;
    };
    let visible_ids: Vec<String> = state
        .visible_sessions()
        .into_iter()
        .map(|s| s.id.clone())
        .collect();
    let Some(pos) = visible_ids.iter().position(|id| id == active) else {
        return;
    };
    let target = pos as isize + delta;
    if target < 0 || target >= visible_ids.len() as isize {
        return;
    }
    let a = &visible_ids[pos];
    let b = &visible_ids[target as usize];
    if let (Some(ia), Some(ib)) = (
        state.tab_order.iter().position(|x| x == a),
        state.tab_order.iter().position(|x| x == b),
    ) {
        state.tab_order.swap(ia, ib);
    }
}

pub fn detach_tab_view(state: &mut AppState, session_id: &str) {
    let visible = state.visible_sessions();
    if visible.len() <= 1 {
        state.status_notice = Some("Cannot close the last open tab".to_owned());
        return;
    }
    let visible_ids: Vec<String> = visible.into_iter().map(|s| s.id.clone()).collect();
    let next_id = visible_ids
        .iter()
        .position(|id| id == session_id)
        .and_then(|pos| {
            visible_ids
                .get(pos + 1)
                .or_else(|| pos.checked_sub(1).and_then(|i| visible_ids.get(i)))
        })
        .cloned();
    state.hidden_session_ids.insert(session_id.to_owned());
    if state.active_session_id.as_deref() == Some(session_id)
        && let Some(next) = next_id
    {
        switch_to_session(state, &next);
        if let Some(t) = crate::transport::current() {
            t.activate_session(next);
        }
    }
}

pub fn submit_or_queue_composer(state: &mut AppState) {
    if state.connection == ConnectionState::Disconnected {
        return;
    }
    let raw = state.draft.text.clone();
    let trimmed = raw.trim().to_owned();
    if trimmed.is_empty() {
        return;
    }
    state.completion = CompletionState::default();
    state.prompt_history_idx = None;
    record_prompt_history(state, &trimmed);
    state.draft.clear();
    if let Some(tab) = state.active_tab_mut() {
        tab.draft_text.clear();
    }

    if let Some(shell_cmd) = trimmed.strip_prefix('!') {
        execute_user_shell(state, shell_cmd.trim());
        return;
    }

    if trimmed.starts_with('/') {
        execute_slash_command(state, &trimmed);
        return;
    }

    dispatch_prompt_text(state, trimmed);
}

pub fn dispatch_prompt_text(state: &mut AppState, trimmed: String) {
    let mentions = extract_mentions(&trimmed, &state.host_info);
    if !mentions.skills.is_empty() || mentions.agent.is_some() {
        let Some(sid) = state.active_session_id.clone() else {
            state.error_banner = Some("No active session".to_owned());
            return;
        };
        if let Some(t) = crate::transport::current() {
            for skill in &mentions.skills {
                t.apply_skill(sid.clone(), skill.clone());
            }
            if let Some(agent) = &mentions.agent {
                t.apply_agent(sid.clone(), agent.clone());
            }
        }
        if mentions.mention_only {
            let tab = state.session_tabs.entry(sid).or_default();
            let mut applied = Vec::new();
            for s in mentions.skills {
                applied.push(format!("skill:{s}"));
            }
            if let Some(a) = mentions.agent {
                applied.push(format!("agent:{a}"));
            }
            push_local_system(tab, format!("Applied {}", applied.join(", ")));
            return;
        }
    }

    let Some(sid) = state.active_session_id.clone() else {
        state.error_banner = Some("No active session".to_owned());
        return;
    };

    if state.is_busy() {
        state
            .session_tabs
            .entry(sid)
            .or_default()
            .pending_queue
            .push_back(trimmed);
        state.status_notice = Some("Queued prompt (click Steer to interrupt now)".to_owned());
        return;
    }

    state.sending = true;
    state.error_banner = None;
    if let Some(t) = crate::transport::current() {
        t.submit_prompt(sid, trimmed);
    }
}

pub fn steer_or_send_queued(state: &mut AppState, index: usize) {
    let Some(sid) = state.active_session_id.clone() else {
        return;
    };
    let Some(prompt) = state
        .session_tabs
        .get_mut(&sid)
        .and_then(|tab| tab.pending_queue.remove(index))
    else {
        return;
    };
    let Some(t) = crate::transport::current() else {
        return;
    };
    if state.is_busy() {
        t.steer_prompt(sid, prompt);
    } else {
        state.sending = true;
        t.submit_prompt(sid, prompt);
    }
}

pub fn drain_next_queued_if_idle(state: &mut AppState) {
    if state.is_busy() || state.connection == ConnectionState::Disconnected {
        return;
    }
    let Some(sid) = state.active_session_id.clone() else {
        return;
    };
    let next = state
        .session_tabs
        .get_mut(&sid)
        .and_then(|tab| tab.pending_queue.pop_front());
    if let Some(prompt) = next {
        state.sending = true;
        if let Some(t) = crate::transport::current() {
            t.submit_prompt(sid, prompt);
        }
    }
}

pub fn execute_user_shell(state: &mut AppState, cmd: &str) {
    if cmd.is_empty() {
        state.modal = Some(ActiveModal::Terminal {
            command: TextBuffer::new(""),
            output: None,
            running: false,
            error: None,
        });
        return;
    }
    if state
        .active_session()
        .is_some_and(|s| s.approval_mode == ApprovalModeDto::Plan)
    {
        state.error_banner = Some("Shell commands (!) are disabled in plan mode.".to_owned());
        return;
    }
    let Some(sid) = state.active_session_id.clone() else {
        state.error_banner = Some("No active session".to_owned());
        return;
    };
    if let Some(t) = crate::transport::current() {
        state.modal = Some(ActiveModal::Terminal {
            command: TextBuffer::new(cmd),
            output: None,
            running: true,
            error: None,
        });
        t.run_shell(sid, cmd.to_owned());
    }
}

pub fn execute_slash_command(state: &mut AppState, raw: &str) {
    let trimmed = raw.trim();
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("");
    let arg = parts.next().map(str::trim).unwrap_or("");

    match cmd {
        "/help" => {
            state.modal = Some(ActiveModal::Help);
        }
        "/sessions" => {
            if !arg.is_empty() && state.sessions.iter().any(|s| s.id == arg) {
                switch_to_session(state, arg);
                if let Some(t) = crate::transport::current() {
                    t.activate_session(arg.to_owned());
                }
            } else {
                open_session_select(state, arg);
            }
        }
        "/new" => {
            if let Some(t) = crate::transport::current() {
                t.create_session(None);
            }
        }
        "/fork" => {
            if state.is_busy() {
                state.error_banner = Some("Cancel the running turn before forking.".to_owned());
            } else if let Some(sid) = state.active_session_id.clone() {
                match state.user_turns().last().map(|(id, _)| id.clone()) {
                    Some(turn) => {
                        if let Some(t) = crate::transport::current() {
                            t.fork_session(sid, Some(turn));
                        }
                    }
                    None => {
                        state.status_notice = Some("No closed turn to fork.".to_owned());
                    }
                }
            }
        }
        "/acp" => {
            if state
                .active_session()
                .is_some_and(|s| s.approval_mode == ApprovalModeDto::Plan)
            {
                state.error_banner = Some("ACP sessions cannot run in plan mode.".to_owned());
                return;
            }
            let preset = if arg.is_empty() { "agy" } else { arg };
            if let Some(t) = crate::transport::current() {
                t.create_session(Some(BackendKind::Acp {
                    preset: preset.to_owned(),
                    remote_session_id: None,
                }));
            }
        }
        "/close" => {
            if let Some(sid) = state.active_session_id.clone()
                && let Some(t) = crate::transport::current()
            {
                t.close_session(sid);
            }
        }
        "/auth" | "/connect" | "/provider" => {
            open_provider_select(state);
        }
        "/model" | "/models" => {
            if arg.is_empty() {
                open_model_input(state);
            } else if let Some(sid) = state.active_session_id.clone() {
                let (provider, model) = parse_provider_model_arg(
                    arg,
                    state
                        .active_session()
                        .map(|s| s.provider.as_str())
                        .unwrap_or("openai"),
                );
                if let Some(t) = crate::transport::current() {
                    t.set_model(sid, provider, model);
                }
            }
        }
        "/agent" => {
            if arg.is_empty() {
                state.status_notice = Some(format!(
                    "Usage: /agent <id>. Available: {}",
                    state
                        .host_info
                        .agents
                        .iter()
                        .map(|a| a.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            } else if let Some(sid) = state.active_session_id.clone()
                && let Some(t) = crate::transport::current()
            {
                t.apply_agent(sid, arg.to_owned());
            }
        }
        "/agents" => {
            state.draft.replace_text("@agent/");
            update_completion(state);
        }
        "/skill" => {
            if arg.is_empty() {
                state.status_notice = Some(format!(
                    "Usage: /skill <id>. Available: {}",
                    state
                        .host_info
                        .skills
                        .iter()
                        .map(|s| s.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            } else if let Some(sid) = state.active_session_id.clone()
                && let Some(t) = crate::transport::current()
            {
                t.apply_skill(sid, arg.to_owned());
            }
        }
        "/skills" => {
            state.draft.replace_text("@skill/");
            update_completion(state);
        }
        "/search" | "/tv" | "/find" => {
            open_search_modal(state, meerai_web_protocol::SearchModeDto::Files, arg);
        }
        "/grep" => {
            open_search_modal(state, meerai_web_protocol::SearchModeDto::Content, arg);
        }
        "/diff" => {
            open_diff_modal(state, (!arg.is_empty()).then(|| arg.to_owned()));
        }
        "/sidebar" => {
            state.sidebar_open = !state.sidebar_open;
        }
        "/clear" => {
            if let Some(tab) = state.active_tab_mut() {
                tab.entries.clear();
                tab.streaming_item_id = None;
                tab.streaming_text.clear();
                tab.streaming_reasoning.clear();
                tab.expanded_digests.clear();
                tab.expanded_tools.clear();
                tab.expanded_reasoning.clear();
                tab.expanded_errors.clear();
                tab.expanded_subagents.clear();
                state.status_notice =
                    Some("Cleared this view. The session log is unchanged.".to_owned());
            }
        }
        "/reset" => {
            if let Some(sid) = state.active_session_id.clone()
                && let Some(t) = crate::transport::current()
            {
                t.reset_session(sid);
            }
        }
        "/cancel" => {
            if let Some(sid) = state.active_session_id.clone()
                && let Some(t) = crate::transport::current()
            {
                t.cancel_turn(sid);
            }
        }
        "/compact" => {
            if let Some(sid) = state.active_session_id.clone()
                && let Some(t) = crate::transport::current()
            {
                t.compact_session(sid);
            }
        }
        "/mode" => {
            if arg.is_empty() {
                cycle_approval_mode(state);
            } else {
                let mode = match arg.to_ascii_lowercase().as_str() {
                    "plan" => Some(ApprovalModeDto::Plan),
                    "ask" => Some(ApprovalModeDto::Ask),
                    "auto" => Some(ApprovalModeDto::Auto),
                    _ => None,
                };
                match mode {
                    Some(m) => set_approval_mode(state, m),
                    None => {
                        state.error_banner =
                            Some(format!("Unknown mode '{arg}'. Use: plan, ask, auto."));
                    }
                }
            }
        }
        "/rewind" => {
            if state.is_busy() {
                state.error_banner = Some("Cannot rewind while a turn is running.".to_owned());
            } else {
                open_rewind_modal(state, (!arg.is_empty()).then_some(arg));
            }
        }
        "/reload" => {
            if let Some(t) = crate::transport::current() {
                t.reload_config();
            }
        }
        "/settings" => {
            state.modal = Some(ActiveModal::Settings(SettingsFormState::from_host(
                &state.host_info,
            )));
        }
        "/update" => {
            let msg = match &state.host_info.update_available {
                Some(v) => format!(
                    "Update available: {v}. Run: {}",
                    state.host_info.install_hint
                ),
                None => format!(
                    "meerai is up to date. Install/upgrade: {}",
                    state.host_info.install_hint
                ),
            };
            if let Some(tab) = state.active_tab_mut() {
                push_local_system(tab, msg.clone());
            } else {
                state.status_notice = Some(msg);
            }
        }
        "/quit" | "/q" => {
            let msg = "Browser companion cannot stop the host process. Close the browser tab or use /close to shut down this session.";
            if let Some(tab) = state.active_tab_mut() {
                push_local_system(tab, msg.to_owned());
            } else {
                state.status_notice = Some(msg.to_owned());
            }
        }
        "/serve" => {
            let msg = "Host web server is already serving this browser UI.";
            if let Some(tab) = state.active_tab_mut() {
                push_local_system(tab, msg.to_owned());
            } else {
                state.status_notice = Some(msg.to_owned());
            }
        }
        _ => {
            state.error_banner = Some(format!(
                "Unknown command: {cmd}. Type /help for available commands."
            ));
        }
    }
}

pub fn set_approval_mode(state: &mut AppState, mode: ApprovalModeDto) {
    let Some(sid) = state.active_session_id.clone() else {
        return;
    };
    if mode == ApprovalModeDto::Plan
        && state
            .active_session()
            .is_some_and(|s| matches!(s.backend, BackendKind::Acp { .. }))
    {
        state.error_banner = Some("Plan mode is not available for ACP sessions.".to_owned());
        return;
    }
    if let Some(t) = crate::transport::current() {
        t.set_mode(sid, mode);
    }
}

pub fn cycle_approval_mode(state: &mut AppState) {
    let Some(session) = state.active_session() else {
        return;
    };
    let is_acp = matches!(session.backend, BackendKind::Acp { .. });
    let next = match session.approval_mode {
        ApprovalModeDto::Plan => ApprovalModeDto::Ask,
        ApprovalModeDto::Ask => ApprovalModeDto::Auto,
        ApprovalModeDto::Auto => {
            if is_acp {
                ApprovalModeDto::Ask
            } else {
                ApprovalModeDto::Plan
            }
        }
    };
    set_approval_mode(state, next);
}

pub fn open_provider_select(state: &mut AppState) {
    if let Some(t) = crate::transport::current() {
        t.fetch_providers();
    }
    state.modal = Some(ActiveModal::ProviderSelect {
        filter: TextBuffer::new(""),
        selected: 0,
    });
}

pub fn open_model_input(state: &mut AppState) {
    if let Some(t) = crate::transport::current() {
        t.fetch_providers();
    }
    let provider_id = state
        .active_session()
        .map(|s| s.provider.clone())
        .or_else(|| {
            state
                .providers
                .as_ref()
                .and_then(|p| p.default_provider.clone())
        });
    state.modal = Some(ActiveModal::ModelInput {
        filter: TextBuffer::new(""),
        provider_id,
        selected: 0,
    });
}

pub fn open_session_select(state: &mut AppState, initial_filter: &str) {
    if let Some(t) = crate::transport::current() {
        t.fetch_history();
    }
    state.modal = Some(ActiveModal::SessionSelect {
        filter: TextBuffer::new(initial_filter),
        history: Vec::new(),
        selected: 0,
        loading: true,
        confirm_delete: None,
    });
}

pub fn open_diff_modal(state: &mut AppState, path: Option<String>) {
    let target = path.or_else(|| state.file_changes.first().map(|c| c.path.clone()));
    if let Some(t) = crate::transport::current() {
        t.fetch_workspace_changes();
        if let Some(p) = &target {
            t.fetch_workspace_diff(p.clone());
        }
    }
    state.modal = Some(ActiveModal::DiffView {
        loading: target.is_some(),
        selected_path: target,
        unified: true,
        diff: None,
        error: None,
    });
}

pub fn open_search_modal(
    state: &mut AppState,
    mode: meerai_web_protocol::SearchModeDto,
    query: &str,
) {
    if let Some(t) = crate::transport::current()
        && !query.is_empty()
    {
        t.search_workspace(mode, query.to_owned());
    }
    state.modal = Some(ActiveModal::Search {
        mode,
        query: TextBuffer::new(query),
        results: None,
        selected: 0,
        preview: None,
        loading: !query.is_empty(),
    });
}

pub fn open_rewind_modal(state: &mut AppState, turn_hint: Option<&str>) {
    let turns = state.user_turns();
    if turns.is_empty() {
        state.status_notice = Some("No recorded turns to rewind to.".to_owned());
        return;
    }
    let selected_turn = turn_hint
        .and_then(|h| turns.iter().position(|(id, _)| id == h))
        .unwrap_or_else(|| turns.len().saturating_sub(1));
    state.modal = Some(ActiveModal::RewindMenu {
        turns,
        selected_turn,
        scope: RewindScopeDto::Both,
    });
}

pub fn activate_provider_from_modal(state: &mut AppState, provider: ProviderItemDto) {
    if provider.configured && provider.auth_kind != meerai_web_protocol::AuthKindDto::None {
        state.modal = Some(ActiveModal::ProviderConnection {
            provider_id: provider.id,
            provider_name: provider.display_name,
            status_badge: provider.status_badge,
            configured: provider.configured,
            base_url: provider.base_url,
            selected: 0,
        });
        return;
    }
    match provider.auth_kind {
        meerai_web_protocol::AuthKindDto::None => {
            state.modal = Some(ActiveModal::ModelInput {
                filter: TextBuffer::new(""),
                provider_id: Some(provider.id),
                selected: 0,
            });
        }
        meerai_web_protocol::AuthKindDto::ApiKey => {
            open_api_key_modal(state, &provider);
        }
        meerai_web_protocol::AuthKindDto::OAuth => {
            start_oauth_modal(state, &provider);
        }
        meerai_web_protocol::AuthKindDto::Either => {
            state.modal = Some(ActiveModal::AuthMethodSelect {
                provider_id: provider.id,
                provider_name: provider.display_name,
                selected: 0,
            });
        }
    }
}

pub fn open_api_key_modal(state: &mut AppState, provider: &ProviderItemDto) {
    state.modal = Some(ActiveModal::ApiKeyInput {
        provider_id: provider.id.clone(),
        provider_name: provider.display_name.clone(),
        status_badge: provider.status_badge.clone(),
        api_key: TextBuffer::new(""),
        base_url: TextBuffer::new(provider.base_url.clone().unwrap_or_default()),
        error: None,
        busy: false,
    });
}

pub fn start_oauth_modal(state: &mut AppState, provider: &ProviderItemDto) {
    if let Some(t) = crate::transport::current() {
        t.start_chatgpt_login(provider.id.clone());
    }
    state.modal = Some(ActiveModal::ChatGptLogin {
        provider_id: provider.id.clone(),
        provider_name: provider.display_name.clone(),
        attempt_id: None,
        url: None,
        paste: TextBuffer::new(""),
        state: None,
        error: None,
        busy: true,
    });
}

pub fn update_completion(state: &mut AppState) {
    let text = state.draft.text.as_str();
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        state.completion = CompletionState::default();
        return;
    }

    if let Some(after_model) = text.strip_prefix("/model ") {
        let q = after_model.trim();
        let active_provider = state
            .active_session()
            .map(|s| s.provider.as_str())
            .unwrap_or("openai");
        let items = model_completion_items(state.providers.as_ref(), active_provider, q);
        state.completion = CompletionState {
            active: !items.is_empty(),
            kind: Some(CompletionKind::Model),
            replace_start: 7,
            replace_end: text.len(),
            query: q.to_owned(),
            items,
            selected: 0,
        };
        return;
    }

    if text.starts_with('/') && !text.contains(char::is_whitespace) {
        let q = text.trim().to_ascii_lowercase();
        let items = SLASH_COMMANDS
            .iter()
            .filter(|c| c.cmd.contains(&q) || c.desc.to_ascii_lowercase().contains(&q[1..]))
            .map(|c| {
                let label = if c.args.is_empty() {
                    c.cmd.to_owned()
                } else {
                    format!("{} {}", c.cmd, c.args)
                };
                CompletionItem {
                    label,
                    insert: if c.takes_arg {
                        format!("{} ", c.cmd)
                    } else {
                        c.cmd.to_owned()
                    },
                    detail: c.desc.to_owned(),
                    category: None,
                }
            })
            .collect::<Vec<_>>();
        state.completion = CompletionState {
            active: !items.is_empty(),
            kind: Some(CompletionKind::Command),
            replace_start: 0,
            replace_end: text.len(),
            query: q,
            items,
            selected: 0,
        };
        return;
    }

    if let Some((start, end, q)) = trailing_at_token(text) {
        if state.workspace_files.is_empty()
            && let Some(t) = crate::transport::current()
        {
            t.fetch_workspace_files();
        }
        let items = mention_completion_items(q, &state.host_info, &state.workspace_files);
        state.completion = CompletionState {
            active: !items.is_empty(),
            kind: Some(CompletionKind::Mention),
            replace_start: start,
            replace_end: end,
            query: q.to_owned(),
            items,
            selected: 0,
        };
        return;
    }

    state.completion = CompletionState::default();
}

pub fn accept_completion(state: &mut AppState, index: usize) -> bool {
    let Some(item) = state.completion.items.get(index).cloned() else {
        return false;
    };
    let start = state.completion.replace_start.min(state.draft.text.len());
    let end = state.completion.replace_end.min(state.draft.text.len());
    let mut updated = String::with_capacity(state.draft.text.len() + item.insert.len());
    updated.push_str(&state.draft.text[..start]);
    updated.push_str(&item.insert);
    if matches!(state.completion.kind, Some(CompletionKind::Mention)) && !item.insert.ends_with(' ')
    {
        updated.push(' ');
    }
    updated.push_str(&state.draft.text[end..]);
    state.draft.replace_text(updated);
    let current_text = state.draft.text.clone();
    if let Some(tab) = state.active_tab_mut() {
        tab.draft_text = current_text;
    }
    state.completion = CompletionState::default();
    true
}

pub fn trailing_at_token(text: &str) -> Option<(usize, usize, &str)> {
    let last_token_start = text.rfind(char::is_whitespace).map_or(0, |idx| {
        idx + text[idx..].chars().next().map_or(1, char::len_utf8)
    });
    let token = &text[last_token_start..];
    token
        .strip_prefix('@')
        .map(|query| (last_token_start, text.len(), query))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtractedMentions {
    pub agent: Option<String>,
    pub skills: Vec<String>,
    pub mention_only: bool,
}

pub fn extract_mentions(input: &str, host: &HostInfoDto) -> ExtractedMentions {
    let agent_ids: HashSet<&str> = host.agents.iter().map(|a| a.id.as_str()).collect();
    let skill_ids: HashSet<&str> = host.skills.iter().map(|s| s.id.as_str()).collect();
    let mut agent = None;
    let mut skills = Vec::new();
    let mut seen_skills = HashSet::new();
    let mut has_non_definition_text = false;

    for token in input.split_whitespace() {
        if let Some(name) = token.strip_prefix("@agent/")
            && agent_ids.contains(name)
        {
            agent = Some(name.to_owned());
            continue;
        }
        if let Some(name) = token.strip_prefix("@skill/")
            && skill_ids.contains(name)
        {
            if seen_skills.insert(name.to_owned()) {
                skills.push(name.to_owned());
            }
            continue;
        }
        has_non_definition_text = true;
    }

    ExtractedMentions {
        mention_only: !has_non_definition_text && (agent.is_some() || !skills.is_empty()),
        agent,
        skills,
    }
}

pub fn model_completion_items(
    providers: Option<&ProvidersResponse>,
    active_provider: &str,
    query: &str,
) -> Vec<CompletionItem> {
    let Some(providers) = providers else {
        return Vec::new();
    };
    let q = query.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    if let Some(recent) = providers.recent_models.get(active_provider) {
        for m in recent {
            if q.is_empty() || m.to_ascii_lowercase().contains(&q) {
                if seen.insert((active_provider.to_owned(), m.clone())) {
                    out.push(CompletionItem {
                        label: format!("{active_provider}/{m}"),
                        insert: format!("{active_provider}/{m}"),
                        detail: "recent".to_owned(),
                        category: None,
                    });
                }
            }
        }
    }

    for model in &providers.models {
        let full = format!("{}/{}", model.provider, model.id);
        if q.is_empty()
            || model.id.to_ascii_lowercase().contains(&q)
            || full.to_ascii_lowercase().contains(&q)
        {
            if seen.insert((model.provider.clone(), model.id.clone())) {
                let detail = model
                    .context_window_tokens
                    .map(|c| format!("{}k ctx", c / 1000))
                    .unwrap_or_else(|| model.provider.clone());
                out.push(CompletionItem {
                    label: full.clone(),
                    insert: full,
                    detail,
                    category: None,
                });
            }
        }
        if out.len() >= 18 {
            break;
        }
    }
    out
}

pub fn mention_completion_items(
    query: &str,
    host: &HostInfoDto,
    files: &[String],
) -> Vec<CompletionItem> {
    let q = query.to_ascii_lowercase();
    let (kind_filter, needle) = if let Some(rest) = q.strip_prefix("agent/") {
        (Some(MentionCategory::Agent), rest)
    } else if let Some(rest) = q.strip_prefix("skill/") {
        (Some(MentionCategory::Skill), rest)
    } else {
        (None, q.as_str())
    };

    let mut ranked: Vec<(u8, String, CompletionItem)> = Vec::new();

    if kind_filter.is_none() || kind_filter == Some(MentionCategory::Agent) {
        for agent in &host.agents {
            let label = format!("@agent/{}", agent.id);
            if needle.is_empty()
                || agent.id.to_ascii_lowercase().contains(needle)
                || agent
                    .description
                    .as_deref()
                    .is_some_and(|d| d.to_ascii_lowercase().contains(needle))
            {
                ranked.push((
                    score_rank(&agent.id, needle),
                    label.clone(),
                    CompletionItem {
                        label: label.clone(),
                        insert: label,
                        detail: agent
                            .description
                            .clone()
                            .unwrap_or_else(|| format!("agent ({})", agent.source)),
                        category: Some(MentionCategory::Agent),
                    },
                ));
            }
        }
    }

    if kind_filter.is_none() || kind_filter == Some(MentionCategory::Skill) {
        for skill in &host.skills {
            let label = format!("@skill/{}", skill.id);
            if needle.is_empty()
                || skill.id.to_ascii_lowercase().contains(needle)
                || skill
                    .description
                    .as_deref()
                    .is_some_and(|d| d.to_ascii_lowercase().contains(needle))
            {
                ranked.push((
                    score_rank(&skill.id, needle),
                    label.clone(),
                    CompletionItem {
                        label: label.clone(),
                        insert: label,
                        detail: skill
                            .description
                            .clone()
                            .unwrap_or_else(|| format!("skill ({})", skill.source)),
                        category: Some(MentionCategory::Skill),
                    },
                ));
            }
        }
    }

    if kind_filter.is_none() {
        for path in files {
            if needle.is_empty() || path.to_ascii_lowercase().contains(needle) {
                let base = path.rsplit('/').next().unwrap_or(path);
                ranked.push((
                    score_rank(base, needle),
                    path.clone(),
                    CompletionItem {
                        label: format!("@{path}"),
                        insert: format!("@{path}"),
                        detail: "file".to_owned(),
                        category: Some(MentionCategory::File),
                    },
                ));
            }
        }
    }

    ranked.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    ranked
        .into_iter()
        .take(16)
        .map(|(_, _, item)| item)
        .collect()
}

fn score_rank(candidate: &str, needle: &str) -> u8 {
    if needle.is_empty() {
        return 2;
    }
    let lower = candidate.to_ascii_lowercase();
    if lower == needle {
        0
    } else if lower.starts_with(needle) {
        1
    } else if lower.contains(needle) {
        2
    } else {
        3
    }
}

pub const QUESTION_TOOL: &str = "question-ask";

pub fn activity_verb(tool_name: &str) -> &'static str {
    match tool_name {
        "code-read_file" => "Reading",
        "code-write_file" | "code-edit_file" => "Editing",
        "code-list_dir" => "Listing",
        "code-grep" => "Searching",
        "code-glob" => "Matching",
        "code-shell" => "Running",
        "sac-run_script" => "Running script",
        "retrieval-repo_map" => "Mapping",
        "question-ask" => "Asking",
        "subagent-run" => "Delegating subagent",
        "acp-run" => "Delegating ACP agent",
        name if name.starts_with("lsp-") => "Inspecting",
        _ => "Using",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityCategory {
    Command,
    Tool,
    Read,
    Edit,
    Agent,
    Thought,
}

pub fn activity_category(tool_name: &str) -> ActivityCategory {
    match tool_name {
        "code-shell" | "sac-run_script" => ActivityCategory::Command,
        "code-read_file" => ActivityCategory::Read,
        "code-write_file" | "code-edit_file" => ActivityCategory::Edit,
        "subagent-run" | "acp-run" => ActivityCategory::Agent,
        _ => ActivityCategory::Tool,
    }
}

pub fn activity_summary(blocks: &[(&str, bool)], thoughts: usize) -> String {
    fn part(count: usize, active: bool, gerund: &str, one: &str, many: &str) -> Option<String> {
        match count {
            0 => None,
            n if active => Some(format!("{gerund} {n}")),
            1 => Some(format!("1 {one}")),
            n => Some(format!("{n} {many}")),
        }
    }
    let mut counts = [0usize; 6];
    let mut active = [false; 6];
    for (name, running) in blocks {
        let idx = activity_category(name) as usize;
        counts[idx] += 1;
        active[idx] |= *running;
    }
    counts[ActivityCategory::Thought as usize] = thoughts;
    [
        part(counts[0], active[0], "Running", "command", "commands"),
        part(counts[1], active[1], "Calling", "tool", "tools"),
        part(counts[2], active[2], "Reading", "read", "reads"),
        part(counts[3], active[3], "Editing", "edit", "edits"),
        part(counts[4], active[4], "Delegating", "agent", "agents"),
        part(counts[5], active[5], "Thinking", "thought", "thoughts"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ")
}

#[derive(Clone, Debug)]
pub enum TranscriptBlock<'a> {
    User {
        turn_num: usize,
        id: &'a str,
        turn_id: Option<&'a str>,
        text: &'a str,
    },
    ActivityDigest {
        digest_id: String,
        tools: Vec<&'a TranscriptEntryDto>,
        reasoning: Vec<&'a TranscriptEntryDto>,
    },
    Assistant(&'a TranscriptEntryDto),
    System(&'a TranscriptEntryDto),
    Error(&'a TranscriptEntryDto),
    Subagent(&'a TranscriptEntryDto),
    Question(&'a TranscriptEntryDto),
}

fn folds_into_digest(entry: &TranscriptEntryDto) -> bool {
    match entry {
        TranscriptEntryDto::ToolCall { name, .. } => name != crate::state::QUESTION_TOOL,
        TranscriptEntryDto::Assistant {
            text, reasoning, ..
        } => text.trim().is_empty() && !reasoning.trim().is_empty(),
        _ => false,
    }
}

pub fn group_transcript_blocks<'a>(entries: &'a [TranscriptEntryDto]) -> Vec<TranscriptBlock<'a>> {
    let mut blocks: Vec<TranscriptBlock<'a>> = Vec::new();
    let mut turn_num = 0usize;
    let mut fold_start: Option<usize> = None;
    let mut folded: Vec<&'a TranscriptEntryDto> = Vec::new();

    let flush = |blocks: &mut Vec<TranscriptBlock<'a>>,
                 fold_start: &mut Option<usize>,
                 folded: &mut Vec<&'a TranscriptEntryDto>| {
        let Some(at) = fold_start.take() else {
            folded.clear();
            return;
        };
        if folded.is_empty() {
            return;
        }
        let items = std::mem::take(folded);
        let (reasoning, tools): (Vec<_>, Vec<_>) = items
            .into_iter()
            .partition(|e| matches!(e, TranscriptEntryDto::Assistant { .. }));
        let digest_id = format!(
            "digest-{at}-{}",
            tools
                .first()
                .or_else(|| reasoning.first())
                .map(|e| entry_id_of(e))
                .unwrap_or("turn")
        );
        blocks.insert(
            at + 1,
            TranscriptBlock::ActivityDigest {
                digest_id,
                tools,
                reasoning,
            },
        );
    };

    for entry in entries {
        if let TranscriptEntryDto::User { id, turn_id, text } = entry {
            flush(&mut blocks, &mut fold_start, &mut folded);
            turn_num += 1;
            blocks.push(TranscriptBlock::User {
                turn_num,
                id,
                turn_id: turn_id.as_deref(),
                text,
            });
            fold_start = Some(blocks.len() - 1);
            continue;
        }
        if folds_into_digest(entry) {
            if fold_start.is_some() {
                folded.push(entry);
            } else if matches!(entry, TranscriptEntryDto::ToolCall { .. }) {
                blocks.push(TranscriptBlock::ActivityDigest {
                    digest_id: format!("digest-{}", entry_id_of(entry)),
                    tools: vec![entry],
                    reasoning: Vec::new(),
                });
            } else {
                blocks.push(TranscriptBlock::Assistant(entry));
            }
            continue;
        }
        blocks.push(match entry {
            TranscriptEntryDto::Assistant { .. } => TranscriptBlock::Assistant(entry),
            TranscriptEntryDto::System { .. } => TranscriptBlock::System(entry),
            TranscriptEntryDto::Error { .. } => TranscriptBlock::Error(entry),
            TranscriptEntryDto::Subagent { .. } => TranscriptBlock::Subagent(entry),
            TranscriptEntryDto::Question { .. } => TranscriptBlock::Question(entry),
            TranscriptEntryDto::User { .. } | TranscriptEntryDto::ToolCall { .. } => continue,
        });
    }
    flush(&mut blocks, &mut fold_start, &mut folded);
    blocks
}

pub fn entry_id_of(entry: &TranscriptEntryDto) -> &str {
    match entry {
        TranscriptEntryDto::User { id, .. }
        | TranscriptEntryDto::Assistant { id, .. }
        | TranscriptEntryDto::System { id, .. }
        | TranscriptEntryDto::Error { id, .. }
        | TranscriptEntryDto::ToolCall { id, .. }
        | TranscriptEntryDto::Subagent { id, .. }
        | TranscriptEntryDto::Question { id, .. } => id,
    }
}

pub fn filtered_providers(
    providers: Option<&ProvidersResponse>,
    filter: &str,
) -> Vec<ProviderItemDto> {
    let Some(resp) = providers else {
        return Vec::new();
    };
    let q = filter.trim().to_ascii_lowercase();
    resp.providers
        .iter()
        .filter(|p| {
            q.is_empty()
                || p.id.to_ascii_lowercase().contains(&q)
                || p.display_name.to_ascii_lowercase().contains(&q)
        })
        .cloned()
        .collect()
}

pub fn filtered_models(
    providers: Option<&ProvidersResponse>,
    provider_filter: Option<&str>,
    filter: &str,
) -> Vec<ModelItemDto> {
    let Some(resp) = providers else {
        return Vec::new();
    };
    let q = filter.trim().to_ascii_lowercase();
    let mut items: Vec<ModelItemDto> = resp
        .models
        .iter()
        .filter(|m| {
            provider_filter.is_none_or(|p| m.provider == p || q.contains('/'))
                && (q.is_empty()
                    || m.id.to_ascii_lowercase().contains(&q)
                    || format!("{}/{}", m.provider, m.id)
                        .to_ascii_lowercase()
                        .contains(&q))
        })
        .cloned()
        .collect();
    if let Some(p) = provider_filter
        && let Some(recent) = resp.recent_models.get(p)
    {
        items.sort_by_key(|m| recent.iter().position(|r| r == &m.id).unwrap_or(usize::MAX));
    }
    items
}

pub fn filtered_history(history: &[StoredSessionDto], filter: &str) -> Vec<StoredSessionDto> {
    let q = filter.trim().to_ascii_lowercase();
    history
        .iter()
        .filter(|s| {
            q.is_empty()
                || s.id.to_ascii_lowercase().contains(&q)
                || s.title
                    .as_deref()
                    .is_some_and(|t| t.to_ascii_lowercase().contains(&q))
                || s.model
                    .as_deref()
                    .is_some_and(|m| m.to_ascii_lowercase().contains(&q))
        })
        .cloned()
        .collect()
}

pub fn filtered_commands(filter: &str) -> Vec<&'static SlashCommandSpec> {
    let q = filter.trim().to_ascii_lowercase();
    SLASH_COMMANDS
        .iter()
        .filter(|c| {
            q.is_empty()
                || c.cmd.to_ascii_lowercase().contains(&q)
                || c.desc.to_ascii_lowercase().contains(&q)
        })
        .collect()
}

pub fn parse_provider_model_arg(raw: &str, fallback_provider: &str) -> (String, String) {
    let trimmed = raw.trim();
    if let Some((p, m)) = trimmed.split_once('/') {
        let p = p.trim();
        let m = m.trim();
        if !p.is_empty() && !m.is_empty() {
            return (p.to_owned(), m.to_owned());
        }
    }
    (fallback_provider.to_owned(), trimmed.to_owned())
}

fn push_local_system(tab: &mut SessionTabState, text: String) {
    let id = format!("local-sys-{}", tab.entries.len() + tab.local_notices.len());
    tab.entries.push(TranscriptEntryDto::System { id, text });
}

fn mark_unread_if_bg(state: &mut AppState, session_id: &str) {
    if state.active_session_id.as_deref() != Some(session_id)
        && let Some(tab) = state.session_tabs.get_mut(session_id)
    {
        tab.unread = true;
    }
}

fn clear_streaming_tab(tab: &mut SessionTabState) {
    tab.streaming_item_id = None;
    tab.streaming_text.clear();
    tab.streaming_reasoning.clear();
}

fn sync_tab_order(state: &mut AppState) {
    let live: HashSet<String> = state.sessions.iter().map(|s| s.id.clone()).collect();
    state.tab_order.retain(|id| live.contains(id));
    for s in &state.sessions {
        if !state.tab_order.contains(&s.id) {
            state.tab_order.push(s.id.clone());
        }
    }
}

fn upsert_session(state: &mut AppState, summary: SessionSummaryDto) {
    let id = summary.id.clone();
    if let Some(existing) = state.sessions.iter_mut().find(|s| s.id == id) {
        *existing = summary;
    } else {
        state.sessions.push(summary);
    }
    if !state.tab_order.contains(&id) {
        state.tab_order.push(id);
    }
}

fn upsert_entry(entries: &mut Vec<TranscriptEntryDto>, entry: TranscriptEntryDto) {
    let id = entry_id_of(&entry).to_owned();
    if let Some(existing) = entries.iter_mut().find(|item| entry_id_of(item) == id) {
        *existing = entry;
    } else {
        entries.push(entry);
    }
}

fn record_prompt_history(state: &mut AppState, prompt: &str) {
    if state.prompt_history.last().map(String::as_str) != Some(prompt) {
        state.prompt_history.push(prompt.to_owned());
        if state.prompt_history.len() > 200 {
            state.prompt_history.remove(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerai_web_protocol::{ApprovalModeDto, BackendKind, SessionStatus, UsageDto};

    fn user(id: &str, turn: Option<&str>, text: &str) -> TranscriptEntryDto {
        TranscriptEntryDto::User {
            id: id.to_owned(),
            turn_id: turn.map(str::to_owned),
            text: text.to_owned(),
        }
    }

    fn tool(id: &str, name: &str, status: &str) -> TranscriptEntryDto {
        TranscriptEntryDto::ToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            args: "{}".to_owned(),
            status: status.to_owned(),
            output: None,
            edit_diff: None,
        }
    }

    fn assistant(id: &str, text: &str, reasoning: &str) -> TranscriptEntryDto {
        TranscriptEntryDto::Assistant {
            id: id.to_owned(),
            text: text.to_owned(),
            reasoning: reasoning.to_owned(),
        }
    }

    #[test]
    fn activity_summary_uses_tui_phrasing() {
        let running = activity_summary(&[("code-read_file", true), ("code-grep", false)], 0);
        assert_eq!(running, "1 tool · Reading 1");
        let done = activity_summary(
            &[
                ("code-shell", false),
                ("code-read_file", false),
                ("code-edit_file", false),
                ("subagent-run", false),
                ("lsp-get_diagnostics", false),
            ],
            2,
        );
        assert_eq!(
            done,
            "1 command · 1 tool · 1 read · 1 edit · 1 agent · 2 thoughts"
        );
    }

    #[test]
    fn digest_follows_user_turn_and_folds_tools_and_reasoning() {
        let entries = vec![
            user("u1", Some("t1"), "first"),
            tool("c1", "code-read_file", "succeeded"),
            tool("c2", "code-edit_file", "running"),
            assistant("a1", "", "thinking"),
            assistant("a2", "visible answer", "some reasoning"),
            user("u2", Some("t2"), "second"),
            tool("c3", "code-shell", "succeeded"),
        ];
        let blocks = group_transcript_blocks(&entries);
        let kinds: Vec<&str> = blocks
            .iter()
            .map(|b| match b {
                TranscriptBlock::User { .. } => "user",
                TranscriptBlock::ActivityDigest { .. } => "digest",
                TranscriptBlock::Assistant(_) => "assistant",
                TranscriptBlock::System(_) => "system",
                TranscriptBlock::Error(_) => "error",
                TranscriptBlock::Subagent(_) => "subagent",
                TranscriptBlock::Question(_) => "question",
            })
            .collect();
        assert_eq!(kinds, ["user", "digest", "assistant", "user", "digest"]);
        match &blocks[1] {
            TranscriptBlock::ActivityDigest {
                tools, reasoning, ..
            } => {
                assert_eq!(tools.len(), 2);
                assert_eq!(reasoning.len(), 1);
            }
            other => panic!("expected digest, got {other:?}"),
        }
    }

    fn state_with_tab() -> AppState {
        let mut state = AppState {
            active_session_id: Some("ses-1".to_owned()),
            ..AppState::default()
        };
        state.sessions.push(SessionSummaryDto {
            id: "ses-1".to_owned(),
            title: Some("demo".to_owned()),
            backend: BackendKind::Native,
            provider: "openai".to_owned(),
            model: "gpt".to_owned(),
            approval_mode: ApprovalModeDto::Ask,
            status: SessionStatus::Idle,
            active_turn_id: None,
            active_tool: None,
            turn_started_at_ms: None,
            user_shell_running: false,
        });
        state
            .session_tabs
            .entry("ses-1".to_owned())
            .or_default()
            .entries
            .push(user("u1", Some("t1"), "hello"));
        state
    }

    #[test]
    fn clear_only_resets_the_local_view() {
        let mut state = state_with_tab();
        state.session_tabs.get_mut("ses-1").unwrap().usage = UsageDto::default();
        execute_slash_command(&mut state, "/clear");
        assert!(
            state
                .session_tabs
                .get("ses-1")
                .is_some_and(|t| t.entries.is_empty())
        );
        assert!(
            state
                .status_notice
                .as_deref()
                .is_some_and(|n| n.contains("Cleared this view"))
        );
        assert!(state.error_banner.is_none());
    }

    #[test]
    fn fork_and_rewind_refuse_while_busy() {
        let mut state = state_with_tab();
        state.sessions[0].status = SessionStatus::Running;
        execute_slash_command(&mut state, "/fork");
        assert!(state.error_banner.is_some());
        state.error_banner = None;
        execute_slash_command(&mut state, "/rewind");
        assert!(state.error_banner.is_some());
    }

    #[test]
    fn palette_actions_cover_non_command_actions() {
        let keys: Vec<&str> = PALETTE_ACTIONS.iter().map(|a| a.key).collect();
        assert!(keys.contains(&"copy-last"));
        assert!(keys.contains(&"next-tab"));
        assert!(keys.contains(&"cycle-mode"));
        let filtered = filtered_palette_actions("copy");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].key, "copy-last");
    }
}
