use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use meerai_core::{ApprovalPreview, GrantScope};
use meerai_core::{ToolCall, ToolObservation};

use crate::ids::{CallId, CheckpointId, CompactionId, RequestId, StepId, TurnId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seq(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeqRange {
    pub start: Seq,
    pub end_inclusive: Seq,
}

impl SeqRange {
    pub fn contains(&self, seq: Seq) -> bool {
        self.start.0 <= seq.0 && seq.0 <= self.end_inclusive.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnTrigger {
    UserPrompt,
    Steering,
    Resume,
    Fork,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbortCause {
    User,
    Cancelled,
    Shutdown,
    Timeout,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TurnEndReason {
    Completed,
    Aborted { cause: AbortCause },
    Failed { error: String },
    MaxTokens,
    Interrupted,
    Forked,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum StepFinish {
    Completed,
    Aborted { cause: AbortCause },
    Failed { error: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputSource {
    User,
    System,
    RuntimeContext,
    Steering,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptSection {
    pub label: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "data")]
pub enum ContentBlock {
    Text(String),
    Reasoning(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<i32>,
}

impl Usage {
    pub fn add(&mut self, other: Usage) {
        fn sum(a: Option<i32>, b: Option<i32>) -> Option<i32> {
            match (a, b) {
                (Some(x), Some(y)) => Some(x.saturating_add(y)),
                (Some(x), None) => Some(x),
                (None, Some(y)) => Some(y),
                (None, None) => None,
            }
        }
        self.prompt_tokens = sum(self.prompt_tokens, other.prompt_tokens);
        self.completion_tokens = sum(self.completion_tokens, other.completion_tokens);
        self.total_tokens = sum(self.total_tokens, other.total_tokens);
        self.cached_tokens = sum(self.cached_tokens, other.cached_tokens);
    }
}

impl From<genai::chat::Usage> for Usage {
    fn from(usage: genai::chat::Usage) -> Self {
        Self {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
            cached_tokens: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StreamDigest {
    pub final_text: String,
    pub duration_ms: u64,
    pub usage: Usage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionTrigger {
    Pressure,
    ContextOverflow,
    Manual,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CompactionResult {
    Applied { freed_tokens: u64 },
    Skipped { reason: String },
    Failed { error: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    ToolCall,
    Spend,
    SandboxEscape,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Accept,
    AcceptForSession,
    Decline,
    DeclineWithReason(String),
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BreakerKind {
    RepeatedToolFailure,
    RepeatedIdenticalEdit,
    NoProgress,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BreakerAction {
    DiagnosticInjected,
    ContinueOnce,
    TurnFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Provider,
    Tool,
    Config,
    ContextOverflow,
    Budget,
    LoopDetected,
    Unsupported,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum SessionEvent {
    TurnStarted {
        turn_id: TurnId,
        trigger: TurnTrigger,
    },
    TurnEnded {
        turn_id: TurnId,
        reason: TurnEndReason,
    },
    StepStarted {
        step_id: StepId,
        turn_id: TurnId,
    },
    StepEnded {
        step_id: StepId,
        finish: StepFinish,
    },
    SystemMessage {
        sections: Vec<PromptSection>,
        generation: u64,
    },
    UserMessage {
        source: InputSource,
        content: Vec<ContentBlock>,
    },
    AssistantMessage {
        provider: String,
        model: String,
        content: Vec<ContentBlock>,
        tool_calls: Vec<ToolCall>,
        usage: Usage,
        stream_digest: StreamDigest,
    },
    AssistantAttempt {
        provider: String,
        model: String,
        error: String,
        usage: Usage,
    },
    ToolCall {
        call_id: CallId,
        name: String,
        args: String,
    },
    ToolResult {
        call_id: CallId,
        output: ToolObservation,
    },
    SpillApplied {
        call_id: CallId,
        bytes_omitted: u64,
        blob_path: PathBuf,
    },
    CompactionStarted {
        compaction_id: CompactionId,
        trigger: CompactionTrigger,
    },
    CompactionSummary {
        compaction_id: CompactionId,
        replaced: SeqRange,
        summary: String,
    },
    CompactionEnded {
        compaction_id: CompactionId,
        result: CompactionResult,
    },
    RevertApplied {
        turn_id: TurnId,
        replaced: SeqRange,
    },
    UsageUpdated {
        usage: Usage,
        cumulative: Usage,
    },
    ApprovalRequested {
        request_id: RequestId,
        kind: ApprovalKind,
        preview: ApprovalPreview,
    },
    ApprovalDecided {
        request_id: RequestId,
        decision: ApprovalDecision,
        scope: Option<GrantScope>,
    },
    HookFired {
        event: String,
        decision: String,
    },
    CheckpointCreated {
        checkpoint_id: CheckpointId,
        turn_id: TurnId,
        paths: Vec<PathBuf>,
    },
    CircuitBreaker {
        kind: BreakerKind,
        action: BreakerAction,
    },
    Error {
        code: ErrorCode,
        message: String,
        retryable: bool,
    },
}

impl SessionEvent {
    pub fn is_model_visible(&self) -> bool {
        matches!(
            self,
            SessionEvent::SystemMessage { .. }
                | SessionEvent::UserMessage { .. }
                | SessionEvent::AssistantMessage { .. }
                | SessionEvent::ToolCall { .. }
                | SessionEvent::ToolResult { .. }
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoggedEvent {
    pub seq: Seq,
    pub event: SessionEvent,
}
