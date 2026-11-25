use crate::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionStatus {
    Idle,
    Running,
    WaitingApproval,
    WaitingQuestion,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalModeDto {
    #[default]
    Ask,
    Auto,
    Plan,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RewindScopeDto {
    Both,
    Code,
    Conversation,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummaryDto {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub backend: BackendKind,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub approval_mode: ApprovalModeDto,
    pub status: SessionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_started_at_ms: Option<u64>,
    #[serde(default)]
    pub user_shell_running: bool,
    #[serde(default)]
    pub transient: bool,
    #[serde(default)]
    pub shell_runs: Vec<ShellRunDto>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetailDto {
    pub summary: SessionSummaryDto,
    pub entries: Vec<TranscriptEntryDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub streaming_item_id: Option<String>,
    pub streaming_text: String,
    pub streaming_reasoning: String,
    #[serde(default)]
    pub pending_approvals: Vec<ApprovalRequestDto>,
    #[serde(default)]
    pub pending_questions: Vec<QuestionRequestDto>,
    #[serde(default)]
    pub usage: UsageDto,
    #[serde(default)]
    pub metrics: TurnMetricsDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextBreakdownDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_state: Option<LoopDto>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<BackendKind>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionResponse {
    pub session: SessionSummaryDto,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkSessionRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BtwSessionRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredSessionDto {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub bytes: u64,
    pub is_open: bool,
    pub is_child: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionHistoryResponse {
    pub sessions: Vec<StoredSessionDto>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEventsResponse {
    pub session_id: String,
    pub entries: Vec<TranscriptEntryDto>,
    pub usage: UsageDto,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewindRequest {
    pub turn_id: String,
    pub scope: RewindScopeDto,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewindResponse {
    pub accepted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored_prompt: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetModelRequest {
    pub provider: String,
    pub model: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetModelResponse {
    pub session: SessionSummaryDto,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetModeRequest {
    pub mode: ApprovalModeDto,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyDefinitionRequest {
    pub name: String,
}
