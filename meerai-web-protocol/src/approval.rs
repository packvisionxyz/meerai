use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type",
    content = "data"
)]
pub enum ApprovalDecisionDto {
    Accept,
    AcceptForSession,
    Decline,
    DeclineWithReason { reason: String },
    Cancel,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GrantScopeDto {
    Turn,
    Session,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalKindDto {
    ToolCall,
    Spend,
    SandboxEscape,
    ExternalAgent,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EffectClassDto {
    Read,
    Write,
    Execute,
    External,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionOptionDto {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionPromptDto {
    pub header: String,
    pub question: String,
    pub options: Vec<QuestionOptionDto>,
    #[serde(default)]
    pub multiple: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionAnswerDto {
    pub header: String,
    pub selected: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequestDto {
    pub request_id: String,
    pub turn_id: String,
    pub kind: ApprovalKindDto,
    pub title: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<EffectClassDto>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionRequestDto {
    pub request_id: String,
    pub turn_id: String,
    pub questions: Vec<QuestionPromptDto>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalDecisionRequest {
    pub decision: ApprovalDecisionDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<GrantScopeDto>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionAnswerRequest {
    #[serde(default)]
    pub answers: Vec<QuestionAnswerDto>,
    #[serde(default)]
    pub cancel: bool,
}
