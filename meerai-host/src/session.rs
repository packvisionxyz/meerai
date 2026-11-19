use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use meerai_agents::{AgentCommand, AgentEvent, TurnId};
use meerai_common::hooks::HookEntry;
use meerai_common::settings::{ContextSettings, SandboxSettings, SessionSettings, ThemeSettings};
use meerai_policy::{ApprovalMode, PermissionRule};
use meerai_session::{LoggedEvent, SessionId, SessionSummary};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::registry::SessionRegistry;
use crate::theme::{HostPalette, ResolvedThemeHex};
use crate::workspace::WorkspaceQuery;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "data")]
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
    pub events: Vec<LoggedEvent>,
    pub cmd_tx: mpsc::UnboundedSender<AgentCommand>,
    pub event_rx: mpsc::UnboundedReceiver<AgentEvent>,
}

#[derive(Debug)]
pub struct ChatGptLoginOutcome {
    pub provider_id: String,
    pub account: Option<String>,
}

pub struct ChatGptLogin {
    pub url: String,
    pub paste_tx: mpsc::UnboundedSender<String>,
    pub cancel_tx: tokio::sync::oneshot::Sender<()>,
    pub result_rx: tokio::sync::oneshot::Receiver<Result<ChatGptLoginOutcome>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpPresetInfo {
    pub id: String,
    pub label: String,
    pub command: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAgentInfo {
    pub id: String,
    pub source: String,
    pub description: Option<String>,
    pub model: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSkillInfo {
    pub id: String,
    pub source: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostMcpServerInfo {
    pub name: String,
    pub kind: String,
    pub enabled: bool,
    pub tools: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostAuthKind {
    ApiKey,
    OAuth,
    Either,
    None,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostProviderItem {
    pub id: String,
    pub display_name: String,
    pub adapter: String,
    pub auth_kind: HostAuthKind,
    pub configured: bool,
    pub status_badge: String,
    pub env_vars: Vec<String>,
    pub default_model: Option<String>,
    pub base_url: Option<String>,
    pub is_custom: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostModelItem {
    pub id: String,
    pub provider: String,
    pub display_name: String,
    pub context_window_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub supports_reasoning: bool,
    pub supports_vision: bool,
    pub supports_tools: bool,
    pub is_custom: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostProvidersSnapshot {
    pub default_provider: Option<String>,
    pub providers: Vec<HostProviderItem>,
    pub models: Vec<HostModelItem>,
    pub recent_models: HashMap<String, Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSettingsView {
    pub theme: ResolvedThemeHex,
    pub raw_theme: ThemeSettings,
    pub context: ContextSettings,
    pub session: SessionSettings,
    pub sandbox: SandboxSettings,
    pub permissions_summary: Vec<String>,
    pub config_writable: bool,
}

impl Default for HostSettingsView {
    fn default() -> Self {
        Self {
            theme: HostPalette::default().to_hex_tokens(),
            raw_theme: ThemeSettings::default(),
            context: ContextSettings::default(),
            session: SessionSettings::default(),
            sandbox: SandboxSettings::default(),
            permissions_summary: Vec::new(),
            config_writable: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInfoSnapshot {
    pub workspace_root: String,
    pub git_branch: Option<String>,
    pub acp_presets: Vec<AcpPresetInfo>,
    pub agents: Vec<HostAgentInfo>,
    pub skills: Vec<HostSkillInfo>,
    pub mcp_servers: Vec<HostMcpServerInfo>,
    pub enabled_tools: Vec<String>,
    pub hooks_count: usize,
    pub update_available: Option<String>,
    pub install_hint: String,
    pub settings: HostSettingsView,
}

impl Default for HostInfoSnapshot {
    fn default() -> Self {
        Self {
            workspace_root: String::new(),
            git_branch: None,
            acp_presets: Vec::new(),
            agents: Vec::new(),
            skills: Vec::new(),
            mcp_servers: Vec::new(),
            enabled_tools: Vec::new(),
            hooks_count: 0,
            update_available: None,
            install_hint: crate::update::install_hint().to_string(),
            settings: HostSettingsView::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct HostReloadPayload {
    pub rules: Option<Vec<PermissionRule>>,
    pub context: Option<ContextSettings>,
    pub system_prompt: String,
    pub hooks: Vec<HookEntry>,
}

#[derive(Clone, Debug)]
pub struct HostReloadOutcome {
    pub host_info: HostInfoSnapshot,
    pub warnings: Vec<String>,
    pub payload: HostReloadPayload,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSettingsUpdate {
    pub reset_theme: bool,
    pub theme: Option<ThemeSettings>,
    pub context: Option<ContextSettings>,
    pub session: Option<SessionSettings>,
    pub sandbox: Option<SandboxSettings>,
    pub forget_model: Option<ForgetModel>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgetModel {
    pub provider: String,
    pub model: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedModelSelection {
    pub provider: String,
    pub display_model: String,
    pub command_model: String,
    pub context_window_tokens: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedAgentApplication {
    pub name: String,
    pub instructions: String,
    pub model: Option<String>,
    pub context_window_tokens: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedSkillApplication {
    pub name: String,
    pub instructions: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebServerStart {
    pub port: u16,
    pub url: String,
    pub secret: String,
}

#[async_trait]
pub trait WebServerHandle: Send + Sync {
    fn port(&self) -> u16;
    fn url(&self) -> &str;
    async fn stop(&self);
}

#[async_trait]
pub trait WebServerLauncher: Send + Sync {
    async fn start(
        &self,
        registry: Arc<SessionRegistry>,
        opener: Arc<dyn SessionOpener>,
        port: Option<u16>,
        secret: Option<String>,
    ) -> Result<(Arc<dyn WebServerHandle>, WebServerStart)>;
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

    async fn ensure_credentials(&self) -> Result<()> {
        Ok(())
    }

    async fn start_chatgpt_login(&self, _provider_id: &str) -> Result<ChatGptLogin> {
        anyhow::bail!("ChatGPT plan sign-in is not supported by this client")
    }

    fn workspace_root(&self) -> Option<PathBuf> {
        None
    }

    fn workspace_query(&self) -> Option<Arc<dyn WorkspaceQuery>> {
        self.workspace_root().map(crate::workspace::local_workspace)
    }

    async fn host_info(&self) -> HostInfoSnapshot {
        HostInfoSnapshot::default()
    }

    async fn providers_snapshot(&self) -> Result<HostProvidersSnapshot> {
        Ok(HostProvidersSnapshot::default())
    }

    async fn set_api_key(&self, _provider_id: &str, _api_key: &str) -> Result<()> {
        anyhow::bail!("updating provider credentials is not supported by this client")
    }

    async fn remove_credential(&self, _provider_id: &str) -> Result<()> {
        anyhow::bail!("removing provider credentials is not supported by this client")
    }

    async fn set_provider_base_url(
        &self,
        _provider_id: &str,
        _base_url: Option<String>,
    ) -> Result<()> {
        anyhow::bail!("updating provider settings is not supported by this client")
    }

    async fn select_model(
        &self,
        _provider_id: &str,
        _model_id: &str,
    ) -> Result<ResolvedModelSelection> {
        anyhow::bail!("model switching is not supported by this client")
    }

    async fn resolve_agent_application(
        &self,
        _name: &str,
        _current_provider: &str,
    ) -> Result<ResolvedAgentApplication> {
        anyhow::bail!("agent definitions are not supported by this client")
    }

    async fn resolve_skill_application(&self, _name: &str) -> Result<ResolvedSkillApplication> {
        anyhow::bail!("skill definitions are not supported by this client")
    }

    async fn update_settings(&self, _update: HostSettingsUpdate) -> Result<HostReloadOutcome> {
        anyhow::bail!("updating settings is not supported by this client")
    }

    async fn reload_config(&self) -> Result<HostReloadOutcome> {
        anyhow::bail!("reloading configuration is not supported by this client")
    }
}
