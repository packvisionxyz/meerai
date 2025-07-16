use crate::event::{LoggedEvent, Seq, SeqRange, SessionEvent};
use crate::ids::{CompactionId, TurnId};

pub const COMPACTED_MARKER: &str = "[compacted-history]";
pub const COMPACTION_INTERRUPTED: &str = "compaction interrupted by process exit";

const STATE_BLOCK_HEADINGS: [&str; 5] = [
    "Goal",
    "Completed progress",
    "Modified files",
    "Key decisions",
    "Open blockers",
];

pub fn plan_compaction_boundary(
    events: &[LoggedEvent],
    verbatim_window_turns: usize,
) -> Option<SeqRange> {
    if events.is_empty() {
        return None;
    }
    let first = events.first()?;
    let turn_starts: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, logged)| matches!(logged.event, SessionEvent::TurnStarted { .. }))
        .map(|(index, _)| index)
        .collect();
    let window_start = if verbatim_window_turns == 0 {
        events.len()
    } else {
        if turn_starts.len() <= verbatim_window_turns {
            return None;
        }
        turn_starts[turn_starts.len() - verbatim_window_turns]
    };
    let candidates: Vec<usize> = events
        .iter()
        .enumerate()
        .take(window_start)
        .filter(|(_, logged)| matches!(logged.event, SessionEvent::TurnEnded { .. }))
        .map(|(index, _)| index)
        .collect();

    for index in candidates.into_iter().rev() {
        if tool_pairing_balanced(&events[..=index]) {
            return Some(SeqRange {
                start: first.seq,
                end_inclusive: events[index].seq,
            });
        }
    }

    None
}

pub fn summary_message(state_block: &str) -> String {
    let mut sections: Vec<String> = Vec::new();
    let block = state_block.trim();
    if !block.is_empty() {
        sections.push(format!("## Summary\n{block}"));
    }
    for heading in STATE_BLOCK_HEADINGS {
        sections.push(format!("## {heading}"));
    }
    format!("{COMPACTED_MARKER}\n\n{}\n", sections.join("\n\n"))
}

pub fn has_open_compaction(events: &[LoggedEvent]) -> bool {
    !open_compactions(events).is_empty()
}

pub(crate) fn open_compactions(events: &[LoggedEvent]) -> Vec<CompactionId> {
    let mut open: Vec<CompactionId> = Vec::new();
    for logged in events {
        match &logged.event {
            SessionEvent::CompactionStarted { compaction_id, .. } => {
                open.push(compaction_id.clone());
            }
            SessionEvent::CompactionEnded { compaction_id, .. } => {
                open.retain(|id| id != compaction_id);
            }
            _ => {}
        }
    }
    open
}

fn tool_pairing_balanced(prefix: &[LoggedEvent]) -> bool {
    let mut pending: Vec<String> = Vec::new();
    for logged in prefix {
        match &logged.event {
            SessionEvent::AssistantMessage { tool_calls, .. } => {
                pending.extend(tool_calls.iter().map(|call| call.id.clone()));
            }
            SessionEvent::ToolResult { call_id, .. } => {
                pending.retain(|id| id.as_str() != call_id.as_str());
            }
            _ => {}
        }
    }
    pending.is_empty()
}

pub(crate) fn open_turns(events: &[LoggedEvent]) -> Vec<TurnId> {
    let mut open: Vec<TurnId> = Vec::new();
    for logged in events {
        match &logged.event {
            SessionEvent::TurnStarted { turn_id, .. } => {
                if !open.contains(turn_id) {
                    open.push(turn_id.clone());
                }
            }
            SessionEvent::TurnEnded { turn_id, .. } => {
                open.retain(|id| id != turn_id);
            }
            _ => {}
        }
    }
    open
}

pub(crate) fn turn_start(events: &[LoggedEvent], turn_id: &TurnId) -> Option<Seq> {
    events
        .iter()
        .find(|logged| matches!(&logged.event, SessionEvent::TurnStarted { turn_id: id, .. } if id == turn_id))
        .map(|logged| logged.seq)
}

#[cfg(test)]
mod tests {
    use meerai_core::{ToolCall, ToolObservation};

    use super::*;
    use crate::event::{ContentBlock, InputSource, Seq, StreamDigest, TurnEndReason, Usage};
    use crate::ids::CallId;

    fn logged(seq: u64, event: SessionEvent) -> LoggedEvent {
        LoggedEvent {
            seq: Seq(seq),
            event,
        }
    }

    fn turn(seq: u64, turn_id: &TurnId) -> (LoggedEvent, LoggedEvent) {
        (
            logged(
                seq,
                SessionEvent::TurnStarted {
                    turn_id: turn_id.clone(),
                    trigger: crate::event::TurnTrigger::UserPrompt,
                },
            ),
            logged(
                seq + 1,
                SessionEvent::TurnEnded {
                    turn_id: turn_id.clone(),
                    reason: TurnEndReason::Completed,
                },
            ),
        )
    }

    fn user(seq: u64, text: &str) -> LoggedEvent {
        logged(
            seq,
            SessionEvent::UserMessage {
                source: InputSource::User,
                content: vec![ContentBlock::Text(text.to_string())],
            },
        )
    }

