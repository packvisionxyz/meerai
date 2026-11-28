use super::fixtures::*;

#[allow(unused_imports)]
use crate::harness::HarnessConfig;
#[allow(unused_imports)]
use crate::intercept::{Interceptor, StepAction, StepPlan, ToolOutcome, TurnStopAction};
#[allow(unused_imports)]
use crate::multi_turn_agent::compaction::*;
#[allow(unused_imports)]
use crate::multi_turn_agent::turn::*;
#[allow(unused_imports)]
use crate::multi_turn_agent::*;
#[allow(unused_imports)]
use futures::stream;
#[allow(unused_imports)]
use genai::chat::{MessageContent, StreamChunk, Usage};
#[allow(unused_imports)]
use meerai_core::{
    FileChangeKind, FileDiff, ObservationStatus, ToolDefinition, ToolError, async_trait,
};
#[allow(unused_imports)]
use meerai_policy::{ActionSelector, ApprovalMode, Effect, PermissionRule, PolicyEngine};
#[allow(unused_imports)]
use meerai_session::{COMPACTED_MARKER, Session, SessionConfig, SessionId};
#[allow(unused_imports)]
use std::collections::VecDeque;
#[allow(unused_imports)]
use std::path::{Path, PathBuf};
#[allow(unused_imports)]
use std::sync::Mutex;
#[allow(unused_imports)]
use std::sync::atomic::{AtomicUsize, Ordering};
#[allow(unused_imports)]
use tokio::sync::{Barrier, Notify};

