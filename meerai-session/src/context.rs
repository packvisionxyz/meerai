use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::blob::estimate_tokens;
use crate::event::{ContentBlock, InputSource, LoggedEvent, SessionEvent};
use crate::ids::TurnId;
use crate::projection::{ProjectionPolicy, shadowed_ranges};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextBreakdown {
    pub context_window_tokens: u64,
    pub high_watermark_tokens: u64,
    pub estimated_total_tokens: u64,
    pub system_prompt_tokens: u64,
    pub tool_schemas_tokens: u64,
    pub memory_tokens: u64,
    pub user_messages_tokens: u64,
    pub assistant_tokens: u64,
    pub tool_results_tokens: u64,
    pub reasoning_tokens: u64,
    pub compaction_summary_tokens: u64,
    pub spilled_blob_count: usize,
    pub cleared_tool_results_count: usize,
    pub pinned_turn_ids: Vec<TurnId>,
    pub turn_ids: Vec<TurnId>,
    pub turns_in_verbatim_window: usize,
    pub verbatim_window_turns: usize,
    pub reasoning_pruning: bool,
    pub clear_old_tool_results: bool,
    pub spilled_blobs: Vec<String>,
    pub last_prompt_cached_tokens: Option<u64>,
}

impl ContextBreakdown {
    pub fn bucket_total(&self) -> u64 {
        self.system_prompt_tokens
            .saturating_add(self.tool_schemas_tokens)
            .saturating_add(self.memory_tokens)
            .saturating_add(self.user_messages_tokens)
            .saturating_add(self.assistant_tokens)
            .saturating_add(self.tool_results_tokens)
            .saturating_add(self.reasoning_tokens)
            .saturating_add(self.compaction_summary_tokens)
    }

    pub fn recompute_total(&mut self) {
        self.estimated_total_tokens = self.bucket_total();
    }

    pub fn headroom_tokens(&self) -> u64 {
        self.context_window_tokens
            .saturating_sub(self.estimated_total_tokens)
    }
}

pub fn pinned_turns(events: &[LoggedEvent]) -> Vec<TurnId> {
    let mut pinned: HashMap<TurnId, bool> = HashMap::new();
    for logged in events {
        if let SessionEvent::ContextTurnPinned {
            turn_id,
            pinned: flag,
        } = &logged.event
        {
            pinned.insert(turn_id.clone(), *flag);
        }
    }
    let mut turns: Vec<TurnId> = pinned
        .into_iter()
        .filter_map(|(turn, pinned)| pinned.then_some(turn))
        .collect();
    turns.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    turns
}

pub fn high_watermark_tokens(context_window_tokens: u64, high_watermark_pct: u8) -> u64 {
    context_window_tokens.saturating_mul(u64::from(high_watermark_pct)) / 100
}

