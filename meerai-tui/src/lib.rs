use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::Result;
use meerai_agents::{AgentCommand, AgentEvent};
use meerai_common::definitions::{AgentDefinition, SkillDefinition};
use meerai_common::providers::RuntimeProviderState;
use meerai_common::secrets::SecretStore;
use meerai_common::settings::AppSettings;
use meerai_session::LoggedEvent;
use tokio::sync::mpsc;

pub mod app;
pub mod completion;
pub mod diff;
pub mod event_loop;
pub mod search;
pub mod terminal;
pub mod ui;

pub use app::{
    ActiveModal, AppState, FileChangeEntry, InputMode, MIN_SIDEBAR_WIDTH, ScreenPos, TextSelection,
    ThinkingHitZone, ToolCallEntry, ToolCallStatus, TranscriptEntry, TurnMetrics, UsageStats,
    approval_mode_badge, approval_mode_label, parse_approval_mode, session_usage_from_events,
    transcript_from_events,
};
pub use completion::{CompletionEngine, CompletionItem, CompletionItemKind, CompletionKind};
pub use diff::{DiffCell, DiffRow, DiffRowKind, FileDiffView};
pub use search::{SearchMatch, SearchMode, SearchPreview};
pub use terminal::TerminalGuard;

pub struct TuiLaunchConfig {
    pub initial_provider: String,
    pub initial_model: String,
    pub workspace_root: PathBuf,
    pub runtime: Arc<RwLock<RuntimeProviderState>>,
    pub secrets: Arc<dyn SecretStore>,
    pub settings: AppSettings,
    pub config_dir: Option<PathBuf>,
    pub approval_mode: meerai_policy::ApprovalMode,
    pub initial_transcript: Vec<TranscriptEntry>,
    pub initial_events: Vec<LoggedEvent>,
    pub agents: Vec<AgentDefinition>,
    pub skills: Vec<SkillDefinition>,
}

pub async fn run_tui(
    cmd_tx: mpsc::UnboundedSender<AgentCommand>,
    event_rx: mpsc::UnboundedReceiver<AgentEvent>,
    config: TuiLaunchConfig,
) -> Result<()> {
    let mut terminal = TerminalGuard::enter()?;
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
    app.approval_mode = config.approval_mode;
    app.transcript = config.initial_transcript;
    app.agents = config.agents;
    app.skills = config.skills;
    if let Some(usage) = session_usage_from_events(&config.initial_events) {
        let prompt = usage.prompt_tokens.unwrap_or(0).max(0) as u64;
        let completion = usage.completion_tokens.unwrap_or(0).max(0) as u64;
        app.metrics.session_prompt_tokens = prompt;
        app.metrics.session_completion_tokens = completion;
        app.metrics.session_total_tokens = usage
            .total_tokens
            .map(|total| total.max(0) as u64)
            .unwrap_or(prompt + completion);
    }
    let result = event_loop::run_event_loop(&mut terminal, &mut app, event_rx).await;
    app.quit();
    result
}
