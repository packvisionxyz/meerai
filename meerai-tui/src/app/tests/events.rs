use super::*;

#[test]
fn agent_event_streaming_updates_transcript_and_status() {
    let (mut app, _rx, _secrets) = test_app();

    let turn_id = TurnId::from_raw("turn-1");
    app.handle_agent_event(AgentEvent::TurnStarted {
        turn_id: turn_id.clone(),
        model: "openai::gpt-4o-mini".to_string(),
    });
    assert!(app.tab().is_running);
    assert_eq!(app.tab().model, "gpt-4o-mini");
    assert_eq!(app.tab().turn_id, Some(turn_id));

    let reasoning_id = ItemId::from_raw("item-reasoning");
    app.handle_agent_event(AgentEvent::ItemStarted {
        item: SessionItem::Reasoning {
            id: reasoning_id.clone(),
            summary: String::new(),
        },
    });
    app.handle_agent_event(AgentEvent::ItemDelta {
        item_id: reasoning_id,
        delta: ItemDelta::ReasoningSummary("thinking...".to_string()),
    });

    let message_id = ItemId::from_raw("item-message");
    app.handle_agent_event(AgentEvent::ItemStarted {
        item: SessionItem::AgentMessage {
            id: message_id.clone(),
            text: String::new(),
        },
    });
    app.handle_agent_event(AgentEvent::ItemDelta {
        item_id: message_id.clone(),
        delta: ItemDelta::AgentText("Hello ".to_string()),
    });
    app.handle_agent_event(AgentEvent::ItemDelta {
        item_id: message_id.clone(),
        delta: ItemDelta::AgentText("world!".to_string()),
    });

    assert_eq!(
        app.tab().transcript,
        vec![TranscriptEntry::Assistant {
            text: "Hello world!".to_string(),
            reasoning: "thinking...".to_string(),
            streaming: true,
            reasoning_expanded: false,
        }]
    );

    let tool_item_id = ItemId::from_raw("item-tool");
    let call_id = meerai_agents::CallId::from_raw("c1");
    app.handle_agent_event(AgentEvent::ItemStarted {
        item: SessionItem::ToolCall {
            id: tool_item_id.clone(),
            call_id: call_id.clone(),
            name: "read_file".to_string(),
            args: "{\"path\":\"Cargo.toml\"}".to_string(),
            effect: EffectClass::Read,
            status: ItemStatus::Proposed,
            output: None,
        },
    });
    assert_eq!(app.tab().active_tool.as_deref(), Some("read_file"));

    app.handle_agent_event(AgentEvent::ItemDelta {
        item_id: tool_item_id.clone(),
        delta: ItemDelta::StatusChanged(ItemStatus::InProgress),
    });
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::ToolCall(ToolCallEntry {
            call_id: "c1".to_string(),
            name: "read_file".to_string(),
            args: "{\"path\":\"Cargo.toml\"}".to_string(),
            status: ToolCallStatus::Running,
            output: None,
            expanded: false,
        }))
    );

    app.handle_agent_event(AgentEvent::ItemCompleted {
        item: SessionItem::ToolCall {
            id: tool_item_id,
            call_id,
            name: "read_file".to_string(),
            args: "{\"path\":\"Cargo.toml\"}".to_string(),
            effect: EffectClass::Read,
            status: ItemStatus::Succeeded,
            output: Some("ok".to_string()),
        },
    });
    assert!(app.tab().active_tool.is_none());
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::ToolCall(ToolCallEntry {
            call_id: "c1".to_string(),
            name: "read_file".to_string(),
            args: "{\"path\":\"Cargo.toml\"}".to_string(),
            status: ToolCallStatus::Succeeded,
            output: Some("ok".to_string()),
            expanded: false,
        }))
    );

    app.handle_agent_event(AgentEvent::UsageUpdated {
        usage: Usage {
            prompt_tokens: Some(120),
            completion_tokens: Some(30),
            total_tokens: Some(150),
            cached_tokens: None,
        },
        cumulative: Usage {
            prompt_tokens: Some(120),
            completion_tokens: Some(30),
            total_tokens: Some(150),
            cached_tokens: None,
        },
        est_cost_usd: None,
    });
    assert_eq!(app.tab().usage.total_tokens, Some(150));
    assert_eq!(app.tab().metrics.session_total_tokens, 150);

    app.handle_agent_event(AgentEvent::ItemCompleted {
        item: SessionItem::AgentMessage {
            id: ItemId::from_raw("item-done"),
            text: "Done".to_string(),
        },
    });
    app.handle_agent_event(AgentEvent::TurnCompleted {
        turn_id: TurnId::from_raw("turn-1"),
        reason: TurnEndReason::Completed,
    });
    assert!(!app.tab().is_running);
    assert_eq!(app.tab().metrics.turn_count, 1);
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::Assistant {
            text: "Done".to_string(),
            reasoning: String::new(),
            streaming: false,
            reasoning_expanded: false,
        })
    );

    app.handle_agent_event(AgentEvent::Error {
        code: ErrorCode::Provider,
        message: "boom".to_string(),
    });
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::Error {
            text: "provider error: boom".to_string(),
            expanded: false,
        })
    );
}

