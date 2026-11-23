use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApiErrorCode {
    Unauthorized,
    Forbidden,
    BadRequest,
    NotFound,
    ConflictBusy,
    ConflictStale,
    UnsupportedBackend,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub code: ApiErrorCode,
    pub message: String,
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RewindScopeDto {
    Both,
    Code,
    Conversation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthKindDto {
    ApiKey,
    OAuth,
    Either,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SearchModeDto {
    Files,
    Content,
}

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
    pub goal: Option<GoalDto>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(default)]
    pub done_when: Vec<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub items: Vec<GoalItemDto>,
    #[serde(default)]
    pub rounds: usize,
    #[serde(default)]
    pub turns: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalItemDto {
    pub id: u32,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<String>,
    pub status: GoalItemStatusDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalItemStatusDto {
    #[default]
    Open,
    Done,
    Failed,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeTokensDto {
    pub background: String,
    pub surface: String,
    pub surface_high: String,
    pub text: String,
    pub muted: String,
    pub faint: String,
    pub accent: String,
    pub interactive: String,
    pub border: String,
    pub error: String,
    pub warning: String,
    pub success: String,
    pub info: String,
    pub diff_added: String,
    pub diff_removed: String,
    pub diff_context: String,
    pub code_function: String,
}

impl Default for ThemeTokensDto {
    fn default() -> Self {
        Self {
            background: "#0b0d10".to_owned(),
            surface: "#12151a".to_owned(),
            surface_high: "#1a1f27".to_owned(),
            text: "#eceef2".to_owned(),
            muted: "#9aa3b2".to_owned(),
            faint: "#6b7485".to_owned(),
            accent: "#2dd4bf".to_owned(),
            interactive: "#60a5fa".to_owned(),
            border: "#262c36".to_owned(),
            error: "#f07178".to_owned(),
            warning: "#f5a962".to_owned(),
            success: "#6ee7a0".to_owned(),
            info: "#56c2d6".to_owned(),
            diff_added: "#4fd6be".to_owned(),
            diff_removed: "#e06c75".to_owned(),
            diff_context: "#8892a0".to_owned(),
            code_function: "#c792ea".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeOverrideDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_scheme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_high: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interactive: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_added: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_removed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_function: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSettingsDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high_watermark_pct: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spill_max_inline_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verbatim_window_turns: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_pruning: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_caching: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_on_verify_failure: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps_per_turn: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_progress_fail_steps: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_limit_continuations: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_session_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turn_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summarizer_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_edit_diagnostics: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_memory: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_prompt_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<EffortDto>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffortDto {
    Low,
    Medium,
    High,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSettingsDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_prune_days: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxSettingsDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_for_scripts: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_for_shell: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict_env: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSnapshotDto {
    pub theme: ThemeTokensDto,
    #[serde(default)]
    pub raw_theme: ThemeOverrideDto,
    #[serde(default)]
    pub context: ContextSettingsDto,
    #[serde(default)]
    pub session: SessionSettingsDto,
    #[serde(default)]
    pub sandbox: SandboxSettingsDto,
    #[serde(default)]
    pub permissions_summary: Vec<String>,
    #[serde(default)]
    pub config_writable: bool,
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
    pub tools: Vec<String>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChatGptLoginStateDto {
    Started,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type",
    content = "data"
)]
pub enum ViewEvent {
    SessionOpened {
        session: SessionSummaryDto,
    },
    SessionUpdated {
        session: SessionSummaryDto,
    },
    ContextUpdated {
        session_id: String,
        breakdown: ContextBreakdownDto,
    },
    GoalUpdated {
        session_id: String,
        goal: GoalDto,
    },
    SessionActivated {
        session_id: String,
    },
    SessionClosed {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_active_session_id: Option<String>,
    },
    SessionReset {
        session_id: String,
    },
    SessionRewound {
        session_id: String,
        turn_id: String,
        scope: RewindScopeDto,
        restored: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entries: Option<Vec<TranscriptEntryDto>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        restored_prompt: Option<String>,
    },
    TurnStarted {
        session_id: String,
        turn_id: String,
        model: String,
    },
    UserPromptSubmitted {
        session_id: String,
        entry_id: String,
        text: String,
    },
    TextDelta {
        session_id: String,
        item_id: String,
        delta: String,
    },
    ReasoningDelta {
        session_id: String,
        item_id: String,
        delta: String,
    },
    EntryCompleted {
        session_id: String,
        entry: TranscriptEntryDto,
    },
    EntryUpdated {
        session_id: String,
        entry: TranscriptEntryDto,
    },
    ApprovalRequested {
        session_id: String,
        request: ApprovalRequestDto,
    },
    ApprovalResolved {
        session_id: String,
        request_id: String,
    },
    QuestionAsked {
        session_id: String,
        request: QuestionRequestDto,
    },
    QuestionResolved {
        session_id: String,
        request_id: String,
    },
    UsageUpdated {
        session_id: String,
        usage: UsageDto,
        metrics: TurnMetricsDto,
    },
    TurnCompleted {
        session_id: String,
        turn_id: String,
        reason: String,
    },
    Error {
        session_id: String,
        code: String,
        message: String,
    },
    FileChangesUpdated {
        changes: Vec<FileChangeDto>,
    },
    Reloaded {
        host_info: Box<HostInfoDto>,
        warnings: Vec<String>,
    },
    ProvidersUpdated,
    ChatGptLoginUpdated {
        attempt_id: String,
        provider_id: String,
        state: ChatGptLoginStateDto,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    ResyncRequired,
    HostDisconnected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitPromptRequest {
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitPromptResponse {
    pub accepted: bool,
    pub session_id: String,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunShellRequest {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunShellResponse {
    pub output: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFilesResponse {
    pub files: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSearchMatchDto {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
    #[serde(default)]
    pub snippet: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSearchResponse {
    pub mode: SearchModeDto,
    pub query: String,
    pub matches: Vec<WorkspaceSearchMatchDto>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePreviewLineDto {
    pub line_no: usize,
    pub text: String,
    pub highlighted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePreviewResponse {
    pub title: String,
    pub lines: Vec<WorkspacePreviewLineDto>,
    pub scroll: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderItemDto {
    pub id: String,
    pub display_name: String,
    pub adapter: String,
    pub auth_kind: AuthKindDto,
    pub configured: bool,
    pub status_badge: String,
    #[serde(default)]
    pub env_vars: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default)]
    pub is_custom: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelItemDto {
    pub id: String,
    pub provider: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default)]
    pub supports_reasoning: bool,
    #[serde(default)]
    pub supports_vision: bool,
    #[serde(default)]
    pub supports_tools: bool,
    #[serde(default)]
    pub is_custom: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvidersResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,
    pub providers: Vec<ProviderItemDto>,
    pub models: Vec<ModelItemDto>,
    #[serde(default)]
    pub recent_models: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetApiKeyRequest {
    pub api_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetProviderBaseUrlRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatGptLoginStartResponse {
    pub attempt_id: String,
    pub provider_id: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatGptLoginPasteRequest {
    pub input: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSettingsRequest {
    #[serde(default)]
    pub reset_theme: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<ThemeOverrideDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextSettingsDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionSettingsDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxSettingsDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forget_model: Option<ForgetModelDto>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgetModelDto {
    pub provider: String,
    pub model: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReloadResponse {
    pub host_info: HostInfoDto,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_summary() -> SessionSummaryDto {
        SessionSummaryDto {
            id: "sess-1".to_owned(),
            title: Some("Fix protocol".to_owned()),
            backend: BackendKind::Native,
            provider: "anthropic".to_owned(),
            model: "claude-sonnet".to_owned(),
            approval_mode: ApprovalModeDto::Ask,
            status: SessionStatus::Running,
            active_turn_id: Some("turn-1".to_owned()),
            active_tool: None,
            turn_started_at_ms: None,
            user_shell_running: false,
            shell_runs: Vec::new(),
        }
    }

    #[test]
    fn api_error_round_trip() {
        let codes = [
            (ApiErrorCode::Unauthorized, "unauthorized"),
            (ApiErrorCode::Forbidden, "forbidden"),
            (ApiErrorCode::BadRequest, "badRequest"),
            (ApiErrorCode::NotFound, "notFound"),
            (ApiErrorCode::ConflictBusy, "conflictBusy"),
            (ApiErrorCode::ConflictStale, "conflictStale"),
            (ApiErrorCode::UnsupportedBackend, "unsupportedBackend"),
            (ApiErrorCode::Internal, "internal"),
        ];

        for (code, expected) in codes {
            let err = ApiError {
                code,
                message: "rejected".to_owned(),
            };
            let value = serde_json::to_value(&err).unwrap();
            assert_eq!(value["code"], expected);
            assert_eq!(value["message"], "rejected");
            let decoded: ApiError = serde_json::from_value(value).unwrap();
            assert_eq!(decoded, err);
        }
    }

    #[test]
    fn transcript_entry_round_trip() {
        let entries = [
            TranscriptEntryDto::User {
                id: "u1".to_owned(),
                turn_id: Some("turn-1".to_owned()),
                text: "hello".to_owned(),
            },
            TranscriptEntryDto::Assistant {
                id: "a1".to_owned(),
                text: "world".to_owned(),
                reasoning: "thought".to_owned(),
            },
            TranscriptEntryDto::System {
                id: "s1".to_owned(),
                text: "note".to_owned(),
            },
            TranscriptEntryDto::Error {
                id: "e1".to_owned(),
                message: "boom".to_owned(),
            },
            TranscriptEntryDto::ToolCall {
                id: "t1".to_owned(),
                name: "code-read_file".to_owned(),
                args: "{}".to_owned(),
                status: "succeeded".to_owned(),
                output: Some("ok".to_owned()),
                edit_diff: Some(vec![UnifiedLineDto {
                    kind: UnifiedLineKindDto::Added,
                    text: "let x = 1;".to_owned(),
                }]),
            },
            TranscriptEntryDto::Subagent {
                id: "sa-1".to_owned(),
                name: Some("scan".to_owned()),
                agent: "research".to_owned(),
                status: "running".to_owned(),
                report: None,
                child_session_id: Some("ses-child".to_owned()),
                model: None,
                approval_mode: "ask".to_owned(),
                steps_used: 0,
                max_steps: 0,
                usage: UsageDto::default(),
                est_cost_usd: None,
                active_tool: None,
            },
            TranscriptEntryDto::Question {
                id: "q-1".to_owned(),
                questions: vec![QuestionPromptDto {
                    header: "Choice".to_owned(),
                    question: "Pick one".to_owned(),
                    options: vec![QuestionOptionDto {
                        label: "A".to_owned(),
                        description: None,
                    }],
                    multiple: false,
                }],
                answers: Some(vec![QuestionAnswerDto {
                    header: "Choice".to_owned(),
                    selected: vec!["A".to_owned()],
                }]),
                resolved: true,
            },
        ];

        for entry in entries {
            let json = serde_json::to_string(&entry).unwrap();
            let decoded: TranscriptEntryDto = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, entry);
        }

        let tool_value = serde_json::to_value(&TranscriptEntryDto::ToolCall {
            id: "t1".to_owned(),
            name: "code-read_file".to_owned(),
            args: String::new(),
            status: "succeeded".to_owned(),
            output: None,
            edit_diff: None,
        })
        .unwrap();
        assert_eq!(tool_value["type"], "toolCall");
        assert_eq!(tool_value["data"]["name"], "code-read_file");
    }

    #[test]
    fn snapshot_response_round_trip() {
        let mut acp_summary = sample_summary();
        acp_summary.id = "sess-2".to_owned();
        acp_summary.title = None;
        acp_summary.backend = BackendKind::Acp {
            preset: "agy".to_owned(),
            remote_session_id: Some("remote-9".to_owned()),
        };
        acp_summary.status = SessionStatus::WaitingApproval;
        acp_summary.active_turn_id = None;

        let snapshot = SnapshotResponse {
            event_id: 42,
            active_session_id: Some("sess-1".to_owned()),
            sessions: vec![sample_summary(), acp_summary],
            active_session: Some(SessionDetailDto {
                summary: sample_summary(),
                entries: vec![TranscriptEntryDto::User {
                    id: "u1".to_owned(),
                    turn_id: None,
                    text: "ping".to_owned(),
                }],
                streaming_item_id: Some("item-2".to_owned()),
                streaming_text: "pong".to_owned(),
                streaming_reasoning: String::new(),
                pending_approvals: Vec::new(),
                pending_questions: Vec::new(),
                usage: UsageDto::default(),
                metrics: TurnMetricsDto::default(),
                context: None,
                goal: None,
            }),
            file_changes: vec![FileChangeDto {
                status: "M".to_owned(),
                path: "src/lib.rs".to_owned(),
            }],
            host_info: Some(HostInfoDto::default()),
        };

        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(value["eventId"], 42);
        assert_eq!(value["activeSessionId"], "sess-1");
        assert_eq!(value["sessions"][1]["backend"]["type"], "acp");
        assert_eq!(
            value["sessions"][1]["backend"]["data"]["remoteSessionId"],
            "remote-9"
        );
        let decoded: SnapshotResponse = serde_json::from_value(value).unwrap();
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn view_event_round_trip() {
        let events = [
            ViewEvent::SessionOpened {
                session: sample_summary(),
            },
            ViewEvent::SessionUpdated {
                session: sample_summary(),
            },
            ViewEvent::SessionActivated {
                session_id: "sess-1".to_owned(),
            },
            ViewEvent::SessionClosed {
                session_id: "sess-1".to_owned(),
                next_active_session_id: None,
            },
            ViewEvent::TurnStarted {
                session_id: "sess-1".to_owned(),
                turn_id: "turn-1".to_owned(),
                model: "claude-sonnet".to_owned(),
            },
            ViewEvent::UserPromptSubmitted {
                session_id: "sess-1".to_owned(),
                entry_id: "u1".to_owned(),
                text: "hi".to_owned(),
            },
            ViewEvent::TextDelta {
                session_id: "sess-1".to_owned(),
                item_id: "a1".to_owned(),
                delta: "hel".to_owned(),
            },
            ViewEvent::ReasoningDelta {
                session_id: "sess-1".to_owned(),
                item_id: "a1".to_owned(),
                delta: "think".to_owned(),
            },
            ViewEvent::EntryCompleted {
                session_id: "sess-1".to_owned(),
                entry: TranscriptEntryDto::Assistant {
                    id: "a1".to_owned(),
                    text: "hello".to_owned(),
                    reasoning: "think".to_owned(),
                },
            },
            ViewEvent::ApprovalRequested {
                session_id: "sess-1".to_owned(),
                request: ApprovalRequestDto {
                    request_id: "apr-1".to_owned(),
                    turn_id: "turn-1".to_owned(),
                    kind: ApprovalKindDto::ToolCall,
                    title: "Edit file".to_owned(),
                    detail: "src/main.rs".to_owned(),
                    effect: Some(EffectClassDto::Write),
                },
            },
            ViewEvent::TurnCompleted {
                session_id: "sess-1".to_owned(),
                turn_id: "turn-1".to_owned(),
                reason: "completed".to_owned(),
            },
            ViewEvent::ChatGptLoginUpdated {
                attempt_id: "att-1".to_owned(),
                provider_id: "openai-codex".to_owned(),
                state: ChatGptLoginStateDto::Started,
                url: Some("https://auth.example.com".to_owned()),
                account: None,
                message: None,
            },
            ViewEvent::Error {
                session_id: "sess-1".to_owned(),
                code: "providerError".to_owned(),
                message: "timeout".to_owned(),
            },
            ViewEvent::ProvidersUpdated,
            ViewEvent::ResyncRequired,
            ViewEvent::HostDisconnected,
        ];

        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let decoded: ViewEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, event);
        }

        let resync = serde_json::to_value(&ViewEvent::ResyncRequired).unwrap();
        assert_eq!(resync["type"], "resyncRequired");
        assert!(resync.get("data").is_none());

        let disconnected = serde_json::to_value(&ViewEvent::HostDisconnected).unwrap();
        assert_eq!(disconnected["type"], "hostDisconnected");
        assert!(disconnected.get("data").is_none());

        let turn_started = serde_json::to_value(&ViewEvent::TurnStarted {
            session_id: "sess-1".to_owned(),
            turn_id: "turn-1".to_owned(),
            model: "claude-sonnet".to_owned(),
        })
        .unwrap();
        assert_eq!(turn_started["type"], "turnStarted");
        assert_eq!(turn_started["data"]["sessionId"], "sess-1");
        assert_eq!(turn_started["data"]["turnId"], "turn-1");
        assert_eq!(turn_started["data"]["model"], "claude-sonnet");
    }

    #[test]
    fn question_answer_request_supports_cancel() {
        let answered: QuestionAnswerRequest =
            serde_json::from_str(r#"{"answers":[{"header":"Choice","selected":["A"]}]}"#).unwrap();
        assert!(!answered.cancel);
        assert_eq!(answered.answers.len(), 1);

        let cancelled: QuestionAnswerRequest =
            serde_json::from_str(r#"{"answers":[],"cancel":true}"#).unwrap();
        assert!(cancelled.cancel);
        assert!(cancelled.answers.is_empty());
    }

    #[test]
    fn request_and_response_round_trip() {
        let req = SubmitPromptRequest {
            prompt: "explain this".to_owned(),
            display: Some("@skill/verify explain this".to_owned()),
        };
        let req_json = serde_json::to_string(&req).unwrap();
        let decoded_req: SubmitPromptRequest = serde_json::from_str(&req_json).unwrap();
        assert_eq!(decoded_req, req);

        let legacy: SubmitPromptRequest = serde_json::from_str(r#"{"prompt":"hi"}"#).unwrap();
        assert_eq!(legacy.prompt, "hi");
        assert!(legacy.display.is_none());

        let resp = SubmitPromptResponse {
            accepted: true,
            session_id: "sess-1".to_owned(),
        };
        let resp_value = serde_json::to_value(&resp).unwrap();
        assert_eq!(resp_value["sessionId"], "sess-1");
        let decoded_resp: SubmitPromptResponse = serde_json::from_value(resp_value).unwrap();
        assert_eq!(decoded_resp, resp);

        let created = CreateSessionResponse {
            session: sample_summary(),
        };
        let created_json = serde_json::to_string(&created).unwrap();
        let decoded_created: CreateSessionResponse = serde_json::from_str(&created_json).unwrap();
        assert_eq!(decoded_created, created);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostActionRequest {
    pub command: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostActionResponse {
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobResponse {
    pub content: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryDto {
    pub id: String,
    pub project_root: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_session_id: Option<String>,
    pub pinned: bool,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteDto {
    pub task_type: String,
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens_per_turn: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackDto {
    pub id: String,
    pub source_kind: String,
    pub source_uri: String,
    pub pinned_rev: String,
    pub enabled: bool,
    pub trusted: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionInfoDto {
    pub id: String,
    pub kind: String,
    pub source: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_invocable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    pub token_estimate: u64,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub supporting_files: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShadowedDefinitionDto {
    pub id: String,
    pub kind: String,
    pub source: String,
    pub path: String,
    pub kept_source: String,
    pub kept_path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionsDiagnosticsDto {
    #[serde(default)]
    pub agents: Vec<DefinitionInfoDto>,
    #[serde(default)]
    pub skills: Vec<DefinitionInfoDto>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub shadows: Vec<ShadowedDefinitionDto>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoriesResponse {
    pub memories: Vec<MemoryDto>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutesResponse {
    pub routes: Vec<RouteDto>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PacksResponse {
    pub packs: Vec<PackDto>,
}
