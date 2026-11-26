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
async fn repeated_failures_trigger_breaker_and_end_turn() {
    let step = |call_id: &str| {
        vec![ChatStreamEvent::End(tool_end(vec![tool_call(
            call_id, "fail",
        )]))]
    };
    let streamer = Arc::new(FakeStreamer::new(vec![
        step("c1"),
        step("c2"),
        step("c3"),
        step("c4"),
        vec![ChatStreamEvent::End(text_end("recovered"))],
    ]));

    let session = memory_session();
    let agent = harness_agent(
        streamer.clone(),
        vec![Box::new(EchoToolset)],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt("go".to_string()))
        .unwrap();

    let mut code = None;
    let mut completions = Vec::new();
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::Error {
                code: error_code, ..
            } => code = Some(error_code),
            AgentEvent::TurnCompleted { reason, .. } => {
                completions.push(reason);
                if completions.len() == 2 {
                    break;
                }
            }
            _ => {}
        }
    }
    assert_eq!(code, Some(ErrorCode::LoopDetected));
    assert_eq!(
        completions[0],
        TurnEndReason::Failed {
            error: format!("{LOOP_DETECTED} (echo-say)")
        }
    );
    assert_eq!(completions[1], TurnEndReason::Completed);

    let events = session_events(&session);
    let breakers: Vec<&SessionEvent> = events
        .iter()
        .filter(|event| matches!(event, SessionEvent::CircuitBreaker { .. }))
        .collect();
    assert_eq!(breakers.len(), 2);
    match breakers[0] {
        SessionEvent::CircuitBreaker { kind, action } => {
            assert_eq!(*kind, BreakerKind::RepeatedToolFailure);
            assert_eq!(*action, BreakerAction::DiagnosticInjected);
        }
        other => panic!("expected circuit breaker, got {other:?}"),
    }
    match breakers[1] {
        SessionEvent::CircuitBreaker { action, .. } => {
            assert_eq!(*action, BreakerAction::TurnFailed);
        }
        other => panic!("expected circuit breaker, got {other:?}"),
    }

    let injected = events.iter().any(|event| {
        matches!(
            event,
            SessionEvent::UserMessage { source: InputSource::RuntimeContext, content }
                if content.iter().any(|block| matches!(
                    block,
                    ContentBlock::Text(text) if text.contains("failed identically 3 times")
                ))
        )
    });
    assert!(injected);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::TurnEnded {
            reason: TurnEndReason::Failed { error },
            ..
        } if error.starts_with(LOOP_DETECTED) && error.contains("echo-say")
    )));

    let reqs = streamer.requests.lock().unwrap();
    assert_eq!(reqs.len(), 5);
    let next_request = format!("{:?}", reqs[3].messages);
    assert!(
        next_request.contains("failed identically 3 times"),
        "the breaker diagnostic must reach the next request: {next_request}"
    );

    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn repeated_identical_tool_batches_end_with_loop_detected() {
    let batch = || {
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "c1".to_string(),
            fn_name: "echo-say".to_string(),
            fn_arguments: serde_json::json!({"text": "hi"}),
            thought_signatures: None,
        }]))]
    };
    let streamer = Arc::new(FakeStreamer::new(vec![
        batch(),
        batch(),
        batch(),
        batch(),
        batch(),
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(EchoToolset)],
        "system",
        &session,
        auto_mode(),
        HarnessConfig {
            max_steps_per_turn: 10,
            ..HarnessConfig::default()
        },
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    let error = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .expect_err("loop must fail");
    assert!(
        format!("{error:#}").contains("(echo-say)"),
        "loop message must name the offending call: {error:#}"
    );
    assert!(session_events(&session).iter().any(|event| matches!(
        event,
        SessionEvent::Error {
            code: ErrorCode::LoopDetected,
            ..
        }
    )));
}

#[tokio::test]
async fn repeated_identical_edits_log_the_edit_breaker() {
    let batch = || {
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "c1".to_string(),
            fn_name: "code-edit_file".to_string(),
            fn_arguments: serde_json::json!({
                "path": "src/lib.rs",
                "old_text": "a",
                "new_text": "b"
            }),
            thought_signatures: None,
        }]))]
    };
    let streamer = Arc::new(FakeStreamer::new(vec![
        batch(),
        batch(),
        batch(),
        batch(),
        batch(),
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer,
        vec![Box::new(StaticToolset::new(
            "code",
            "code-edit_file",
            "edited",
            EffectClass::Write,
        ))],
        "system",
        &session,
        auto_mode(),
        HarnessConfig {
            max_steps_per_turn: 10,
            ..HarnessConfig::default()
        },
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    let _ = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await;

    let breakers: Vec<(BreakerKind, BreakerAction)> = session_events(&session)
        .iter()
        .filter_map(|event| match event {
            SessionEvent::CircuitBreaker { kind, action } => Some((*kind, *action)),
            _ => None,
        })
        .collect();
    assert!(breakers.contains(&(
        BreakerKind::RepeatedIdenticalEdit,
        BreakerAction::DiagnosticInjected
    )));
    assert!(breakers.contains(&(
        BreakerKind::RepeatedIdenticalEdit,
        BreakerAction::TurnFailed
    )));
}

#[tokio::test]
async fn no_progress_breaker_emits_diagnostic_and_fails() {
    let batches: Vec<Vec<ChatStreamEvent>> = (0..12)
        .map(|index| {
            vec![ChatStreamEvent::End(tool_end(vec![
                ToolCall {
                    call_id: format!("read-{index}"),
                    fn_name: "code-read_file".to_string(),
                    fn_arguments: serde_json::json!({ "path": format!("src/{index}.rs") }),
                    thought_signatures: None,
                },
                ToolCall {
                    call_id: format!("edit-{index}"),
                    fn_name: "code-edit_file".to_string(),
                    fn_arguments: serde_json::json!({
                        "path": format!("src/{index}.rs"),
                        "old_text": format!("old-{index}"),
                        "new_text": format!("new-{index}")
                    }),
                    thought_signatures: None,
                },
            ]))]
        })
        .collect();
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
    let error = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .expect_err("no progress must fail");
    let message = format!("{error:#}");
    assert!(message.contains("no progress"));
    assert!(
        message.contains("last write: code-edit_file src/11.rs"),
        "the breaker must name the write that made no change: {message}"
    );

    let breakers: Vec<(BreakerKind, BreakerAction)> = session_events(&session)
        .iter()
        .filter_map(|event| match event {
            SessionEvent::CircuitBreaker { kind, action } => Some((*kind, *action)),
            _ => None,
        })
        .collect();
    assert!(breakers.contains(&(BreakerKind::NoProgress, BreakerAction::DiagnosticInjected)));
    assert!(breakers.contains(&(BreakerKind::NoProgress, BreakerAction::TurnFailed)));
}

#[derive(Default)]
struct RecordingInterceptor {
    plans: Mutex<Vec<StepPlan>>,
    outcomes: Mutex<Vec<ToolOutcome>>,
    stop_at_step: Option<usize>,
    feedback: Option<String>,
    continue_turn_stop: bool,
    turn_stop_used: Mutex<bool>,
}

impl Interceptor for RecordingInterceptor {
    fn before_step(&self, plan: &mut StepPlan) -> StepAction {
        self.plans.lock().unwrap().push(plan.clone());
        if self.stop_at_step == Some(plan.step_index) {
            StepAction::Stop {
                reason: Some("hook stop".to_string()),
            }
        } else {
            StepAction::Continue
        }
    }

    fn after_tool(&self, outcome: &ToolOutcome) -> Option<String> {
        self.outcomes.lock().unwrap().push(outcome.clone());
        self.feedback.clone()
    }

    fn turn_stopping(&self, _turn_id: &TurnId, _step_index: usize) -> TurnStopAction {
        if !self.continue_turn_stop {
            return TurnStopAction::Finish;
        }
        let mut used = self.turn_stop_used.lock().unwrap();
        if *used {
            TurnStopAction::Finish
        } else {
            *used = true;
            TurnStopAction::Continue {
                reason: "verify before done".to_string(),
            }
        }
    }
}

#[tokio::test]
async fn interceptors_observe_steps_and_tool_outcomes() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![tool_call("c1", "ok")]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer,
        vec![Box::new(EchoToolset)],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    let recorder = Arc::new(RecordingInterceptor {
        feedback: Some("hook feedback".to_string()),
        ..Default::default()
    });
    agent.add_shared_interceptor(recorder.clone());

    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "done");

    let plans = recorder.plans.lock().unwrap();
    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0].step_index, 0);
    assert_eq!(plans[0].tool_count, 1);
    assert!(!plans[0].final_step);
    assert_eq!(plans[1].step_index, 1);

    let outcomes = recorder.outcomes.lock().unwrap();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].tool_name, "echo-say");
    assert_eq!(outcomes[0].status, ItemStatus::Succeeded);
    assert_eq!(outcomes[0].step_index, 0);

    let logged = session.events();
    assert!(logged.iter().any(|entry| matches!(
        &entry.event,
        SessionEvent::UserMessage { source: InputSource::RuntimeContext, content }
            if matches!(content.first(), Some(ContentBlock::Text(text)) if text == "hook feedback")
    )));
}

