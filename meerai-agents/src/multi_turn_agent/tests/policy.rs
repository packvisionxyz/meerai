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
async fn policy_denies_tool_call_and_turn_continues() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![tool_call("c1", "ok")]))],
        vec![ChatStreamEvent::End(text_end("recovered"))],
    ]));

    let session = memory_session();
    let policy = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![any_rule(Effect::Deny)]);
    let mut agent = MultiTurnAgent::with_streamer(
        streamer.clone(),
        "gpt-4o-mini",
        vec![Box::new(EchoToolset)],
        "",
        MultiTurnAgentConfig::default(),
    );
    agent.set_harness(harness(&session, policy, HarnessConfig::default()));

    let (tx, mut rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "recovered");

    let mut denied = None;
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::ItemCompleted {
            item: SessionItem::ToolCall { status, output, .. },
        } = event
        {
            denied = Some((status, output));
        }
    }
    let (status, output) = denied.expect("tool item completed");
    assert_eq!(status, ItemStatus::Denied);
    assert!(output.is_some_and(|text| text.contains("denied")));

    let events = session_events(&session);
    let refused = events.iter().any(|event| {
        matches!(
            event,
            SessionEvent::ToolResult { output, .. }
                if output.status == ObservationStatus::Refused
        )
    });
    assert!(refused);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::TurnEnded {
            reason: TurnEndReason::Completed,
            ..
        }
    )));

    let reqs = streamer.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    assert!(format!("{:?}", reqs[1].messages).contains("Refused: denied by permission rule"));
}

#[tokio::test]
async fn approval_accept_executes_the_tool() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![tool_call("c1", "ok")]))],
        vec![ChatStreamEvent::End(text_end("approved"))],
    ]));

    let session = memory_session();
    let policy = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![any_rule(Effect::Ask)]);
    let mut agent = MultiTurnAgent::with_streamer(
        streamer.clone(),
        "gpt-4o-mini",
        vec![Box::new(EchoToolset)],
        "",
        MultiTurnAgentConfig::default(),
    );
    agent.set_harness(harness(&session, policy, HarnessConfig::default()));

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt("go".to_string()))
        .unwrap();

    let mut requested = 0;
    let mut resolved = 0;
    let mut flow: Vec<String> = Vec::new();
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::ItemDelta {
                delta: ItemDelta::StatusChanged(status),
                ..
            } => flow.push(format!("{status:?}")),
            AgentEvent::ApprovalRequested { request } => {
                assert_eq!(request.kind, ApprovalKind::ToolCall);
                assert_eq!(request.effect, Some(EffectClass::Read));
                requested += 1;
                flow.push("requested".to_string());
                cmd_tx
                    .send(AgentCommand::Approve {
                        request_id: request.request_id,
                        decision: ApprovalDecision::Accept,
                        scope: None,
                    })
                    .unwrap();
            }
            AgentEvent::ApprovalResolved { .. } => {
                resolved += 1;
                flow.push("resolved".to_string());
            }
            AgentEvent::TurnCompleted { .. } => break,
            _ => {}
        }
    }
    assert_eq!(requested, 1);
    assert_eq!(resolved, 1);
    assert_eq!(
        flow,
        vec![
            format!("{:?}", ItemStatus::Pending),
            "requested".to_string(),
            "resolved".to_string(),
            format!("{:?}", ItemStatus::InProgress),
        ],
        "pending before the request, in-progress only after the accept"
    );

    let events = session_events(&session);
    let decided_without_scope = events.iter().any(|event| {
        matches!(
            event,
            SessionEvent::ApprovalDecided {
                decision: ApprovalDecision::Accept,
                scope: None,
                ..
            }
        )
    });
    assert!(decided_without_scope);

    let reqs = streamer.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    assert!(format!("{:?}", reqs[1].messages).contains("echoed:ok"));

    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn accept_for_session_records_grant_and_skips_reprompt() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![
            tool_call("c1", "ok"),
            tool_call("c2", "ok"),
        ]))],
        vec![ChatStreamEvent::End(text_end("both"))],
    ]));

    let session = memory_session();
    let policy = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![any_rule(Effect::Ask)]);
    let mut agent = MultiTurnAgent::with_streamer(
        streamer.clone(),
        "gpt-4o-mini",
        vec![Box::new(EchoToolset)],
        "",
        MultiTurnAgentConfig::default(),
    );
    agent.set_harness(harness(&session, policy, HarnessConfig::default()));

    let (commands_tx, mut commands_rx) = mpsc::unbounded_channel();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let driver = tokio::spawn(async move {
        let mut requested = 0;
        while let Some(event) = event_rx.recv().await {
            match event {
                AgentEvent::ApprovalRequested { request } => {
                    requested += 1;
                    let _ = commands_tx.send(AgentCommand::Approve {
                        request_id: request.request_id,
                        decision: ApprovalDecision::AcceptForSession,
                        scope: None,
                    });
                }
                AgentEvent::TurnCompleted { .. } => break,
                _ => {}
            }
        }
        requested
    });

    let out = agent
        .prompt_streaming_with_commands("go", &event_tx, CancellationToken::new(), &mut commands_rx)
        .await
        .unwrap();
    assert_eq!(out, "both");
    assert_eq!(driver.await.unwrap(), 1);
    assert_eq!(agent.harness().unwrap().policy.grants().len(), 1);

    let reqs = streamer.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    let rendered = format!("{:?}", reqs[1].messages);
    assert_eq!(rendered.matches("echoed:ok").count(), 2);
}

