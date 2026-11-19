mod session_panel;

pub use session_panel::{HistoryStatus, SessionPanel, SessionPanelAction, SessionPanelRow};

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::text::{TextBuffer, UiEffects};
use meerai_web_protocol::{
    ApprovalModeDto, ApprovalRequestDto, BackendKind, ChatGptLoginStateDto, ContextSettingsDto,
    EffortDto, FileChangeDto, FileDiffResponse, HostInfoDto, ModelItemDto, ProviderItemDto,
    ProvidersResponse, QuestionRequestDto, ReloadResponse, RewindScopeDto, SandboxSettingsDto,
    SessionDetailDto, SessionSettingsDto, SessionStatus, SessionSummaryDto, SnapshotResponse,
    ThemeOverrideDto, TranscriptEntryDto, TurnMetricsDto, UpdateSettingsRequest, UsageDto,
    ViewEvent, WorkspacePreviewResponse, WorkspaceSearchResponse,
};

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
        args: "[id|filter]",
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
        cmd: "/effort",
        args: "[low|medium|high]",
        desc: "Show, set, or cycle reasoning effort",
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
        desc: "Clear visible chat transcript",
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
pub enum ProviderModalReturn {
    Close,
    ProviderSelect,
    SettingsConnections,
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
        hint: "Alt+C",
    },
    PaletteActionSpec {
        key: "next-tab",
        label: "Next session tab",
        hint: "Alt+]",
    },
    PaletteActionSpec {
        key: "prev-tab",
        label: "Previous session tab",
        hint: "Alt+[",
    },
    PaletteActionSpec {
        key: "move-tab-left",
        label: "Move tab left",
        hint: "Alt+Shift+[",
    },
    PaletteActionSpec {
        key: "move-tab-right",
        label: "Move tab right",
        hint: "Alt+Shift+]",
    },
    PaletteActionSpec {
        key: "cycle-mode",
        label: "Cycle approval mode",
        hint: "Alt+M",
    },
    PaletteActionSpec {
        key: "cycle-effort",
        label: "Cycle reasoning effort",
        hint: "/effort",
    },
    PaletteActionSpec {
        key: "new-session",
        label: "New session tab",
        hint: "/new",
    },
    PaletteActionSpec {
        key: "toggle-sidebar",
        label: "Toggle workspace sidebar",
        hint: "Ctrl+B",
    },
    PaletteActionSpec {
        key: "open-diff",
        label: "Inspect workspace diff",
        hint: "Alt+Shift+D",
    },
    PaletteActionSpec {
        key: "open-search",
        label: "Search workspace",
        hint: "Alt+Shift+F",
    },
    PaletteActionSpec {
        key: "open-settings",
        label: "Open settings",
        hint: "/settings",
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
                set_status_notice(state, "Copied the latest reply");
            } else {
                set_status_notice(state, "No assistant reply to copy");
            }
        }
        "next-tab" => cycle_tab(state, 1),
        "prev-tab" => cycle_tab(state, -1),
        "move-tab-left" => reorder_active_tab(state, -1),
        "move-tab-right" => reorder_active_tab(state, 1),
        "cycle-mode" => cycle_approval_mode(state),
        "cycle-effort" => execute_slash_command(state, "/effort"),
        "new-session" => {
            if let Some(t) = crate::transport::current() {
                t.create_session(None);
            }
        }
        "toggle-sidebar" => state.sidebar_open = !state.sidebar_open,
        "open-diff" => open_diff_modal(state, None),
        "open-search" => open_search_modal(state, meerai_web_protocol::SearchModeDto::Content, ""),
        "open-settings" => open_settings(state, None),
        _ => {}
    }
}

#[derive(Clone, Debug)]
pub struct SettingsFormState {
    pub section: SettingsSection,
    pub provider_filter: TextBuffer,
    pub reset_theme: bool,
    pub color_scheme: Option<String>,
    pub theme_fields: BTreeMap<&'static str, TextBuffer>,
    pub inputs: BTreeMap<&'static str, TextBuffer>,
    pub reasoning_pruning: bool,
    pub prompt_caching: bool,
    pub rollback_on_verify_failure: bool,
    pub post_edit_diagnostics: bool,
    pub effort: Option<EffortDto>,
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
            section: SettingsSection::Connections,
            provider_filter: TextBuffer::new(""),
            reset_theme: false,
            color_scheme: raw.color_scheme.clone(),
            theme_fields,
            inputs,
            reasoning_pruning: c.reasoning_pruning.unwrap_or(true),
            prompt_caching: c.prompt_caching.unwrap_or(true),
            rollback_on_verify_failure: c.rollback_on_verify_failure.unwrap_or(false),
            post_edit_diagnostics: c.post_edit_diagnostics.unwrap_or(false),
            effort: c.effort,
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

        let opt_color = |key: &str| -> Result<Option<String>, String> {
            let v = self
                .theme_fields
                .get(key)
                .map(|e| e.text.trim().to_owned())
                .unwrap_or_default();
            if v.is_empty() {
                return Ok(None);
            }
            if !crate::theme::is_valid_color(&v) {
                return Err(format!(
                    "Invalid color for {key}: {v} (use #rgb, #rrggbb, or a CSS color name)"
                ));
            }
            Ok(Some(v))
        };