#[tokio::test]
async fn spill_stores_blob_and_retains_notice() {
    let root = std::env::temp_dir().join(format!("meerai-spill-{}", SessionId::new()));
    let session = persistent_session(&root).await;
    let output = big_output(300);
    let tool = StaticToolset::new("big", "big-out", output.clone(), EffectClass::Read);
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "c1".to_string(),
            fn_name: "big-out".to_string(),
            fn_arguments: serde_json::json!({}),
            thought_signatures: None,
        }]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let config = HarnessConfig {
        spill_max_inline_tokens: 20,
        ..HarnessConfig::default()
    };
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(tool)],
        "",
        &session,
        auto_mode(),
        config,
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "done");

    let mut completed = None;
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::ItemCompleted {
            item: SessionItem::ToolCall { output, .. },
        } = event
        {
            completed = output;
        }
    }
    let completed = completed.expect("tool item completed");
    assert!(completed.contains("(Omitted "));
    assert!(completed.contains("Full result stored as blob"));

    let events = session_events(&session);
    let (bytes_omitted, blob_key) = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::SpillApplied {
                bytes_omitted,
                blob_key,
                ..
            } => Some((*bytes_omitted, blob_key.clone())),
            _ => None,
        })
        .expect("spill applied");
    assert!(bytes_omitted > 0);
    assert!(session.read_blob(&blob_key).await.unwrap().is_some());

    let observation = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ToolResult { output, .. } => Some(output.clone()),
            _ => None,
        })
        .expect("tool result logged");
    assert!(observation.stdout.starts_with("row 0 with"));
    assert!(observation.stdout.contains("Full result stored as blob"));

    let reqs = streamer.requests.lock().unwrap();
    assert!(format!("{:?}", reqs[1].messages).contains("Full result stored as blob"));

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn read_file_output_is_not_spilled() {
    let root = std::env::temp_dir().join(format!("meerai-nospill-{}", SessionId::new()));
    let session = persistent_session(&root).await;
    let output = big_output(300);
    let tool = StaticToolset::new("code", "code-read_file", output.clone(), EffectClass::Read)
        .without_spill();
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "c1".to_string(),
            fn_name: "code-read_file".to_string(),
            fn_arguments: serde_json::json!({ "path": "a.rs" }),
            thought_signatures: None,
        }]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let config = HarnessConfig {
        spill_max_inline_tokens: 20,
        ..HarnessConfig::default()
    };
    let mut agent = harness_agent(
        streamer,
        vec![Box::new(tool)],
        "",
        &session,
        auto_mode(),
        config,
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();

    let spilled = session_events(&session)
        .iter()
        .any(|event| matches!(event, SessionEvent::SpillApplied { .. }));
    assert!(!spilled);

    let mut completed = None;
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::ItemCompleted {
            item: SessionItem::ToolCall { output, .. },
        } = event
        {
            completed = output;
        }
    }
    assert_eq!(completed, Some(output));

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn pressure_compaction_brackets_and_shrinks_history() {
    let reply = "x".repeat(800);
    let summary = "Goal: ship the test\nCompleted progress: first turn";
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(text_end(&reply))],
        vec![
            ChatStreamEvent::Chunk(StreamChunk {
                content: summary.to_string(),
            }),
            ChatStreamEvent::End(text_end(summary)),
        ],
        vec![ChatStreamEvent::End(text_end("second reply"))],
    ]));

    let session = memory_session();
    let config = HarnessConfig {
        context_window_tokens: 80,
        high_watermark_pct: 50,
        verbatim_window_turns: 1,
        ..HarnessConfig::default()
    };
    let mut agent = harness_agent(
        streamer.clone(),
        vec![],
        "system",
        &session,
        auto_mode(),
        config,
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("first", &tx, CancellationToken::new())
        .await
        .unwrap();
    let out = agent
        .prompt_streaming("second", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "second reply");

    let events = session_events(&session);
    let kinds: Vec<&str> = events.iter().map(event_kind).collect();
    let start = kinds
        .iter()
        .position(|kind| *kind == "compaction_started")
        .expect("compaction bracketed");
    let summary_index = kinds[start..]
        .iter()
        .position(|kind| *kind == "compaction_summary")
        .map(|offset| start + offset)
        .expect("compaction summary logged");
    let ended_index = kinds[summary_index..]
        .iter()
        .position(|kind| *kind == "compaction_ended")
        .map(|offset| summary_index + offset)
        .expect("compaction ended logged");

    match &events[start] {
        SessionEvent::CompactionStarted {
            trigger: CompactionTrigger::Pressure,
            ..
        } => {}
        other => panic!("expected pressure trigger, got {other:?}"),
    }

    match &events[summary_index] {
        SessionEvent::CompactionSummary { summary, .. } => {
            assert!(summary.starts_with(COMPACTED_MARKER));
        }
        other => panic!("expected compaction summary, got {other:?}"),
    }

    match &events[ended_index] {
        SessionEvent::CompactionEnded {
            result: CompactionResult::Applied { freed_tokens },
            ..
        } => assert!(*freed_tokens > 0),
        other => panic!("expected applied compaction, got {other:?}"),
    }

    let mut items = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::ItemCompleted {
            item:
                SessionItem::Compaction {
                    trigger,
                    freed_tokens,
                    ..
                },
        } = event
        {
            items.push((trigger, freed_tokens));
        }
    }
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].0, CompactionTrigger::Pressure);
    assert!(items[0].1 > 0);

    let policy = ProjectionPolicy {
        strip_ephemera: true,
        prune_reasoning: true,
        verbatim_window_turns: 1,
        clear_old_tool_results: false,
        include_system_messages: false,
        pinned_turns: Vec::new(),
    };
    let derived = session.derive_messages(&policy);
    assert_eq!(derived.len(), 3);
    assert!(
        derived[0]
            .content
            .first_text()
            .is_some_and(|text| text.starts_with(COMPACTED_MARKER))
    );

    let reqs = streamer.requests.lock().unwrap();
    assert_eq!(reqs.len(), 3);
    assert_eq!(reqs[2].messages.len(), 2);
    assert!(
        reqs[1]
            .messages
            .last()
            .and_then(|message| message.content.first_text())
            .is_some_and(|text| text.contains("Goal"))
    );
}