#[tokio::test]
async fn missing_approval_channel_auto_declines() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![tool_call("c1", "ok")]))],
        vec![ChatStreamEvent::End(text_end("after refusal"))],
    ]));

    let session = memory_session();
    let policy = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![any_rule(Effect::Ask)]);
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(EchoToolset)],
        "",
        &session,
        policy,
        HarnessConfig::default(),
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "after refusal");

    let mut requested = 0;
    let mut resolved = 0;
    while let Ok(event) = rx.try_recv() {
        match event {
            AgentEvent::ApprovalRequested { .. } => requested += 1,
            AgentEvent::ApprovalResolved { .. } => resolved += 1,
            _ => {}
        }
    }
    assert_eq!(requested, 1);
    assert_eq!(resolved, 1);

    let decided = session_events(&session).iter().any(|event| {
        matches!(
            event,
            SessionEvent::ApprovalDecided {
                decision: ApprovalDecision::Decline,
                scope: None,
                ..
            }
        )
    });
    assert!(decided);

    let reqs = streamer.requests.lock().unwrap();
    assert!(format!("{:?}", reqs[1].messages).contains(NO_APPROVAL_CHANNEL));
}

#[tokio::test]
async fn read_only_calls_run_concurrently_and_settle_in_call_order() {
    let barrier = Arc::new(Barrier::new(2));
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![
            bare_call("c1", "barrier-a"),
            bare_call("c2", "barrier-b"),
        ]))],
        vec![ChatStreamEvent::End(text_end("both ran"))],
    ]));

    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(BarrierToolset {
            barrier: barrier.clone(),
        })],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("run both", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "both ran");

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
            (CallId::from_raw("c1"), ItemStatus::Succeeded),
            (CallId::from_raw("c2"), ItemStatus::Succeeded),
        ]
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
            ("c1".to_string(), "ran:barrier-a".to_string()),
            ("c2".to_string(), "ran:barrier-b".to_string()),
        ]
    );
}

