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
async fn executes_multiple_tool_calls_and_feeds_failures_back() {
    let calls = vec![
        ToolCall {
            call_id: "c1".to_string(),
            fn_name: "echo-say".to_string(),
            fn_arguments: serde_json::json!({"text": "ok"}),
            thought_signatures: None,
        },
        ToolCall {
            call_id: "c2".to_string(),
            fn_name: "echo-say".to_string(),
            fn_arguments: serde_json::json!({"text": "fail"}),
            thought_signatures: None,
        },
    ];

    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(calls))],
        vec![
            ChatStreamEvent::Chunk(StreamChunk {
                content: "Handled both tools".to_string(),
            }),
            ChatStreamEvent::End(text_end("Handled both tools")),
        ],
    ]));

    let mut agent = MultiTurnAgent::with_streamer(
        streamer.clone(),
        "gpt-4o-mini",
        vec![Box::new(EchoToolset)],
        "",
        MultiTurnAgentConfig::default(),
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("Run tools", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "Handled both tools");

    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }

    let mut started = Vec::new();
    let mut running = Vec::new();
    let mut completed = Vec::new();
    for ev in &events {
        match ev {
            AgentEvent::ItemStarted {
                item:
                    SessionItem::ToolCall {
                        call_id,
                        name,
                        args,
                        effect,
                        status,
                        output,
                        ..
                    },
            } => {
                started.push((call_id.clone(), name.clone(), args.clone(), *effect));
                assert_eq!(*status, ItemStatus::Proposed);
                assert!(output.is_none());
            }
            AgentEvent::ItemDelta {
                delta: ItemDelta::StatusChanged(status),
                ..
            } => running.push(*status),
            AgentEvent::ItemCompleted {
                item:
                    SessionItem::ToolCall {
                        call_id,
                        status,
                        output,
                        ..
                    },
            } => completed.push((call_id.clone(), *status, output.clone())),
            _ => {}
        }
    }

    assert_eq!(started.len(), 2);
    assert_eq!(started[0].0, CallId::from_raw("c1"));
    assert_eq!(started[0].1, "echo-say");
    assert_eq!(started[0].2, "{\"text\":\"ok\"}");
    assert_eq!(started[0].3, EffectClass::Read);
    assert_eq!(started[1].0, CallId::from_raw("c2"));
    assert_eq!(
        running,
        vec![ItemStatus::InProgress, ItemStatus::InProgress]
    );
    assert_eq!(
        completed,
        vec![
            (
                CallId::from_raw("c1"),
                ItemStatus::Succeeded,
                Some("echoed:ok".to_string())
            ),
            (
                CallId::from_raw("c2"),
                ItemStatus::Failed,
                Some("soft failure".to_string())
            ),
        ]
    );

    assert!(events.iter().any(|ev| matches!(
        ev,
        AgentEvent::TurnCompleted {
            reason: TurnEndReason::Completed,
            ..
        }
    )));

    let reqs = streamer.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[1].messages.len(), 4);
}

#[tokio::test]
async fn stops_immediately_on_tool_output_stop() {
    let calls = vec![ToolCall {
        call_id: "c-stop".to_string(),
        fn_name: "echo-say".to_string(),
        fn_arguments: serde_json::json!({"text": "stop"}),
        thought_signatures: None,
    }];

    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        tool_end(calls),
    )]]));

    let mut agent = MultiTurnAgent::with_streamer(
        streamer,
        "gpt-4o-mini",
        vec![Box::new(EchoToolset)],
        "",
        MultiTurnAgentConfig::default(),
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("Stop please", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "done now");

    let mut stopped = None;
    let mut completed_turn = false;
    while let Ok(ev) = rx.try_recv() {
        match ev {
            AgentEvent::ItemCompleted {
                item: SessionItem::ToolCall { status, output, .. },
            } => stopped = Some((status, output)),
            AgentEvent::TurnCompleted {
                reason: TurnEndReason::Completed,
                ..
            } => completed_turn = true,
            _ => {}
        }
    }
    assert_eq!(
        stopped,
        Some((ItemStatus::Stopped, Some("done now".to_string())))
    );
    assert!(completed_turn);
}