pub fn context_breakdown(
    events: &[LoggedEvent],
    policy: &ProjectionPolicy,
    context_window_tokens: u64,
    high_watermark_pct: u8,
) -> ContextBreakdown {
    let shadowed = shadowed_ranges(events);
    let pinned = pinned_turns(events);
    let pinned_set: Vec<&TurnId> = pinned.iter().chain(policy.pinned_turns.iter()).collect();
    let current_turn_start = events
        .iter()
        .rev()
        .find(|logged| matches!(logged.event, SessionEvent::TurnStarted { .. }))
        .map(|logged| logged.seq);
    let visible: Vec<&LoggedEvent> = events
        .iter()
        .filter(|logged| !shadowed.iter().any(|range| range.contains(logged.seq)))
        .collect();
    let turn_count = visible
        .iter()
        .filter(|logged| matches!(logged.event, SessionEvent::TurnStarted { .. }))
        .count();

    let mut breakdown = ContextBreakdown {
        context_window_tokens,
        high_watermark_tokens: high_watermark_tokens(context_window_tokens, high_watermark_pct),
        turns_in_verbatim_window: turn_count.min(policy.verbatim_window_turns.max(1)),
        verbatim_window_turns: policy.verbatim_window_turns,
        reasoning_pruning: policy.prune_reasoning,
        clear_old_tool_results: policy.clear_old_tool_results,
        ..ContextBreakdown::default()
    };

    let mut turn_index: Option<usize> = None;
    let mut turn_id: Option<TurnId> = None;

    for logged in events {
        if shadowed.iter().any(|range| range.contains(logged.seq)) {
            continue;
        }
        match &logged.event {
            SessionEvent::TurnStarted {
                turn_id: started, ..
            } => {
                turn_index = Some(turn_index.map_or(0, |index| index + 1));
                turn_id = Some(started.clone());
                breakdown.turn_ids.push(started.clone());
            }
            SessionEvent::SystemMessage { sections, .. } => {
                if !policy.include_system_messages {
                    continue;
                }
                for section in sections {
                    breakdown.system_prompt_tokens += estimate_tokens(&section.content);
                }
            }
            SessionEvent::UserMessage { source, content } => {
                let stale_ephemera = *source == InputSource::RuntimeContext
                    && !current_turn_start.is_some_and(|start| logged.seq.0 > start.0);
                if policy.strip_ephemera && stale_ephemera {
                    continue;
                }
                for block in content {
                    if let ContentBlock::Text(text) = block {
                        breakdown.user_messages_tokens += estimate_tokens(text);
                    }
                }
            }
            SessionEvent::AssistantMessage { content, usage, .. } => {
                for block in content {
                    match block {
                        ContentBlock::Text(text) => {
                            breakdown.assistant_tokens += estimate_tokens(text)
                        }
                        ContentBlock::Reasoning(text) => {
                            breakdown.reasoning_tokens += estimate_tokens(text)
                        }
                    }
                }
                if let Some(cached) = usage.cached_tokens.filter(|tokens| *tokens >= 0) {
                    breakdown.last_prompt_cached_tokens = Some(cached as u64);
                }
            }
            SessionEvent::ToolResult { output, .. } => {
                let clears = policy.clear_old_tool_results
                    && !turn_is_pinned(&turn_id, &pinned_set)
                    && turn_index.is_some_and(|index| {
                        turn_count.saturating_sub(index) > policy.verbatim_window_turns.max(1)
                    });
                if clears {
                    breakdown.cleared_tool_results_count += 1;
                    breakdown.tool_results_tokens += cleared_stub_tokens(output);
                } else {
                    breakdown.tool_results_tokens += estimate_tokens(&output.render());
                }
            }
            SessionEvent::SpillApplied { blob_key, .. } => {
                breakdown.spilled_blob_count += 1;
                if !breakdown.spilled_blobs.contains(blob_key) {
                    breakdown.spilled_blobs.push(blob_key.clone());
                }
            }
            SessionEvent::CompactionSummary { summary, .. } => {
                breakdown.compaction_summary_tokens += estimate_tokens(summary);
            }
            _ => {}
        }
    }

    let mut pinned_turn_ids = pinned;
    for turn in &policy.pinned_turns {
        if !pinned_turn_ids.contains(turn) {
            pinned_turn_ids.push(turn.clone());
        }
    }
    pinned_turn_ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    breakdown.pinned_turn_ids = pinned_turn_ids;
    breakdown.recompute_total();
    breakdown
}

pub(crate) fn turn_is_pinned(turn_id: &Option<TurnId>, pinned: &[&TurnId]) -> bool {
    turn_id.as_ref().is_some_and(|turn| pinned.contains(&turn))
}