#[test]
fn authoritative_agent_message_replaces_streamed_text() {
    let (mut app, _rx, _secrets) = test_app();
    app.handle_agent_event(AgentEvent::TurnStarted {
        turn_id: TurnId::from_raw("turn-authoritative"),
        model: "openai::gpt-4o-mini".to_string(),
    });

    let id = ItemId::from_raw("item-message-authoritative");
    app.handle_agent_event(AgentEvent::ItemStarted {
        item: SessionItem::AgentMessage {
            id: id.clone(),
            text: String::new(),
        },
    });
    app.handle_agent_event(AgentEvent::ItemDelta {
        item_id: id.clone(),
        delta: ItemDelta::AgentText("streamed draft".to_string()),
    });
    app.handle_agent_event(AgentEvent::ItemCompleted {
        item: SessionItem::AgentMessage {
            id: id.clone(),
            text: "final answer".to_string(),
        },
    });
    assert_eq!(
        app.tab().transcript,
        vec![TranscriptEntry::Assistant {
            text: "final answer".to_string(),
            reasoning: String::new(),
            streaming: false,
            reasoning_expanded: false,
        }]
    );

    let empty_id = ItemId::from_raw("item-message-empty");
    app.handle_agent_event(AgentEvent::ItemStarted {
        item: SessionItem::AgentMessage {
            id: empty_id.clone(),
            text: String::new(),
        },
    });
    app.handle_agent_event(AgentEvent::ItemDelta {
        item_id: empty_id.clone(),
        delta: ItemDelta::AgentText("kept stream".to_string()),
    });
    app.handle_agent_event(AgentEvent::ItemCompleted {
        item: SessionItem::AgentMessage {
            id: empty_id,
            text: String::new(),
        },
    });
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::Assistant {
            text: "kept stream".to_string(),
            reasoning: String::new(),
            streaming: false,
            reasoning_expanded: false,
        })
    );
}

#[test]
fn aborted_and_failed_turns_render_terminal_entries() {
    let (mut app, _rx, _secrets) = test_app();

    app.handle_agent_event(AgentEvent::TurnStarted {
        turn_id: TurnId::from_raw("turn-abort"),
        model: "openai::gpt-4o-mini".to_string(),
    });
    app.handle_agent_event(AgentEvent::TurnCompleted {
        turn_id: TurnId::from_raw("turn-abort"),
        reason: TurnEndReason::Aborted {
            cause: AbortCause::User,
        },
    });
    assert!(!app.tab().is_running);
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::System {
            text: "Turn cancelled.".to_string(),
        })
    );

    app.handle_agent_event(AgentEvent::TurnStarted {
        turn_id: TurnId::from_raw("turn-failed"),
        model: "openai::gpt-4o-mini".to_string(),
    });
    app.handle_agent_event(AgentEvent::Error {
        code: ErrorCode::Budget,
        message: "Exceeded maximum number of cycles (16)".to_string(),
    });
    app.handle_agent_event(AgentEvent::TurnCompleted {
        turn_id: TurnId::from_raw("turn-failed"),
        reason: TurnEndReason::Failed {
            error: "Exceeded maximum number of cycles (16)".to_string(),
        },
    });
    assert!(!app.tab().is_running);
    assert_eq!(app.tab().metrics.turn_count, 2);
    assert_eq!(app.tab().transcript.len(), 2);
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::Error {
            text: "budget error: Exceeded maximum number of cycles (16)".to_string(),
            expanded: false,
        })
    );
}

