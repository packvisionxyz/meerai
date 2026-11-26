use super::completion::*;
use super::digest::*;
use super::forms::*;
use super::runs::*;
use super::views::*;
use super::*;

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

#[derive(Clone, Debug)]
pub enum ActiveModal {
    WorkspacePanel {
        tab: WorkspacePanelTab,
        context: Option<Box<ContextBreakdownDto>>,
        loop_state: Option<Box<LoopDto>>,
        memories: Vec<MemoryDto>,
        routes: Vec<RouteDto>,
        packs: Vec<PackDto>,
        definitions: Box<DefinitionsDiagnosticsDto>,
        selected: usize,
        loading: bool,
        error: Option<String>,
        subagent_name: Box<TextBuffer>,
        subagent_agent: String,
        subagent_task: Box<TextBuffer>,
        subagent_model: Box<TextBuffer>,
        subagent_mode: Option<ApprovalModeDto>,
        subagent_field: usize,
        subagent_steps: Box<TextBuffer>,
        subagent_tokens: Box<TextBuffer>,
        subagent_cost: Box<TextBuffer>,
        subagent_tools: Box<TextBuffer>,
        route_input: Box<TextBuffer>,
        pack_input: Box<TextBuffer>,
        pack_pin_input: Box<TextBuffer>,
        context_turn: usize,
        context_focus: Box<TextBuffer>,
        context_blob: Option<(String, String)>,
    },
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
        run_id: Option<String>,
    },
    Runs {
        kind: RunKindDto,
        selected: usize,
    },
    ShellRun {
        run_id: String,
        command: String,
        started_at_ms: u64,
        from_runs: Option<RunKindDto>,
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
    VaultReset {
        input: TextBuffer,
        error: Option<String>,
    },
    Help,
    SubagentOverlay {
        entry_id: String,
        name: Option<String>,
        agent: String,
        status: String,
        child_session_id: Option<String>,
        entries: Vec<TranscriptEntryDto>,
        usage: Option<UsageDto>,
        loading: bool,
        error: Option<String>,
        from_runs: Option<RunKindDto>,
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
    pub context: Option<ContextBreakdownDto>,
    pub loop_state: Option<LoopDto>,
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
    pub sidebar_diagnose_open: bool,
    pub sidebar_changes_open: bool,
    pub modal: Option<ActiveModal>,
    pub sending: bool,
    pub reset_confirm_pending: bool,
    pub error_banner: Option<String>,
    pub status_notice: Option<String>,
    pub notice_seq: u64,
    pub last_title_sent: String,
    pub shell_outputs: HashMap<String, Result<String, String>>,
    pub next_shell_run_id: u64,
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

    pub fn sidebar_sessions(&self) -> Vec<SessionSummaryDto> {
        self.sessions
            .iter()
            .filter(|session| !session.transient)
            .cloned()
            .collect()
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
                .is_some_and(|s| s.status != SessionStatus::Idle)
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

impl AppState {
    pub fn can_submit_composer(&self) -> bool {
        let draft = self.draft.text.trim();
        self.connection != ConnectionState::Disconnected
            && !draft.is_empty()
            && (self.active_session_id.is_some() || draft.starts_with('/'))
    }
}