#[tokio::test]
async fn auto_mode_notifies_once_per_turn_for_side_effects() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![bare_call(
            "c1",
            "shell-run",
        )]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));

    let session = memory_session();
    let policy = PolicyEngine::new(ApprovalMode::Auto);
    let agent = harness_agent(
        streamer,
        vec![Box::new(StaticToolset::new(
            "shell",
            "shell-run",
            "ran",
            EffectClass::Execute,
        ))],
        "",
        &session,
        policy,
        HarnessConfig::default(),
    );

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt("go".to_string()))
        .unwrap();

    let mut notices = 0;
    loop {
        match rx.recv().await {
            Some(AgentEvent::AutoApproved { tool, effect }) => {
                notices += 1;
                assert_eq!(tool, "shell-run");
                assert_eq!(effect, EffectClass::Execute);
            }
            Some(AgentEvent::TurnCompleted { .. }) => break,
            Some(_) => {}
            None => panic!("worker stopped mid-turn"),
        }
    }
    assert_eq!(notices, 1);
    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn ask_mode_never_notifies_an_auto_approval() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![bare_call(
            "c1",
            "shell-run",
        )]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));

    let session = memory_session();
    let policy = PolicyEngine::new(ApprovalMode::Ask);
    let agent = harness_agent(
        streamer,
        vec![Box::new(StaticToolset::new(
            "shell",
            "shell-run",
            "ran",
            EffectClass::Execute,
        ))],
        "",
        &session,
        policy,
        HarnessConfig::default(),
    );

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt("go".to_string()))
        .unwrap();

    let mut notices = 0;
    loop {
        match rx.recv().await {
            Some(AgentEvent::AutoApproved { .. }) => notices += 1,
            Some(AgentEvent::ApprovalRequested { request }) => {
                cmd_tx
                    .send(AgentCommand::Approve {
                        request_id: request.request_id,
                        decision: ApprovalDecision::Accept,
                        scope: None,
                    })
                    .unwrap();
            }
            Some(AgentEvent::TurnCompleted { .. }) => break,
            Some(_) => {}
            None => panic!("worker stopped mid-turn"),
        }
    }
    assert_eq!(notices, 0);
    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn idle_set_permission_mode_applies_to_the_next_turn() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![bare_call(
            "c1",
            "write-file",
        )]))],
        vec![ChatStreamEvent::End(text_end("first done"))],
        vec![ChatStreamEvent::End(tool_end(vec![bare_call(
            "c2",
            "write-file",
        )]))],
        vec![ChatStreamEvent::End(text_end("second done"))],
    ]));

    let session = memory_session();
    let policy = PolicyEngine::new(ApprovalMode::Auto);
    let agent = harness_agent(
        streamer,
        vec![Box::new(StaticToolset::new(
            "write",
            "write-file",
            "wrote",
            EffectClass::Write,
        ))],
        "",
        &session,
        policy,
        HarnessConfig::default(),
    );

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt("first".to_string()))
        .unwrap();

    let mut statuses = Vec::new();
    loop {
        match rx.recv().await {
            Some(AgentEvent::ItemCompleted {
                item: SessionItem::ToolCall { status, .. },
            }) => statuses.push(status),
            Some(AgentEvent::TurnCompleted { .. }) => break,
            Some(_) => {}
            None => panic!("worker stopped mid-turn"),
        }
    }
    assert_eq!(statuses, vec![ItemStatus::Succeeded]);

    tokio::time::sleep(Duration::from_millis(50)).await;
    cmd_tx
        .send(AgentCommand::SetPermissionMode(ApprovalMode::Ask))
        .unwrap();
    cmd_tx
        .send(AgentCommand::SubmitPrompt("second".to_string()))
        .unwrap();

    let mut prompts = 0;
    loop {
        match rx.recv().await {
            Some(AgentEvent::ApprovalRequested { request }) => {
                prompts += 1;
                cmd_tx
                    .send(AgentCommand::Approve {
                        request_id: request.request_id,
                        decision: ApprovalDecision::Decline,
                        scope: None,
                    })
                    .unwrap();
            }
            Some(AgentEvent::ItemCompleted {
                item: SessionItem::ToolCall { status, .. },
            }) => statuses.push(status),
            Some(AgentEvent::TurnCompleted { .. }) => break,
            Some(_) => {}
            None => panic!("worker stopped mid-turn"),
        }
    }
    assert_eq!(prompts, 1, "Ask mode must prompt for a write tool");
    assert_eq!(
        statuses,
        vec![ItemStatus::Succeeded, ItemStatus::Denied],
        "idle SetPermissionMode must apply to the next turn"
    );

    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn turn_scope_grant_skips_reprompt_until_the_turn_ends() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![
            tool_call("c1", "ok"),
            tool_call("c2", "ok"),
        ]))],
        vec![ChatStreamEvent::End(text_end("first"))],
        vec![ChatStreamEvent::End(tool_end(vec![tool_call("c3", "ok")]))],
        vec![ChatStreamEvent::End(text_end("second"))],
    ]));

    let session = memory_session();
    let policy = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![any_rule(Effect::Ask)]);
    let agent = harness_agent(
        streamer,
        vec![Box::new(EchoToolset)],
        "",
        &session,
        policy,
        HarnessConfig::default(),
    );

    let (cmd_tx, mut rx) = spawn_worker(agent);
    let mut prompts = 0;
    for prompt in ["first", "second"] {
        cmd_tx
            .send(AgentCommand::SubmitPrompt(prompt.to_string()))
            .unwrap();
        loop {
            match rx.recv().await {
                Some(AgentEvent::ApprovalRequested { request }) => {
                    prompts += 1;
                    cmd_tx
                        .send(AgentCommand::Approve {
                            request_id: request.request_id,
                            decision: ApprovalDecision::Accept,
                            scope: Some(GrantScope::Turn),
                        })
                        .unwrap();
                }
                Some(AgentEvent::TurnCompleted { .. }) => break,
                Some(_) => {}
                None => panic!("worker stopped mid-turn"),
            }
        }
    }
    assert_eq!(prompts, 2);

    let scopes: Vec<Option<GrantScope>> = session_events(&session)
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::ApprovalDecided { scope, .. } => Some(scope),
            _ => None,
        })
        .collect();
    assert_eq!(scopes, vec![Some(GrantScope::Turn); 2]);

    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn sandbox_denial_asks_and_reruns_the_command() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "c1".to_string(),
            fn_name: "code-shell".to_string(),
            fn_arguments: serde_json::json!({ "command": "flutter --version" }),
            thought_signatures: None,
        }]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let tool = Arc::new(SandboxEscapeToolset::default());
    let session = memory_session();
    let agent = harness_agent(
        streamer,
        vec![Box::new(SandboxEscapeTool {
            inner: tool.clone(),
        })],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt("go".to_string()))
        .unwrap();

    let mut approvals = 0;
    let mut output = None;
    loop {
        match rx.recv().await {
            Some(AgentEvent::ApprovalRequested { request }) => {
                assert_eq!(request.kind, ApprovalKind::SandboxEscape);
                assert!(request.preview.detail.contains("flutter --version"));
                approvals += 1;
                cmd_tx
                    .send(AgentCommand::Approve {
                        request_id: request.request_id,
                        decision: ApprovalDecision::Accept,
                        scope: None,
                    })
                    .unwrap();
            }
            Some(AgentEvent::ItemCompleted {
                item:
                    SessionItem::ToolCall {
                        output: item_output,
                        ..
                    },
            }) => output = item_output,
            Some(AgentEvent::TurnCompleted { .. }) => break,
            Some(_) => {}
            None => panic!("worker stopped mid-turn"),
        }
    }

    assert_eq!(approvals, 1);
    assert_eq!(output.as_deref(), Some("escaped"));
    let calls = tool.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(calls[1].contains("\"unsandboxed\":true"), "{:?}", calls[1]);

    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn sandbox_escape_session_grant_skips_later_prompts() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "c1".to_string(),
            fn_name: "code-shell".to_string(),
            fn_arguments: serde_json::json!({ "command": "flutter --version" }),
            thought_signatures: None,
        }]))],
        vec![ChatStreamEvent::End(text_end("first"))],
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "c2".to_string(),
            fn_name: "code-shell".to_string(),
            fn_arguments: serde_json::json!({ "command": "dart analyze" }),
            thought_signatures: None,
        }]))],
        vec![ChatStreamEvent::End(text_end("second"))],
    ]));
    let tool = Arc::new(SandboxEscapeToolset::default());
    let session = memory_session();
    let agent = harness_agent(
        streamer,
        vec![Box::new(SandboxEscapeTool {
            inner: tool.clone(),
        })],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    let (cmd_tx, mut rx) = spawn_worker(agent);
    let mut approvals = 0;

    cmd_tx
        .send(AgentCommand::SubmitPrompt("first".to_string()))
        .unwrap();
    loop {
        match rx.recv().await {
            Some(AgentEvent::ApprovalRequested { request }) => {
                assert_eq!(request.kind, ApprovalKind::SandboxEscape);
                approvals += 1;
                cmd_tx
                    .send(AgentCommand::Approve {
                        request_id: request.request_id,
                        decision: ApprovalDecision::AcceptForSession,
                        scope: None,
                    })
                    .unwrap();
            }
            Some(AgentEvent::TurnCompleted { .. }) => break,
            Some(_) => {}
            None => panic!("worker stopped mid-turn"),
        }
    }

    cmd_tx
        .send(AgentCommand::SubmitPrompt("second".to_string()))
        .unwrap();
    loop {
        match rx.recv().await {
            Some(AgentEvent::ApprovalRequested { .. }) => approvals += 1,
            Some(AgentEvent::TurnCompleted { .. }) => break,
            Some(_) => {}
            None => panic!("worker stopped mid-turn"),
        }
    }

    assert_eq!(approvals, 1);
    let calls = tool.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(calls[2].contains("\"unsandboxed\":true"), "{:?}", calls[2]);

    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn read_only_turns_do_not_trip_no_progress() {
    let mut batches: Vec<Vec<ChatStreamEvent>> = (0..10)
        .map(|index| {
            vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
                call_id: format!("call-{index}"),
                fn_name: "code-read_file".to_string(),
                fn_arguments: serde_json::json!({ "path": format!("src/{index}.rs") }),
                thought_signatures: None,
            }]))]
        })
        .collect();
    batches.push(vec![ChatStreamEvent::End(text_end("done"))]);
    let streamer = Arc::new(FakeStreamer::new(batches));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer,
        vec![
            Box::new(StaticToolset::new(
                "code",
                "code-read_file",
                "content",
                EffectClass::Read,
            )),
            Box::new(StaticToolset::new(
                "code",
                "code-edit_file",
                "edited",
                EffectClass::Write,
            )),
        ],
        "system",
        &session,
        auto_mode(),
        HarnessConfig {
            max_steps_per_turn: 14,
            ..HarnessConfig::default()
        },
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "done");
    assert!(!session_events(&session).iter().any(|event| matches!(
        event,
        SessionEvent::CircuitBreaker {
            kind: BreakerKind::NoProgress,
            ..
        }
    )));
}

