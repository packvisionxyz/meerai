use std::path::PathBuf;

use meerai_common::hooks::HookEntry;
use meerai_common::settings::{ContextSettings, EffortLevel};
use meerai_core::{EffectClass, FileDiff};
use meerai_policy::{ApprovalMode, PermissionRule};
pub use meerai_session::{
    AbortCause, ApprovalDecision, ApprovalKind, ApprovalPreview, CallId, CompactionTrigger,
    ContentBlock, ContextBreakdown, ErrorCode, GrantScope, ItemId, QuestionAnswer, QuestionOption,
    QuestionPrompt, RequestId, RewindScope, StepFinish, TurnEndReason, TurnId, TurnTrigger, Usage,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub agent: String,
    pub task: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<ApprovalMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,
}

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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        agent: String,
        status: ItemStatus,
        report: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        child_session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default)]
        approval_mode: ApprovalMode,
        #[serde(default)]
        steps_used: usize,
        #[serde(default)]
        max_steps: usize,
        #[serde(default)]
        usage: Usage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        est_cost_usd: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        active_tool: Option<String>,
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
    SubagentProgress {
        steps_used: usize,
        max_steps: usize,
        usage: Usage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        est_cost_usd: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        active_tool: Option<String>,
    },
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
#[serde(rename_all = "camelCase")]
pub struct QuestionRequest {
    pub request_id: RequestId,
    pub turn_id: TurnId,
    pub questions: Vec<QuestionPrompt>,
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
    ItemDiscarded {
        item_id: ItemId,
    },
    ApprovalRequested {
        request: ApprovalRequest,
    },
    ApprovalResolved {
        request_id: RequestId,
    },
    QuestionAsked {
        request: QuestionRequest,
    },
    QuestionResolved {
        request_id: RequestId,
    },
    RewindCompleted {
        turn_id: TurnId,
        scope: RewindScope,
        restored: Vec<String>,
        error: Option<String>,
    },
    RecoveryStarted {
        attempt: usize,
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
    HookCompleted {
        event: String,
        decision: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    AutoApproved {
        tool: String,
        effect: EffectClass,
    },
    ContextUpdated {
        breakdown: ContextBreakdown,
    },
    LoopUpdated {
        state: meerai_session::LoopState,
    },
    LoopContinued {
        turn: usize,
        max_turns: usize,
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
    AnswerQuestion {
        request_id: RequestId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answers: Option<Vec<QuestionAnswer>>,
    },
    Rewind {
        turn_id: TurnId,
        scope: RewindScope,
    },
    SetModel {
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_window_tokens: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort_levels: Option<Vec<EffortLevel>>,
    },
    SetPlanUsage {
        plan_usage: bool,
    },
    SetPermissionMode(ApprovalMode),
    Compact {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focus: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_turn_id: Option<TurnId>,
    },
    SetProjectionOverrides {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        verbatim_window_turns: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_pruning: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clear_old_tool_results: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pin_turns: Option<Vec<TurnId>>,
    },
    InspectContext,
    SetTurnPinned {
        turn_id: TurnId,
        pinned: bool,
    },
    SpawnSubagent {
        spec: SubagentSpec,
    },
    CancelSubagent {
        item_id: ItemId,
    },
    ApplyAgent {
        name: String,
        instructions: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_window_tokens: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort_levels: Option<Vec<EffortLevel>>,
    },
    ApplySkill {
        name: String,
        instructions: String,
    },
    Reload {
        rules: Option<Vec<PermissionRule>>,
        context: Option<ContextSettings>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort_levels: Option<Vec<EffortLevel>>,
        system_prompt: String,
    },
    SetHooks(Vec<HookEntry>),
    Cancel,
    Reset,
    Quit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerai_session::ContextBreakdown;

    #[test]
    fn context_commands_round_trip() {
        let commands = [
            AgentCommand::Compact {
                focus: Some("verify the parser".to_string()),
                from_turn_id: Some(TurnId::from("turn-3")),
            },
            AgentCommand::SetProjectionOverrides {
                verbatim_window_turns: Some(6),
                reasoning_pruning: Some(false),
                clear_old_tool_results: Some(false),
                pin_turns: Some(vec![TurnId::from("turn-1")]),
            },
            AgentCommand::InspectContext,
            AgentCommand::CancelSubagent {
                item_id: ItemId::from("item-1"),
            },
        ];
        for command in commands {
            let encoded = serde_json::to_string(&command).unwrap();
            let decoded: AgentCommand = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, command);
        }
    }

    #[test]
    fn spawned_subagent_spec_round_trips() {
        let command = AgentCommand::SpawnSubagent {
            spec: SubagentSpec {
                name: Some("parser scan".to_string()),
                agent: "explore".to_string(),
                task: "find the parser".to_string(),
                model: Some("anthropic::fast".to_string()),
                approval_mode: Some(ApprovalMode::Plan),
                allowed_tools: Some(vec!["code-grep".to_string()]),
                max_steps: Some(4),
                max_tokens: Some(20_000),
                max_cost_usd: Some(0.25),
            },
        };
        let encoded = serde_json::to_string(&command).unwrap();
        let decoded: AgentCommand = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, command);
    }

    #[test]
    fn subagent_progress_delta_round_trips() {
        let delta = ItemDelta::SubagentProgress {
            steps_used: 3,
            max_steps: 8,
            usage: Usage {
                prompt_tokens: Some(120),
                completion_tokens: Some(40),
                total_tokens: Some(160),
                cached_tokens: Some(20),
                cache_creation_tokens: None,
            },
            est_cost_usd: Some(0.004),
            active_tool: Some("code-grep".to_string()),
        };
        let encoded = serde_json::to_string(&delta).unwrap();
        let decoded: ItemDelta = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, delta);
    }

    #[test]
    fn context_updated_round_trips() {
        let event = AgentEvent::ContextUpdated {
            breakdown: ContextBreakdown {
                context_window_tokens: 128_000,
                high_watermark_tokens: 96_000,
                estimated_total_tokens: 1_000,
                pinned_turn_ids: vec![TurnId::from("turn-1")],
                ..ContextBreakdown::default()
            },
        };
        let encoded = serde_json::to_string(&event).unwrap();
        let decoded: AgentEvent = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, event);
    }
}