#[test]
fn compaction_items_render_system_entries() {
    let (mut app, _rx, _secrets) = test_app();
    app.handle_agent_event(AgentEvent::ItemCompleted {
        item: SessionItem::Compaction {
            id: ItemId::from_raw("item-compact"),
            trigger: meerai_agents::events::CompactionTrigger::Manual,
            freed_tokens: 2048,
        },
    });
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::System {
            text: "Compaction freed 2048 tokens".to_string(),
        })
    );
}

#[test]
fn hook_completed_events_render_system_entries() {
    let (mut app, _rx, _secrets) = test_app();

    app.handle_agent_event(AgentEvent::HookCompleted {
        event: "PreToolUse".to_string(),
        decision: "deny".to_string(),
        reason: Some("no shell".to_string()),
    });
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::System {
            text: "hook PreToolUse: deny — no shell".to_string(),
        })
    );

    app.handle_agent_event(AgentEvent::HookCompleted {
        event: "SessionStart".to_string(),
        decision: "pass".to_string(),
        reason: None,
    });
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::System {
            text: "hook SessionStart: pass".to_string(),
        })
    );

    app.handle_agent_event(AgentEvent::HookCompleted {
        event: "Stop".to_string(),
        decision: "block".to_string(),
        reason: Some("é".repeat(250)),
    });
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::System {
            text: format!("hook Stop: block — {}", "é".repeat(200)),
        })
    );
}

#[test]
fn transcript_from_events_replays_known_entries_and_skips_runtime_events() {
    use meerai_core::ToolObservation;
    use meerai_session::{
        CallId, CompactionId, ContentBlock, InputSource, LoggedEvent, Seq, SeqRange, SessionEvent,
    };

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
                usage: Usage::default(),
                stream_digest: meerai_session::StreamDigest::default(),
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
                step_id: meerai_session::StepId::from_raw("step-1"),
                turn_id: meerai_session::TurnId::from_raw("turn-1"),
            },
        },
    ];

    assert_eq!(
        transcript_from_events(&events),
        vec![
            TranscriptEntry::User {
                text: "hello".to_string(),
            },
            TranscriptEntry::Assistant {
                text: "answer".to_string(),
                reasoning: "thinking".to_string(),
                streaming: false,
                reasoning_expanded: false,
            },
            TranscriptEntry::ToolCall(ToolCallEntry {
                call_id: "call-1".to_string(),
                name: "code-read_file".to_string(),
                args: "{\"path\":\"src/lib.rs\"}".to_string(),
                status: ToolCallStatus::Succeeded,
                output: Some("file contents".to_string()),
                expanded: false,
            }),
            TranscriptEntry::ToolCall(ToolCallEntry {
                call_id: "call-2".to_string(),
                name: "code-shell".to_string(),
                args: "{}".to_string(),
                status: ToolCallStatus::Cancelled,
                output: None,
                expanded: false,
            }),
            TranscriptEntry::System {
                text: "Compacted history".to_string(),
            },
        ]
    );
}

#[test]
fn transcript_from_events_marks_unmatched_tool_calls_cancelled() {
    use meerai_session::{CallId, LoggedEvent, Seq, SessionEvent};

    let events = vec![LoggedEvent {
        seq: Seq(1),
        event: SessionEvent::ToolCall {
            call_id: CallId::from_raw("call-crashed"),
            name: "code-shell".to_string(),
            args: "{}".to_string(),
        },
    }];

    assert_eq!(
        transcript_from_events(&events),
        vec![TranscriptEntry::ToolCall(ToolCallEntry {
            call_id: "call-crashed".to_string(),
            name: "code-shell".to_string(),
            args: "{}".to_string(),
            status: ToolCallStatus::Cancelled,
            output: None,
            expanded: false,
        })]
    );
}