#[tokio::test]
async fn compaction_usage_counts_toward_cumulative() {
    let reply = "y".repeat(800);
    let summary = "Goal: account for summarizer spend";
    let summary_end = StreamEnd {
        captured_content: Some(MessageContent::from_text(summary)),
        captured_usage: Some(Usage {
            prompt_tokens: Some(40),
            completion_tokens: Some(10),
            total_tokens: Some(50),
            ..Default::default()
        }),
        ..Default::default()
    };
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(text_end(&reply))],
        vec![
            ChatStreamEvent::Chunk(StreamChunk {
                content: summary.to_string(),
            }),
            ChatStreamEvent::End(summary_end),
        ],
        vec![ChatStreamEvent::End(text_end("second reply"))],
    ]));

    let session = memory_session();
    let config = HarnessConfig {
        context_window_tokens: 80,
        high_watermark_pct: 50,
        verbatim_window_turns: 1,
        ..HarnessConfig::default()
    };
    let mut agent = harness_agent(
        streamer.clone(),
        vec![],
        "system",
        &session,
        auto_mode(),
        config,
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("first", &tx, CancellationToken::new())
        .await
        .unwrap();
    agent
        .prompt_streaming("second", &tx, CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(agent.cumulative_usage.total_tokens, Some(80));

    let mut live = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::UsageUpdated {
            usage, cumulative, ..
        } = event
        {
            live.push((usage.total_tokens, cumulative.total_tokens));
        }
    }
    assert_eq!(
        live,
        vec![
            (Some(15), Some(15)),
            (Some(50), Some(65)),
            (Some(15), Some(80)),
        ]
    );

    let mut logged = Vec::new();
    for event in session_events(&session) {
        if let SessionEvent::UsageUpdated { usage, cumulative } = event {
            logged.push((usage.total_tokens, cumulative.total_tokens));
        }
    }
    assert_eq!(
        logged,
        vec![
            (Some(15), Some(15)),
            (Some(50), Some(65)),
            (Some(15), Some(80)),
        ]
    );
}

#[tokio::test]
async fn manual_compact_runs_at_step_boundary() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let streamer = Arc::new(GatedStreamer {
        turns: Mutex::new(
            vec![
                vec![ChatStreamEvent::End(text_end("one"))],
                vec![ChatStreamEvent::End(tool_end(vec![tool_call("c1", "ok")]))],
                vec![
                    ChatStreamEvent::Chunk(StreamChunk {
                        content: "Goal: manual compaction".to_string(),
                    }),
                    ChatStreamEvent::End(text_end("Goal: manual compaction")),
                ],
                vec![ChatStreamEvent::End(text_end("two"))],
            ]
            .into(),
        ),
        requests: Mutex::new(Vec::new()),
        gate_index: 1,
        started: started.clone(),
        release: release.clone(),
    });

    let session = memory_session();
    let config = HarnessConfig {
        verbatim_window_turns: 1,
        ..HarnessConfig::default()
    };
    let agent = harness_agent(
        streamer.clone(),
        vec![Box::new(EchoToolset)],
        "",
        &session,
        auto_mode(),
        config,
    );

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt("first".to_string()))
        .unwrap();
    loop {
        if matches!(rx.recv().await, Some(AgentEvent::TurnCompleted { .. })) {
            break;
        }
    }

    cmd_tx
        .send(AgentCommand::SubmitPrompt("second".to_string()))
        .unwrap();
    started.notified().await;
    cmd_tx
        .send(AgentCommand::Compact {
            focus: None,
            from_turn_id: None,
        })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(25)).await;
    release.notify_one();

    loop {
        if matches!(rx.recv().await, Some(AgentEvent::TurnCompleted { .. })) {
            break;
        }
    }

    let events = session_events(&session);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::CompactionStarted {
            trigger: CompactionTrigger::Manual,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::CompactionEnded {
            result: CompactionResult::Applied { .. },
            ..
        }
    )));
    assert_eq!(streamer.requests.lock().unwrap().len(), 4);

    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn context_overflow_retries_once_after_compaction() {
    let summary = "Goal: retried after overflow";
    let streamer = Arc::new(FakeStreamer::with_script(vec![
        ScriptedStream::Events(vec![ChatStreamEvent::End(text_end("one"))]),
        ScriptedStream::Error("maximum context length exceeded".to_string()),
        ScriptedStream::Events(vec![
            ChatStreamEvent::Chunk(StreamChunk {
                content: summary.to_string(),
            }),
            ChatStreamEvent::End(text_end(summary)),
        ]),
        ScriptedStream::Events(vec![ChatStreamEvent::End(text_end("two"))]),
    ]));

    let session = memory_session();
    let config = HarnessConfig {
        verbatim_window_turns: 1,
        ..HarnessConfig::default()
    };
    let mut agent = harness_agent(streamer.clone(), vec![], "", &session, auto_mode(), config);

    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("first", &tx, CancellationToken::new())
        .await
        .unwrap();
    let out = agent
        .prompt_streaming("second", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "two");

    let events = session_events(&session);
    let attempts: Vec<&SessionEvent> = events
        .iter()
        .filter(|event| matches!(event, SessionEvent::AssistantAttempt { .. }))
        .collect();
    assert_eq!(attempts.len(), 1);
    match attempts[0] {
        SessionEvent::AssistantAttempt { error, .. } => {
            assert!(error.contains("maximum context length"));
        }
        other => panic!("expected assistant attempt, got {other:?}"),
    }
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::CompactionStarted {
            trigger: CompactionTrigger::ContextOverflow,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::CompactionEnded {
            result: CompactionResult::Applied { .. },
            ..
        }
    )));

    let reqs = streamer.requests.lock().unwrap();
    assert_eq!(reqs.len(), 4);
    assert!(
        reqs[3].messages[0]
            .content
            .first_text()
            .is_some_and(|text| text.starts_with(COMPACTED_MARKER))
    );
}

