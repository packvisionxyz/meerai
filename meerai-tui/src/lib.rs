use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::Result;
use meerai_agents::{AgentCommand, AgentEvent, TurnId};
use meerai_client::providers::RuntimeProviderState;
use meerai_client::secrets::SecretStore;
use meerai_common::definitions::{AgentDefinition, SkillDefinition};
use meerai_common::settings::AppSettings;
use meerai_core::async_trait;
use meerai_policy::ApprovalMode;
use meerai_session::{LoggedEvent, SessionId, SessionSummary};
use tokio::sync::mpsc;

pub mod app;
pub mod completion;
pub mod diff;
pub mod event_loop;
pub mod logo;
pub mod markdown;
pub mod search;
pub mod syntax;
pub mod tabs;
pub mod terminal;
pub mod theme;
pub mod ui;
pub mod update;
pub mod workspace;

pub use app::{
    ActiveModal, AppState, FileChangeEntry, HoverZone, InputMode, MIN_SIDEBAR_WIDTH,
    PendingSessionAction, ScreenPos, SessionTab, TabHitKind, TabHitZone, TextSelection,
    ThinkingHitZone, ToolCallEntry, ToolCallStatus, TranscriptEntry, TurnMetrics, UsageStats,
    approval_mode_badge, approval_mode_label, parse_approval_mode, session_usage_from_events,
    transcript_from_events,
};
pub use completion::{CompletionEngine, CompletionItem, CompletionItemKind, CompletionKind};
pub use diff::{DiffCell, DiffRow, DiffRowKind, FileDiffView};
pub use search::{SearchMatch, SearchMode, SearchPreview};
pub use terminal::TerminalGuard;
pub use workspace::{LocalWorkspace, WorkspaceMatch, WorkspaceQuery, local_workspace};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Backend {
    #[default]
    Native,
    Acp {
        preset: String,
        remote_session_id: Option<String>,
    },
}

pub struct OpenedSession {
    pub session_id: SessionId,
    pub title: Option<String>,
    pub backend: Backend,
    pub provider: String,
    pub model: String,
    pub approval_mode: ApprovalMode,
    pub transcript: Vec<TranscriptEntry>,
    pub events: Vec<LoggedEvent>,
    pub cmd_tx: mpsc::UnboundedSender<AgentCommand>,
    pub event_rx: mpsc::UnboundedReceiver<AgentEvent>,
}

#[async_trait]
pub trait SessionOpener: Send + Sync {
    async fn list_sessions(&self) -> Result<Vec<SessionSummary>>;
    async fn open_session(&self, id: &SessionId) -> Result<OpenedSession>;
    async fn create_session(&self) -> Result<OpenedSession>;
    async fn fork_session(&self, id: &SessionId, turn: &TurnId) -> Result<OpenedSession>;
    async fn delete_session(&self, id: &SessionId) -> Result<bool>;
    async fn create_acp_session(&self, _preset: &str) -> Result<OpenedSession> {
        anyhow::bail!("ACP sessions are not supported by this client")
    }
    async fn read_session_events(&self, _id: &SessionId) -> Result<Vec<LoggedEvent>> {
        Err(anyhow::anyhow!("reading session events is not supported"))
    }
    async fn run_user_shell(&self, _command: &str) -> Result<String> {
        Err(anyhow::anyhow!("user shell commands are not supported"))
    }
}

pub struct TuiLaunchConfig {
    pub initial_session_id: SessionId,
    pub initial_session_title: Option<String>,
    pub initial_backend: Backend,
    pub initial_provider: String,
    pub initial_model: String,
    pub workspace_root: PathBuf,
    pub runtime: Arc<RwLock<RuntimeProviderState>>,
    pub secrets: Arc<dyn SecretStore>,
    pub settings: AppSettings,
    pub context_temperature: Option<f32>,
    pub explicit_context_window: Option<u64>,
    pub config_dir: Option<PathBuf>,
    pub approval_mode: ApprovalMode,
    pub initial_transcript: Vec<TranscriptEntry>,
    pub initial_events: Vec<LoggedEvent>,
    pub agents: Vec<AgentDefinition>,
    pub skills: Vec<SkillDefinition>,
    pub session_opener: Option<Arc<dyn SessionOpener>>,
    pub update_available: Option<String>,
    pub restore_sessions: Vec<SessionId>,
    pub launch_system: String,
    pub launch_definitions: String,
    pub sandbox_active: bool,
    pub sandbox_kind: String,
    pub sandbox_warning: Option<String>,
    pub workspace_query: Option<Arc<dyn WorkspaceQuery>>,
}

pub struct TuiExit {
    pub session_id: SessionId,
    pub title: Option<String>,
}

pub async fn run_tui(
    cmd_tx: mpsc::UnboundedSender<AgentCommand>,
    event_rx: mpsc::UnboundedReceiver<AgentEvent>,
    config: TuiLaunchConfig,
) -> Result<TuiExit> {
    let mut terminal = TerminalGuard::enter()?;
    let workspace_root = config.workspace_root.clone();
    let mut app = AppState::new(
        config.initial_provider,
        config.initial_model,
        config.workspace_root,
        cmd_tx,
        config.runtime,
        config.secrets,
        config.settings,
        config.config_dir,
    );
    app.context_temperature = config.context_temperature;
    app.explicit_context_window = config.explicit_context_window;
    app.tab_persistence = true;
    app.git_branch = crate::diff::current_branch(&workspace_root);
    if let Some(workspace) = config.workspace_query {
        app.set_workspace_query(workspace);
    }
    app.set_initial_session(
        config.initial_session_id,
        config.initial_session_title,
        config.initial_backend,
        config.initial_transcript,
        &config.initial_events,
        config.approval_mode,
    );
    app.agents = config.agents;
    app.skills = config.skills;
    app.session_opener = config.session_opener;
    app.update_available = config.update_available;
    app.launch_system = config.launch_system;
    app.launch_definitions = config.launch_definitions;
    app.sandbox_active = config.sandbox_active;
    app.sandbox_kind = config.sandbox_kind;
    app.sandbox_warning = config.sandbox_warning;
    for id in config.restore_sessions {
        app.pending_session_actions
            .push(PendingSessionAction::OpenSessionId(id));
    }
    let result = event_loop::run_event_loop(&mut terminal, &mut app, event_rx).await;
    let exit = TuiExit {
        session_id: app.tab().session_id.clone(),
        title: app.tab().title.clone(),
    };
    app.quit();
    result.map(|()| exit)
}
