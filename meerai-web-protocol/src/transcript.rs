use crate::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UnifiedLineKindDto {
    FileHeader,
    HunkHeader,
    Context,
    Added,
    Removed,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedLineDto {
    pub kind: UnifiedLineKindDto,
    pub text: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type",
    content = "data"
)]
pub enum DiffRowKindDto {
    HunkHeader { text: String },
    Context,
    Modified,
    Added,
    Removed,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffCellDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_no: Option<usize>,
    pub text: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffRowDto {
    pub kind: DiffRowKindDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<DiffCellDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<DiffCellDto>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiffResponse {
    pub path: String,
    pub status: String,
    pub additions: usize,
    pub deletions: usize,
    pub rows: Vec<DiffRowDto>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeDto {
    pub status: String,
    pub path: String,
    #[serde(default)]
    pub staged: bool,
    #[serde(default)]
    pub unstaged: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub est_cost_usd: Option<f64>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnMetricsDto {
    #[serde(default)]
    pub turn_count: usize,
    #[serde(default)]
    pub tool_calls: usize,
    #[serde(default)]
    pub session_prompt_tokens: u64,
    #[serde(default)]
    pub session_completion_tokens: u64,
    #[serde(default)]
    pub session_total_tokens: u64,
    #[serde(default)]
    pub session_cached_tokens: u64,
    #[serde(default)]
    pub session_cache_creation_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_turn_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_per_sec: Option<f64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type",
    content = "data"
)]
pub enum TranscriptEntryDto {
    User {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        text: String,
    },
    Assistant {
        id: String,
        text: String,
        reasoning: String,
    },
    System {
        id: String,
        text: String,
    },
    Error {
        id: String,
        message: String,
    },
    ToolCall {
        id: String,
        name: String,
        #[serde(default)]
        args: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edit_diff: Option<Vec<UnifiedLineDto>>,
    },
    Subagent {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        agent: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        report: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        child_session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default)]
        approval_mode: String,
        #[serde(default)]
        steps_used: usize,
        #[serde(default)]
        max_steps: usize,
        #[serde(default)]
        usage: UsageDto,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        est_cost_usd: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        active_tool: Option<String>,
    },
    Question {
        id: String,
        questions: Vec<QuestionPromptDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answers: Option<Vec<QuestionAnswerDto>>,
        resolved: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellRunDto {
    pub id: String,
    pub command: String,
    pub started_at_ms: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextBreakdownDto {
    #[serde(default)]
    pub context_window_tokens: u64,
    #[serde(default)]
    pub high_watermark_tokens: u64,
    #[serde(default)]
    pub estimated_total_tokens: u64,
    #[serde(default)]
    pub system_prompt_tokens: u64,
    #[serde(default)]
    pub tool_schemas_tokens: u64,
    #[serde(default)]
    pub memory_tokens: u64,
    #[serde(default)]
    pub user_messages_tokens: u64,
    #[serde(default)]
    pub assistant_tokens: u64,
    #[serde(default)]
    pub tool_results_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
    #[serde(default)]
    pub compaction_summary_tokens: u64,
    #[serde(default)]
    pub spilled_blob_count: usize,
    #[serde(default)]
    pub cleared_tool_results_count: usize,
    #[serde(default)]
    pub pinned_turn_ids: Vec<String>,
    #[serde(default)]
    pub turn_ids: Vec<String>,
    #[serde(default)]
    pub turns_in_verbatim_window: usize,
    #[serde(default)]
    pub verbatim_window_turns: usize,
    #[serde(default)]
    pub reasoning_pruning: bool,
    #[serde(default)]
    pub clear_old_tool_results: bool,
    #[serde(default)]
    pub spilled_blobs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_prompt_cached_tokens: Option<u64>,
}