#[tokio::test]
async fn second_context_overflow_fails_the_turn() {
    let streamer = Arc::new(FakeStreamer::with_script(vec![
        ScriptedStream::Events(vec![ChatStreamEvent::End(text_end("one"))]),
        ScriptedStream::Error("maximum context length exceeded".to_string()),
        ScriptedStream::Events(vec![ChatStreamEvent::End(text_end("Goal: retried"))]),
        ScriptedStream::Error("maximum context length exceeded".to_string()),
    ]));

    let session = memory_session();
    let config = HarnessConfig {
        verbatim_window_turns: 1,
        ..HarnessConfig::default()
    };
    let agent = harness_agent(streamer.clone(), vec![], "", &session, auto_mode(), config);

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt("first".to_string()))
        .unwrap();
    loop {
        if matches!(rx.recv().await, Some(AgentEvent::TurnCompleted { .. })) {
            break;
        }
    }
    cmd_tx
        .send(AgentCommand::SubmitPrompt("second".to_string()))
        .unwrap();

    let mut code = None;
    let mut failure = None;
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::Error {
                code: error_code, ..
            } => code = Some(error_code),
            AgentEvent::TurnCompleted {
                reason: TurnEndReason::Failed { error },
                ..
            } => {
                failure = Some(error);
                break;
            }
            AgentEvent::TurnCompleted { .. } => break,
            _ => {}
        }
    }
    assert_eq!(code, Some(ErrorCode::ContextOverflow));
    assert!(
        failure
            .as_deref()
            .is_some_and(|message| message.contains("maximum context length"))
    );

    let events = session_events(&session);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::Error {
            code: ErrorCode::ContextOverflow,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::TurnEnded {
            reason: TurnEndReason::Failed { .. },
            ..
        }
    )));

    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn context_overflow_does_not_retry_when_compaction_skips() {
    let streamer = Arc::new(FakeStreamer::with_script(vec![ScriptedStream::Error(
        "maximum context length exceeded".to_string(),
    )]));

    let session = memory_session();
    let config = HarnessConfig {
        verbatim_window_turns: 4,
        ..HarnessConfig::default()
    };
    let mut agent = harness_agent(streamer.clone(), vec![], "", &session, auto_mode(), config);

    let (tx, _rx) = mpsc::unbounded_channel();
    let err = agent
        .prompt_streaming("too big", &tx, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("maximum context length"));

    let reqs = streamer.requests.lock().unwrap();
    assert_eq!(reqs.len(), 1);

    let events = session_events(&session);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::CompactionEnded {
            result: CompactionResult::Skipped { .. },
            ..
        }
    )));
}

