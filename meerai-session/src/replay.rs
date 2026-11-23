use meerai_core::ObservationStatus;

use crate::{
    ContentBlock, InputSource, LoggedEvent, QuestionAnswer, QuestionPrompt, SeqRange, SessionEvent,
    Usage, join_text_blocks,
};

const QUESTION_TOOL: &str = "question-ask";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolCallStatus {
    Proposed,
    Running,
    Succeeded,
    Failed,
    Stopped,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayToolCall {
    pub call_id: String,
    pub name: String,
    pub args: String,
    pub status: ToolCallStatus,
    pub output: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplaySubagent {
    pub id: String,
    pub name: Option<String>,
    pub agent: String,
    pub status: ToolCallStatus,
    pub report: Option<String>,
    pub child_session_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayQuestion {
    pub request_id: String,
    pub questions: Vec<QuestionPrompt>,
    pub answers: Option<Vec<QuestionAnswer>>,
    pub resolved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayEntry {
    User { text: String },
    Assistant { text: String, reasoning: String },
    ToolCall(ReplayToolCall),
    Subagent(ReplaySubagent),
    Question(ReplayQuestion),
    System { text: String },
}

pub fn replay_from_events(events: &[LoggedEvent]) -> Vec<ReplayEntry> {
    let shadowed: Vec<SeqRange> = events
        .iter()
        .filter_map(|logged| match &logged.event {
            SessionEvent::RewindApplied {
                replaced: Some(replaced),
                ..
            } => Some(*replaced),
            _ => None,
        })
        .collect();
    let mut replay = Vec::new();
    for logged in events {
        if shadowed.iter().any(|range| range.contains(logged.seq)) {
            continue;
        }
        match &logged.event {
            SessionEvent::UserMessage {
                source: InputSource::User | InputSource::Steering,
                content,
            } => {
                let text = join_text_blocks(content);
                if !text.is_empty() {
                    replay.push(ReplayEntry::User { text });
                }
            }
            SessionEvent::AssistantMessage { content, .. } => {
                let text = join_text_blocks(content);
                let reasoning = content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Reasoning(text) => Some(text.as_str()),
                        ContentBlock::Text(_) => None,
                    })
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                if !text.is_empty() || !reasoning.is_empty() {
                    replay.push(ReplayEntry::Assistant { text, reasoning });
                }
            }
            SessionEvent::ToolCall {
                call_id,
                name,
                args,
            } => {
                if name == QUESTION_TOOL {
                    continue;
                }
                replay.push(ReplayEntry::ToolCall(ReplayToolCall {
                    call_id: call_id.to_string(),
                    name: name.clone(),
                    args: args.clone(),
                    status: ToolCallStatus::Running,
                    output: None,
                }));
            }
            SessionEvent::ToolResult { call_id, output } => {
                let call_id = call_id.to_string();
                let status = observation_status(output.status);
                let rendered = output.render();
                let existing = replay.iter_mut().rev().find_map(|entry| match entry {
                    ReplayEntry::ToolCall(entry) if entry.call_id == call_id => Some(entry),
                    _ => None,
                });
                if let Some(entry) = existing {
                    entry.status = status;
                    if !rendered.is_empty() {
                        entry.output = Some(rendered);
                    }
                }
            }
            SessionEvent::CompactionSummary { .. } => {
                replay.push(ReplayEntry::System {
                    text: "Compacted history".to_string(),
                });
            }
            SessionEvent::QuestionAsked {
                request_id,
                questions,
            } => {
                replay.push(ReplayEntry::Question(ReplayQuestion {
                    request_id: request_id.to_string(),
                    questions: questions.clone(),
                    answers: None,
                    resolved: false,
                }));
            }
            SessionEvent::QuestionAnswered {
                request_id,
                answers,
            } => {
                let request_id = request_id.to_string();
                if let Some(ReplayEntry::Question(question)) = replay.iter_mut().rev().find(
                    |entry| matches!(entry, ReplayEntry::Question(q) if q.request_id == request_id),
                ) {
                    question.answers = answers.clone();
                    question.resolved = true;
                }
            }
            _ => {}
        }
    }
    for entry in &mut replay {
        if let ReplayEntry::ToolCall(entry) = entry
            && entry.status == ToolCallStatus::Running
        {
            entry.status = ToolCallStatus::Cancelled;
            entry.output = None;
        }
    }
    for entry in &mut replay {
        if let ReplayEntry::ToolCall(tc) = entry
            && matches!(tc.name.as_str(), "subagent-run" | "acp-run")
            && let Some(child_id) = tc.output.as_deref().and_then(parse_child_session_hint)
        {
            let agent = serde_json::from_str::<serde_json::Value>(&tc.args)
                .ok()
                .and_then(|value| {
                    value
                        .get("agent")
                        .and_then(|agent| agent.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "subagent".to_string());
            let name = serde_json::from_str::<serde_json::Value>(&tc.args)
                .ok()
                .and_then(|value| {
                    value
                        .get("name")
                        .and_then(|name| name.as_str())
                        .map(str::to_string)
                });
            *entry = ReplayEntry::Subagent(ReplaySubagent {
                id: tc.call_id.clone(),
                name,
                agent,
                status: tc.status,
                report: tc.output.clone(),
                child_session_id: Some(child_id),
            });
        }
    }
    replay
}

pub fn session_usage_from_events(events: &[LoggedEvent]) -> Option<Usage> {
    events.iter().rev().find_map(|logged| match &logged.event {
        SessionEvent::UsageUpdated { cumulative, .. } => Some(*cumulative),
        _ => None,
    })
}

fn parse_child_session_hint(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("Hint: child_session_id: ")
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
    })
}

fn observation_status(status: ObservationStatus) -> ToolCallStatus {
    match status {
        ObservationStatus::Success => ToolCallStatus::Succeeded,
        ObservationStatus::Error | ObservationStatus::Refused => ToolCallStatus::Failed,
        ObservationStatus::Stopped => ToolCallStatus::Stopped,
    }
}

#[cfg(test)]
mod tests {
    use meerai_core::ToolObservation;

    use super::*;
    use crate::{
        CallId, CompactionId, QuestionOption, RequestId, RewindScope, Seq, StreamDigest, TurnId,
        TurnTrigger,
    };

    #[test]
    fn replays_known_entries_and_skips_runtime_events() {
        let events = vec![
            LoggedEvent {
                seq: Seq(1),
                event: SessionEvent::SystemMessage {
                    sections: Vec::new(),
                    generation: 0,
                },
            },
            LoggedEvent {
                seq: Seq(2),
                event: SessionEvent::UserMessage {
                    source: InputSource::RuntimeContext,
                    content: vec![ContentBlock::Text("runtime context".to_string())],
                },
            },
            LoggedEvent {
                seq: Seq(3),
                event: SessionEvent::UserMessage {
                    source: InputSource::User,
                    content: vec![ContentBlock::Text("hello".to_string())],
                },
            },
            LoggedEvent {
                seq: Seq(4),
                event: SessionEvent::AssistantMessage {
                    provider: "gemini".to_string(),
                    model: "gemini-3.8-flash".to_string(),
                    content: vec![
                        ContentBlock::Reasoning("thinking".to_string()),
                        ContentBlock::Text("answer".to_string()),
                    ],
                    tool_calls: Vec::new(),
                    call_signatures: Vec::new(),
                    usage: Usage::default(),
                    stream_digest: StreamDigest::default(),
                },
            },
            LoggedEvent {
                seq: Seq(5),
                event: SessionEvent::ToolCall {
                    call_id: CallId::from_raw("call-1"),
                    name: "code-read_file".to_string(),
                    args: "{\"path\":\"src/lib.rs\"}".to_string(),
                },
            },
            LoggedEvent {
                seq: Seq(6),
                event: SessionEvent::ToolResult {
                    call_id: CallId::from_raw("call-1"),
                    output: ToolObservation::success("file contents"),
                },
            },
            LoggedEvent {
                seq: Seq(7),
                event: SessionEvent::ToolCall {
                    call_id: CallId::from_raw("call-2"),
                    name: "code-shell".to_string(),
                    args: "{}".to_string(),
                },
            },
            LoggedEvent {
                seq: Seq(8),
                event: SessionEvent::CompactionSummary {
                    compaction_id: CompactionId::from_raw("cmp-1"),
                    replaced: SeqRange {
                        start: Seq(1),
                        end_inclusive: Seq(4),
                    },
                    summary: "summary".to_string(),
                },
            },
            LoggedEvent {
                seq: Seq(9),
                event: SessionEvent::StepStarted {
                    step_id: crate::StepId::from_raw("step-1"),
                    turn_id: TurnId::from_raw("turn-1"),
                },
            },
        ];

        assert_eq!(
            replay_from_events(&events),
            vec![
                ReplayEntry::User {
                    text: "hello".to_string(),
                },
                ReplayEntry::Assistant {
                    text: "answer".to_string(),
                    reasoning: "thinking".to_string(),
                },
                ReplayEntry::ToolCall(ReplayToolCall {
                    call_id: "call-1".to_string(),
                    name: "code-read_file".to_string(),
                    args: "{\"path\":\"src/lib.rs\"}".to_string(),
                    status: ToolCallStatus::Succeeded,
                    output: Some("file contents".to_string()),
                }),
                ReplayEntry::ToolCall(ReplayToolCall {
                    call_id: "call-2".to_string(),
                    name: "code-shell".to_string(),
                    args: "{}".to_string(),
                    status: ToolCallStatus::Cancelled,
                    output: None,
                }),
                ReplayEntry::System {
                    text: "Compacted history".to_string(),
                },
            ]
        );
    }

    #[test]
    fn marks_unmatched_tool_calls_cancelled() {
        let events = vec![LoggedEvent {
            seq: Seq(1),
            event: SessionEvent::ToolCall {
                call_id: CallId::from_raw("call-crashed"),
                name: "code-shell".to_string(),
                args: "{}".to_string(),
            },
        }];

        assert_eq!(
            replay_from_events(&events),
            vec![ReplayEntry::ToolCall(ReplayToolCall {
                call_id: "call-crashed".to_string(),
                name: "code-shell".to_string(),
                args: "{}".to_string(),
                status: ToolCallStatus::Cancelled,
                output: None,
            })]
        );
    }

    #[test]
    fn excludes_shadowed_events() {
        let events = vec![
            LoggedEvent {
                seq: Seq(1),
                event: SessionEvent::UserMessage {
                    source: InputSource::User,
                    content: vec![ContentBlock::Text("keep prompt".to_string())],
                },
            },
            LoggedEvent {
                seq: Seq(2),
                event: SessionEvent::AssistantMessage {
                    provider: "gemini".to_string(),
                    model: "gemini-3.8-flash".to_string(),
                    content: vec![ContentBlock::Text("keep reply".to_string())],
                    tool_calls: Vec::new(),
                    call_signatures: Vec::new(),
                    usage: Usage::default(),
                    stream_digest: StreamDigest::default(),
                },
            },
            LoggedEvent {
                seq: Seq(3),
                event: SessionEvent::TurnStarted {
                    turn_id: TurnId::from_raw("turn-reverted"),
                    trigger: TurnTrigger::UserPrompt,
                },
            },
            LoggedEvent {
                seq: Seq(4),
                event: SessionEvent::UserMessage {
                    source: InputSource::User,
                    content: vec![ContentBlock::Text("reverted prompt".to_string())],
                },
            },
            LoggedEvent {
                seq: Seq(5),
                event: SessionEvent::ToolCall {
                    call_id: CallId::from_raw("call-reverted"),
                    name: "code-write_file".to_string(),
                    args: "{}".to_string(),
                },
            },
            LoggedEvent {
                seq: Seq(6),
                event: SessionEvent::ToolResult {
                    call_id: CallId::from_raw("call-reverted"),
                    output: ToolObservation::success("written"),
                },
            },
            LoggedEvent {
                seq: Seq(7),
                event: SessionEvent::AssistantMessage {
                    provider: "gemini".to_string(),
                    model: "gemini-3.8-flash".to_string(),
                    content: vec![ContentBlock::Text("reverted reply".to_string())],
                    tool_calls: Vec::new(),
                    call_signatures: Vec::new(),
                    usage: Usage::default(),
                    stream_digest: StreamDigest::default(),
                },
            },
            LoggedEvent {
                seq: Seq(8),
                event: SessionEvent::RewindApplied {
                    turn_id: TurnId::from_raw("turn-reverted"),
                    scope: RewindScope::Conversation,
                    restored: Vec::new(),
                    replaced: Some(SeqRange {
                        start: Seq(3),
                        end_inclusive: Seq(7),
                    }),
                },
            },
            LoggedEvent {
                seq: Seq(9),
                event: SessionEvent::CompactionSummary {
                    compaction_id: CompactionId::from_raw("cmp-keep"),
                    replaced: SeqRange {
                        start: Seq(1),
                        end_inclusive: Seq(2),
                    },
                    summary: "summary".to_string(),
                },
            },
            LoggedEvent {
                seq: Seq(10),
                event: SessionEvent::UserMessage {
                    source: InputSource::User,
                    content: vec![ContentBlock::Text("after revert".to_string())],
                },
            },
        ];

        assert_eq!(
            replay_from_events(&events),
            vec![
                ReplayEntry::User {
                    text: "keep prompt".to_string(),
                },
                ReplayEntry::Assistant {
                    text: "keep reply".to_string(),
                    reasoning: String::new(),
                },
                ReplayEntry::System {
                    text: "Compacted history".to_string(),
                },
                ReplayEntry::User {
                    text: "after revert".to_string(),
                },
            ]
        );
    }

    #[test]
    fn converts_subagent_run_with_child_hint() {
        let events = vec![
            LoggedEvent {
                seq: Seq(1),
                event: SessionEvent::ToolCall {
                    call_id: CallId::from_raw("sub-call"),
                    name: "subagent-run".to_string(),
                    args: "{\"agent\":\"explore\",\"task\":\"find it\",\"name\":\"parser scan\"}"
                        .to_string(),
                },
            },
            LoggedEvent {
                seq: Seq(2),
                event: SessionEvent::ToolResult {
                    call_id: CallId::from_raw("sub-call"),
                    output: ToolObservation {
                        hint: Some("child_session_id: ses-child-replay".to_string()),
                        ..ToolObservation::success("found it")
                    },
                },
            },
        ];

        assert_eq!(
            replay_from_events(&events),
            vec![ReplayEntry::Subagent(ReplaySubagent {
                id: "sub-call".to_string(),
                name: Some("parser scan".to_string()),
                agent: "explore".to_string(),
                status: ToolCallStatus::Succeeded,
                report: Some("found it\nHint: child_session_id: ses-child-replay".to_string()),
                child_session_id: Some("ses-child-replay".to_string()),
            })]
        );
    }

    #[test]
    fn converts_acp_run_with_child_hint() {
        let events = vec![
            LoggedEvent {
                seq: Seq(1),
                event: SessionEvent::ToolCall {
                    call_id: CallId::from_raw("acp-call"),
                    name: "acp-run".to_string(),
                    args: "{\"agent\":\"agy\",\"task\":\"fix the test\"}".to_string(),
                },
            },
            LoggedEvent {
                seq: Seq(2),
                event: SessionEvent::ToolResult {
                    call_id: CallId::from_raw("acp-call"),
                    output: ToolObservation {
                        hint: Some("child_session_id: ses-acp-child".to_string()),
                        ..ToolObservation::success("test fixed")
                    },
                },
            },
        ];

        assert_eq!(
            replay_from_events(&events),
            vec![ReplayEntry::Subagent(ReplaySubagent {
                id: "acp-call".to_string(),
                name: None,
                agent: "agy".to_string(),
                status: ToolCallStatus::Succeeded,
                report: Some("test fixed\nHint: child_session_id: ses-acp-child".to_string()),
                child_session_id: Some("ses-acp-child".to_string()),
            })]
        );
    }

    #[test]
    fn session_usage_from_events_returns_last_cumulative_usage() {
        let events = vec![
            LoggedEvent {
                seq: Seq(1),
                event: SessionEvent::UsageUpdated {
                    usage: Usage::default(),
                    cumulative: Usage {
                        prompt_tokens: Some(10),
                        completion_tokens: Some(5),
                        total_tokens: Some(15),
                        cached_tokens: None,
                        cache_creation_tokens: None,
                    },
                },
            },
            LoggedEvent {
                seq: Seq(2),
                event: SessionEvent::UsageUpdated {
                    usage: Usage::default(),
                    cumulative: Usage {
                        prompt_tokens: Some(20),
                        completion_tokens: Some(7),
                        total_tokens: Some(27),
                        cached_tokens: None,
                        cache_creation_tokens: None,
                    },
                },
            },
        ];

        assert_eq!(
            session_usage_from_events(&events),
            Some(Usage {
                prompt_tokens: Some(20),
                completion_tokens: Some(7),
                total_tokens: Some(27),
                cached_tokens: None,
                cache_creation_tokens: None,
            })
        );
        assert_eq!(session_usage_from_events(&[]), None);
    }

    #[test]
    fn replays_questions_with_answers_and_skips_the_tool_row() {
        let request_id = RequestId::from_raw("req-1");
        let questions = vec![QuestionPrompt {
            header: "Update scope".to_string(),
            question: "Code or docs?".to_string(),
            options: vec![QuestionOption {
                label: "Code + docs".to_string(),
                description: None,
            }],
            multiple: false,
        }];
        let answers = vec![QuestionAnswer {
            header: "Update scope".to_string(),
            selected: vec!["Code + docs".to_string()],
        }];
        let events = vec![
            LoggedEvent {
                seq: Seq(1),
                event: SessionEvent::ToolCall {
                    call_id: CallId::from_raw("call-q"),
                    name: "question-ask".to_string(),
                    args: "{}".to_string(),
                },
            },
            LoggedEvent {
                seq: Seq(2),
                event: SessionEvent::QuestionAsked {
                    request_id: request_id.clone(),
                    questions: questions.clone(),
                },
            },
            LoggedEvent {
                seq: Seq(3),
                event: SessionEvent::QuestionAnswered {
                    request_id,
                    answers: Some(answers.clone()),
                },
            },
        ];
        assert_eq!(
            replay_from_events(&events),
            vec![ReplayEntry::Question(ReplayQuestion {
                request_id: "req-1".to_string(),
                questions,
                answers: Some(answers),
                resolved: true,
            })]
        );
    }
}