#[test]
fn transcript_from_events_excludes_reverted_turns() {
    use meerai_core::ToolObservation;
    use meerai_session::{
        CallId, CompactionId, ContentBlock, InputSource, LoggedEvent, Seq, SeqRange, SessionEvent,
        TurnTrigger,
    };

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
                usage: Usage::default(),
                stream_digest: meerai_session::StreamDigest::default(),
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
                usage: Usage::default(),
                stream_digest: meerai_session::StreamDigest::default(),
            },
        },
        LoggedEvent {
            seq: Seq(8),
            event: SessionEvent::RewindApplied {
                turn_id: TurnId::from_raw("turn-reverted"),
                scope: meerai_session::RewindScope::Conversation,
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
        transcript_from_events(&events),
        vec![
            TranscriptEntry::User {
                text: "keep prompt".to_string(),
            },
            TranscriptEntry::Assistant {
                text: "keep reply".to_string(),
                reasoning: String::new(),
                streaming: false,
                reasoning_expanded: false,
            },
            TranscriptEntry::System {
                text: "Compacted history".to_string(),
            },
            TranscriptEntry::User {
                text: "after revert".to_string(),
            },
        ]
    );
}

#[test]
fn session_usage_from_events_returns_last_cumulative_usage() {
    use meerai_session::{LoggedEvent, Seq, SessionEvent};

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
        })
    );
    assert_eq!(session_usage_from_events(&[]), None);
}

#[test]
fn submit_while_running_queues_and_keeps_turn() {
    let (mut app, mut rx, _secrets) = test_app();
    app.tab_mut().is_running = true;
    app.tab_mut().turn_id = Some(TurnId::from_raw("turn-steer"));

    type_text(&mut app, "focus on tests");
    app.handle_key_event(key(KeyCode::Enter));

    assert!(rx.try_recv().is_err());
    assert_eq!(app.tab().queued, vec!["focus on tests".to_string()]);
    assert!(app.tab().is_running);
    assert_eq!(app.tab().turn_id, Some(TurnId::from_raw("turn-steer")));
    assert!(app.tab().input.is_empty());
    assert!(app.tab().transcript.is_empty());
}

#[test]
fn subagent_run_lifecycle_folds_and_renders() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let (mut app, _rx, _secrets) = test_app();
    app.handle_agent_event(AgentEvent::TurnStarted {
        turn_id: TurnId::from_raw("turn-subagent"),
        model: "gemini::gemini-3.8-flash".to_string(),
    });

    let subagent_id = ItemId::from_raw("item-sub-1");
    app.handle_agent_event(AgentEvent::ItemStarted {
        item: SessionItem::SubagentRun {
            id: subagent_id.clone(),
            agent: "reviewer".to_string(),
            status: ItemStatus::Proposed,
            report: None,
            child_session_id: Some("ses-child-1".to_string()),
        },
    });
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::Subagent(SubagentEntry {
            id: "item-sub-1".to_string(),
            agent: "reviewer".to_string(),
            status: ToolCallStatus::Proposed,
            report: None,
            child_session_id: Some("ses-child-1".to_string()),
            expanded: false,
            nested: None,
            nested_refreshed_at: None,
            nested_loading: false,
        }))
    );

    app.handle_agent_event(AgentEvent::ItemDelta {
        item_id: subagent_id.clone(),
        delta: ItemDelta::StatusChanged(ItemStatus::InProgress),
    });
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::Subagent(SubagentEntry {
            id: "item-sub-1".to_string(),
            agent: "reviewer".to_string(),
            status: ToolCallStatus::Running,
            report: None,
            child_session_id: Some("ses-child-1".to_string()),
            expanded: false,
            nested: None,
            nested_refreshed_at: None,
            nested_loading: false,
        }))
    );

    app.handle_agent_event(AgentEvent::ItemCompleted {
        item: SessionItem::SubagentRun {
            id: subagent_id,
            agent: "reviewer".to_string(),
            status: ItemStatus::Succeeded,
            report: Some("Line one\nLine two".to_string()),
            child_session_id: Some("ses-child-1".to_string()),
        },
    });
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::Subagent(SubagentEntry {
            id: "item-sub-1".to_string(),
            agent: "reviewer".to_string(),
            status: ToolCallStatus::Succeeded,
            report: Some("Line one\nLine two".to_string()),
            child_session_id: Some("ses-child-1".to_string()),
            expanded: false,
            nested: None,
            nested_refreshed_at: None,
            nested_loading: false,
        }))
    );

    let backend = TestBackend::new(120, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| crate::ui::render(frame, &mut app))
        .unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(rendered.contains("+ [subagent] reviewer [ok]"));
    assert!(rendered.contains("Line one Line two"));
}