#[tokio::test]
async fn read_only_steps_after_a_write_do_not_trip_no_progress() {
    let mut batches: Vec<Vec<ChatStreamEvent>> =
        vec![vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "edit-0".to_string(),
            fn_name: "code-edit_file".to_string(),
            fn_arguments: serde_json::json!({
                "path": "src/0.rs",
                "old_text": "old",
                "new_text": "new"
            }),
            thought_signatures: None,
        }]))]];
    for index in 0..13 {
        batches.push(vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: format!("read-{index}"),
            fn_name: "code-read_file".to_string(),
            fn_arguments: serde_json::json!({ "path": format!("src/{index}.rs") }),
            thought_signatures: None,
        }]))]);
    }
    batches.push(vec![ChatStreamEvent::End(text_end("done"))]);

    let streamer = Arc::new(FakeStreamer::new(batches));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer,
        vec![
            Box::new(StaticToolset::new(
                "code",
                "code-read_file",
                "content",
                EffectClass::Read,
            )),
            Box::new(StaticToolset::new(
                "code",
                "code-edit_file",
                "edited",
                EffectClass::Write,
            )),
        ],
        "system",
        &session,
        auto_mode(),
        HarnessConfig {
            max_steps_per_turn: 20,
            ..HarnessConfig::default()
        },
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "done");
    assert!(!session_events(&session).iter().any(|event| matches!(
        event,
        SessionEvent::CircuitBreaker {
            kind: BreakerKind::NoProgress,
            ..
        }
    )));
}