        let theme = if self.reset_theme {
            None
        } else {
            Some(ThemeOverrideDto {
                color_scheme: self.color_scheme.clone(),
                background: opt_color("background")?,
                surface: opt_color("surface")?,
                surface_high: opt_color("surface_high")?,
                text: opt_color("text")?,
                muted: opt_color("muted")?,
                faint: opt_color("faint")?,
                accent: opt_color("accent")?,
                interactive: opt_color("interactive")?,
                border: opt_color("border")?,
                error: opt_color("error")?,
                warning: opt_color("warning")?,
                success: opt_color("success")?,
                info: opt_color("info")?,
                diff_added: opt_color("diff_added")?,
                diff_removed: opt_color("diff_removed")?,
                diff_context: opt_color("diff_context")?,
                code_function: opt_color("code_function")?,
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
                effort: self.effort,
            }),
            forget_model: None,
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
        return_to: ProviderModalReturn,
        selected: usize,
    },
    ProviderConnection {
        provider_id: String,
        provider_name: String,
        auth_kind: meerai_web_protocol::AuthKindDto,
        status_badge: String,
        configured: bool,
        base_url: Option<String>,
        env_vars: Vec<String>,
        default_model: Option<String>,
        return_to: ProviderModalReturn,
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
        return_to: ProviderModalReturn,
    },
    ApiKeyInput {
        provider_id: String,
        provider_name: String,
        auth_kind: meerai_web_protocol::AuthKindDto,
        status_badge: String,
        configured: bool,
        env_vars: Vec<String>,
        api_key: TextBuffer,
        base_url: TextBuffer,
        error: Option<String>,
        busy: bool,
        return_to: ProviderModalReturn,
    },
    ModelInput {
        filter: TextBuffer,
        provider_id: Option<String>,
        selected: usize,
        return_to: ProviderModalReturn,
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
    CommandPalette {
        filter: TextBuffer,
        selected: usize,
    },
    Settings(SettingsFormState),
    QueuedEdit {
        index: usize,
        input: TextBuffer,
        error: Option<String>,
    },
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
    pub entries_version: u64,
    pub hydrated: bool,
    pub streaming_item_id: Option<String>,
    pub streaming_text: String,
    pub streaming_reasoning: String,
    pub pending_approvals: Vec<ApprovalRequestDto>,
    pub pending_questions: Vec<QuestionRequestDto>,
    pub usage: UsageDto,
    pub metrics: TurnMetricsDto,
    pub pending_submit: Option<String>,
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
}

pub struct AppState {
    pub active_session_id: Option<String>,
    pub sessions: Vec<SessionSummaryDto>,
    pub tab_order: Vec<String>,
    pub hidden_session_ids: HashSet<String>,
    pub pending_replace_session: Option<String>,
    pub session_tabs: HashMap<String, SessionTabState>,
    pub session_panel: SessionPanel,
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
    pub sidebar_tools_expanded: bool,
    pub modal: Option<ActiveModal>,
    pub sending: bool,
    pub reset_confirm_pending: bool,
    pub error_banner: Option<String>,
    pub status_notice: Option<String>,
    pub notice_seq: u64,
    pub last_title_sent: String,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            active_session_id: None,
            sessions: Vec::new(),
            tab_order: Vec::new(),
            hidden_session_ids: HashSet::new(),
            pending_replace_session: None,
            session_tabs: HashMap::new(),
            session_panel: SessionPanel::default(),
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
            sidebar_tools_expanded: false,
            modal: None,
            sending: false,
            reset_confirm_pending: false,
            error_banner: None,
            status_notice: None,
            notice_seq: 0,
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

pub fn set_status_notice(state: &mut AppState, msg: impl Into<String>) {
    state.notice_seq = state.notice_seq.wrapping_add(1);
    state.status_notice = Some(msg.into());
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
    tab.entries_version = tab.entries_version.wrapping_add(1);
    tab.hydrated = true;
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

pub fn apply_session_detail(state: &mut AppState, detail: SessionDetailDto) {
    let id = detail.summary.id.clone();
    upsert_session(state, detail.summary);
    let tab = state.session_tabs.entry(id.clone()).or_default();
    tab.entries = detail.entries;
    tab.entries_version = tab.entries_version.wrapping_add(1);
    tab.hydrated = true;
    tab.streaming_item_id = detail.streaming_item_id;
    tab.streaming_text = detail.streaming_text;
    tab.streaming_reasoning = detail.streaming_reasoning;
    tab.pending_approvals = detail.pending_approvals;
    tab.pending_questions = detail.pending_questions;
    tab.usage = detail.usage;
    tab.metrics = detail.metrics;
    if state.active_session_id.as_deref() == Some(id.as_str()) {
        tab.unread = false;
        sync_pending_modal_for_active(state);
    }
}

pub fn apply_created_session(state: &mut AppState, session: SessionSummaryDto) {
    let id = session.id.clone();
    if let Some(prev) = state.pending_replace_session.take()
        && prev != id
    {
        state.hidden_session_ids.insert(prev);
    }
    state.hidden_session_ids.remove(&id);
    upsert_session(state, session);
    switch_to_session(state, &id);
    let tab = state.session_tabs.entry(id).or_default();
    tab.entries.clear();
    tab.entries_version = tab.entries_version.wrapping_add(1);
    tab.hydrated = true;
    clear_streaming_tab(tab);
    state.modal = None;
    state.error_banner = None;
}

pub fn apply_opened_session(state: &mut AppState, session: SessionSummaryDto) {
    let id = session.id.clone();
    state.hidden_session_ids.remove(&id);
    upsert_session(state, session);
    if let Some(tab) = state.session_tabs.get_mut(&id) {
        tab.hydrated = false;
    }
    switch_to_session(state, &id);
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
            if became_idle {
                drain_next_queued_if_idle(state, &id);
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
            tab.entries_version = tab.entries_version.wrapping_add(1);
            tab.pending_submit = None;
            clear_streaming_tab(tab);
            tab.pending_approvals.clear();
            tab.pending_questions.clear();
            tab.usage = UsageDto::default();
            tab.metrics = TurnMetricsDto::default();
            if state.active_session_id.as_deref() == Some(session_id.as_str()) {
                state.reset_confirm_pending = false;
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
            tab.entries_version = tab.entries_version.wrapping_add(1);
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
                state.draft.replace_text(prompt.clone());
                if let Some(tab) = state.active_tab_mut() {
                    tab.draft_text = prompt;
                }
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
            tab.pending_submit = None;
            if !tab.entries.iter().any(|e| entry_id_of(e) == entry_id) {
                tab.entries.push(TranscriptEntryDto::User {
                    id: entry_id,
                    turn_id: None,
                    text,
                });
                tab.entries_version = tab.entries_version.wrapping_add(1);
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
            tab.entries_version = tab.entries_version.wrapping_add(1);
            mark_unread_if_bg(state, &session_id);
            EventEffect::None
        }
        ViewEvent::EntryUpdated { session_id, entry } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            upsert_entry(&mut tab.entries, entry);
            tab.entries_version = tab.entries_version.wrapping_add(1);
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
            drain_next_queued_if_idle(state, &session_id);
            EventEffect::None
        }
        ViewEvent::Error {
            session_id,
            code,
            message,
        } => {
            let tab = state.session_tabs.entry(session_id.clone()).or_default();
            tab.pending_submit = None;
            clear_streaming_tab(tab);
            if state.active_session_id.as_deref() == Some(session_id.as_str()) {
                state.error_banner = Some(format!("{code}: {message}"));
            }
            EventEffect::None
        }
        ViewEvent::FileChangesUpdated { changes } => {
            state.file_changes = changes;
            sync_diff_modal_on_changes(state);
            EventEffect::None
        }
        ViewEvent::Reloaded {
            host_info,
            warnings,
        } => {
            state.host_info = *host_info;
            let notice = if !warnings.is_empty() {
                format!("Reload warnings: {}", warnings.join("; "))
            } else {
                "Reloaded configuration".to_owned()
            };
            set_status_notice(state, notice.clone());
            if let Some(ActiveModal::Settings(form)) = &mut state.modal {
                let prev_section = form.section;
                let prev_filter = form.provider_filter.clone();
                *form = SettingsFormState::from_host(&state.host_info);
                form.section = prev_section;
                form.provider_filter = prev_filter;
                form.status = Some(notice);
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
            let mut completed_return = None;
            if let Some(ActiveModal::ChatGptLogin {
                provider_id: pid,
                attempt_id: att,
                url: modal_url,
                state: st,
                error,
                busy,
                return_to,
                ..
            }) = &mut state.modal
                && pid == &provider_id
                && (att.is_none() || att.as_deref() == Some(attempt_id.as_str()))
            {
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
                        completed_return = Some((*return_to, account));
                    }
                    ChatGptLoginStateDto::Failed => {
                        *busy = false;
                        *error = message;
                    }
                    ChatGptLoginStateDto::Cancelled => {
                        let origin = *return_to;
                        return_from_provider_modal(state, origin);
                    }
                }
            }
            if let Some((origin, account)) = completed_return {
                let acct = account.unwrap_or_else(|| "signed in".to_owned());
                set_status_notice(state, format!("Connected {provider_id} ({acct})"));
                if origin == ProviderModalReturn::SettingsConnections {
                    open_settings(state, Some(SettingsSection::Connections));
                } else {
                    open_model_input_for(state, Some(provider_id), origin);
                }
                if let Some(t) = crate::transport::current() {
                    t.fetch_providers();
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

pub fn apply_providers_response(state: &mut AppState, providers: ProvidersResponse) {
    state.providers = Some(providers);
    update_completion(state);
}

pub fn apply_reload_response(state: &mut AppState, resp: ReloadResponse) {
    state.host_info = resp.host_info;
    let notice = if resp.warnings.is_empty() {
        "Reloaded configuration".to_owned()
    } else {
        format!("Reloaded with warnings: {}", resp.warnings.join("; "))
    };
    set_status_notice(state, notice.clone());
    if let Some(ActiveModal::Settings(form)) = &mut state.modal {
        let prev_section = form.section;
        let prev_filter = form.provider_filter.clone();
        *form = SettingsFormState::from_host(&state.host_info);
        form.section = prev_section;
        form.provider_filter = prev_filter;
        form.status = Some(notice);
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

pub fn reopen_pending_request(state: &mut AppState) {
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

pub fn sync_pending_modal_for_active(state: &mut AppState) {
    if state.modal.is_some() {
        return;
    }
    reopen_pending_request(state);
}

pub fn activate_and_switch_session(state: &mut AppState, session_id: &str) {
    if !state.sessions.iter().any(|s| s.id == session_id) {
        return;
    }
    switch_to_session(state, session_id);
    if let Some(t) = crate::transport::current() {
        t.activate_session(session_id.to_owned());
    }
}

pub fn switch_to_session(state: &mut AppState, session_id: &str) {
    if !state.sessions.iter().any(|s| s.id == session_id) {
        return;
    }
    state.hidden_session_ids.remove(session_id);
    let changed = state.active_session_id.as_deref() != Some(session_id);
    if let Some(prev) = state.active_session_id.clone()
        && prev != session_id
    {
        let prev_draft = state.draft.text.clone();
        state.session_tabs.entry(prev).or_default().draft_text = prev_draft;
    }
    if changed
        && matches!(
            state.modal,
            Some(
                ActiveModal::Approval { .. }
                    | ActiveModal::Question { .. }
                    | ActiveModal::RewindMenu { .. }
                    | ActiveModal::SubagentOverlay { .. }
            )
        )
    {
        state.modal = None;
    }
    state.reset_confirm_pending = false;
    state.active_session_id = Some(session_id.to_owned());
    let (next_draft, needs_hydrate) = {
        let tab = state.session_tabs.entry(session_id.to_owned()).or_default();
        tab.unread = false;
        (tab.draft_text.clone(), !tab.hydrated)
    };
    state.draft.replace_text(next_draft);
    state.error_banner = None;
    update_completion(state);
    if needs_hydrate && let Some(t) = crate::transport::current() {
        t.fetch_session_detail(session_id.to_owned());
    }
    drain_next_queued_if_idle(state, session_id);
    sync_pending_modal_for_active(state);
}

pub fn select_tab_index(state: &mut AppState, idx: usize) {
    let visible_ids: Vec<String> = state
        .visible_sessions()
        .into_iter()
        .map(|s| s.id.clone())
        .collect();
    if let Some(id) = visible_ids.get(idx).cloned() {
        activate_and_switch_session(state, &id);
    }
}

pub fn move_tab_to(state: &mut AppState, from_id: &str, to_id: &str) {
    if from_id == to_id {
        return;
    }
    sync_tab_order(state);
    let Some(from_idx) = state.tab_order.iter().position(|id| id == from_id) else {
        return;
    };
    let Some(to_idx) = state.tab_order.iter().position(|id| id == to_id) else {
        return;
    };
    let item = state.tab_order.remove(from_idx);
    state.tab_order.insert(to_idx, item);
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
        state.pending_replace_session = Some(session_id.to_owned());
        set_status_notice(state, "Closing the last tab, opening a new session");
        if let Some(t) = crate::transport::current() {
            t.create_session(None);
        }
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
        activate_and_switch_session(state, &next);
    }
}

impl AppState {
    pub fn can_submit_composer(&self) -> bool {
        let draft = self.draft.text.trim();
        self.connection != ConnectionState::Disconnected
            && !draft.is_empty()
            && (self.active_session_id.is_some() || draft.starts_with('/'))
    }
}

pub fn submit_or_queue_composer(state: &mut AppState) {
    if !state.can_submit_composer() {
        return;
    }
    let trimmed = state.draft.text.trim().to_owned();
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
    let prompt = if mentions.skills.is_empty() && mentions.agent.is_none() {
        trimmed.clone()
    } else {
        mentions.cleaned
    };

    if state.is_busy() {
        state
            .session_tabs
            .entry(sid)
            .or_default()
            .pending_queue
            .push_back(prompt);
        set_status_notice(state, "Queued prompt (click item to steer now)");
        return;
    }

    state.sending = true;
    state.error_banner = None;
    state
        .session_tabs
        .entry(sid.clone())
        .or_default()
        .pending_submit = Some(prompt.clone());
    if let Some(t) = crate::transport::current() {
        t.submit_prompt(sid, prompt, Some(trimmed));
    }
}

pub fn forget_model(state: &mut AppState, provider: &str, model: &str) {
    if let Some(providers) = state.providers.as_mut()
        && let Some(list) = providers.recent_models.get_mut(provider)
    {
        list.retain(|m| m != model);
        if list.is_empty() {
            providers.recent_models.remove(provider);
        }
    }
    if let Some(t) = crate::transport::current() {
        t.update_settings(UpdateSettingsRequest {
            reset_theme: false,
            theme: None,
            context: None,
            session: None,
            sandbox: None,
            forget_model: Some(meerai_web_protocol::ForgetModelDto {
                provider: provider.to_owned(),
                model: model.to_owned(),
            }),
        });
    }
    set_status_notice(state, format!("Removed {provider}/{model}"));
}

pub fn open_queued_edit(state: &mut AppState, index: usize) {
    let Some(prompt) = state
        .active_tab()
        .and_then(|tab| tab.pending_queue.get(index))
        .cloned()
    else {
        return;
    };
    state.modal = Some(ActiveModal::QueuedEdit {
        index,
        input: TextBuffer::new(prompt),
        error: None,
    });
}

pub fn commit_queued_edit(state: &mut AppState) {
    let Some(ActiveModal::QueuedEdit { index, input, .. }) = state.modal.clone() else {
        return;
    };
    let text = input.text.trim().to_owned();
    if text.is_empty() {
        if let Some(ActiveModal::QueuedEdit { error, .. }) = &mut state.modal {
            *error = Some("Message is required".to_owned());
        }
        return;
    }
    if let Some(tab) = state.active_tab_mut()
        && let Some(slot) = tab.pending_queue.get_mut(index)
    {
        *slot = text;
        state.modal = None;
        set_status_notice(state, "Queued message updated");
    }
}

fn adjust_queued_edit_modal(state: &mut AppState, session_id: &str, removed_idx: usize) {
    if state.active_session_id.as_deref() != Some(session_id) {
        return;
    }
    if let Some(ActiveModal::QueuedEdit { index, .. }) = &mut state.modal {
        if *index == removed_idx {
            state.modal = None;
            set_status_notice(state, "Queued message was removed or sent");
        } else if *index > removed_idx {
            *index -= 1;
        }
    }
}

pub fn remove_queued(state: &mut AppState, index: usize) {
    let Some(sid) = state.active_session_id.clone() else {
        return;
    };
    if let Some(tab) = state.active_tab_mut()
        && index < tab.pending_queue.len()
    {
        tab.pending_queue.remove(index);
        adjust_queued_edit_modal(state, &sid, index);
        set_status_notice(state, "Removed queued message");
    }
}

pub fn steer_or_send_queued(state: &mut AppState, index: usize) {
    let Some(sid) = state.active_session_id.clone() else {
        return;
    };
    if let Some(prompt) = state
        .session_tabs
        .get_mut(&sid)
        .and_then(|tab| tab.pending_queue.remove(index))
    {
        adjust_queued_edit_modal(state, &sid, index);
        let mentions = extract_mentions(&prompt, &state.host_info);
        if let Some(t) = crate::transport::current() {
            for skill in &mentions.skills {
                t.apply_skill(sid.clone(), skill.clone());
            }
            if let Some(agent) = &mentions.agent {
                t.apply_agent(sid.clone(), agent.clone());
            }
        }
        let has_mentions = !mentions.skills.is_empty() || mentions.agent.is_some();
        let clean = if has_mentions {
            mentions.cleaned
        } else {
            prompt.clone()
        };
        if clean.is_empty() {
            let tab = state.session_tabs.entry(sid).or_default();
            push_applied_mentions_notice(tab, &mentions.skills, mentions.agent.as_deref());
            return;
        }
        let display = has_mentions.then_some(prompt);
        if !state.is_busy() {
            state.sending = true;
            state
                .session_tabs
                .entry(sid.clone())
                .or_default()
                .pending_submit = Some(clean.clone());
        }
        let Some(t) = crate::transport::current() else {
            return;
        };
        if state.is_busy() {
            t.steer_prompt(sid, clean, display);
        } else {
            t.submit_prompt(sid, clean, display);
        }
        return;
    }

    if index == 0 && state.is_busy() {
        let trimmed = state.draft.text.trim().to_owned();
        if trimmed.is_empty() || trimmed.starts_with('/') || trimmed.starts_with('!') {
            return;
        }
        let mentions = extract_mentions(&trimmed, &state.host_info);
        if let Some(t) = crate::transport::current() {
            for skill in &mentions.skills {
                t.apply_skill(sid.clone(), skill.clone());
            }
            if let Some(agent) = &mentions.agent {
                t.apply_agent(sid.clone(), agent.clone());
            }
        }
        let prompt = if mentions.skills.is_empty() && mentions.agent.is_none() {
            trimmed.clone()
        } else {
            mentions.cleaned
        };
        if prompt.is_empty() {
            return;
        }
        state.completion = CompletionState::default();
        state.prompt_history_idx = None;
        record_prompt_history(state, &trimmed);
        state.draft.clear();
        if let Some(tab) = state.active_tab_mut() {
            tab.draft_text.clear();
        }
        if let Some(t) = crate::transport::current() {
            t.steer_prompt(sid, prompt, Some(trimmed));
        }
    }
}

fn push_applied_mentions_notice(tab: &mut SessionTabState, skills: &[String], agent: Option<&str>) {
    let mut applied = Vec::new();
    for s in skills {
        applied.push(format!("skill:{s}"));
    }
    if let Some(a) = agent {
        applied.push(format!("agent:{a}"));
    }
    if !applied.is_empty() {
        push_local_system(tab, format!("Applied {}", applied.join(", ")));
    }
}

pub fn drain_next_queued_if_idle(state: &mut AppState, session_id: &str) {
    if state.sending || state.connection == ConnectionState::Disconnected {
        return;
    }
    let is_session_idle = state
        .sessions
        .iter()
        .find(|s| s.id == session_id)
        .is_some_and(|s| s.status == SessionStatus::Idle && !s.user_shell_running);
    if !is_session_idle {
        return;
    }
    while let Some(queued_text) = state
        .session_tabs
        .get_mut(session_id)
        .and_then(|tab| tab.pending_queue.pop_front())
    {
        adjust_queued_edit_modal(state, session_id, 0);
        let mentions = extract_mentions(&queued_text, &state.host_info);
        let has_mentions = !mentions.skills.is_empty() || mentions.agent.is_some();
        if has_mentions && let Some(t) = crate::transport::current() {
            for skill in &mentions.skills {
                t.apply_skill(session_id.to_owned(), skill.clone());
            }
            if let Some(agent) = &mentions.agent {
                t.apply_agent(session_id.to_owned(), agent.clone());
            }
        }
        let prompt = if has_mentions {
            mentions.cleaned
        } else {
            queued_text.clone()
        };
        if prompt.is_empty() {
            if let Some(tab) = state.session_tabs.get_mut(session_id) {
                push_applied_mentions_notice(tab, &mentions.skills, mentions.agent.as_deref());
            }
            continue;
        }
        state.sending = true;
        if let Some(tab) = state.session_tabs.get_mut(session_id) {
            tab.pending_submit = Some(prompt.clone());
        }
        if let Some(t) = crate::transport::current() {
            t.submit_prompt(
                session_id.to_owned(),
                prompt,
                has_mentions.then_some(queued_text),
            );
        }
        break;
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
                activate_and_switch_session(state, arg);
            } else {
                state.session_panel.reveal(Some(arg));
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
                        set_status_notice(state, "No closed turn to fork.");
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
        "/effort" => {
            set_effort_from_arg(state, arg);
        }
        "/agent" => {
            if arg.is_empty() {
                let msg = format!(
                    "Usage: /agent <id>. Available: {}",
                    state
                        .host_info
                        .agents
                        .iter()
                        .map(|a| a.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                set_status_notice(state, msg);
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
                let msg = format!(
                    "Usage: /skill <id>. Available: {}",
                    state
                        .host_info
                        .skills
                        .iter()
                        .map(|s| s.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                set_status_notice(state, msg);
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
                tab.entries_version = tab.entries_version.wrapping_add(1);
                tab.streaming_item_id = None;
                tab.streaming_text.clear();
                tab.streaming_reasoning.clear();
                tab.expanded_digests.clear();
                tab.expanded_tools.clear();
                tab.expanded_reasoning.clear();
                tab.expanded_errors.clear();
                tab.expanded_subagents.clear();
                set_status_notice(state, "Cleared this view. The session log is unchanged.");
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
        "/settings" => open_settings(state, None),
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
                push_local_system(tab, msg);
            } else {
                set_status_notice(state, msg);
            }
        }
        "/quit" | "/q" => {
            let msg = "Browser companion cannot stop the host process. Close the browser tab or use /close to shut down this session.";
            if let Some(tab) = state.active_tab_mut() {
                push_local_system(tab, msg.to_owned());
            } else {
                set_status_notice(state, msg);
            }
        }
        "/serve" => {
            let msg = "Host web server is already serving this browser UI.";
            if let Some(tab) = state.active_tab_mut() {
                push_local_system(tab, msg.to_owned());
            } else {
                set_status_notice(state, msg);
            }
        }
        _ => {
            state.error_banner = Some(format!(
                "Unknown command: {cmd}. Type /help for available commands."
            ));
        }
    }
}

pub fn effort_label(effort: EffortDto) -> &'static str {
    match effort {
        EffortDto::Low => "low",
        EffortDto::Medium => "medium",
        EffortDto::High => "high",
    }
}

pub fn effort_default_temperature(effort: EffortDto) -> f32 {
    match effort {
        EffortDto::Low => 0.0,
        EffortDto::Medium => 0.2,
        EffortDto::High => 0.5,
    }
}

pub fn set_effort_from_arg(state: &mut AppState, arg: &str) {
    let value = arg.trim().to_ascii_lowercase();
    let current = state
        .host_info
        .settings
        .context
        .effort
        .unwrap_or(EffortDto::Medium);
    let next = match value.as_str() {
        "" => match current {
            EffortDto::Low => EffortDto::Medium,
            EffortDto::Medium => EffortDto::High,
            EffortDto::High => EffortDto::Low,
        },
        "low" => EffortDto::Low,
        "medium" | "mid" => EffortDto::Medium,
        "high" => EffortDto::High,
        other => {
            set_status_notice(
                state,
                format!("Unknown effort '{other}' · use low, medium, or high"),
            );
            return;
        }
    };
    if !state.host_info.settings.config_writable {
        set_status_notice(state, "User settings file is read-only on this host.");
        return;
    }
    state.host_info.settings.context.effort = Some(next);
    state.host_info.settings.context.temperature = None;
    let context = state.host_info.settings.context.clone();
    if let Some(t) = crate::transport::current() {
        t.update_settings(UpdateSettingsRequest {
            reset_theme: false,
            theme: None,
            context: Some(context),
            session: None,
            sandbox: None,
            forget_model: None,
        });
    }
    set_status_notice(
        state,
        format!(
            "Effort {} · temp {:.2}",
            effort_label(next),
            effort_default_temperature(next)
        ),
    );
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

pub fn open_settings(state: &mut AppState, section: Option<SettingsSection>) {
    if let Some(t) = crate::transport::current() {
        t.fetch_providers();
    }
    let mut form = SettingsFormState::from_host(&state.host_info);
    if let Some(sec) = section {
        form.section = sec;
    }
    state.modal = Some(ActiveModal::Settings(form));
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
    open_model_input_for(state, None, ProviderModalReturn::Close);
}

pub fn open_model_input_for(
    state: &mut AppState,
    provider_id: Option<String>,
    return_to: ProviderModalReturn,
) {
    if let Some(t) = crate::transport::current() {
        t.fetch_providers();
    }
    state.modal = Some(ActiveModal::ModelInput {
        filter: TextBuffer::new(""),
        provider_id,
        selected: 0,
        return_to,
    });
}

pub fn return_from_provider_modal(state: &mut AppState, return_to: ProviderModalReturn) {
    match return_to {
        ProviderModalReturn::Close => state.modal = None,
        ProviderModalReturn::ProviderSelect => open_provider_select(state),
        ProviderModalReturn::SettingsConnections => {
            open_settings(state, Some(SettingsSection::Connections));
        }
    }
}

pub fn open_diff_modal(state: &mut AppState, path: Option<String>) {
    open_diff_modal_with_status(state, path, None);
}

pub fn open_diff_modal_with_status(
    state: &mut AppState,
    path: Option<String>,
    status: Option<String>,
) {
    let target = path.or_else(|| state.file_changes.first().map(|c| c.path.clone()));
    let resolved_status = status.or_else(|| {
        target.as_ref().and_then(|p| {
            state
                .file_changes
                .iter()
                .find(|c| &c.path == p)
                .map(|c| c.status.clone())
        })
    });
    if let Some(t) = crate::transport::current() {
        t.fetch_workspace_changes();
        if let Some(p) = &target {
            t.fetch_workspace_diff(p.clone(), resolved_status);
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

pub fn sync_diff_modal_on_changes(state: &mut AppState) {
    let Some(ActiveModal::DiffView {
        selected_path,
        diff,
        loading,
        error,
        ..
    }) = state.modal.as_mut()
    else {
        return;
    };

    if let Some(current_path) = selected_path.as_ref()
        && let Some(entry) = state.file_changes.iter().find(|c| &c.path == current_path)
    {
        if diff.is_none() {
            *loading = true;
        }
        if let Some(t) = crate::transport::current() {
            t.fetch_workspace_diff(entry.path.clone(), Some(entry.status.clone()));
        }
        return;
    }

    if let Some(first) = state.file_changes.first().cloned() {
        *selected_path = Some(first.path.clone());
        *diff = None;
        *loading = true;
        *error = None;
        if let Some(t) = crate::transport::current() {
            t.fetch_workspace_diff(first.path, Some(first.status));
        }
    } else {
        *selected_path = None;
        *diff = None;
        *loading = false;
        *error = None;
    }
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
        set_status_notice(state, "No recorded turns to rewind to.");
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

pub fn activate_provider_from_modal(
    state: &mut AppState,
    provider: ProviderItemDto,
    return_to: ProviderModalReturn,
) {
    if provider.configured {
        state.modal = Some(ActiveModal::ProviderConnection {
            provider_id: provider.id,
            provider_name: provider.display_name,
            auth_kind: provider.auth_kind,
            status_badge: provider.status_badge,
            configured: provider.configured,
            base_url: provider.base_url,
            env_vars: provider.env_vars,
            default_model: provider.default_model,
            return_to,
            selected: 0,
        });
        return;
    }
    match provider.auth_kind {
        meerai_web_protocol::AuthKindDto::None => {
            open_model_input_for(state, Some(provider.id), return_to);
        }
        meerai_web_protocol::AuthKindDto::ApiKey => {
            open_api_key_modal(state, &provider, return_to);
        }
        meerai_web_protocol::AuthKindDto::OAuth => {
            start_oauth_modal(state, &provider, return_to);
        }
        meerai_web_protocol::AuthKindDto::Either => {
            state.modal = Some(ActiveModal::AuthMethodSelect {
                provider_id: provider.id,
                provider_name: provider.display_name,
                return_to,
                selected: 0,
            });
        }
    }
}

pub fn open_provider_model_select(
    state: &mut AppState,
    provider_id: String,
    return_to: ProviderModalReturn,
) {
    open_model_input_for(state, Some(provider_id), return_to);
}

pub fn open_provider_credential_update(
    state: &mut AppState,
    provider: &ProviderItemDto,
    return_to: ProviderModalReturn,
) {
    match provider.auth_kind {
        meerai_web_protocol::AuthKindDto::None | meerai_web_protocol::AuthKindDto::ApiKey => {
            open_api_key_modal(state, provider, return_to);
        }
        meerai_web_protocol::AuthKindDto::OAuth => {
            start_oauth_modal(state, provider, return_to);
        }
        meerai_web_protocol::AuthKindDto::Either => {
            state.modal = Some(ActiveModal::AuthMethodSelect {
                provider_id: provider.id.clone(),
                provider_name: provider.display_name.clone(),
                return_to,
                selected: 0,
            });
        }
    }
}

pub fn open_api_key_modal(
    state: &mut AppState,
    provider: &ProviderItemDto,
    return_to: ProviderModalReturn,
) {
    state.modal = Some(ActiveModal::ApiKeyInput {
        provider_id: provider.id.clone(),
        provider_name: provider.display_name.clone(),
        auth_kind: provider.auth_kind,
        status_badge: provider.status_badge.clone(),
        configured: provider.configured,
        env_vars: provider.env_vars.clone(),
        api_key: TextBuffer::new(""),
        base_url: TextBuffer::new(provider.base_url.clone().unwrap_or_default()),
        error: None,
        busy: false,
        return_to,
    });
}

pub fn start_oauth_modal(
    state: &mut AppState,
    provider: &ProviderItemDto,
    return_to: ProviderModalReturn,
) {
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
        return_to,
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
    pub cleaned: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MentionSegment {
    pub category: Option<MentionCategory>,
    pub text: String,
}

pub fn mention_segments(input: &str, host: &HostInfoDto) -> Vec<MentionSegment> {
    let agent_ids: HashSet<&str> = host.agents.iter().map(|a| a.id.as_str()).collect();
    let skill_ids: HashSet<&str> = host.skills.iter().map(|s| s.id.as_str()).collect();
    let mut segments = Vec::new();
    let mut plain_start = 0;
    let mut index = 0;

    while let Some(offset) = input[index..].find('@') {
        let at = index + offset;
        if let Some((category, id, end)) = scan_mention(input, at, &agent_ids, &skill_ids) {
            if at > plain_start {
                segments.push(MentionSegment {
                    category: None,
                    text: input[plain_start..at].to_owned(),
                });
            }
            segments.push(MentionSegment {
                category: Some(category),
                text: id.to_owned(),
            });
            index = end;
            plain_start = end;
        } else {
            index = at + 1;
        }
    }

    if plain_start < input.len() {
        segments.push(MentionSegment {
            category: None,
            text: input[plain_start..].to_owned(),
        });
    }
    segments
}

fn is_mention_id_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')
}

fn scan_mention<'a>(
    input: &'a str,
    at: usize,
    agent_ids: &HashSet<&str>,
    skill_ids: &HashSet<&str>,
) -> Option<(MentionCategory, &'a str, usize)> {
    let rest = input.get(at + 1..)?;
    let (category, rest) = if let Some(rest) = rest.strip_prefix("agent/") {
        (MentionCategory::Agent, rest)
    } else {
        (MentionCategory::Skill, rest.strip_prefix("skill/")?)
    };
    let id_len = rest
        .char_indices()
        .find(|(_, ch)| !is_mention_id_char(*ch))
        .map(|(idx, _)| idx)
        .unwrap_or(rest.len());
    if id_len == 0 {
        return None;
    }
    let id = &rest[..id_len];
    let known = match category {
        MentionCategory::Agent => agent_ids.contains(id),
        MentionCategory::Skill => skill_ids.contains(id),
        MentionCategory::File => false,
    };
    known.then_some((category, id, at + 1 + 6 + id_len))
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

pub fn extract_mentions(input: &str, host: &HostInfoDto) -> ExtractedMentions {
    let mut agent = None;
    let mut skills = Vec::new();
    let mut seen_skills = HashSet::new();
    let mut cleaned = String::new();
    let mut has_non_definition_text = false;

    for segment in mention_segments(input, host) {
        match segment.category {
            Some(MentionCategory::Agent) => agent = Some(segment.text),
            Some(MentionCategory::Skill) => {
                if seen_skills.insert(segment.text.clone()) {
                    skills.push(segment.text);
                }
            }
            Some(MentionCategory::File) | None => {
                if !segment.text.trim().is_empty() {
                    has_non_definition_text = true;
                }
                cleaned.push_str(&segment.text);
            }
        }
    }

    ExtractedMentions {
        mention_only: !has_non_definition_text && (agent.is_some() || !skills.is_empty()),
        agent,
        skills,
        cleaned: collapse_whitespace(&cleaned),
    }
}

pub fn resolved_mention_chips(
    input: &str,
    host: &HostInfoDto,
) -> Vec<(MentionCategory, String, String)> {
    let agent_ids: HashSet<&str> = host.agents.iter().map(|a| a.id.as_str()).collect();
    let skill_ids: HashSet<&str> = host.skills.iter().map(|s| s.id.as_str()).collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for token in input.split_whitespace() {
        if let Some(name) = token.strip_prefix("@agent/")
            && agent_ids.contains(name)
            && seen.insert(token.to_owned())
        {
            out.push((MentionCategory::Agent, name.to_owned(), token.to_owned()));
        } else if let Some(name) = token.strip_prefix("@skill/")
            && skill_ids.contains(name)
            && seen.insert(token.to_owned())
        {
            out.push((MentionCategory::Skill, name.to_owned(), token.to_owned()));
        }
    }
    out
}

pub fn remove_mention_token(input: &str, token: &str) -> String {
    let kept: Vec<&str> = input.split_whitespace().filter(|t| *t != token).collect();
    kept.join(" ")
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
            if (q.is_empty() || m.to_ascii_lowercase().contains(&q))
                && seen.insert((active_provider.to_owned(), m.clone()))
            {
                out.push(CompletionItem {
                    label: format!("{active_provider}/{m}"),
                    insert: format!("{active_provider}/{m}"),
                    detail: "recent".to_owned(),
                    category: None,
                });
            }
        }
    }

    for model in &providers.models {
        let full = format!("{}/{}", model.provider, model.id);
        if (q.is_empty()
            || model.id.to_ascii_lowercase().contains(&q)
            || full.to_ascii_lowercase().contains(&q))
            && seen.insert((model.provider.clone(), model.id.clone()))
        {
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

#[derive(Clone, Copy, Debug)]
pub enum DigestItem<'a> {
    Reasoning(&'a TranscriptEntryDto),
    ToolCall(&'a TranscriptEntryDto),
    Subagent(&'a TranscriptEntryDto),
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
        items: Vec<DigestItem<'a>>,
    },
    Assistant(&'a TranscriptEntryDto),
    System(&'a TranscriptEntryDto),
    Error(&'a TranscriptEntryDto),
    Question(&'a TranscriptEntryDto),
}

pub fn group_transcript_blocks<'a>(entries: &'a [TranscriptEntryDto]) -> Vec<TranscriptBlock<'a>> {
    let mut blocks: Vec<TranscriptBlock<'a>> = Vec::new();
    let mut turn_num = 0usize;
    let mut folded: Vec<DigestItem<'a>> = Vec::new();

    let flush = |blocks: &mut Vec<TranscriptBlock<'a>>, folded: &mut Vec<DigestItem<'a>>| {
        if folded.is_empty() {
            return;
        }
        let items = std::mem::take(folded);
        let first_id = match items[0] {
            DigestItem::Reasoning(e) | DigestItem::ToolCall(e) | DigestItem::Subagent(e) => {
                entry_id_of(e)
            }
        };
        blocks.push(TranscriptBlock::ActivityDigest {
            digest_id: format!("digest-{first_id}"),
            items,
        });
    };

    for entry in entries {
        match entry {
            TranscriptEntryDto::ToolCall { name, .. } if name == crate::state::QUESTION_TOOL => {
                continue;
            }
            TranscriptEntryDto::Assistant {
                text, reasoning, ..
            } if text.trim().is_empty() && reasoning.trim().is_empty() => continue,
            TranscriptEntryDto::User { id, turn_id, text } => {
                flush(&mut blocks, &mut folded);
                turn_num += 1;
                blocks.push(TranscriptBlock::User {
                    turn_num,
                    id,
                    turn_id: turn_id.as_deref(),
                    text,
                });
            }
            TranscriptEntryDto::Assistant {
                text, reasoning, ..
            } if text.trim().is_empty() => {
                if !reasoning.trim().is_empty() {
                    folded.push(DigestItem::Reasoning(entry));
                }
            }
            TranscriptEntryDto::ToolCall { .. } => {
                folded.push(DigestItem::ToolCall(entry));
            }
            TranscriptEntryDto::Subagent { .. } => {
                folded.push(DigestItem::Subagent(entry));
            }
            TranscriptEntryDto::Assistant { reasoning, .. } => {
                if !reasoning.trim().is_empty() {
                    folded.push(DigestItem::Reasoning(entry));
                }
                flush(&mut blocks, &mut folded);
                blocks.push(TranscriptBlock::Assistant(entry));
            }
            TranscriptEntryDto::System { .. } => {
                flush(&mut blocks, &mut folded);
                blocks.push(TranscriptBlock::System(entry));
            }
            TranscriptEntryDto::Error { .. } => {
                flush(&mut blocks, &mut folded);
                blocks.push(TranscriptBlock::Error(entry));
            }
            TranscriptEntryDto::Question { .. } => {
                flush(&mut blocks, &mut folded);
                blocks.push(TranscriptBlock::Question(entry));
            }
        }
    }
    flush(&mut blocks, &mut folded);
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

pub fn is_removable_credential_badge(badge: &str) -> bool {
    matches!(badge, "saved" | "plan")
}

pub fn provider_for_model<'a>(
    providers: Option<&'a ProvidersResponse>,
    provider_id: &str,
) -> Option<&'a ProviderItemDto> {
    providers.and_then(|resp| resp.providers.iter().find(|p| p.id == provider_id))
}

pub fn is_recent_model(
    providers: Option<&ProvidersResponse>,
    provider_id: &str,
    model_id: &str,
) -> bool {
    providers
        .and_then(|resp| resp.recent_models.get(provider_id))
        .is_some_and(|list| list.iter().any(|m| m == model_id))
}

pub fn filtered_providers(
    providers: Option<&ProvidersResponse>,
    filter: &str,
) -> Vec<ProviderItemDto> {
    filtered_providers_sorted(providers, None, filter)
}

pub fn filtered_providers_sorted(
    providers: Option<&ProvidersResponse>,
    active_provider: Option<&str>,
    filter: &str,
) -> Vec<ProviderItemDto> {
    let Some(resp) = providers else {
        return Vec::new();
    };
    let q = filter.trim().to_ascii_lowercase();
    let mut items: Vec<ProviderItemDto> = resp
        .providers
        .iter()
        .filter(|p| {
            q.is_empty()
                || p.id.to_ascii_lowercase().contains(&q)
                || p.display_name.to_ascii_lowercase().contains(&q)
                || p.adapter.to_ascii_lowercase().contains(&q)
                || p.status_badge.to_ascii_lowercase().contains(&q)
        })
        .cloned()
        .collect();
    items.sort_by_key(|p| {
        let is_active = active_provider.is_some_and(|a| a == p.id);
        let is_default = resp.default_provider.as_deref() == Some(p.id.as_str());
        (!is_active, !p.configured, !is_default)
    });
    items
}

pub fn filtered_models(
    providers: Option<&ProvidersResponse>,
    provider_filter: Option<&str>,
    filter: &str,
) -> Vec<ModelItemDto> {
    filtered_models_sorted(providers, provider_filter, None, filter)
}

pub fn filtered_models_sorted(
    providers: Option<&ProvidersResponse>,
    provider_filter: Option<&str>,
    active_provider: Option<&str>,
    filter: &str,
) -> Vec<ModelItemDto> {
    let Some(resp) = providers else {
        return Vec::new();
    };
    let mut combined = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();

    for m in &resp.models {
        if seen.insert((m.provider.clone(), m.id.clone())) {
            combined.push(m.clone());
        }
    }
    for (provider, recent) in &resp.recent_models {
        for model_id in recent {
            if seen.insert((provider.clone(), model_id.clone())) {
                combined.push(ModelItemDto {
                    id: model_id.clone(),
                    provider: provider.clone(),
                    display_name: model_id.clone(),
                    context_window_tokens: None,
                    max_output_tokens: None,
                    supports_reasoning: true,
                    supports_vision: false,
                    supports_tools: true,
                    is_custom: true,
                });
            }
        }
    }
    for provider in &resp.providers {
        if let Some(model_id) = &provider.default_model
            && !model_id.trim().is_empty()
            && seen.insert((provider.id.clone(), model_id.clone()))
        {
            combined.push(ModelItemDto {
                id: model_id.clone(),
                provider: provider.id.clone(),
                display_name: model_id.clone(),
                context_window_tokens: None,
                max_output_tokens: None,
                supports_reasoning: true,
                supports_vision: false,
                supports_tools: true,
                is_custom: false,
            });
        }
    }

    let q = filter.trim().to_ascii_lowercase();
    let mut items: Vec<ModelItemDto> = combined
        .into_iter()
        .filter(|m| {
            provider_filter.is_none_or(|p| m.provider == p || q.contains('/'))
                && (q.is_empty()
                    || m.id.to_ascii_lowercase().contains(&q)
                    || m.display_name.to_ascii_lowercase().contains(&q)
                    || format!("{}/{}", m.provider, m.id)
                        .to_ascii_lowercase()
                        .contains(&q))
        })
        .collect();

    let configured_map: HashMap<&str, bool> = resp
        .providers
        .iter()
        .map(|p| (p.id.as_str(), p.configured))
        .collect();
    let default_map: HashMap<&str, &str> = resp
        .providers
        .iter()
        .filter_map(|p| p.default_model.as_deref().map(|d| (p.id.as_str(), d)))
        .collect();

    items.sort_by_key(|m| {
        let scope_tier = match provider_filter {
            Some(p) => {
                if m.provider == p {
                    0u8
                } else {
                    1
                }
            }
            None => {
                let is_active = active_provider.is_some_and(|a| a == m.provider);
                let is_configured = configured_map
                    .get(m.provider.as_str())
                    .copied()
                    .unwrap_or(false);
                if is_active {
                    0
                } else if is_configured {
                    1
                } else {
                    2
                }
            }
        };
        let recent_rank = resp
            .recent_models
            .get(&m.provider)
            .and_then(|list| list.iter().position(|r| r == &m.id))
            .unwrap_or(usize::MAX);
        let is_default = default_map
            .get(m.provider.as_str())
            .is_some_and(|d| *d == m.id);
        (scope_tier, recent_rank, !is_default)
    });
    items
}

pub fn resolve_model_input_target(
    state: &AppState,
    provider_scope: Option<&str>,
    raw: &str,
) -> Option<(String, String)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let fallback = provider_scope
        .map(str::to_owned)
        .or_else(|| state.active_session().map(|s| s.provider.clone()))
        .or_else(|| {
            state
                .providers
                .as_ref()
                .and_then(|p| p.default_provider.clone())
        })
        .unwrap_or_else(|| "openai".to_owned());
    Some(parse_provider_model_arg(trimmed, &fallback))
}

pub fn custom_model_candidate(
    state: &AppState,
    provider_scope: Option<&str>,
    filter: &str,
    models: &[ModelItemDto],
) -> Option<(String, String)> {
    let (provider, model) = resolve_model_input_target(state, provider_scope, filter)?;
    let exact = models
        .iter()
        .any(|m| m.provider == provider && m.id == model);
    (!exact).then_some((provider, model))
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

pub fn summarize_tool_args(name: &str, raw_args: &str) -> String {
    let trimmed = raw_args.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(trimmed) {
        let pick_str = |keys: &[&str]| -> Option<String> {
            keys.iter().find_map(|k| {
                map.get(*k)
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            })
        };
        let summary = match name {
            "code-read_file" | "code-write_file" | "code-edit_file" | "code-list_dir" => {
                pick_str(&["path", "Path", "file_path", "dir"])
            }
            "code-shell" => pick_str(&["command", "CommandLine", "cmd"]),
            "code-grep" | "code-glob" => pick_str(&["pattern", "query", "glob", "path"]),
            "subagent-run" | "acp-run" => pick_str(&["agent", "preset", "role", "task", "prompt"]),
            _ => pick_str(&["path", "Path", "command", "query", "pattern", "name", "url"]),
        };
        if let Some(text) = summary {
            return text.lines().next().unwrap_or(&text).to_owned();
        }
    }
    trimmed.lines().next().unwrap_or(trimmed).to_owned()
}

pub fn format_tokens(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}k", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

pub fn format_duration_ms(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else {
        let total_secs = ms / 1_000;
        let mins = total_secs / 60;
        let secs = total_secs % 60;
        format!("{mins}m {secs:02}s")
    }
}

fn push_local_system(tab: &mut SessionTabState, text: String) {
    let id = format!("local-sys-{}", tab.entries.len());
    tab.entries.push(TranscriptEntryDto::System { id, text });
    tab.entries_version = tab.entries_version.wrapping_add(1);
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
    fn digest_groups_contiguous_activity_before_visible_messages() {
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
                TranscriptBlock::Question(_) => "question",
            })
            .collect();
        assert_eq!(kinds, ["user", "digest", "assistant", "user", "digest"]);
        match &blocks[1] {
            TranscriptBlock::ActivityDigest { items, .. } => {
                assert_eq!(items.len(), 4);
                assert!(matches!(items[0], DigestItem::ToolCall(_)));
                assert!(matches!(items[1], DigestItem::ToolCall(_)));
                assert!(matches!(items[2], DigestItem::Reasoning(_)));
                assert!(matches!(items[3], DigestItem::Reasoning(_)));
            }
            other => panic!("expected digest, got {other:?}"),
        }
    }

    #[test]
    fn digest_splits_runs_between_assistant_messages_and_folds_subagents() {
        let entries = vec![
            user("u1", Some("t1"), "fix it"),
            assistant("a0", "I do ...", "t0"),
            tool("c1", "code-grep", "succeeded"),
            tool("c2", "code-grep", "succeeded"),
            assistant("a1", "", "t1"),
            assistant("a2", "", "t2"),
            assistant("a3", "", "t3"),
            assistant("a4", "", "t4"),
            assistant("a5", "....", "t5"),
            TranscriptEntryDto::Subagent {
                id: "sub1".to_owned(),
                agent: "explore".to_owned(),
                status: "succeeded".to_owned(),
                report: None,
                child_session_id: None,
            },
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
                TranscriptBlock::Question(_) => "question",
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "user",
                "digest",
                "assistant",
                "digest",
                "assistant",
                "digest"
            ]
        );
        match (&blocks[1], &blocks[3], &blocks[5]) {
            (
                TranscriptBlock::ActivityDigest { items: first, .. },
                TranscriptBlock::ActivityDigest { items: second, .. },
                TranscriptBlock::ActivityDigest { items: third, .. },
            ) => {
                assert_eq!(first.len(), 1);
                assert!(matches!(first[0], DigestItem::Reasoning(_)));
                assert_eq!(second.len(), 7);
                assert_eq!(third.len(), 1);
                assert!(matches!(third[0], DigestItem::Subagent(_)));
            }
            _ => panic!("unexpected blocks"),
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

    #[test]
    fn tool_summary_and_formatting_helpers_produce_concise_labels() {
        assert_eq!(
            summarize_tool_args("code-read_file", r#"{"path":"src/main.rs","limit":10}"#),
            "src/main.rs"
        );
        assert_eq!(
            summarize_tool_args(
                "code-shell",
                r#"{"command":"cargo test --workspace\necho ok"}"#
            ),
            "cargo test --workspace"
        );
        assert_eq!(
            summarize_tool_args("code-grep", r#"{"pattern":"AppState"}"#),
            "AppState"
        );
        assert_eq!(format_tokens(950), "950");
        assert_eq!(format_tokens(12_450), "12.4k");
        assert_eq!(format_tokens(2_500_000), "2.5M");
        assert_eq!(format_duration_ms(450), "450ms");
        assert_eq!(format_duration_ms(4_250), "4.2s");
        assert_eq!(format_duration_ms(125_000), "2m 05s");
    }
}