#[test]
fn subagent_entry_finalizes_on_parent_turn_failure_and_abort() {
    let (mut app, _rx, _secrets) = test_app();

    app.handle_agent_event(AgentEvent::TurnStarted {
        turn_id: TurnId::from_raw("t-abort"),
        model: "gemini::gemini-3.8-flash".to_string(),
    });
    app.handle_agent_event(AgentEvent::ItemStarted {
        item: SessionItem::SubagentRun {
            id: ItemId::from_raw("sub-abort"),
            agent: "explore".to_string(),
            status: ItemStatus::InProgress,
            report: None,
            child_session_id: None,
        },
    });
    app.handle_agent_event(AgentEvent::TurnCompleted {
        turn_id: TurnId::from_raw("t-abort"),
        reason: TurnEndReason::Aborted {
            cause: AbortCause::User,
        },
    });
    assert!(app.tab().transcript.iter().any(|entry| {
        matches!(
            entry,
            TranscriptEntry::Subagent(s) if s.id == "sub-abort" && s.status == ToolCallStatus::Cancelled
        )
    }));

    app.handle_agent_event(AgentEvent::TurnStarted {
        turn_id: TurnId::from_raw("t-fail"),
        model: "gemini::gemini-3.8-flash".to_string(),
    });
    app.handle_agent_event(AgentEvent::ItemStarted {
        item: SessionItem::SubagentRun {
            id: ItemId::from_raw("sub-fail"),
            agent: "build".to_string(),
            status: ItemStatus::InProgress,
            report: None,
            child_session_id: None,
        },
    });
    app.handle_agent_event(AgentEvent::TurnCompleted {
        turn_id: TurnId::from_raw("t-fail"),
        reason: TurnEndReason::Failed {
            error: "budget exceeded".to_string(),
        },
    });
    assert!(app.tab().transcript.iter().any(|entry| {
        matches!(
            entry,
            TranscriptEntry::Subagent(s) if s.id == "sub-fail" && s.status == ToolCallStatus::Failed
        )
    }));
}

#[test]
fn child_session_load_queues_action_and_folds_nested_transcript() {
    use meerai_core::ToolObservation;
    use meerai_session::{CallId, ContentBlock, InputSource, LoggedEvent, Seq, SessionEvent};

    let (mut app, _rx, _secrets) = test_app();
    app.handle_agent_event(AgentEvent::TurnStarted {
        turn_id: TurnId::from_raw("turn-child-read"),
        model: "gemini::gemini-3.8-flash".to_string(),
    });
    app.handle_agent_event(AgentEvent::ItemStarted {
        item: SessionItem::SubagentRun {
            id: ItemId::from_raw("sub-read"),
            agent: "explore".to_string(),
            status: ItemStatus::InProgress,
            report: None,
            child_session_id: Some("ses-child-read".to_string()),
        },
    });

    let entry = app
        .tab_mut()
        .transcript
        .iter_mut()
        .find_map(|entry| match entry {
            TranscriptEntry::Subagent(sub) if sub.id == "sub-read" => Some(sub),
            _ => None,
        })
        .expect("subagent entry");
    entry.expanded = true;

    app.request_child_session_load("sub-read");
    let tab_id = app.tab().tab_id;
    assert!(matches!(
        app.pending_session_actions.last(),
        Some(PendingSessionAction::ReadChildSession { item_id, session_id, tab_id: t })
            if item_id == "sub-read" && session_id.as_str() == "ses-child-read" && *t == tab_id
    ));
    app.request_child_session_load("sub-read");
    assert_eq!(app.pending_session_actions.len(), 1);

    let events = vec![
        LoggedEvent {
            seq: Seq(1),
            event: SessionEvent::UserMessage {
                source: InputSource::User,
                content: vec![ContentBlock::Text("find the bug".to_string())],
            },
        },
        LoggedEvent {
            seq: Seq(2),
            event: SessionEvent::ToolCall {
                call_id: CallId::from_raw("c1"),
                name: "code-grep".to_string(),
                args: "{}".to_string(),
            },
        },
        LoggedEvent {
            seq: Seq(3),
            event: SessionEvent::ToolResult {
                call_id: CallId::from_raw("c1"),
                output: ToolObservation::success("match at src/lib.rs:10"),
            },
        },
    ];
    app.apply_child_session_read(tab_id, "sub-read", Ok(events));

    let entry = app
        .tab()
        .transcript
        .iter()
        .find_map(|entry| match entry {
            TranscriptEntry::Subagent(sub) if sub.id == "sub-read" => Some(sub),
            _ => None,
        })
        .expect("subagent entry");
    assert!(!entry.nested_loading);
    assert!(entry.nested_refreshed_at.is_some());
    let nested = entry.nested.as_ref().expect("nested transcript");
    assert_eq!(nested.len(), 2);
    assert!(matches!(&nested[0], TranscriptEntry::User { text } if text == "find the bug"));
    assert!(
        matches!(&nested[1], TranscriptEntry::ToolCall(tc) if tc.name == "code-grep" && tc.status == ToolCallStatus::Succeeded)
    );
}