fn mode_probe_tools() -> Vec<Box<dyn Toolset>> {
    vec![
        Box::new(StaticToolset::new(
            "code",
            "code-read_file",
            "content",
            EffectClass::Read,
        )),
        Box::new(StaticToolset::new(
            "code",
            "code-write_file",
            "written",
            EffectClass::Write,
        )),
        Box::new(StaticToolset::new(
            "code",
            "code-edit_file",
            "edited",
            EffectClass::Write,
        )),
        Box::new(StaticToolset::new(
            "code",
            "code-shell",
            "ran",
            EffectClass::Execute,
        )),
        Box::new(StaticToolset::new(
            "sac",
            "sac-run_script",
            "ran",
            EffectClass::Execute,
        )),
        Box::new(StaticToolset::new(
            "subagent",
            "subagent-run",
            "report",
            EffectClass::External,
        )),
        Box::new(StaticToolset::new(
            "acp",
            "acp-run",
            "report",
            EffectClass::External,
        )),
    ]
}

fn sent_tool_names(streamer: &FakeStreamer, index: usize) -> Vec<String> {
    let requests = streamer.requests.lock().unwrap();
    requests[index]
        .tools
        .as_ref()
        .map(|tools| {
            tools
                .iter()
                .map(|tool| tool.name.as_str().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn plan_mode_sends_only_read_effect_tools() {
    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("ok"),
    )]]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        mode_probe_tools(),
        "system",
        &session,
        PolicyEngine::new(ApprovalMode::Plan),
        HarnessConfig::default(),
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();

    let names = sent_tool_names(&streamer, 0);
    assert_eq!(names, vec!["code-read_file".to_string()]);

    let read_definition =
        StaticToolset::new("code", "code-read_file", "content", EffectClass::Read)
            .definition()
            .pop()
            .expect("definition");
    let read_schema_tokens = estimate_tokens(&read_definition.name)
        + estimate_tokens(&read_definition.description)
        + estimate_tokens(&read_definition.parameters.to_string());
    assert_eq!(agent.tool_schema_tokens, read_schema_tokens);
}

#[tokio::test]
async fn ask_mode_still_sends_write_and_execute_tools() {
    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("ok"),
    )]]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        mode_probe_tools(),
        "system",
        &session,
        PolicyEngine::new(ApprovalMode::Ask),
        HarnessConfig::default(),
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();

    let names = sent_tool_names(&streamer, 0);
    for expected in [
        "code-read_file",
        "code-write_file",
        "code-edit_file",
        "code-shell",
        "sac-run_script",
        "subagent-run",
        "acp-run",
    ] {
        assert!(
            names.iter().any(|name| name == expected),
            "missing {expected} in {names:?}"
        );
    }
}