#[tokio::test]
async fn harness_preserves_thought_signatures_across_tool_steps() {
    let mut call = tool_call("c1", "hello");
    call.thought_signatures = Some(vec!["sig-1".to_string()]);
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![call]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let session = memory_session();
    let mut agent = MultiTurnAgent::with_streamer(
        streamer.clone(),
        "gemini::gemini-2.5-pro",
        vec![Box::new(EchoToolset)],
        "Be precise.",
        MultiTurnAgentConfig::default(),
    );
    agent.set_harness(harness(&session, auto_mode(), HarnessConfig::default()));

    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("do it", &tx, CancellationToken::new())
        .await
        .unwrap();

    let logged: Vec<Vec<Option<Vec<String>>>> = session_events(&session)
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AssistantMessage {
                call_signatures, ..
            } if !call_signatures.is_empty() => Some(call_signatures.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(logged, vec![vec![Some(vec!["sig-1".to_string()])]]);

    let requests = streamer.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let replayed = requests[1]
        .messages
        .iter()
        .flat_map(|message| message.content.tool_calls())
        .find_map(|call| call.thought_signatures.clone());
    assert_eq!(replayed, Some(vec!["sig-1".to_string()]));
}

#[tokio::test]
async fn tool_timeout_fails_the_call() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "c1".to_string(),
            fn_name: "slow-run".to_string(),
            fn_arguments: serde_json::json!({}),
            thought_signatures: None,
        }]))],
        vec![ChatStreamEvent::End(text_end("after timeout"))],
    ]));

    let session = memory_session();
    let config = HarnessConfig {
        tool_timeout_ms: 10,
        ..HarnessConfig::default()
    };
    let mut agent = MultiTurnAgent::with_streamer(
        streamer.clone(),
        "gpt-4o-mini",
        vec![Box::new(SlowToolset)],
        "",
        MultiTurnAgentConfig::default(),
    );
    agent.set_harness(harness(&session, auto_mode(), config));

    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("run", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "after timeout");

    let observation = session_events(&session)
        .into_iter()
        .find_map(|event| match event {
            SessionEvent::ToolResult { output, .. } => Some(output),
            _ => None,
        })
        .expect("tool result logged");
    assert_eq!(observation.status, ObservationStatus::Error);
    assert!(observation.stderr.contains("tool timed out after 10ms"));

    let reqs = streamer.requests.lock().unwrap();
    assert!(format!("{:?}", reqs[1].messages).contains("tool timed out after 10ms"));
}

#[tokio::test]
async fn final_step_strips_tools_and_requests_summary() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![tool_call("c1", "ok")]))],
        vec![ChatStreamEvent::End(text_end("wrapped up"))],
    ]));

    let session = memory_session();
    let config = HarnessConfig {
        max_steps_per_turn: 2,
        step_limit_continuations: 0,
        ..HarnessConfig::default()
    };
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(EchoToolset)],
        "",
        &session,
        auto_mode(),
        config,
    );

    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "wrapped up");

    let reqs = streamer.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    assert!(reqs[0].tools.is_some());
    assert!(reqs[1].tools.is_none());
    assert!(format!("{:?}", reqs[1].messages).contains("Step limit reached."));

    let notice = session_events(&session).iter().any(|event| {
        matches!(
            event,
            SessionEvent::UserMessage { source: InputSource::RuntimeContext, content }
                if content.iter().any(|block| matches!(
                    block,
                    ContentBlock::Text(text) if text.starts_with("Step limit reached.")
                ))
        )
    });
    assert!(notice);
}

#[tokio::test]
async fn step_budget_continuation_keeps_tools_and_asks_to_finish() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![tool_call("c1", "a")]))],
        vec![ChatStreamEvent::End(tool_end(vec![tool_call("c2", "b")]))],
        vec![ChatStreamEvent::End(tool_end(vec![tool_call("c3", "c")]))],
        vec![ChatStreamEvent::End(text_end("finished the work"))],
    ]));

    let session = memory_session();
    let config = HarnessConfig {
        max_steps_per_turn: 2,
        step_limit_continuations: 1,
        ..HarnessConfig::default()
    };
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(EchoToolset)],
        "",
        &session,
        auto_mode(),
        config,
    );

    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "finished the work");

    let reqs = streamer.requests.lock().unwrap();
    assert_eq!(reqs.len(), 4);
    assert!(reqs[0].tools.is_some());
    assert!(reqs[1].tools.is_some());
    assert!(reqs[2].tools.is_some(), "a continuation keeps the tools");
    assert!(
        reqs[3].tools.is_none(),
        "the absolute final step strips tools"
    );
    assert!(
        format!("{:?}", reqs[2].messages).contains("Step budget reached."),
        "the continuation notice must reach the request"
    );
    assert!(format!("{:?}", reqs[3].messages).contains("Step limit reached."));

    let notices: Vec<String> = session_events(&session)
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::UserMessage {
                source: InputSource::RuntimeContext,
                content,
            } => content.into_iter().find_map(|block| match block {
                ContentBlock::Text(text) => Some(text),
                _ => None,
            }),
            _ => None,
        })
        .collect();
    assert!(
        notices
            .iter()
            .any(|text| text.starts_with("Step budget reached."))
    );
    assert!(
        notices
            .iter()
            .any(|text| text.starts_with("Step limit reached."))
    );
}