#[test]
fn transcript_from_events_converts_subagent_run_with_child_hint() {
    use meerai_core::ToolObservation;
    use meerai_session::{CallId, LoggedEvent, Seq, SessionEvent};

    let events = vec![
        LoggedEvent {
            seq: Seq(1),
            event: SessionEvent::ToolCall {
                call_id: CallId::from_raw("sub-call"),
                name: "subagent-run".to_string(),
                args: "{\"agent\":\"explore\",\"task\":\"find it\"}".to_string(),
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
    let transcript = transcript_from_events(&events);
    assert_eq!(transcript.len(), 1);
    match &transcript[0] {
        TranscriptEntry::Subagent(entry) => {
            assert_eq!(entry.agent, "explore");
            assert_eq!(entry.child_session_id.as_deref(), Some("ses-child-replay"));
            assert_eq!(entry.status, ToolCallStatus::Succeeded);
            assert!(!entry.expanded);
        }
        other => panic!("expected Subagent, got {other:?}"),
    }
}

#[test]
fn submit_while_running_queues_instead_of_steering() {
    let (mut app, mut rx, _secrets) = test_app();
    app.tab_mut().is_running = true;
    type_text(&mut app, "second task");
    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(app.tab().queued, vec!["second task".to_string()]);
    assert!(app.tab().transcript.is_empty());
    assert!(rx.try_recv().is_err());
    assert!(app.status_notice.unwrap().contains("Queued 1"));
}

#[test]
fn queued_message_sends_after_turn_completes() {
    let (mut app, mut rx, _secrets) = test_app();
    app.tab_mut().is_running = true;
    app.tab_mut().queued.push("next task".to_string());

    let turn_id = TurnId::from_raw("turn-queue");
    app.tab_mut().apply_agent_event(AgentEvent::TurnCompleted {
        turn_id,
        reason: meerai_agents::TurnEndReason::Completed,
    });

    assert!(app.tab().queued.is_empty());
    assert!(app.tab().is_running);
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::SubmitPrompt("next task".to_string())
    );
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::User { text }) if text == "next task"
    ));
}

#[test]
fn ctrl_g_sends_queued_message_now_as_steering() {
    let (mut app, mut rx, _secrets) = test_app();
    app.tab_mut().is_running = true;
    app.tab_mut().queued.push("urgent".to_string());
    app.tab_mut().queued.push("later".to_string());

    app.handle_key_event(key_mod(KeyCode::Char('g'), KeyModifiers::CONTROL));

    assert_eq!(app.tab().queued, vec!["later".to_string()]);
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::Steer("urgent".to_string())
    );
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::User { text }) if text == "urgent"
    ));
}
