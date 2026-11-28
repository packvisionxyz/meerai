use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::Result;
use meerai_agents::{AgentCommand, AgentEvent};
use meerai_client::providers::RuntimeProviderState;
use meerai_client::secrets::SecretStore;
use meerai_common::definitions::{AgentDefinition, SkillDefinition};
use meerai_common::settings::AppSettings;
use meerai_policy::ApprovalMode;
use meerai_session::{LoggedEvent, SessionId};
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
    ActiveModal, AppDeps, AppState, FileChangeEntry, HoverZone, InputMode, MIN_SIDEBAR_WIDTH,
    PendingSessionAction, ScreenPos, SessionTab, TabHitKind, TabHitZone, TextSelection,
    ThinkingHitZone, ToolCallEntry, ToolCallStatus, TranscriptEntry, TurnMetrics, UsageStats,
    approval_mode_badge, approval_mode_label, parse_approval_mode, session_usage_from_events,
    transcript_from_events,
};
pub use completion::{CompletionEngine, CompletionItem, CompletionItemKind, CompletionKind};
pub use diff::{DiffCell, DiffRow, DiffRowKind, FileDiffView};
pub use meerai_host::{
    Backend, ChatGptLogin, ChatGptLoginOutcome, LocalWorkspace, OpenedSession, SessionOpener,
    SessionRegistry, WebServerHandle, WebServerLauncher, WebServerStart, WorkspaceMatch,
    WorkspaceQuery, local_workspace,
};
pub use search::{SearchMatch, SearchMode, SearchPreview};
pub use terminal::TerminalGuard;

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
    pub context_effort: Option<meerai_common::settings::EffortLevel>,
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
    pub registry: Option<Arc<SessionRegistry>>,
    pub web_launcher: Option<Arc<dyn WebServerLauncher>>,
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
    let mut app = AppState::new(AppDeps {
        active_provider: config.initial_provider.clone(),
        model: config.initial_model.clone(),
        workspace_root: config.workspace_root,
        cmd_tx: cmd_tx.clone(),
        runtime: config.runtime,
        secrets: config.secrets,
        settings: config.settings,
        config_dir: config.config_dir,
    });
    app.context_temperature = config.context_temperature;
    app.context_effort = config.context_effort;
    app.explicit_context_window = config.explicit_context_window;
    app.tab_persistence = true;
    app.git_branch = crate::diff::current_branch(&workspace_root);
    if let Some(workspace) = config.workspace_query {
        app.set_workspace_query(workspace);
    }
    app.set_initial_session(
        config.initial_session_id.clone(),
        config.initial_session_title.clone(),
        config.initial_backend.clone(),
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
    app.web_launcher = config.web_launcher;
    if let Some(opener) = app.session_opener.clone() {
        let info = opener.host_info().await;
        app.apply_host_info(&info);
    }
    let (active_rx, _keep_tx) = if let Some(reg) = config.registry.clone() {
        reg.register_opened_active(OpenedSession {
            session_id: config.initial_session_id,
            title: config.initial_session_title,
            backend: config.initial_backend,
            provider: config.initial_provider,
            model: config.initial_model,
            approval_mode: config.approval_mode,
            transient: false,
            events: config.initial_events,
            cmd_tx,
            event_rx,
        });
        app.registry = Some(reg);
        let (tx, rx) = mpsc::unbounded_channel();
        (rx, Some(tx))
    } else {
        (event_rx, None)
    };
    for id in config.restore_sessions {
        app.pending_session_actions
            .push(PendingSessionAction::RestoreSessionId(id));
    }
    let result = event_loop::run_event_loop(&mut terminal, &mut app, active_rx).await;
    if let Some(web) = app.web_server.take() {
        web.handle.stop().await;
    }
    if let Some(reg) = app.registry.take() {
        reg.shutdown_all().await;
    }
    let exit = TuiExit {
        session_id: app.tab().session_id.clone(),
        title: app.tab().title.clone(),
    };
    app.quit();
    result.map(|()| exit)
}