#[tokio::test]
async fn tool_batches_preserve_call_order_across_effect_classes() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![
            bare_call("c1", "order-write"),
            bare_call("c2", "order-read"),
        ]))],
        vec![ChatStreamEvent::End(text_end("first done"))],
        vec![ChatStreamEvent::End(tool_end(vec![
            bare_call("c3", "order-read"),
            bare_call("c4", "order-write"),
            bare_call("c5", "order-read"),
        ]))],
        vec![ChatStreamEvent::End(text_end("second done"))],
    ]));

    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(OrderToolset {
            order: order.clone(),
        })],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );

    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("first", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(*order.lock().unwrap(), vec!["write", "read"]);

    agent
        .prompt_streaming("second", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        *order.lock().unwrap(),
        vec!["write", "read", "read", "write", "read"]
    );

    let results: Vec<(String, String)> = session_events(&session)
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::ToolResult { call_id, output } => {
                Some((call_id.as_str().to_string(), output.stdout))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        results,
        vec![
            ("c1".to_string(), "did:write".to_string()),
            ("c2".to_string(), "did:read".to_string()),
            ("c3".to_string(), "did:read".to_string()),
            ("c4".to_string(), "did:write".to_string()),
            ("c5".to_string(), "did:read".to_string()),
        ]
    );
}

#[tokio::test]
async fn stop_skips_the_rest_of_the_tool_batch() {
    let calls = Arc::new(AtomicUsize::new(0));
    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        tool_end(vec![
            bare_call("c1", "batch-stop"),
            bare_call("c2", "batch-after"),
        ]),
    )]]));

    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(StopBatchToolset {
            effect: EffectClass::Write,
            calls: calls.clone(),
        })],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("stop it", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "stopped the batch");
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let mut completed = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::ItemCompleted {
            item: SessionItem::ToolCall {
                call_id, status, ..
            },
        } = event
        {
            completed.push((call_id, status));
        }
    }
    assert_eq!(
        completed,
        vec![
            (CallId::from_raw("c1"), ItemStatus::Stopped),
            (CallId::from_raw("c2"), ItemStatus::Cancelled),
        ]
    );

    let results: Vec<(String, ObservationStatus, String)> = session_events(&session)
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::ToolResult { call_id, output } => {
                Some((call_id.as_str().to_string(), output.status, output.stderr))
            }
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].0, "c1");
    assert_eq!(results[0].1, ObservationStatus::Stopped);
    assert_eq!(results[1].0, "c2");
    assert_eq!(results[1].1, ObservationStatus::Error);
    assert!(results[1].2.contains(NOT_EXECUTED_STOPPED));

    let reqs = streamer.requests.lock().unwrap();
    assert_eq!(reqs.len(), 1);
    drop(reqs);
    assert_eq!(agent.history().len(), 4);
}

#[tokio::test]
async fn captured_reasoning_only_stream_is_not_empty() {
    let end = StreamEnd {
        captured_reasoning_content: Some("considering the repository".to_string()),
        captured_stop_reason: Some(StopReason::Completed("STOP".to_string())),
        ..Default::default()
    };
    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(end)]]));

    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        vec![],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );

    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(out, "");
    assert_eq!(streamer.requests.lock().unwrap().len(), 1);
    assert!(session_events(&session).iter().any(|event| matches!(
        event,
        SessionEvent::AssistantMessage { content, .. }
            if content.iter().any(|block| matches!(
                block,
                ContentBlock::Reasoning(text) if text == "considering the repository"
            ))
    )));
}

#[tokio::test]
async fn harness_requests_include_previous_assistant_turn_and_tool_results() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(text_end("first reply"))],
        vec![ChatStreamEvent::End(text_end("second reply"))],
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        vec![],
        "system",
        &session,
        auto_mode(),
        HarnessConfig::default(),
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

    let reqs = streamer.requests.lock().unwrap();
    let debug = format!("{:?}", reqs[1].messages);
    println!("REQ1: {debug}");
    assert!(
        debug.contains("first reply"),
        "missing assistant reply: {debug}"
    );
    assert!(debug.contains("second"), "missing new prompt: {debug}");
    assert_eq!(reqs[1].messages.len(), 3, "{debug}");
}

