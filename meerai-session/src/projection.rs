use genai::chat::{
    ChatMessage, ContentPart, MessageContent, ToolCall as GenaiToolCall, ToolResponse,
};
use meerai_core::ToolObservation;
use serde::{Deserialize, Serialize};

use crate::compaction::{COMPACTED_MARKER, summary_message};
use crate::context::{pinned_turns, turn_is_pinned};
use crate::event::{ContentBlock, InputSource, LoggedEvent, SeqRange, SessionEvent};
use crate::ids::{CallId, TurnId};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectionPolicy {
    pub strip_ephemera: bool,
    pub prune_reasoning: bool,
    pub verbatim_window_turns: usize,
    pub clear_old_tool_results: bool,
    pub include_system_messages: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned_turns: Vec<TurnId>,
}

impl Default for ProjectionPolicy {
    fn default() -> Self {
        Self {
            strip_ephemera: true,
            prune_reasoning: true,
            verbatim_window_turns: 4,
            clear_old_tool_results: true,
            include_system_messages: true,
            pinned_turns: Vec::new(),
        }
    }
}

pub fn derive_messages(events: &[LoggedEvent], policy: &ProjectionPolicy) -> Vec<ChatMessage> {
    let shadowed = shadowed_ranges(events);
    let summaries = pending_summaries(events, &shadowed);
    let pinned = pinned_turns(events);
    let pinned_turns: Vec<&TurnId> = pinned.iter().chain(policy.pinned_turns.iter()).collect();
    let current_turn_start = events
        .iter()
        .rev()
        .find(|logged| matches!(logged.event, SessionEvent::TurnStarted { .. }))
        .map(|logged| logged.seq);
    let turn_count = events
        .iter()
        .filter(|logged| !shadowed.iter().any(|range| range.contains(logged.seq)))
        .filter(|logged| matches!(logged.event, SessionEvent::TurnStarted { .. }))
        .count();
    let spills = spill_paths(events, &shadowed);
    let mut turn_index: Option<usize> = None;
    let mut current_turn: Option<TurnId> = None;
    let mut next_summary = 0;
    let mut messages = Vec::new();
    let mut latest_calls: Vec<GenaiToolCall> = Vec::new();

    for logged in events {
        while summaries
            .get(next_summary)
            .is_some_and(|(range, _)| range.start.0 <= logged.seq.0)
        {
            messages.push(ChatMessage::user(summaries[next_summary].1.clone()));
            next_summary += 1;
        }

        if shadowed.iter().any(|range| range.contains(logged.seq)) {
            continue;
        }

        match &logged.event {
            SessionEvent::TurnStarted { turn_id, .. } => {
                turn_index = Some(turn_index.map_or(0, |index| index + 1));
                current_turn = Some(turn_id.clone());
            }
            SessionEvent::SystemMessage { sections, .. } => {
                if !policy.include_system_messages {
                    continue;
                }
                let content = sections
                    .iter()
                    .map(|section| section.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                messages.push(ChatMessage::system(content));
            }
            SessionEvent::UserMessage { source, content } => {
                let stale_ephemera = *source == InputSource::RuntimeContext
                    && !current_turn_start.is_some_and(|start| logged.seq.0 > start.0);
                if policy.strip_ephemera && stale_ephemera {
                    continue;
                }
                let text = join_text(content, false);
                if !text.is_empty() {
                    messages.push(ChatMessage::user(text));
                }
            }
            SessionEvent::AssistantMessage {
                content,
                tool_calls,
                call_signatures,
                ..
            } => {
                latest_calls = tool_calls
                    .iter()
                    .enumerate()
                    .map(|(index, call)| {
                        let signature = call_signatures.get(index).and_then(|entry| entry.as_ref());
                        to_genai_tool_call(call, signature)
                    })
                    .collect();
                let text = join_text(content, !policy.prune_reasoning);
                if latest_calls.is_empty() {
                    if !text.is_empty() {
                        messages.push(ChatMessage::assistant(text));
                    }
                } else if text.is_empty() {
                    messages.push(ChatMessage::from(latest_calls.clone()));
                } else {
                    let mut parts = Vec::with_capacity(latest_calls.len() + 1);
                    parts.push(ContentPart::Text(text));
                    parts.extend(latest_calls.iter().cloned().map(ContentPart::ToolCall));
                    messages.push(ChatMessage::assistant(MessageContent::from_parts(parts)));
                }
            }
            SessionEvent::ToolResult { call_id, output } => {
                let Some(call) = latest_calls
                    .iter()
                    .find(|call| call.call_id.as_str() == call_id.as_str())
                else {
                    continue;
                };
                let content = tool_result_content(
                    policy,
                    turn_index,
                    turn_count,
                    &spills,
                    turn_is_pinned(&current_turn, &pinned_turns),
                    call_id,
                    output,
                );
                let response =
                    ToolResponse::from_tool_call(call, content).with_fn_name(&call.fn_name);
                messages.push(ChatMessage::from(response));
            }
            _ => {}
        }
    }

    while next_summary < summaries.len() {
        messages.push(ChatMessage::user(summaries[next_summary].1.clone()));
        next_summary += 1;
    }

    messages
}

pub(crate) fn shadowed_ranges(events: &[LoggedEvent]) -> Vec<SeqRange> {
    events
        .iter()
        .filter_map(|logged| match &logged.event {
            SessionEvent::CompactionSummary { replaced, .. } => Some(*replaced),
            SessionEvent::RewindApplied { replaced, .. } => *replaced,
            _ => None,
        })
        .collect()
}

fn pending_summaries(events: &[LoggedEvent], shadowed: &[SeqRange]) -> Vec<(SeqRange, String)> {
    let mut summaries: Vec<(SeqRange, String)> = events
        .iter()
        .filter(|logged| !shadowed.iter().any(|range| range.contains(logged.seq)))
        .filter_map(|logged| match &logged.event {
            SessionEvent::CompactionSummary {
                replaced, summary, ..
            } => Some((*replaced, render_summary(summary))),
            _ => None,
        })
        .collect();
    summaries.sort_by_key(|(range, _)| range.start.0);
    summaries
}

fn render_summary(summary: &str) -> String {
    if summary.trim_start().starts_with(COMPACTED_MARKER) {
        summary.to_string()
    } else {
        summary_message(summary)
    }
}

fn to_genai_tool_call(
    call: &meerai_core::ToolCall,
    signature: Option<&Vec<String>>,
) -> GenaiToolCall {
    GenaiToolCall {
        call_id: call.id.clone(),
        fn_name: call.name.clone(),
        fn_arguments: serde_json::from_str(&call.args)
            .unwrap_or_else(|_| serde_json::Value::String(call.args.clone())),
        thought_signatures: signature.cloned(),
    }
}

fn spill_paths(events: &[LoggedEvent], shadowed: &[SeqRange]) -> Vec<(CallId, String, u64)> {
    events
        .iter()
        .filter(|logged| !shadowed.iter().any(|range| range.contains(logged.seq)))
        .filter_map(|logged| match &logged.event {
            SessionEvent::SpillApplied {
                call_id,
                bytes_omitted,
                blob_key,
            } => Some((call_id.clone(), blob_key.clone(), *bytes_omitted)),
            _ => None,
        })
        .collect()
}

fn tool_result_content(
    policy: &ProjectionPolicy,
    turn_index: Option<usize>,
    turn_count: usize,
    spills: &[(CallId, String, u64)],
    pinned: bool,
    call_id: &CallId,
    output: &ToolObservation,
) -> String {
    let clears = policy.clear_old_tool_results
        && !pinned
        && turn_index.is_some_and(|index| {
            turn_count.saturating_sub(index) > policy.verbatim_window_turns.max(1)
        });
    if !clears {
        return output.render();
    }
    let spill = spills.iter().find(|(id, _, _)| id == call_id);
    let status = format!("{:?}", output.status).to_lowercase();
    let mut text = match spill {
        Some((_, key, omitted)) => format!(
            "[tool result cleared from an earlier turn: {status}, {omitted} bytes omitted; full output stored as blob {key}; re-run the tool if needed]"
        ),
        None => format!(
            "[tool result cleared from an earlier turn: {status}, {} bytes omitted; re-run the tool if needed]",
            output.stdout.len().saturating_add(output.stderr.len())
        ),
    };
    if let Some(hint) = output.hint.as_deref().filter(|hint| !hint.is_empty()) {
        text.push_str("\nhint: ");
        text.push_str(hint);
    }
    text
}

pub fn join_text_blocks(blocks: &[ContentBlock]) -> String {
    join_text(blocks, false)
}

fn join_text(blocks: &[ContentBlock], keep_reasoning: bool) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            ContentBlock::Reasoning(text) if keep_reasoning => Some(text.as_str()),
            ContentBlock::Reasoning(_) => None,
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use genai::chat::ChatRole;
    use meerai_core::{ToolCall, ToolObservation};

    use super::*;
    use crate::event::{
        PromptSection, RewindScope, Seq, StreamDigest, TurnEndReason, TurnTrigger, Usage,
    };
    use crate::ids::{CallId, TurnId};

    fn logged(seq: u64, event: SessionEvent) -> LoggedEvent {
        LoggedEvent {
            seq: Seq(seq),
            event,
        }
    }

    fn system(sections: &[&str]) -> SessionEvent {
        SessionEvent::SystemMessage {
            sections: sections
                .iter()
                .map(|content| PromptSection {
                    label: "section".to_string(),
                    content: content.to_string(),
                })
                .collect(),
            generation: 0,
        }
    }

    fn user(text: &str) -> SessionEvent {
        SessionEvent::UserMessage {
            source: InputSource::User,
            content: vec![ContentBlock::Text(text.to_string())],
        }
    }

    fn runtime_context(text: &str) -> SessionEvent {
        SessionEvent::UserMessage {
            source: InputSource::RuntimeContext,
            content: vec![ContentBlock::Text(text.to_string())],
        }
    }

    fn assistant(content: Vec<ContentBlock>, tool_calls: Vec<ToolCall>) -> SessionEvent {
        SessionEvent::AssistantMessage {
            provider: "test".to_string(),
            model: "test".to_string(),
            content,
            tool_calls,
            call_signatures: Vec::new(),
            usage: Usage::default(),
            stream_digest: StreamDigest::default(),
        }
    }

    fn tool_call(id: &str, name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            args: args.to_string(),
        }
    }

    fn tool_result(id: &str, output: ToolObservation) -> SessionEvent {
        SessionEvent::ToolResult {
            call_id: CallId::from(id),
            output,
        }
    }

    fn turn_started(seq: u64, turn_id: &TurnId) -> LoggedEvent {
        logged(
            seq,
            SessionEvent::TurnStarted {
                turn_id: turn_id.clone(),
                trigger: TurnTrigger::UserPrompt,
            },
        )
    }

    fn turn_ended(seq: u64, turn_id: &TurnId) -> LoggedEvent {
        logged(
            seq,
            SessionEvent::TurnEnded {
                turn_id: turn_id.clone(),
                reason: TurnEndReason::Completed,
            },
        )
    }

    fn render(messages: &[ChatMessage]) -> String {
        serde_json::to_string_pretty(messages).unwrap()
    }

    fn plain_fixture() -> Vec<LoggedEvent> {
        let turn = TurnId::from("turn-1");
        vec![
            turn_started(1, &turn),
            logged(2, system(&["you are meerai", "follow the rules"])),
            logged(3, user("read a then answer")),
            logged(
                4,
                assistant(
                    Vec::new(),
                    vec![tool_call("call-1", "code-read_file", r#"{"path":"a"}"#)],
                ),
            ),
            logged(
                5,
                tool_result("call-1", ToolObservation::success("alpha\n")),
            ),
            logged(
                6,
                assistant(vec![ContentBlock::Text("done".to_string())], Vec::new()),
            ),
            turn_ended(7, &turn),
        ]
    }

    #[test]
    fn projects_plain_transcript() {
        let events = vec![
            logged(1, system(&["one", "two"])),
            logged(2, user("hello")),
            logged(
                3,
                assistant(vec![ContentBlock::Text("hi".to_string())], vec![]),
            ),
        ];

        let messages = derive_messages(&events, &ProjectionPolicy::default());

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, ChatRole::System);
        assert_eq!(messages[0].content.first_text(), Some("one\n\ntwo"));
        assert_eq!(messages[1].role, ChatRole::User);
        assert_eq!(messages[1].content.first_text(), Some("hello"));
        assert_eq!(messages[2].role, ChatRole::Assistant);
        assert_eq!(messages[2].content.first_text(), Some("hi"));
    }

    #[test]
    fn strips_ephemeral_runtime_context() {
        let events = vec![
            logged(1, runtime_context("file tree")),
            logged(2, user("go")),
        ];

        let messages = derive_messages(&events, &ProjectionPolicy::default());
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content.first_text(), Some("go"));

        let policy = ProjectionPolicy {
            strip_ephemera: false,
            ..ProjectionPolicy::default()
        };
        let messages = derive_messages(&events, &policy);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn prunes_reasoning_from_assistant_messages() {
        let events = vec![logged(
            1,
            assistant(
                vec![
                    ContentBlock::Text("answer".to_string()),
                    ContentBlock::Reasoning("thinking".to_string()),
                ],
                vec![],
            ),
        )];

        let messages = derive_messages(&events, &ProjectionPolicy::default());
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content.first_text(), Some("answer"));

        let policy = ProjectionPolicy {
            prune_reasoning: false,
            ..ProjectionPolicy::default()
        };
        let messages = derive_messages(&events, &policy);
        assert_eq!(messages[0].content.first_text(), Some("answer\n\nthinking"));
    }

    #[test]
    fn replays_call_signatures_onto_tool_calls() {
        let events = vec![logged(
            1,
            SessionEvent::AssistantMessage {
                provider: "gemini".to_string(),
                model: "gemini-2.5-pro".to_string(),
                content: Vec::new(),
                tool_calls: vec![
                    tool_call("call-1", "code-read_file", "{}"),
                    tool_call("call-2", "code-grep", "{}"),
                ],
                call_signatures: vec![Some(vec!["sig-1".to_string()]), None],
                usage: Usage::default(),
                stream_digest: StreamDigest::default(),
            },
        )];

        let messages = derive_messages(&events, &ProjectionPolicy::default());
        let calls = messages[0].content.tool_calls();
        assert_eq!(
            calls[0].thought_signatures.as_deref(),
            Some(&["sig-1".to_string()][..])
        );
        assert_eq!(calls[1].thought_signatures, None);
    }

    #[test]
    fn skips_empty_assistant_messages() {
        let events = vec![
            logged(
                1,
                assistant(vec![ContentBlock::Reasoning("only".to_string())], vec![]),
            ),
            logged(2, assistant(vec![], vec![])),
        ];

        let messages = derive_messages(&events, &ProjectionPolicy::default());
        assert!(messages.is_empty());
    }

    #[test]
    fn skips_system_messages_when_disabled() {
        let events = vec![logged(1, system(&["rules"])), logged(2, user("go"))];

        let policy = ProjectionPolicy {
            include_system_messages: false,
            ..ProjectionPolicy::default()
        };
        let messages = derive_messages(&events, &policy);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, ChatRole::User);
    }

    #[test]
    fn pairs_tool_results_with_latest_assistant_calls() {
        let events = vec![
            logged(1, user("read a")),
            logged(
                2,
                assistant(
                    vec![],
                    vec![tool_call("call-1", "code-read_file", r#"{"path":"a"}"#)],
                ),
            ),
            logged(
                3,
                tool_result("call-1", ToolObservation::success("content")),
            ),
        ];

        let messages = derive_messages(&events, &ProjectionPolicy::default());

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1].role, ChatRole::Assistant);
        let calls = messages[1].content.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call_id, "call-1");
        assert_eq!(calls[0].fn_name, "code-read_file");
        assert_eq!(calls[0].fn_arguments, serde_json::json!({"path": "a"}));
        assert_eq!(messages[2].role, ChatRole::Tool);
        let responses = messages[2].content.tool_responses();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].call_id, "call-1");
        assert_eq!(responses[0].fn_name.as_deref(), Some("code-read_file"));
        assert_eq!(responses[0].content, "content");
    }

    #[test]
    fn keeps_assistant_text_before_tool_calls() {
        let events = vec![logged(
            1,
            assistant(
                vec![ContentBlock::Text("plan: read a".to_string())],
                vec![tool_call("call-1", "code-read_file", r#"{"path":"a"}"#)],
            ),
        )];

        let messages = derive_messages(&events, &ProjectionPolicy::default());

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, ChatRole::Assistant);
        let parts = messages[0].content.parts();
        assert_eq!(parts.len(), 2);
        assert!(matches!(&parts[0], ContentPart::Text(text) if text == "plan: read a"));
        assert!(
            matches!(&parts[1], ContentPart::ToolCall(call) if call.call_id == "call-1"),
            "the tool call must follow the text"
        );
    }

    #[test]
    fn pruned_reasoning_leaves_bare_tool_calls() {
        let events = vec![logged(
            1,
            assistant(
                vec![ContentBlock::Reasoning("think".to_string())],
                vec![tool_call("call-1", "code-read_file", r#"{"path":"a"}"#)],
            ),
        )];
        let policy = ProjectionPolicy {
            prune_reasoning: true,
            ..ProjectionPolicy::default()
        };

        let messages = derive_messages(&events, &policy);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, ChatRole::Assistant);
        let parts = messages[0].content.parts();
        assert_eq!(parts.len(), 1);
        assert!(matches!(&parts[0], ContentPart::ToolCall(call) if call.call_id == "call-1"));
    }

    #[test]
    fn skips_orphan_tool_results() {
        let events = vec![logged(
            1,
            tool_result("call-missing", ToolObservation::success("content")),
        )];

        let messages = derive_messages(&events, &ProjectionPolicy::default());
        assert!(messages.is_empty());
    }

    fn multi_turn_results(turns: usize) -> Vec<LoggedEvent> {
        let mut events = Vec::new();
        let mut seq = 1;
        for index in 0..turns {
            let turn = TurnId::from(format!("turn-{index}").as_str());
            events.push(turn_started(seq, &turn));
            seq += 1;
            events.push(logged(seq, user(&format!("request {index}"))));
            seq += 1;
            events.push(logged(
                seq,
                assistant(
                    Vec::new(),
                    vec![tool_call(&format!("call-{index}"), "code-read_file", "{}")],
                ),
            ));
            seq += 1;
            events.push(logged(
                seq,
                tool_result(
                    &format!("call-{index}"),
                    ToolObservation::success(format!("output-{index}")),
                ),
            ));
            seq += 1;
            events.push(turn_ended(seq, &turn));
            seq += 1;
        }
        events
    }

    fn tool_response_contents(messages: &[ChatMessage]) -> Vec<String> {
        messages
            .iter()
            .flat_map(|message| {
                message
                    .content
                    .tool_responses()
                    .into_iter()
                    .map(|response| response.content.clone())
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn clears_tool_results_outside_the_verbatim_window() {
        let events = multi_turn_results(6);
        let messages = derive_messages(&events, &ProjectionPolicy::default());
        let responses = tool_response_contents(&messages);

        assert_eq!(responses.len(), 6);
        assert!(responses[0].contains("cleared from an earlier turn"));
        assert!(responses[1].contains("cleared from an earlier turn"));
        assert_eq!(responses[2], "output-2");
        assert_eq!(responses[5], "output-5");
        assert_eq!(
            messages
                .iter()
                .flat_map(|message| message.content.tool_calls())
                .count(),
            6,
            "tool calls must stay paired after their result is cleared"
        );
    }

    #[test]
    fn tool_result_clearing_can_be_disabled() {
        let events = multi_turn_results(6);
        let policy = ProjectionPolicy {
            clear_old_tool_results: false,
            ..ProjectionPolicy::default()
        };
        let responses = tool_response_contents(&derive_messages(&events, &policy));
        assert_eq!(responses[0], "output-0");
        assert_eq!(responses[5], "output-5");
    }

    #[test]
    fn cleared_tool_result_points_at_the_spill_blob() {
        let mut events = multi_turn_results(5);
        events.push(logged(
            events.len() as u64 + 1,
            SessionEvent::SpillApplied {
                call_id: CallId::from("call-0"),
                bytes_omitted: 900,
                blob_key: "/tmp/blob-0.txt".to_string(),
            },
        ));

        let messages = derive_messages(&events, &ProjectionPolicy::default());
        let responses = tool_response_contents(&messages);
        assert!(responses[0].contains("900 bytes omitted"));
        assert!(responses[0].contains("/tmp/blob-0.txt"));
        assert_eq!(responses[1], "output-1");
    }

    #[test]
    fn zero_window_keeps_only_the_newest_turn() {
        let events = multi_turn_results(4);
        let policy = ProjectionPolicy {
            verbatim_window_turns: 0,
            ..ProjectionPolicy::default()
        };
        let responses = tool_response_contents(&derive_messages(&events, &policy));
        assert!(responses[0].contains("cleared"));
        assert!(responses[2].contains("cleared"));
        assert_eq!(responses[3], "output-3");
    }

    #[test]
    fn keeps_unparsable_tool_arguments_as_text() {
        let events = vec![logged(
            1,
            assistant(vec![], vec![tool_call("call-1", "shell", "not json")]),
        )];

        let messages = derive_messages(&events, &ProjectionPolicy::default());
        let calls = messages[0].content.tool_calls();

        assert_eq!(
            calls[0].fn_arguments,
            serde_json::Value::String("not json".to_string())
        );
    }

    #[test]
    fn failed_attempts_project_nothing() {
        let events = vec![
            logged(1, user("go")),
            logged(
                2,
                SessionEvent::AssistantAttempt {
                    provider: "test".to_string(),
                    model: "test".to_string(),
                    error: "rate limited".to_string(),
                    usage: Usage::default(),
                },
            ),
        ];

        let messages = derive_messages(&events, &ProjectionPolicy::default());
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, ChatRole::User);
    }

    #[test]
    fn compaction_shadows_replaced_range() {
        let events = vec![
            logged(1, user("first request")),
            logged(
                2,
                assistant(vec![ContentBlock::Text("first reply".to_string())], vec![]),
            ),
            logged(3, user("second request")),
            logged(
                4,
                SessionEvent::CompactionSummary {
                    compaction_id: crate::ids::CompactionId::from("cmp-1"),
                    replaced: SeqRange {
                        start: Seq(1),
                        end_inclusive: Seq(2),
                    },
                    summary: "Goal: ship the parser.\nCompleted: first two turns.".to_string(),
                },
            ),
            logged(5, user("third request")),
        ];

        let messages = derive_messages(&events, &ProjectionPolicy::default());

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, ChatRole::User);
        let summary = messages[0].content.first_text().unwrap();
        assert!(summary.starts_with(COMPACTED_MARKER));
        assert!(summary.contains("ship the parser"));
        assert_eq!(messages[1].content.first_text(), Some("second request"));
        assert_eq!(messages[2].content.first_text(), Some("third request"));
    }

    #[test]
    fn rewind_shadows_later_turns_without_deleting_events() {
        let turn_a = TurnId::from("turn-a");
        let turn_b = TurnId::from("turn-b");
        let events = vec![
            turn_started(1, &turn_a),
            logged(2, user("keep me")),
            turn_ended(3, &turn_a),
            turn_started(4, &turn_b),
            logged(5, user("drop me")),
            logged(
                6,
                assistant(
                    vec![ContentBlock::Text("dropped reply".to_string())],
                    Vec::new(),
                ),
            ),
            turn_ended(7, &turn_b),
            logged(
                8,
                SessionEvent::RewindApplied {
                    turn_id: turn_b.clone(),
                    scope: RewindScope::Conversation,
                    restored: Vec::new(),
                    replaced: Some(SeqRange {
                        start: Seq(4),
                        end_inclusive: Seq(7),
                    }),
                },
            ),
        ];

        let messages = derive_messages(&events, &ProjectionPolicy::default());

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content.first_text(), Some("keep me"));
        assert_eq!(events.len(), 8);
    }

    #[test]
    fn shadowing_skips_tool_pairs_inside_range() {
        let events = vec![
            logged(1, user("keep")),
            logged(
                2,
                assistant(
                    Vec::new(),
                    vec![tool_call("call-1", "code-read_file", r#"{"path":"a"}"#)],
                ),
            ),
            logged(3, tool_result("call-1", ToolObservation::success("alpha"))),
            logged(
                4,
                SessionEvent::RewindApplied {
                    turn_id: TurnId::from("turn-a"),
                    scope: RewindScope::Conversation,
                    restored: Vec::new(),
                    replaced: Some(SeqRange {
                        start: Seq(2),
                        end_inclusive: Seq(3),
                    }),
                },
            ),
            logged(5, user("after")),
        ];

        let messages = derive_messages(&events, &ProjectionPolicy::default());

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content.first_text(), Some("keep"));
        assert_eq!(messages[1].content.first_text(), Some("after"));
    }

    #[tokio::test]
    async fn ephemera_scope_is_the_current_turn() {
        let turn_a = TurnId::from("turn-a");
        let turn_b = TurnId::from("turn-b");
        let events = vec![
            turn_started(1, &turn_a),
            logged(2, runtime_context("turn one context")),
            logged(3, user("one")),
            turn_ended(4, &turn_a),
            turn_started(5, &turn_b),
            logged(6, runtime_context("turn two context")),
            logged(7, user("two")),
        ];

        let messages = derive_messages(&events, &ProjectionPolicy::default());
        let texts: Vec<&str> = messages
            .iter()
            .filter_map(|message| message.content.first_text())
            .collect();
        assert_eq!(texts, vec!["one", "turn two context", "two"]);

        let policy = ProjectionPolicy {
            strip_ephemera: false,
            ..ProjectionPolicy::default()
        };
        assert_eq!(derive_messages(&events, &policy).len(), 4);
    }

    #[test]
    fn snapshot_plain_transcript() {
        let messages = derive_messages(&plain_fixture(), &ProjectionPolicy::default());
        insta::assert_snapshot!("plain_transcript", render(&messages));
    }

    #[test]
    fn snapshot_ephemera_stripping() {
        let events = vec![
            logged(1, system(&["base rules"])),
            logged(2, runtime_context("file tree: src")),
            logged(3, user("go")),
            logged(
                4,
                assistant(vec![ContentBlock::Text("ok".to_string())], Vec::new()),
            ),
        ];
        let messages = derive_messages(&events, &ProjectionPolicy::default());
        insta::assert_snapshot!("ephemera_stripping", render(&messages));
    }

    #[test]
    fn snapshot_reasoning_pruning() {
        let events = vec![logged(
            1,
            assistant(
                vec![
                    ContentBlock::Reasoning("private thoughts".to_string()),
                    ContentBlock::Text("public answer".to_string()),
                ],
                Vec::new(),
            ),
        )];
        let messages = derive_messages(&events, &ProjectionPolicy::default());
        insta::assert_snapshot!("reasoning_pruning", render(&messages));
    }

    #[test]
    fn snapshot_compaction_shadowing() {
        let events = vec![
            logged(1, user("first request")),
            logged(
                2,
                assistant(
                    vec![ContentBlock::Text("first reply".to_string())],
                    Vec::new(),
                ),
            ),
            logged(
                3,
                SessionEvent::CompactionSummary {
                    compaction_id: crate::ids::CompactionId::from("cmp-1"),
                    replaced: SeqRange {
                        start: Seq(1),
                        end_inclusive: Seq(2),
                    },
                    summary: "Goal: ship the parser.\nCompleted: first turn.".to_string(),
                },
            ),
            logged(4, user("second request")),
        ];
        let messages = derive_messages(&events, &ProjectionPolicy::default());
        insta::assert_snapshot!("compaction_shadowing", render(&messages));
    }

    #[test]
    fn snapshot_rewind_shadowing() {
        let turn_a = TurnId::from("turn-a");
        let turn_b = TurnId::from("turn-b");
        let events = vec![
            turn_started(1, &turn_a),
            logged(2, user("keep this turn")),
            turn_ended(3, &turn_a),
            turn_started(4, &turn_b),
            logged(5, user("rewound turn")),
            turn_ended(6, &turn_b),
            logged(
                7,
                SessionEvent::RewindApplied {
                    turn_id: turn_b,
                    scope: RewindScope::Conversation,
                    restored: Vec::new(),
                    replaced: Some(SeqRange {
                        start: Seq(4),
                        end_inclusive: Seq(6),
                    }),
                },
            ),
        ];
        let messages = derive_messages(&events, &ProjectionPolicy::default());
        insta::assert_snapshot!("rewind_shadowing", render(&messages));
    }

    #[test]
    fn snapshot_replay_is_deterministic() {
        let events = plain_fixture();
        let first = render(&derive_messages(&events, &ProjectionPolicy::default()));
        let second = render(&derive_messages(&events, &ProjectionPolicy::default()));
        assert_eq!(first, second);
        insta::assert_snapshot!("replay_determinism", first);
    }
}