#[tokio::test]
async fn idle_compact_appends_bracketed_events() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(text_end("first reply"))],
        vec![
            ChatStreamEvent::Chunk(StreamChunk {
                content: "Goal: idle compaction".to_string(),
            }),
            ChatStreamEvent::End(text_end("Goal: idle compaction")),
        ],
    ]));

    let session = memory_session();
    let config = HarnessConfig {
        verbatim_window_turns: 0,
        ..HarnessConfig::default()
    };
    let agent = harness_agent(streamer.clone(), vec![], "", &session, auto_mode(), config);

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt("first".to_string()))
        .unwrap();
    loop {
        match rx.recv().await {
            Some(AgentEvent::TurnCompleted { .. }) => break,
            Some(_) => {}
            None => panic!("worker stopped mid-turn"),
        }
    }

    tokio::time::sleep(Duration::from_millis(50)).await;
    cmd_tx
        .send(AgentCommand::Compact {
            focus: None,
            from_turn_id: None,
        })
        .unwrap();
    let compacted = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = rx.recv().await {
            if matches!(
                event,
                AgentEvent::ItemCompleted {
                    item: SessionItem::Compaction { .. },
                }
            ) {
                return true;
            }
        }
        false
    })
    .await
    .expect("idle compaction did not emit an item");
    assert!(compacted);

    let events = session_events(&session);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::CompactionStarted {
            trigger: CompactionTrigger::Manual,
            ..
        }
    )));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::CompactionSummary { .. }))
    );
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::CompactionEnded {
            result: CompactionResult::Applied { .. },
            ..
        }
    )));
    assert_eq!(streamer.requests.lock().unwrap().len(), 2);

    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn summarizer_model_is_used_for_compaction() {
    let reply = "x".repeat(800);
    let summary = "Goal: use the summarizer model";
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(text_end(&reply))],
        vec![ChatStreamEvent::End(text_end(summary))],
        vec![ChatStreamEvent::End(text_end("second reply"))],
    ]));

    let session = memory_session();
    let config = HarnessConfig {
        context_window_tokens: 80,
        high_watermark_pct: 50,
        verbatim_window_turns: 1,
        summarizer_model: Some("openrouter:tiny-summarizer".to_string()),
        ..HarnessConfig::default()
    };
    let mut agent = harness_agent(
        streamer.clone(),
        vec![],
        "system",
        &session,
        auto_mode(),
        config,
    );

    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("first", &tx, CancellationToken::new())
        .await
        .unwrap();
    agent
        .prompt_streaming("second", &tx, CancellationToken::new())
        .await
        .unwrap();

    let models = streamer.models.lock().unwrap().clone();
    assert_eq!(
        models,
        vec![
            "gpt-4o-mini".to_string(),
            "openrouter:tiny-summarizer".to_string(),
            "gpt-4o-mini".to_string(),
        ]
    );
    assert!(session_events(&session).iter().any(|event| matches!(
        event,
        SessionEvent::CompactionEnded {
            result: CompactionResult::Applied { .. },
            ..
        }
    )));
}

#[tokio::test]
async fn tool_schema_tokens_count_toward_the_watermark() {
    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("done"),
    )]]));
    let session = memory_session();
    let mut agent = MultiTurnAgent::with_streamer(
        streamer,
        "test-model",
        vec![Box::new(LargeSchemaToolset)],
        "",
        MultiTurnAgentConfig::default(),
    );
    let config = HarnessConfig {
        context_window_tokens: 2_000,
        high_watermark_pct: 100,
        ..HarnessConfig::default()
    };
    agent.set_harness(harness(&session, auto_mode(), config));

    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();

    assert!(
        agent.tool_schema_tokens > 20_000,
        "large schema must be counted, got {}",
        agent.tool_schema_tokens
    );
    assert!(agent.over_high_watermark());
}

#[tokio::test]
async fn pressure_compaction_can_run_inside_a_long_turn() {
    let streamer = Arc::new(LongTurnStreamer {
        tool_steps: AtomicUsize::new(0),
        summaries: AtomicUsize::new(0),
        steps: 6,
    });
    let session = memory_session();
    let config = HarnessConfig {
        context_window_tokens: 100,
        high_watermark_pct: 10,
        verbatim_window_turns: 2,
        ..HarnessConfig::default()
    };
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(EchoToolset)],
        "system",
        &session,
        auto_mode(),
        config,
    );

    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "done");
    assert!(streamer.summaries.load(Ordering::SeqCst) >= 1);
    let applied = session_events(&session).iter().any(|event| {
        matches!(
            event,
            SessionEvent::CompactionEnded {
                result: CompactionResult::Applied { .. },
                ..
            }
        )
    });
    assert!(applied, "in-turn pressure compaction must apply");
}

