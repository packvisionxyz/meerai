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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<i32>,
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
        self.cache_creation_tokens = sum(self.cache_creation_tokens, other.cache_creation_tokens);
    }
}

impl From<genai::chat::Usage> for Usage {
    fn from(usage: genai::chat::Usage) -> Self {
        let (cached_tokens, cache_creation_tokens) = usage
            .prompt_tokens_details
            .map(|details| (details.cached_tokens, details.cache_creation_tokens))
            .unwrap_or_default();
        Self {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
            cached_tokens,
            cache_creation_tokens,
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
    ExternalAgent,
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookDetail {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail_closed: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewindScope {
    Both,
    Code,
    Conversation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionPrompt {
    pub header: String,
    pub question: String,
    pub options: Vec<QuestionOption>,
    #[serde(default)]
    pub multiple: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAnswer {
    pub header: String,
    pub selected: Vec<String>,
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
        /// Opaque provider reasoning signatures, index-aligned with `tool_calls`.
        #[serde(default, skip_serializing_if = "signatures_absent")]
        call_signatures: Vec<Option<Vec<String>>>,
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
        blob_key: String,
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
    QuestionAsked {
        request_id: RequestId,
        questions: Vec<QuestionPrompt>,
    },
    QuestionAnswered {
        request_id: RequestId,
        answers: Option<Vec<QuestionAnswer>>,
    },
    RewindApplied {
        turn_id: TurnId,
        scope: RewindScope,
        restored: Vec<PathBuf>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replaced: Option<SeqRange>,
    },
    HookFired {
        event: String,
        decision: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<HookDetail>,
    },
    ContextTurnPinned {
        turn_id: TurnId,
        pinned: bool,
    },
    LoopUpdated {
        state: crate::loop_state::LoopState,
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

fn signatures_absent(signatures: &[Option<Vec<String>>]) -> bool {
    signatures.iter().all(Option::is_none)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_message_accepts_legacy_payload_without_call_signatures() {
        let legacy = r#"{"seq":5,"event":{"type":"assistant_message","provider":"gemini","model":"gemini-2.5-pro","content":[],"tool_calls":[{"id":"c1","name":"code-read_file","args":"{}"}],"usage":{},"stream_digest":{"final_text":"","duration_ms":0,"usage":{}}}}"#;
        let logged: LoggedEvent = serde_json::from_str(legacy).unwrap();
        match logged.event {
            SessionEvent::AssistantMessage {
                call_signatures,
                tool_calls,
                ..
            } => {
                assert!(call_signatures.is_empty());
                assert_eq!(tool_calls[0].id, "c1");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn assistant_message_omits_absent_call_signatures() {
        let logged = LoggedEvent {
            seq: Seq(6),
            event: SessionEvent::AssistantMessage {
                provider: "gemini".to_string(),
                model: "gemini-2.5-pro".to_string(),
                content: Vec::new(),
                tool_calls: vec![meerai_core::ToolCall {
                    id: "c1".to_string(),
                    name: "code-read_file".to_string(),
                    args: "{}".to_string(),
                }],
                call_signatures: vec![None],
                usage: Usage::default(),
                stream_digest: StreamDigest::default(),
            },
        };
        let raw = serde_json::to_string(&logged).unwrap();
        assert!(!raw.contains("call_signatures"), "{raw}");
    }

    #[test]
    fn hook_fired_accepts_legacy_payload_without_detail() {
        let legacy =
            r#"{"seq":3,"event":{"type":"hook_fired","event":"PreToolUse","decision":"allow"}}"#;
        let logged: LoggedEvent = serde_json::from_str(legacy).unwrap();
        match logged.event {
            SessionEvent::HookFired {
                event,
                decision,
                detail,
            } => {
                assert_eq!(event, "PreToolUse");
                assert_eq!(decision, "allow");
                assert_eq!(detail, None);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn hook_fired_round_trips_typed_detail() {
        let logged = LoggedEvent {
            seq: Seq(4),
            event: SessionEvent::HookFired {
                event: "PostToolUse".to_string(),
                decision: "block".to_string(),
                detail: Some(HookDetail {
                    tool: Some("code-edit_file".to_string()),
                    reason: Some("fmt check failed".to_string()),
                    exit_code: Some(2),
                    duration_ms: Some(17),
                    fail_closed: Some(true),
                }),
            },
        };
        let encoded = serde_json::to_string(&logged).unwrap();
        let decoded: LoggedEvent = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, logged);
    }
}

#[cfg(test)]
mod question_tests {
    use super::*;

    #[test]
    fn question_events_round_trip() {
        let questions = vec![QuestionPrompt {
            header: "DB".to_string(),
            question: "Which database?".to_string(),
            options: vec![
                QuestionOption {
                    label: "Postgres".to_string(),
                    description: None,
                },
                QuestionOption {
                    label: "SQLite".to_string(),
                    description: Some("local".to_string()),
                },
            ],
            multiple: false,
        }];
        let asked = SessionEvent::QuestionAsked {
            request_id: RequestId::new(),
            questions: questions.clone(),
        };
        let encoded = serde_json::to_string(&asked).unwrap();
        let decoded: SessionEvent = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, asked);

        let answered = SessionEvent::QuestionAnswered {
            request_id: RequestId::new(),
            answers: Some(vec![QuestionAnswer {
                header: "DB".to_string(),
                selected: vec!["Postgres".to_string()],
            }]),
        };
        let encoded = serde_json::to_string(&answered).unwrap();
        let decoded: SessionEvent = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, answered);
    }
}