    fn assistant_with_call(seq: u64, call_id: &str) -> LoggedEvent {
        logged(
            seq,
            SessionEvent::AssistantMessage {
                provider: "test".to_string(),
                model: "test".to_string(),
                content: Vec::new(),
                tool_calls: vec![ToolCall {
                    id: call_id.to_string(),
                    name: "read_file".to_string(),
                    args: "{}".to_string(),
                }],
                usage: Usage::default(),
                stream_digest: StreamDigest::default(),
            },
        )
    }

    fn tool_result(seq: u64, call_id: &str) -> LoggedEvent {
        logged(
            seq,
            SessionEvent::ToolResult {
                call_id: CallId::from(call_id),
                output: ToolObservation::success("ok"),
            },
        )
    }

    #[test]
    fn plans_largest_balanced_boundary_outside_verbatim_window() {
        let turn_a = TurnId::from("a");
        let turn_b = TurnId::from("b");
        let turn_c = TurnId::from("c");
        let (a_start, a_end) = turn(1, &turn_a);
        let (b_start, b_end) = turn(3, &turn_b);
        let (c_start, c_end) = turn(5, &turn_c);
        let events = vec![a_start, a_end, b_start, b_end, c_start, c_end];

        let range = plan_compaction_boundary(&events, 1).unwrap();
        assert_eq!(range.start, Seq(1));
        assert_eq!(range.end_inclusive, Seq(4));
        assert!(plan_compaction_boundary(&events, 3).is_none());

        let full = plan_compaction_boundary(&events, 0).unwrap();
        assert_eq!(full.end_inclusive, Seq(6));
    }

    #[test]
    fn boundary_snaps_back_to_keep_tool_pairs_together() {
        let turn_a = TurnId::from("a");
        let turn_b = TurnId::from("b");
        let turn_c = TurnId::from("c");
        let events = vec![
            logged(
                1,
                SessionEvent::TurnStarted {
                    turn_id: turn_a.clone(),
                    trigger: crate::event::TurnTrigger::UserPrompt,
                },
            ),
            logged(
                2,
                SessionEvent::TurnEnded {
                    turn_id: turn_a,
                    reason: TurnEndReason::Completed,
                },
            ),
            logged(
                3,
                SessionEvent::TurnStarted {
                    turn_id: turn_b.clone(),
                    trigger: crate::event::TurnTrigger::UserPrompt,
                },
            ),
            assistant_with_call(4, "call-1"),
            logged(
                5,
                SessionEvent::TurnEnded {
                    turn_id: turn_b,
                    reason: TurnEndReason::Completed,
                },
            ),
            tool_result(6, "call-1"),
            logged(
                7,
                SessionEvent::TurnStarted {
                    turn_id: turn_c.clone(),
                    trigger: crate::event::TurnTrigger::UserPrompt,
                },
            ),
            logged(
                8,
                SessionEvent::TurnEnded {
                    turn_id: turn_c,
                    reason: TurnEndReason::Completed,
                },
            ),
        ];

        let range = plan_compaction_boundary(&events, 1).unwrap();
        assert_eq!(range.end_inclusive, Seq(2));
    }

    #[test]
    fn summary_message_carries_marker_and_state_headings() {
        let message = summary_message("Fixed the parser.\n");
        assert!(message.starts_with(COMPACTED_MARKER));
        assert!(message.contains("Fixed the parser."));
        for heading in STATE_BLOCK_HEADINGS {
            assert!(message.contains(&format!("## {heading}")), "{heading}");
        }
        assert!(summary_message("").starts_with(COMPACTED_MARKER));
    }

    #[test]
    fn detects_open_compaction_brackets() {
        let id = CompactionId::from("cmp-1");
        let started = logged(
            1,
            SessionEvent::CompactionStarted {
                compaction_id: id.clone(),
                trigger: crate::event::CompactionTrigger::Manual,
            },
        );
        let ended = logged(
            2,
            SessionEvent::CompactionEnded {
                compaction_id: id,
                result: crate::event::CompactionResult::Applied { freed_tokens: 0 },
            },
        );

        assert!(has_open_compaction(std::slice::from_ref(&started)));
        assert!(!has_open_compaction(&[started, ended]));
        assert!(!has_open_compaction(&[]));
    }

    #[test]
    fn open_turn_scan_ignores_closed_turns() {
        let turn_a = TurnId::from("a");
        let turn_b = TurnId::from("b");
        let (a_start, a_end) = turn(1, &turn_a);
        let events = vec![
            a_start,
            a_end,
            logged(
                3,
                SessionEvent::TurnStarted {
                    turn_id: turn_b.clone(),
                    trigger: crate::event::TurnTrigger::UserPrompt,
                },
            ),
        ];

        assert_eq!(open_turns(&events), vec![turn_b.clone()]);
        assert_eq!(turn_start(&events, &turn_b), Some(Seq(3)));
        assert_eq!(turn_start(&events, &TurnId::from("missing")), None);
    }

    #[test]
    fn turn_start_ignores_turn_end() {
        let turn_a = TurnId::from("a");
        let events = vec![
            user(0, "before"),
            logged(
                1,
                SessionEvent::TurnStarted {
                    turn_id: turn_a.clone(),
                    trigger: crate::event::TurnTrigger::UserPrompt,
                },
            ),
            user(2, "inside"),
            logged(
                3,
                SessionEvent::TurnEnded {
                    turn_id: turn_a.clone(),
                    reason: TurnEndReason::Completed,
                },
            ),
        ];

        assert_eq!(turn_start(&events, &turn_a), Some(Seq(1)));
    }
}
