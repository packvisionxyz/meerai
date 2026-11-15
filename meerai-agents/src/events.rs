use std::path::PathBuf;

use meerai_core::{EffectClass, FileDiff};
use meerai_policy::ApprovalMode;
pub use meerai_session::{
    AbortCause, ApprovalDecision, ApprovalKind, ApprovalPreview, CallId, CompactionTrigger,
    ContentBlock, ErrorCode, GrantScope, ItemId, RequestId, StepFinish, TurnEndReason, TurnId,
    TurnTrigger, Usage,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemStatus {
    Proposed,
    Pending,
    InProgress,
    Succeeded,
    Failed,
    Stopped,
    Denied,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlanStepStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStep {
    pub text: String,
    pub status: PlanStepStatus,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum SessionItem {
    UserMessage {
        id: ItemId,
        content: Vec<ContentBlock>,
    },
    AgentMessage {
        id: ItemId,
        text: String,
    },
    Reasoning {
        id: ItemId,
        summary: String,
    },
    ToolCall {
        id: ItemId,
        call_id: CallId,
        name: String,
        args: String,
        effect: EffectClass,
        status: ItemStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    CommandRun {
        id: ItemId,
        command: String,
        cwd: PathBuf,
        aggregated_output: String,
        exit_code: Option<i32>,
        duration_ms: u64,
        status: ItemStatus,
    },
    FileChange {
        id: ItemId,
        changes: Vec<FileDiff>,
        status: ItemStatus,
    },
    SubagentRun {
        id: ItemId,
        agent: String,
        status: ItemStatus,
        report: Option<String>,
    },
    Compaction {
        id: ItemId,
        trigger: CompactionTrigger,
        freed_tokens: u64,
    },
    Plan {
        id: ItemId,
        steps: Vec<PlanStep>,
    },
}

impl SessionItem {
    pub fn id(&self) -> &ItemId {
        match self {
            Self::UserMessage { id, .. }
            | Self::AgentMessage { id, .. }
            | Self::Reasoning { id, .. }
            | Self::ToolCall { id, .. }
            | Self::CommandRun { id, .. }
            | Self::FileChange { id, .. }
            | Self::SubagentRun { id, .. }
            | Self::Compaction { id, .. }
            | Self::Plan { id, .. } => id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "data")]
pub enum ItemDelta {
    AgentText(String),
    ReasoningSummary(String),
    CommandOutput(String),
    PlanUpdated(Vec<PlanStep>),
    StatusChanged(ItemStatus),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub request_id: RequestId,
    pub turn_id: TurnId,
    pub kind: ApprovalKind,
    pub preview: ApprovalPreview,
    pub effect: Option<EffectClass>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum AgentEvent {
    TurnStarted {
        turn_id: TurnId,
        model: String,
    },
    ItemStarted {
        item: SessionItem,
    },
    ItemDelta {
        item_id: ItemId,
        delta: ItemDelta,
    },
    ItemCompleted {
        item: SessionItem,
    },
    ApprovalRequested {
        request: ApprovalRequest,
    },
    ApprovalResolved {
        request_id: RequestId,
    },
    UsageUpdated {
        usage: Usage,
        cumulative: Usage,
        est_cost_usd: Option<f64>,
    },
    TurnCompleted {
        turn_id: TurnId,
        reason: TurnEndReason,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "data")]
pub enum AgentCommand {
    SubmitPrompt(String),
    Steer(String),
    Approve {
        request_id: RequestId,
        decision: ApprovalDecision,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<GrantScope>,
    },
    SetModel(String),
    SetPermissionMode(ApprovalMode),
    Compact,
    ApplyAgent {
        name: String,
        instructions: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    ApplySkill {
        name: String,
        instructions: String,
    },
    Revert {
        turn_id: TurnId,
    },
    Cancel,
    Reset,
    Quit,
}