#[tokio::test]
async fn before_step_can_stop_the_turn() {
    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("never sent"),
    )]]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer,
        Vec::new(),
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    let recorder = Arc::new(RecordingInterceptor {
        stop_at_step: Some(0),
        ..Default::default()
    });
    agent.add_shared_interceptor(recorder.clone());

    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "hook stop");
    assert_eq!(recorder.plans.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn turn_stopping_can_continue_the_turn_once() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(text_end("first"))],
        vec![ChatStreamEvent::End(text_end("second"))],
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        Vec::new(),
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    let recorder = Arc::new(RecordingInterceptor {
        continue_turn_stop: true,
        ..Default::default()
    });
    agent.add_shared_interceptor(recorder.clone());

    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "second");
    assert_eq!(streamer.requests.lock().unwrap().len(), 2);
    assert_eq!(recorder.plans.lock().unwrap().len(), 2);
}

struct CountingStopInterceptor {
    left: Mutex<usize>,
}

impl Interceptor for CountingStopInterceptor {
    fn turn_stopping(&self, _turn_id: &TurnId, _step_index: usize) -> TurnStopAction {
        let mut left = self.left.lock().unwrap();
        if *left == 0 {
            return TurnStopAction::Finish;
        }
        *left -= 1;
        TurnStopAction::Continue {
            reason: "keep going".to_string(),
        }
    }
}