#[tokio::test]
async fn harness_tool_step_requests_include_calls_and_results() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "c1".to_string(),
            fn_name: "echo-say".to_string(),
            fn_arguments: serde_json::json!({"text": "hi"}),
            thought_signatures: None,
        }]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(EchoToolset)],
        "system",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();

    let reqs = streamer.requests.lock().unwrap();
    let debug = format!("{:?}", reqs[1].messages);
    println!("TOOL REQ: {debug}");
    assert!(debug.contains("echoed:hi"), "{debug}");
    assert!(debug.contains("ToolCall"), "{debug}");
    assert_eq!(reqs[1].messages.len(), 3, "{debug}");
}

#[tokio::test]
async fn harness_two_tool_calls_pair_both_results() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![
            ToolCall {
                call_id: "c1".to_string(),
                fn_name: "echo-say".to_string(),
                fn_arguments: serde_json::json!({"text": "one"}),
                thought_signatures: None,
            },
            ToolCall {
                call_id: "c2".to_string(),
                fn_name: "echo-say".to_string(),
                fn_arguments: serde_json::json!({"text": "two"}),
                thought_signatures: None,
            },
        ]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(EchoToolset)],
        "system",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();

    let reqs = streamer.requests.lock().unwrap();
    let debug = format!("{:?}", reqs[1].messages);
    println!("TWO REQ: {debug}");
    assert!(debug.contains("echoed:one"), "{debug}");
    assert!(debug.contains("echoed:two"), "{debug}");
    assert_eq!(reqs[1].messages.len(), 4, "{debug}");
}

#[tokio::test]
async fn failed_tool_result_is_persisted_and_sent_next_step() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "c1".to_string(),
            fn_name: "echo-say".to_string(),
            fn_arguments: serde_json::json!({"text": "fail"}),
            thought_signatures: None,
        }]))],
        vec![ChatStreamEvent::End(text_end("after failure"))],
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(EchoToolset)],
        "system",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "after failure");

    let events = session_events(&session);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::AssistantMessage { tool_calls, .. } if !tool_calls.is_empty()
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::ToolResult { output, .. } if output.render().contains("soft failure")
    )));

    let reqs = streamer.requests.lock().unwrap();
    let debug = format!("{:?}", reqs[1].messages);
    assert!(debug.contains("soft failure"), "{debug}");
    assert!(debug.contains("ToolCall"), "{debug}");
}

#[tokio::test]
async fn tool_dispatch_coerces_arguments_to_the_schema() {
    let seen = Arc::new(Mutex::new(serde_json::json!({})));
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "c1".to_string(),
            fn_name: "coerce-run".to_string(),
            fn_arguments: serde_json::json!({ "count": "3" }),
            thought_signatures: None,
        }]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer,
        vec![Box::new(CoerceToolset { seen: seen.clone() })],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(*seen.lock().unwrap(), serde_json::json!({ "count": 3 }));
}

#[tokio::test]
async fn wrong_arguments_add_a_schema_repair_hint() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "c1".to_string(),
            fn_name: "invalid-run".to_string(),
            fn_arguments: serde_json::json!({ "count": 3 }),
            thought_signatures: None,
        }]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer,
        vec![Box::new(InvalidArgsToolset)],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();

    let hint = session_events(&session).iter().any(|event| {
        matches!(
            event,
            SessionEvent::UserMessage { source: InputSource::RuntimeContext, content }
                if content.iter().any(|block| matches!(
                    block,
                    ContentBlock::Text(text)
                        if text.contains("Retry the same tool") && text.contains("\"count\"")
                ))
        )
    });
    assert!(hint, "argument repair hint must include the schema");
}

#[tokio::test]
async fn tool_call_chunks_execute_when_capture_is_empty() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![
            ChatStreamEvent::ToolCallChunk(genai::chat::ToolChunk {
                tool_call: ToolCall {
                    call_id: "chunk-call".to_string(),
                    fn_name: "echo-say".to_string(),
                    fn_arguments: serde_json::json!({"text": "chunked"}),
                    thought_signatures: None,
                },
            }),
            ChatStreamEvent::End(StreamEnd::default()),
        ],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(EchoToolset)],
        "system",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "done");

    let events = session_events(&session);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::ToolResult { output, .. } if output.render().contains("echoed:chunked")
    )));
}