struct LongTurnStreamer {
    tool_steps: AtomicUsize,
    summaries: AtomicUsize,
    steps: usize,
}

#[async_trait]
impl ChatStreamer for LongTurnStreamer {
    async fn stream_chat(
        &self,
        _provider: Option<&str>,
        _model: &str,
        request: ChatRequest,
        _options: &ChatOptions,
    ) -> Result<BoxedChatStream> {
        let wants_summary = request
            .system
            .as_deref()
            .is_some_and(|system| system.contains("Summarize the transcript"))
            || request
                .messages
                .last()
                .and_then(|message| message.content.first_text())
                .is_some_and(|text| text.contains("Summarize the transcript"));
        if wants_summary {
            self.summaries.fetch_add(1, Ordering::SeqCst);
            let events: Vec<Result<ChatStreamEvent>> = vec![
                Ok(ChatStreamEvent::Chunk(StreamChunk {
                    content: "Goal: finish".to_string(),
                })),
                Ok(ChatStreamEvent::End(text_end("Goal: finish"))),
            ];
            return Ok(Box::pin(stream::iter(events)));
        }
        let step = self.tool_steps.fetch_add(1, Ordering::SeqCst);
        let events: Vec<Result<ChatStreamEvent>> = if step < self.steps {
            vec![Ok(ChatStreamEvent::End(tool_end(vec![tool_call(
                &format!("call-{step}"),
                &format!("go-{step}"),
            )])))]
        } else {
            vec![Ok(ChatStreamEvent::End(text_end("done")))]
        };
        Ok(Box::pin(stream::iter(events)))
    }
}

struct SummaryProbeStreamer {
    requests: Mutex<Vec<ChatRequest>>,
    turn_requests: AtomicUsize,
}

#[async_trait]
impl ChatStreamer for SummaryProbeStreamer {
    async fn stream_chat(
        &self,
        _provider: Option<&str>,
        _model: &str,
        request: ChatRequest,
        _options: &ChatOptions,
    ) -> Result<BoxedChatStream> {
        let wants_summary = request
            .messages
            .last()
            .and_then(|message| message.content.first_text())
            .is_some_and(|text| text.contains("Summarize the transcript"));
        self.requests.lock().unwrap().push(request);
        if wants_summary {
            let events: Vec<Result<ChatStreamEvent>> =
                vec![Ok(ChatStreamEvent::End(text_end("Goal: probe")))];
            return Ok(Box::pin(stream::iter(events)));
        }
        let index = self.turn_requests.fetch_add(1, Ordering::SeqCst);
        let events: Vec<Result<ChatStreamEvent>> = if index.is_multiple_of(2) {
            vec![Ok(ChatStreamEvent::End(tool_end(vec![tool_call(
                &format!("call-{index}"),
                &format!("go-{index}"),
            )])))]
        } else {
            vec![Ok(ChatStreamEvent::End(text_end(&format!(
                "reply-{index}"
            ))))]
        };
        Ok(Box::pin(stream::iter(events)))
    }
}

#[tokio::test]
async fn compaction_reuses_the_parent_prefix_for_cache_hits() {
    let streamer = Arc::new(SummaryProbeStreamer {
        requests: Mutex::new(Vec::new()),
        turn_requests: AtomicUsize::new(0),
    });
    let session = memory_session();
    let config = HarnessConfig {
        verbatim_window_turns: 1,
        ..HarnessConfig::default()
    };
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(EchoToolset)],
        "system",
        &session,
        auto_mode(),
        config,
    );

    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("one", &tx, CancellationToken::new())
        .await
        .unwrap();
    agent
        .prompt_streaming("two", &tx, CancellationToken::new())
        .await
        .unwrap();

    let (compact_tx, _compact_rx) = mpsc::unbounded_channel();
    let applied = agent
        .run_compaction(
            CompactionTrigger::Manual,
            &compact_tx,
            &CancellationToken::new(),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert!(applied, "manual compaction must apply");

    let requests = streamer.requests.lock().unwrap();
    let summary = requests
        .iter()
        .find(|request| {
            request
                .messages
                .last()
                .and_then(|message| message.content.first_text())
                .is_some_and(|text| text.contains("Summarize the transcript"))
        })
        .expect("summary request");
    let parent = requests
        .iter()
        .rfind(|request| {
            !request
                .messages
                .last()
                .and_then(|message| message.content.first_text())
                .is_some_and(|text| text.contains("Summarize the transcript"))
        })
        .expect("parent request");
    assert_eq!(summary.system, parent.system);
    assert!(
        summary
            .tools
            .as_ref()
            .is_some_and(|tools| !tools.is_empty()),
        "summary request must carry the parent tool definitions"
    );
    assert_eq!(
        serde_json::to_value(&summary.tools).unwrap(),
        serde_json::to_value(&parent.tools).unwrap(),
        "summary tools must match the parent byte for byte"
    );
    let prefix = parent.messages.len();
    assert_eq!(
        serde_json::to_value(&summary.messages[..prefix]).unwrap(),
        serde_json::to_value(&parent.messages).unwrap(),
        "summary history must reuse the parent prefix"
    );
}