#[tokio::test]
async fn spend_cap_gate_asks_before_the_next_step() {
    use meerai_common::settings::{ModelRoute, RoutingSettings, TaskType};
    use meerai_session::ApprovalKind;

    use crate::routing::{DEFAULT_GLOBAL_CONCURRENCY, RoutingCoordinator};

    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("should not stream"),
    )]]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer,
        vec![],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    let coordinator = Arc::new(RoutingCoordinator::from_settings(
        &RoutingSettings {
            routes: vec![ModelRoute {
                task_type: TaskType::Default,
                provider: "openai".to_string(),
                model: "gpt-4o-mini".to_string(),
                effort: None,
                max_tokens_per_turn: None,
                max_cost_usd: Some(1.0),
                max_concurrency: None,
            }],
        },
        DEFAULT_GLOBAL_CONCURRENCY,
    ));
    coordinator.note_cost(2.0);
    agent.harness.as_mut().unwrap().routing = Some(coordinator);

    let (tx, mut rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();

    let mut spend_approvals = 0;
    let mut failed_reason = None;
    while let Ok(event) = rx.try_recv() {
        match event {
            AgentEvent::ApprovalRequested { request } if request.kind == ApprovalKind::Spend => {
                spend_approvals += 1;
            }
            AgentEvent::TurnCompleted { reason, .. } => failed_reason = Some(reason),
            _ => {}
        }
    }
    assert_eq!(spend_approvals, 1);
    assert!(matches!(
        failed_reason,
        Some(TurnEndReason::Failed { error }) if error.contains("spend cap")
    ));
}
