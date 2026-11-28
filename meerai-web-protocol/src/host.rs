use crate::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type",
    content = "data"
)]
pub enum BackendKind {
    Native,
    Acp {
        preset: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote_session_id: Option<String>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPresetDto {
    pub id: String,
    pub label: String,
    pub command: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfoDto {
    pub id: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfoDto {
    pub id: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInfoDto {
    pub name: String,
    pub kind: String,
    pub enabled: bool,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfoDto {
    pub workspace_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub acp_presets: Vec<AcpPresetDto>,
    #[serde(default)]
    pub agents: Vec<AgentInfoDto>,
    #[serde(default)]
    pub skills: Vec<SkillInfoDto>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerInfoDto>,
    #[serde(default)]
    pub enabled_tools: Vec<String>,
    #[serde(default)]
    pub hooks_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_available: Option<String>,
    #[serde(default)]
    pub install_hint: String,
    #[serde(default)]
    pub settings: SettingsSnapshotDto,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotResponse {
    pub event_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_session_id: Option<String>,
    pub sessions: Vec<SessionSummaryDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_session: Option<SessionDetailDto>,
    #[serde(default)]
    pub file_changes: Vec<FileChangeDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_info: Option<HostInfoDto>,
}