#[tokio::test]
async fn compaction_captures_project_memories_when_enabled() {
    use meerai_store::Store;

    let summary = "Goal: ship the store\nCompleted progress\n- schema\n- vault\n\
                   Key decisions\n- Use SQLite\n- Keep overlays file-based\n\
                   Open blockers\n- none";
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(text_end("one"))],
        vec![ChatStreamEvent::End(text_end("two"))],
        vec![
            ChatStreamEvent::Chunk(StreamChunk {
                content: summary.to_string(),
            }),
            ChatStreamEvent::End(text_end(summary)),
        ],
    ]));
    let session = memory_session();
    let config = HarnessConfig {
        verbatim_window_turns: 1,
        auto_memory: true,
        ..HarnessConfig::default()
    };
    let mut agent = harness_agent(streamer, vec![], "system", &session, auto_mode(), config);
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("memory.db")).unwrap();
    agent.set_memory_capture(crate::memory::MemoryCapture::new(
        store.clone(),
        dir.path(),
        None,
    ));

    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("one", &tx, CancellationToken::new())
        .await
        .unwrap();
    agent
        .prompt_streaming("two", &tx, CancellationToken::new())
        .await
        .unwrap();
    let (compact_tx, _compact_rx) = mpsc::unbounded_channel();
    let applied = agent
        .run_compaction(
            CompactionTrigger::Manual,
            &compact_tx,
            &CancellationToken::new(),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert!(applied);

    let memories = store
        .memories()
        .list(&dir.path().display().to_string())
        .unwrap();
    assert!(
        memories
            .iter()
            .any(|memory| memory.title == "ship the store")
    );
    assert!(
        memories
            .iter()
            .filter(|memory| memory.kind == meerai_store::ProjectMemoryKind::Decision)
            .count()
            >= 2
    );
}

#[tokio::test]
async fn plan_usage_compaction_sends_no_cache_control() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(text_end("one"))],
        vec![ChatStreamEvent::End(text_end("two"))],
        vec![ChatStreamEvent::End(text_end("Goal: summary"))],
    ]));
    let session = memory_session();
    let config = HarnessConfig {
        plan_usage: true,
        verbatim_window_turns: 1,
        ..HarnessConfig::default()
    };
    let mut agent = harness_agent(
        streamer.clone(),
        Vec::new(),
        "system",
        &session,
        auto_mode(),
        config,
    );

    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("one", &tx, CancellationToken::new())
        .await
        .unwrap();
    agent
        .prompt_streaming("two", &tx, CancellationToken::new())
        .await
        .unwrap();

    let (compact_tx, _compact_rx) = mpsc::unbounded_channel();
    let applied = agent
        .run_compaction(
            CompactionTrigger::Manual,
            &compact_tx,
            &CancellationToken::new(),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert!(applied, "manual compaction must apply");

    let summary_index = {
        let requests = streamer.requests.lock().unwrap();
        requests
            .iter()
            .position(|request| {
                request
                    .messages
                    .last()
                    .and_then(|message| message.content.first_text())
                    .is_some_and(|text| text.contains("Summarize the transcript"))
            })
            .expect("summary request")
    };

    let cached = streamer.options.lock().unwrap();
    assert_eq!(cached[summary_index].cache_control, None);
}