#[tokio::test]
async fn later_tool_call_chunks_replace_earlier_partial_arguments() {
    let partial = genai::chat::ToolChunk {
        tool_call: ToolCall {
            call_id: "chunk-call".to_string(),
            fn_name: "echo-say".to_string(),
            fn_arguments: serde_json::Value::String(String::new()),
            thought_signatures: None,
        },
    };
    let complete = genai::chat::ToolChunk {
        tool_call: ToolCall {
            call_id: "chunk-call".to_string(),
            fn_name: "echo-say".to_string(),
            fn_arguments: serde_json::json!({"text": "chunked"}),
            thought_signatures: None,
        },
    };
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![
            ChatStreamEvent::ToolCallChunk(partial),
            ChatStreamEvent::ToolCallChunk(complete),
            ChatStreamEvent::End(StreamEnd::default()),
        ],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(EchoToolset)],
        "system",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "done");

    let events = session_events(&session);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::ToolResult { output, .. } if output.render().contains("echoed:chunked")
    )));
}

#[tokio::test]
async fn captured_signature_fills_streamed_call_without_one() {
    let mut captured = tool_call("c1", "hello");
    captured.thought_signatures = Some(vec!["sig-late".to_string()]);
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![
            ChatStreamEvent::ToolCallChunk(genai::chat::ToolChunk {
                tool_call: tool_call("c1", "hello"),
            }),
            ChatStreamEvent::End(tool_end(vec![captured])),
        ],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(EchoToolset)],
        "system",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();

    let signatures = session_events(&session)
        .iter()
        .find_map(|event| match event {
            SessionEvent::AssistantMessage {
                call_signatures, ..
            } if !call_signatures.is_empty() => Some(call_signatures.clone()),
            _ => None,
        })
        .expect("assistant message with calls");
    assert_eq!(signatures, vec![Some(vec!["sig-late".to_string()])]);
}

#[tokio::test]
async fn double_encoded_tool_arguments_are_normalized() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "c1".to_string(),
            fn_name: "echo-say".to_string(),
            fn_arguments: serde_json::Value::String(r#"{"text":"hi"}"#.to_string()),
            thought_signatures: None,
        }]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(EchoToolset)],
        "system",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "done");

    let events = session_events(&session);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::ToolCall { args, .. } if args == r#"{"text":"hi"}"#
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::ToolResult { output, .. } if output.render().contains("echoed:hi")
    )));
}

struct WrongArgsToolset {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Toolset for WrongArgsToolset {
    fn name(&self) -> String {
        "wrong".to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            r#type: "function".to_string(),
            name: "wrong-call".to_string(),
            description: "always fails to parse".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }]
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == "wrong-call"
    }

    async fn invoke(&self, _fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Err(ToolError::WrongArguments(
            serde_json::from_str::<u32>("not a number").unwrap_err(),
        ))
    }
}

#[tokio::test]
async fn argument_parse_errors_are_not_retried() {
    let calls = Arc::new(AtomicUsize::new(0));
    let toolset = WrongArgsToolset {
        calls: calls.clone(),
    };
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "c1".to_string(),
            fn_name: "wrong-call".to_string(),
            fn_arguments: serde_json::json!({}),
            thought_signatures: None,
        }]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer,
        vec![Box::new(toolset)],
        "system",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn requests_include_tool_schemas() {
    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("hi"),
    )]]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(EchoToolset)],
        "system",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();

    let reqs = streamer.requests.lock().unwrap();
    let tools = reqs[0].tools.as_ref().expect("tools sent");
    let echo = tools
        .iter()
        .find(|tool| tool.name.to_string() == "echo-say")
        .expect("echo-say tool");
    let schema = echo.schema.as_ref().expect("schema");
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["text"]["type"], "string");
}

#[tokio::test]
async fn repeated_cancelled_questions_disable_the_tool() {
    let question_call = |id: &str| ToolCall {
        call_id: id.to_string(),
        fn_name: "question-ask".to_string(),
        fn_arguments: serde_json::json!({
            "questions": [{"header": "DB", "question": "Which?", "options": ["a", "b"]}]
        }),
        thought_signatures: None,
    };
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![question_call("q1")]))],
        vec![ChatStreamEvent::End(tool_end(vec![question_call("q2")]))],
        vec![ChatStreamEvent::End(tool_end(vec![question_call("q3")]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer,
        vec![Box::new(crate::question::QuestionToolset)],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );

    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "done");

    let events = session.events();
    let asked = events
        .iter()
        .filter(|entry| matches!(&entry.event, SessionEvent::QuestionAsked { .. }))
        .count();
    assert_eq!(asked, 2);
    let results: Vec<&ToolObservation> = events
        .iter()
        .filter_map(|entry| match &entry.event {
            SessionEvent::ToolResult { output, .. } => Some(output),
            _ => None,
        })
        .collect();
    assert!(results.iter().any(|output| {
        output.stdout.contains("question-ask is disabled")
            || output.stderr.contains("question-ask is disabled")
    }));
}

