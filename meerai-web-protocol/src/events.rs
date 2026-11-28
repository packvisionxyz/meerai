use crate::*;
use serde::{Deserialize, Serialize};

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
    LoopUpdated {
        session_id: String,
        state: LoopDto,
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
    WorkspaceChanged {
        session_id: String,
        path: String,
        carry: bool,
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
    StreamDiscarded {
        session_id: String,
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