#[tokio::test]
async fn turn_stopping_can_continue_more_than_once() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(text_end("first"))],
        vec![ChatStreamEvent::End(text_end("second"))],
        vec![ChatStreamEvent::End(text_end("third"))],
        vec![ChatStreamEvent::End(text_end("fourth"))],
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        Vec::new(),
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    agent.add_shared_interceptor(Arc::new(CountingStopInterceptor {
        left: Mutex::new(2),
    }));

    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "third");
    assert_eq!(streamer.requests.lock().unwrap().len(), 3);
}

fn loop_call(call_id: &str, fn_name: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        call_id: call_id.to_string(),
        fn_name: fn_name.to_string(),
        fn_arguments: args,
        thought_signatures: None,
    }
}

#[tokio::test]
async fn the_loop_continues_until_every_item_is_done() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![loop_call(
            "l1",
            "loop-plan",
            serde_json::json!({
                "objective": "Fix the failing test",
                "done_when": ["cargo test passes"],
                "items": [{ "text": "Fix it", "expect": "cargo test passes" }]
            }),
        )]))],
        vec![ChatStreamEvent::End(text_end("done early"))],
        vec![ChatStreamEvent::End(tool_end(vec![
            loop_call(
                "l2",
                "loop-act",
                serde_json::json!({
                    "item_id": 1,
                    "action": "changed the assertion",
                    "expect": "cargo test passes"
                }),
            ),
            loop_call(
                "l3",
                "loop-observe",
                serde_json::json!({
                    "item_id": 1,
                    "observation": "test result: ok. 12 passed"
                }),
            ),
            loop_call(
                "l4",
                "loop-adjust",
                serde_json::json!({ "item_id": 1, "status": "done" }),
            ),
        ]))],
        vec![ChatStreamEvent::End(text_end("complete"))],
    ]));
    let session = memory_session();
    let state = meerai_skills::new_loop_handle();
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(meerai_skills::LoopToolset::new(state.clone()))],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    agent.add_shared_interceptor(Arc::new(crate::LoopInterceptor::new(state.clone())));

    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("ship the fix", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "complete");
    assert_eq!(streamer.requests.lock().unwrap().len(), 4);
    assert!(state.lock().all_done());

    let continued = session.events().iter().any(|entry| matches!(
        &entry.event,
        SessionEvent::UserMessage { source: InputSource::RuntimeContext, content }
            if matches!(content.first(), Some(ContentBlock::Text(text)) if text.contains("Loop round 1/"))
    ));
    assert!(continued, "the early stop must be continued once");
}