#[tokio::test]
async fn post_edit_diagnostics_returns_lsp_text() {
    let tools: Vec<Box<dyn Toolset>> = vec![Box::new(StaticToolset::new(
        "lsp",
        "lsp-get_diagnostics",
        "warning: unused variable",
        EffectClass::Read,
    ))];

    let text = crate::multi_turn_agent::tools::collect_post_edit_diagnostics(&tools, "src/main.rs")
        .await
        .unwrap();
    assert_eq!(text, "warning: unused variable");
}

#[tokio::test]
async fn post_edit_diagnostics_without_lsp_tool_returns_none() {
    let tools: Vec<Box<dyn Toolset>> = Vec::new();
    assert!(
        crate::multi_turn_agent::tools::collect_post_edit_diagnostics(&tools, "src/main.rs")
            .await
            .is_none()
    );
}

#[tokio::test]
async fn effort_tier_flows_into_chat_options_and_plan_usage_omits_it() {
    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("done"),
    )]]));
    let session = memory_session();
    let mut agent = MultiTurnAgent::with_streamer(
        streamer.clone(),
        "openai_resp::gpt-5",
        Vec::new(),
        "",
        MultiTurnAgentConfig::default(),
    );
    let config =
        HarnessConfig::from_context_settings(Some(&meerai_common::settings::ContextSettings {
            effort: Some(meerai_common::settings::EffortLevel::High),
            ..Default::default()
        }));
    agent.set_harness(harness(&session, auto_mode(), config));

    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();

    {
        let cached = streamer.options.lock().unwrap();
        assert_eq!(cached[0].temperature, Some(0.5));
        assert!(matches!(
            cached[0].reasoning_effort,
            Some(genai::chat::ReasoningEffort::High)
        ));
    }

    let plan_streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("done"),
    )]]));
    let plan_session = memory_session();
    let mut plan_agent = MultiTurnAgent::with_streamer(
        plan_streamer.clone(),
        "openai_resp::gpt-5",
        Vec::new(),
        "",
        MultiTurnAgentConfig::default(),
    );
    let plan_config = HarnessConfig {
        plan_usage: true,
        effort: Some(meerai_common::settings::EffortLevel::High),
        temperature: Some(0.5),
        ..HarnessConfig::default()
    };
    plan_agent.set_harness(harness(&plan_session, auto_mode(), plan_config));
    plan_agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();

    let plan_cached = plan_streamer.options.lock().unwrap();
    assert_eq!(plan_cached[0].temperature, None);
    assert!(plan_cached[0].reasoning_effort.is_none());
}

#[tokio::test]
async fn routing_applies_effort_and_output_cap_to_the_request() {
    use genai::chat::ReasoningEffort;
    use meerai_common::settings::{EffortLevel, ModelRoute, RoutingSettings, TaskType};

    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("routed"),
    )]]));
    let session = memory_session();
    let config = HarnessConfig::default().with_routing(RoutingSettings {
        routes: vec![ModelRoute {
            task_type: TaskType::Default,
            provider: "openai".to_string(),
            model: "gpt-4o-mini".to_string(),
            effort: Some("high".to_string()),
            max_tokens_per_turn: Some(1_234),
            max_cost_usd: None,
            max_concurrency: None,
        }],
    });
    let mut config = config;
    config.effort_levels = Some(vec![EffortLevel::Low]);
    let mut agent = harness_agent(streamer.clone(), vec![], "", &session, auto_mode(), config);
    assert_eq!(
        agent.harness.as_ref().unwrap().config.effort,
        Some(EffortLevel::Medium)
    );

    agent.prompt("go").await.unwrap();

    let options = streamer.options.lock().unwrap();
    assert_eq!(options[0].max_tokens, Some(1_234));
    assert!(
        matches!(options[0].reasoning_effort, Some(ReasoningEffort::High)),
        "an explicit route effort is sent as-is, not clamped to the session model"
    );
}