fn cleared_stub_tokens(output: &meerai_core::ToolObservation) -> u64 {
    estimate_tokens(&format!(
        "[tool result cleared from an earlier turn: {}, {} bytes omitted; re-run the tool if needed]",
        format!("{:?}", output.status).to_lowercase(),
        output.stdout.len().saturating_add(output.stderr.len())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Seq, TurnEndReason, TurnTrigger};
    use crate::ids::TurnId;
    use meerai_core::{ToolCall, ToolObservation};

    fn logged(seq: u64, event: SessionEvent) -> LoggedEvent {
        LoggedEvent {
            seq: Seq(seq),
            event,
        }
    }

    fn turn(seq: u64, id: &str) -> LoggedEvent {
        logged(
            seq,
            SessionEvent::TurnStarted {
                turn_id: TurnId::from(id),
                trigger: TurnTrigger::UserPrompt,
            },
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

    fn assistant(seq: u64, text: &str, reasoning: &str, calls: Vec<ToolCall>) -> LoggedEvent {
        logged(
            seq,
            SessionEvent::AssistantMessage {
                provider: "test".to_string(),
                model: "test".to_string(),
                content: vec![
                    ContentBlock::Text(text.to_string()),
                    ContentBlock::Reasoning(reasoning.to_string()),
                ],
                tool_calls: calls,
                call_signatures: Vec::new(),
                usage: crate::event::Usage {
                    cached_tokens: Some(7),
                    ..crate::event::Usage::default()
                },
                stream_digest: crate::event::StreamDigest::default(),
            },
        )
    }

    fn result(seq: u64, call: &str, text: &str) -> LoggedEvent {
        logged(
            seq,
            SessionEvent::ToolResult {
                call_id: crate::ids::CallId::from(call),
                output: ToolObservation::success(text),
            },
        )
    }

    #[test]
    fn counts_each_bucket() {
        let events = vec![
            turn(1, "turn-1"),
            user(2, "hello world"),
            assistant(
                3,
                "answer",
                "thinking",
                vec![ToolCall {
                    id: "c1".to_string(),
                    name: "code-read_file".to_string(),
                    args: "{}".to_string(),
                }],
            ),
            result(4, "c1", "some output here"),
            logged(
                5,
                SessionEvent::TurnEnded {
                    turn_id: TurnId::from("turn-1"),
                    reason: TurnEndReason::Completed,
                },
            ),
        ];
        let breakdown = context_breakdown(&events, &ProjectionPolicy::default(), 100_000, 80);
        assert_eq!(
            breakdown.user_messages_tokens,
            estimate_tokens("hello world")
        );
        assert_eq!(breakdown.assistant_tokens, estimate_tokens("answer"));
        assert_eq!(breakdown.reasoning_tokens, estimate_tokens("thinking"));
        assert_eq!(
            breakdown.tool_results_tokens,
            estimate_tokens("some output here")
        );
        assert_eq!(breakdown.high_watermark_tokens, 80_000);
        assert_eq!(breakdown.last_prompt_cached_tokens, Some(7));
        assert_eq!(breakdown.turns_in_verbatim_window, 1);
        assert_eq!(breakdown.estimated_total_tokens, breakdown.bucket_total());
    }

    #[test]
    fn counts_cleared_results_and_spills() {
        let mut events = Vec::new();
        for index in 0..6u64 {
            let id = format!("turn-{index}");
            events.push(turn(index * 5 + 1, &id));
            events.push(user(index * 5 + 2, "go"));
            events.push(assistant(
                index * 5 + 3,
                "ok",
                "",
                vec![ToolCall {
                    id: format!("c{index}"),
                    name: "code-read_file".to_string(),
                    args: "{}".to_string(),
                }],
            ));
            events.push(result(index * 5 + 4, &format!("c{index}"), "output"));
            events.push(logged(
                index * 5 + 5,
                SessionEvent::TurnEnded {
                    turn_id: TurnId::from(id.as_str()),
                    reason: TurnEndReason::Completed,
                },
            ));
        }
        events.push(logged(
            100,
            SessionEvent::SpillApplied {
                call_id: crate::ids::CallId::from("c0"),
                bytes_omitted: 10,
                blob_key: "/tmp/blob".to_string(),
            },
        ));
        let breakdown = context_breakdown(&events, &ProjectionPolicy::default(), 10_000, 75);
        assert_eq!(breakdown.cleared_tool_results_count, 2);
        assert_eq!(breakdown.spilled_blob_count, 1);
        assert_eq!(breakdown.turns_in_verbatim_window, 4);
    }

    #[test]
    fn pinned_turns_keep_their_results_verbatim() {
        let mut events = Vec::new();
        for index in 0..5u64 {
            let id = format!("turn-{index}");
            events.push(turn(index * 5 + 1, &id));
            events.push(user(index * 5 + 2, "go"));
            events.push(assistant(
                index * 5 + 3,
                "ok",
                "",
                vec![ToolCall {
                    id: format!("c{index}"),
                    name: "code-read_file".to_string(),
                    args: "{}".to_string(),
                }],
            ));
            events.push(result(index * 5 + 4, &format!("c{index}"), "output"));
            events.push(logged(
                index * 5 + 5,
                SessionEvent::TurnEnded {
                    turn_id: TurnId::from(id.as_str()),
                    reason: TurnEndReason::Completed,
                },
            ));
        }
        events.push(logged(
            90,
            SessionEvent::ContextTurnPinned {
                turn_id: TurnId::from("turn-0"),
                pinned: true,
            },
        ));
        let breakdown = context_breakdown(&events, &ProjectionPolicy::default(), 10_000, 75);
        assert_eq!(breakdown.pinned_turn_ids, vec![TurnId::from("turn-0")]);
        assert_eq!(breakdown.cleared_tool_results_count, 0);
        let messages = crate::projection::derive_messages(&events, &ProjectionPolicy::default());
        let contents: Vec<String> = messages
            .iter()
            .flat_map(|message| {
                message
                    .content
                    .tool_responses()
                    .into_iter()
                    .map(|response| response.content.clone())
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(contents[0], "output");
    }
}