#[tokio::test]
async fn the_worker_starts_a_new_turn_while_the_loop_has_open_items() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(text_end("blocked"))],
        vec![ChatStreamEvent::End(tool_end(vec![
            loop_call(
                "l1",
                "loop-plan",
                serde_json::json!({
                    "objective": "Fix it",
                    "items": [{ "text": "Fix", "expect": "it passes" }]
                }),
            ),
            loop_call(
                "l2",
                "loop-act",
                serde_json::json!({ "item_id": 1, "action": "fixed it" }),
            ),
            loop_call(
                "l3",
                "loop-observe",
                serde_json::json!({ "item_id": 1, "observation": "it passes" }),
            ),
            loop_call(
                "l4",
                "loop-adjust",
                serde_json::json!({ "item_id": 1, "status": "done" }),
            ),
        ]))],
        vec![ChatStreamEvent::End(text_end("complete"))],
    ]));
    let session = memory_session();
    let state = meerai_skills::new_loop_handle();
    {
        let mut state = state.lock();
        state.objective = Some("Fix it".to_string());
        state.items = vec![meerai_session::LoopItem {
            id: 1,
            text: "Fix".to_string(),
            status: meerai_session::LoopItemStatus::Open,
            ..meerai_session::LoopItem::default()
        }];
        state.current = Some(1);
        state.phase = meerai_session::LoopPhase::Act;
        state.rounds = meerai_session::MAX_LOOP_ROUNDS;
    }
    let mut agent = harness_agent(
        streamer,
        vec![Box::new(meerai_skills::LoopToolset::new(state.clone()))],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    agent.set_loop_handle(state.clone());
    agent.add_shared_interceptor(Arc::new(crate::LoopInterceptor::new(state.clone())));

    let (cmd_tx, mut event_rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt("go".to_string()))
        .unwrap();

    let mut starts = 0usize;
    let mut continued = false;
    let deadline = tokio::time::Duration::from_secs(5);
    tokio::time::timeout(deadline, async {
        while let Some(event) = event_rx.recv().await {
            match event {
                AgentEvent::TurnStarted { .. } => starts += 1,
                AgentEvent::LoopContinued { turn, .. } => {
                    assert_eq!(turn, 1);
                    continued = true;
                }
                AgentEvent::TurnCompleted { .. } if starts >= 2 => break,
                _ => {}
            }
        }
    })
    .await
    .expect("worker did not finish");

    let _ = cmd_tx.send(AgentCommand::Quit);
    assert!(continued, "the worker must announce the loop continuation");
    assert_eq!(starts, 2, "the loop must run one follow-up turn");
    assert!(state.lock().all_done());
}

#[tokio::test]
async fn loop_detected_message_names_the_offending_calls() {
    let names = ["tool-alpha", "tool-beta", "tool-delta", "tool-gamma"];
    let batch = || {
        let calls = names
            .iter()
            .enumerate()
            .map(|(index, name)| ToolCall {
                call_id: format!("call-{index}"),
                fn_name: name.to_string(),
                fn_arguments: serde_json::json!({ "index": index }),
                thought_signatures: None,
            })
            .collect();
        vec![ChatStreamEvent::End(tool_end(calls))]
    };
    let streamer = Arc::new(FakeStreamer::new(vec![
        batch(),
        batch(),
        batch(),
        batch(),
        batch(),
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer,
        Vec::new(),
        "system",
        &session,
        auto_mode(),
        HarnessConfig {
            max_steps_per_turn: 10,
            ..HarnessConfig::default()
        },
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    let error = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .expect_err("loop must fail");
    let message = format!("{error:#}");
    assert!(
        message.contains("loop detected: repeated identical tool calls (tool-alpha (+3 more))"),
        "{message}"
    );
}

#[tokio::test]
async fn batch_diagnostics_land_after_every_tool_result() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![
            ToolCall {
                call_id: "c1".to_string(),
                fn_name: "invalid-run".to_string(),
                fn_arguments: serde_json::json!({ "count": 1 }),
                thought_signatures: None,
            },
            ToolCall {
                call_id: "c2".to_string(),
                fn_name: "invalid-run".to_string(),
                fn_arguments: serde_json::json!({ "count": 2 }),
                thought_signatures: None,
            },
        ]))],
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

    let events = session_events(&session);
    let last_result = events
        .iter()
        .rposition(|event| matches!(event, SessionEvent::ToolResult { .. }))
        .expect("tool results");
    let notes: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, event)| {
            matches!(
                event,
                SessionEvent::UserMessage { source: InputSource::RuntimeContext, content }
                    if content.iter().any(|block| matches!(
                        block,
                        ContentBlock::Text(text) if text.contains("Retry the same tool")
                    ))
            )
        })
        .map(|(index, _)| index)
        .collect();

    assert_eq!(notes.len(), 1, "batch notes must merge into one message");
    assert!(
        notes[0] > last_result,
        "runtime notes must follow every tool result: {notes:?} <= {last_result}"
    );
}
