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
async fn streams_text_and_persists_history() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![
            ChatStreamEvent::Start,
            ChatStreamEvent::Chunk(StreamChunk {
                content: "Hello ".to_string(),
            }),
            ChatStreamEvent::Chunk(StreamChunk {
                content: "world!".to_string(),
            }),
            ChatStreamEvent::End(text_end("Hello world!")),
        ],
        vec![
            ChatStreamEvent::Chunk(StreamChunk {
                content: "Second reply".to_string(),
            }),
            ChatStreamEvent::End(text_end("Second reply")),
        ],
    ]));

    let mut agent = MultiTurnAgent::with_streamer(
        streamer.clone(),
        "gemini-3.8-flash",
        vec![],
        "Be concise.",
        MultiTurnAgentConfig::default(),
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    let reply1 = agent
        .prompt_streaming("Hi", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(reply1, "Hello world!");

    let reply2 = agent
        .prompt_streaming("Follow up", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(reply2, "Second reply");
    assert_eq!(agent.history().len(), 4);

    let reqs = streamer.requests.lock().unwrap();
    assert_eq!(reqs[1].messages.len(), 3);

    let initial_session = agent.session_id().to_string();
    assert!(initial_session.starts_with("ses_"));
    let opts = streamer.options.lock().unwrap();
    assert_eq!(opts.len(), 2);
    for opt in opts.iter() {
        let headers = opt.extra_headers.as_ref().expect("extra_headers");
        let session_hdr = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("x-opencode-session"))
            .map(|(_, v)| v.as_str());
        assert_eq!(session_hdr, Some(initial_session.as_str()));
        let ua_hdr = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
            .map(|(_, v)| v.as_str());
        assert!(ua_hdr.is_some_and(|v| v.starts_with("meerai/")));
    }

    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }

    let mut deltas = Vec::new();
    let mut started_messages = 0;
    let mut completed_messages = 0;
    let mut turn_starts = 0;
    let mut turn_completions = 0;
    let mut usage_updates = Vec::new();
    for ev in &events {
        match ev {
            AgentEvent::ItemDelta {
                delta: ItemDelta::AgentText(d),
                ..
            } => deltas.push(d.clone()),
            AgentEvent::ItemStarted {
                item: SessionItem::AgentMessage { text, .. },
            } => {
                assert!(text.is_empty());
                started_messages += 1;
            }
            AgentEvent::ItemCompleted {
                item: SessionItem::AgentMessage { .. },
            } => completed_messages += 1,
            AgentEvent::TurnStarted { .. } => turn_starts += 1,
            AgentEvent::TurnCompleted {
                reason: TurnEndReason::Completed,
                ..
            } => turn_completions += 1,
            AgentEvent::UsageUpdated {
                usage, cumulative, ..
            } => usage_updates.push((*usage, *cumulative)),
            _ => {}
        }
    }
    assert_eq!(deltas, vec!["Hello ", "world!", "Second reply"]);
    assert_eq!(started_messages, 2);
    assert_eq!(completed_messages, 2);
    assert_eq!(turn_starts, 2);
    assert_eq!(turn_completions, 2);
    assert_eq!(usage_updates.len(), 2);
    assert_eq!(usage_updates[0].0.total_tokens, Some(15));
    assert_eq!(usage_updates[0].1.total_tokens, Some(15));
    assert_eq!(usage_updates[1].0.total_tokens, Some(15));
    assert_eq!(usage_updates[1].1.total_tokens, Some(30));
    assert_eq!(usage_updates[1].1.prompt_tokens, Some(20));
    assert_eq!(usage_updates[1].1.completion_tokens, Some(10));

    agent.reset();
    assert!(agent.history().is_empty());
    assert_ne!(agent.session_id(), initial_session);
}

#[tokio::test]
async fn reports_budget_errors_as_failed_turns() {
    let calls = vec![ToolCall {
        call_id: "c1".to_string(),
        fn_name: "echo-say".to_string(),
        fn_arguments: serde_json::json!({"text": "ok"}),
        thought_signatures: None,
    }];

    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(calls.clone()))],
        vec![ChatStreamEvent::End(tool_end(calls))],
    ]));

    let agent = MultiTurnAgent::with_streamer(
        streamer,
        "gpt-4o-mini",
        vec![Box::new(EchoToolset)],
        "",
        MultiTurnAgentConfig {
            max_cycles: 2,
            max_retries: 1,
        },
    );

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt("first".to_string()))
        .unwrap();

    let mut saw_error = None;
    let mut saw_failed_turn = None;
    while let Some(ev) = rx.recv().await {
        match ev {
            AgentEvent::Error { code, message } => saw_error = Some((code, message)),
            AgentEvent::TurnCompleted {
                reason: TurnEndReason::Failed { error },
                ..
            } => {
                saw_failed_turn = Some(error);
                break;
            }
            _ => {}
        }
    }
    assert_eq!(saw_error.map(|(code, _)| code), Some(ErrorCode::Budget));
    assert!(
        saw_failed_turn
            .as_deref()
            .is_some_and(|msg| msg.contains("Exceeded maximum number of cycles"))
    );

    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn worker_recovers_once_after_a_provider_failure() {
    let streamer = Arc::new(FakeStreamer::with_script(vec![
        ScriptedStream::Error("temporary provider failure".to_string()),
        ScriptedStream::Error("temporary provider failure".to_string()),
        ScriptedStream::Error("temporary provider failure".to_string()),
        ScriptedStream::Events(vec![ChatStreamEvent::End(text_end("recovered"))]),
    ]));
    let session = memory_session();
    let agent = harness_agent(
        streamer,
        Vec::new(),
        "system",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt("go".to_string()))
        .unwrap();

    let mut completions = Vec::new();
    while let Some(event) = rx.recv().await {
        if let AgentEvent::TurnCompleted { reason, .. } = event {
            completions.push(reason);
            if completions.len() == 2 {
                break;
            }
        }
    }
    assert!(matches!(completions[0], TurnEndReason::Failed { .. }));
    assert!(matches!(completions[1], TurnEndReason::Completed));

    let recovered = session_events(&session).iter().any(|event| {
        match event {
        SessionEvent::UserMessage { content, .. } => content.iter().any(|block| {
            matches!(block, ContentBlock::Text(text) if text.contains("previous turn failed"))
        }),
        _ => false,
    }
    });
    assert!(recovered, "recovery prompt must be logged");

    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn worker_uses_the_recovery_model_for_the_recovery_turn() {
    let streamer = Arc::new(FakeStreamer::with_script(vec![
        ScriptedStream::Error("temporary provider failure".to_string()),
        ScriptedStream::Error("temporary provider failure".to_string()),
        ScriptedStream::Error("temporary provider failure".to_string()),
        ScriptedStream::Events(vec![ChatStreamEvent::End(text_end("recovered"))]),
    ]));
    let session = memory_session();
    let agent = harness_agent(
        streamer.clone(),
        Vec::new(),
        "system",
        &session,
        auto_mode(),
        HarnessConfig {
            recovery_model: Some("fallback-model".to_string()),
            ..HarnessConfig::default()
        },
    );
    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt("go".to_string()))
        .unwrap();

    while let Some(event) = rx.recv().await {
        if matches!(
            event,
            AgentEvent::TurnCompleted {
                reason: TurnEndReason::Completed,
                ..
            }
        ) {
            break;
        }
    }

    let models = streamer.models.lock().unwrap();
    assert_eq!(models[0], "gpt-4o-mini");
    assert_eq!(models[1], "gpt-4o-mini");
    assert_eq!(models[2], "gpt-4o-mini");
    assert_eq!(models[3], "fallback-model");

    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn harness_dual_writes_session_events() {
    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        tool_end(vec![tool_call("c1", "stop")]),
    )]]));
    let session = memory_session();
    let mut agent = MultiTurnAgent::with_streamer(
        streamer.clone(),
        "gemini::gemini-3.8-flash",
        vec![Box::new(EchoToolset)],
        "Be precise.",
        MultiTurnAgentConfig::default(),
    );
    agent.set_harness(harness(&session, auto_mode(), HarnessConfig::default()));

    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("do it", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "done now");

    let cached = streamer.options.lock().unwrap();
    assert_eq!(
        cached[0].cache_control,
        Some(genai::chat::CacheControl::Ephemeral)
    );
    drop(cached);

    let events = session_events(&session);
    let kinds: Vec<&str> = events.iter().map(event_kind).collect();
    assert_eq!(
        kinds,
        vec![
            "turn_started",
            "user_message",
            "system_message",
            "step_started",
            "assistant_message",
            "tool_call",
            "tool_result",
            "usage_updated",
            "step_ended",
            "turn_ended",
        ]
    );

    match &events[2] {
        SessionEvent::SystemMessage {
            sections,
            generation,
        } => {
            assert_eq!(*generation, 0);
            assert_eq!(sections.len(), 1);
            assert_eq!(sections[0].label, "instructions");
            assert_eq!(
                sections[0].content,
                crate::prompt::append_model_identity(
                    "Be precise.".to_string(),
                    "gemini::gemini-3.8-flash"
                )
            );
        }
        other => panic!("expected SystemMessage, got {other:?}"),
    }

    match &events[4] {
        SessionEvent::AssistantMessage {
            provider,
            model,
            tool_calls,
            ..
        } => {
            assert_eq!(provider, "gemini");
            assert_eq!(model, "gemini::gemini-3.8-flash");
            assert_eq!(tool_calls.len(), 1);
            assert_eq!(tool_calls[0].id, "c1");
            assert_eq!(tool_calls[0].name, "echo-say");
        }
        other => panic!("expected AssistantMessage, got {other:?}"),
    }

    match &events[6] {
        SessionEvent::ToolResult { call_id, output } => {
            assert_eq!(call_id.as_str(), "c1");
            assert_eq!(output.status, ObservationStatus::Stopped);
            assert_eq!(output.stdout, "done now");
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }

    match &events[9] {
        SessionEvent::TurnEnded { reason, .. } => {
            assert_eq!(reason, &TurnEndReason::Completed);
        }
        other => panic!("expected TurnEnded, got {other:?}"),
    }

    assert_eq!(agent.history().len(), 3);
}

#[tokio::test]
async fn stream_options_carry_inactivity_timeouts() {
    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("done"),
    )]]));
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
    agent
        .prompt_streaming("hi", &tx, CancellationToken::new())
        .await
        .unwrap();

    let options = streamer.options.lock().unwrap();
    assert_eq!(
        options[0].stream_header_timeout,
        Some(STREAM_HEADER_TIMEOUT)
    );
    assert_eq!(options[0].stream_read_timeout, Some(STREAM_READ_TIMEOUT));
}

#[tokio::test]
async fn harness_builds_requests_from_projection() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![
            ChatStreamEvent::ReasoningChunk(StreamChunk {
                content: "secret thoughts".to_string(),
            }),
            ChatStreamEvent::End(tool_end(vec![tool_call("c1", "ok")])),
        ],
        vec![
            ChatStreamEvent::ReasoningChunk(StreamChunk {
                content: "more thoughts".to_string(),
            }),
            ChatStreamEvent::Chunk(StreamChunk {
                content: "final".to_string(),
            }),
            ChatStreamEvent::End(text_end("final")),
        ],
    ]));

    let session = memory_session();
    let mut agent = MultiTurnAgent::with_streamer(
        streamer.clone(),
        "gpt-4o-mini",
        vec![Box::new(EchoToolset)],
        "Be precise.",
        MultiTurnAgentConfig::default(),
    );
    agent.set_harness(harness(&session, auto_mode(), HarnessConfig::default()));

    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("read it", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "final");

    let reqs = streamer.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[1].messages.len(), 3);
    assert_eq!(
        reqs[1].system,
        Some(crate::prompt::append_model_identity(
            "Be precise.".to_string(),
            "gpt-4o-mini"
        ))
    );
    let rendered = format!("{:?}", reqs[1].messages);
    assert!(!rendered.contains("secret thoughts"));
    assert!(!rendered.contains("more thoughts"));
    assert!(rendered.contains("echoed:ok"));

    let reasoning_logged = session_events(&session).iter().any(|event| {
        matches!(
            event,
            SessionEvent::AssistantMessage { content, .. }
                if content.contains(&ContentBlock::Reasoning("secret thoughts".to_string()))
        )
    });
    assert!(reasoning_logged);
}

#[tokio::test]
async fn projection_cache_reflects_new_events() {
    let streamer = Arc::new(FakeStreamer::new(Vec::new()));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer,
        Vec::new(),
        "system",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );

    let first = agent.request_messages();
    let second = agent.request_messages();
    assert_eq!(format!("{first:?}"), format!("{second:?}"));

    session
        .append(SessionEvent::UserMessage {
            source: InputSource::User,
            content: vec![ContentBlock::Text("next".to_string())],
        })
        .await
        .unwrap();
    let third = agent.request_messages();
    assert!(third.iter().any(|message| {
        message
            .content
            .first_text()
            .is_some_and(|text| text == "next")
    }));
}

#[tokio::test]
async fn file_changes_emit_items() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "c1".to_string(),
            fn_name: "diff-write".to_string(),
            fn_arguments: serde_json::json!({}),
            thought_signatures: None,
        }]))],
        vec![ChatStreamEvent::End(text_end("wrote"))],
    ]));

    let session = memory_session();
    let mut agent = MultiTurnAgent::with_streamer(
        streamer,
        "gpt-4o-mini",
        vec![Box::new(DiffToolset)],
        "",
        MultiTurnAgentConfig::default(),
    );
    agent.set_harness(harness(&session, auto_mode(), HarnessConfig::default()));

    let (tx, mut rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("write", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "wrote");

    let mut started_changes = 0;
    let mut completed_changes = Vec::new();
    while let Ok(event) = rx.try_recv() {
        match event {
            AgentEvent::ItemStarted {
                item: SessionItem::FileChange { status, .. },
            } => {
                assert_eq!(status, ItemStatus::InProgress);
                started_changes += 1;
            }
            AgentEvent::ItemCompleted {
                item:
                    SessionItem::FileChange {
                        changes, status, ..
                    },
            } => completed_changes.push((changes, status)),
            _ => {}
        }
    }
    assert_eq!(started_changes, 1);
    assert_eq!(completed_changes.len(), 1);
    assert_eq!(completed_changes[0].0.len(), 1);
    assert_eq!(completed_changes[0].0[0].path, "a.rs");
    assert_eq!(completed_changes[0].1, ItemStatus::Succeeded);
}

#[test]
fn estimates_context_tokens_from_message_text() {
    assert_eq!(estimate_context_tokens(&[]), 0);
    assert_eq!(estimate_context_tokens(&[ChatMessage::user("abcdefgh")]), 2);
    assert_eq!(
        estimate_context_tokens(&[
            ChatMessage::user("abcdefgh"),
            ChatMessage::assistant("ijkl")
        ]),
        3
    );

    let with_system =
        estimate_context_tokens(&[ChatMessage::user("abcd")]) + estimate_tokens("efgh");
    assert_eq!(with_system, 2);

    assert_eq!(truncate_tokens("abcdefgh", 1), "abcd\n... [truncated]");
    assert_eq!(truncate_tokens("abc", 1), "abc");
}

#[tokio::test]
async fn token_cap_accept_raises_the_limit() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end_counting(
            vec![tool_call("c1", "ok")],
            15,
        ))],
        vec![ChatStreamEvent::End(text_end("budget raised"))],
    ]));

    let session = memory_session();
    let config = HarnessConfig {
        max_session_tokens: Some(15),
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

    let (commands_tx, mut commands_rx) = mpsc::unbounded_channel();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let driver = tokio::spawn(async move {
        let mut spend_requests = 0;
        while let Some(event) = event_rx.recv().await {
            match event {
                AgentEvent::ApprovalRequested { request } => {
                    if request.kind == ApprovalKind::Spend {
                        assert_eq!(request.preview.title, "Session token budget reached");
                        spend_requests += 1;
                    }
                    let _ = commands_tx.send(AgentCommand::Approve {
                        request_id: request.request_id,
                        decision: ApprovalDecision::Accept,
                        scope: None,
                    });
                }
                AgentEvent::TurnCompleted { .. } => break,
                _ => {}
            }
        }
        spend_requests
    });

    let out = agent
        .prompt_streaming_with_commands("go", &event_tx, CancellationToken::new(), &mut commands_rx)
        .await
        .unwrap();
    assert_eq!(out, "budget raised");
    assert_eq!(driver.await.unwrap(), 1);
    assert_eq!(
        agent.harness().unwrap().config.max_session_tokens,
        Some(15 + 128_000)
    );
    assert_eq!(streamer.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn zero_token_cap_means_unlimited() {
    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("no cap"),
    )]]));

    let session = memory_session();
    let config = HarnessConfig {
        max_session_tokens: Some(0),
        ..HarnessConfig::default()
    };
    let mut agent = harness_agent(streamer.clone(), vec![], "", &session, auto_mode(), config);

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("go", &event_tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "no cap");

    let mut spend_requests = 0;
    while let Ok(event) = event_rx.try_recv() {
        if let AgentEvent::ApprovalRequested { request } = event
            && request.kind == ApprovalKind::Spend
        {
            spend_requests += 1;
        }
    }
    assert_eq!(spend_requests, 0);
}

#[tokio::test]
async fn token_cap_decline_fails_the_turn() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end_counting(
            vec![tool_call("c1", "ok")],
            15,
        ))],
        vec![ChatStreamEvent::End(text_end("unreachable"))],
    ]));

    let session = memory_session();
    let config = HarnessConfig {
        max_session_tokens: Some(15),
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

    let (commands_tx, mut commands_rx) = mpsc::unbounded_channel();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let driver = tokio::spawn(async move {
        let mut errors = Vec::new();
        let mut outcome = None;
        while let Some(event) = event_rx.recv().await {
            match event {
                AgentEvent::ApprovalRequested { request } => {
                    let _ = commands_tx.send(AgentCommand::Approve {
                        request_id: request.request_id,
                        decision: ApprovalDecision::Decline,
                        scope: None,
                    });
                }
                AgentEvent::Error { code, .. } => errors.push(code),
                AgentEvent::TurnCompleted { reason, .. } => {
                    outcome = Some(reason);
                    break;
                }
                _ => {}
            }
        }
        (errors, outcome)
    });

    let _ = agent
        .prompt_streaming_with_commands("go", &event_tx, CancellationToken::new(), &mut commands_rx)
        .await;

    let (errors, outcome) = driver.await.unwrap();
    assert_eq!(errors, vec![ErrorCode::Budget]);
    assert_eq!(
        outcome,
        Some(TurnEndReason::Failed {
            error: TOKEN_BUDGET_DECLINED.to_string()
        })
    );
    assert_eq!(streamer.requests.lock().unwrap().len(), 1);

    let events = session_events(&session);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::Error {
            code: ErrorCode::Budget,
            retryable: false,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::TurnEnded {
            reason: TurnEndReason::Failed { error },
            ..
        } if error == TOKEN_BUDGET_DECLINED
    )));
}

#[async_trait]
impl Toolset for BarrierToolset {
    fn name(&self) -> String {
        "barrier".to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        ["barrier-a", "barrier-b"]
            .into_iter()
            .map(|name| ToolDefinition {
                r#type: "function".to_string(),
                name: name.to_string(),
                description: "Waits for its sibling to run".to_string(),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
            })
            .collect()
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == "barrier-a" || fn_name == "barrier-b"
    }

    fn effect_of(&self, _fn_name: &str) -> EffectClass {
        EffectClass::Read
    }

    async fn invoke(&self, fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
        tokio::time::timeout(Duration::from_secs(2), self.barrier.wait())
            .await
            .map_err(|_| anyhow!("barrier timed out; calls did not overlap"))?;
        Ok(ToolOutput::Text(format!("ran:{fn_name}")))
    }
}

#[async_trait]
impl Toolset for StopBatchToolset {
    fn name(&self) -> String {
        "batch".to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        ["batch-stop", "batch-after"]
            .into_iter()
            .map(|name| ToolDefinition {
                r#type: "function".to_string(),
                name: name.to_string(),
                description: "Stops the batch or records a later call".to_string(),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
            })
            .collect()
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == "batch-stop" || fn_name == "batch-after"
    }

    fn effect_of(&self, _fn_name: &str) -> EffectClass {
        self.effect
    }

    async fn invoke(&self, fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
        if fn_name == "batch-stop" {
            return Ok(ToolOutput::Stop("stopped the batch".to_string()));
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::Text("should not run".to_string()))
    }
}

#[async_trait]
impl Toolset for OrderToolset {
    fn name(&self) -> String {
        "order".to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        ["order-read", "order-write"]
            .into_iter()
            .map(|name| ToolDefinition {
                r#type: "function".to_string(),
                name: name.to_string(),
                description: "Records its execution position".to_string(),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
            })
            .collect()
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == "order-read" || fn_name == "order-write"
    }

    fn effect_of(&self, fn_name: &str) -> EffectClass {
        if fn_name == "order-read" {
            EffectClass::Read
        } else {
            EffectClass::Write
        }
    }

    async fn invoke(&self, fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
        let label = fn_name.strip_prefix("order-").unwrap_or(fn_name);
        self.order.lock().unwrap().push(label.to_string());
        Ok(ToolOutput::Text(format!("did:{label}")))
    }
}

#[test]
fn turn_deadline_comparison_uses_elapsed_seconds() {
    let started = Instant::now();
    assert!(!turn_deadline_exceeded(started, 30, started));
    assert!(!turn_deadline_exceeded(
        started,
        30,
        started + Duration::from_secs(29)
    ));
    assert!(turn_deadline_exceeded(
        started,
        30,
        started + Duration::from_secs(30)
    ));
    assert!(!turn_deadline_exceeded(
        started,
        0,
        started + Duration::from_secs(3_600)
    ));

    assert_eq!(provider_backoff_after(1), Duration::from_millis(250));
    assert_eq!(provider_backoff_after(2), Duration::from_millis(500));
    assert_eq!(provider_backoff_after(3), Duration::from_millis(1_000));
    for _ in 0..16 {
        let delay = provider_backoff_with_jitter(1);
        assert!(delay >= Duration::from_millis(250));
        assert!(delay < Duration::from_millis(375));
    }
    assert!(status_is_transient(429));
    assert!(status_is_transient(503));
    assert!(!status_is_transient(400));
    assert!(!status_is_transient(401));
    assert_eq!(provider_error_status(&anyhow::Error::msg("plain")), None);
}

#[test]
fn plan_usage_codes_are_terminal_over_the_status_rule() {
    for code in PLAN_USAGE_TERMINAL_CODES {
        assert!(is_terminal_plan_usage_error(code));
        assert!(!is_transient_provider_error_with_status(Some(429), code));
        assert!(!is_transient_provider_error_with_status(Some(503), code));
    }

    assert!(!is_terminal_plan_usage_error(
        "subscription_sharing_usage_unavailable"
    ));
    assert!(is_transient_provider_error_with_status(
        Some(503),
        "subscription_sharing_usage_unavailable"
    ));
    assert!(is_transient_provider_error_with_status(
        None,
        "subscription_sharing_user_unavailable"
    ));

    assert!(is_transient_provider_error_with_status(
        Some(429),
        "rate limited"
    ));
    assert!(is_transient_provider_error_with_status(
        Some(500),
        "upstream"
    ));
    assert!(!is_transient_provider_error_with_status(
        Some(400),
        "bad body"
    ));
    assert!(!is_transient_provider_error_with_status(
        None,
        "invalid model"
    ));
}

#[test]
fn remaining_turn_budget_handles_disabled_exhausted_and_remaining() {
    let started = Instant::now();
    assert_eq!(remaining_turn_budget(started, 0), None);
    assert_eq!(
        remaining_turn_budget(started - Duration::from_secs(120), 30),
        None
    );

    let left = remaining_turn_budget(started, 30).expect("budget remaining");
    assert!(left > Duration::from_secs(29));
    assert!(left <= Duration::from_secs(30));
}

#[tokio::test]
async fn executed_reads_keep_their_result_when_a_sibling_stops() {
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
            effect: EffectClass::Read,
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
    assert_eq!(calls.load(Ordering::SeqCst), 1);

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
            (CallId::from_raw("c2"), ItemStatus::Succeeded),
        ]
    );

    let results: Vec<(String, ObservationStatus, String)> = session_events(&session)
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::ToolResult { call_id, output } => {
                Some((call_id.as_str().to_string(), output.status, output.stdout))
            }
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].0, "c1");
    assert_eq!(results[0].1, ObservationStatus::Stopped);
    assert_eq!(results[1].0, "c2");
    assert_eq!(results[1].1, ObservationStatus::Success);
    assert_eq!(results[1].2, "should not run");
}

#[tokio::test]
async fn stalled_stream_ends_the_turn_at_the_wall_clock_budget() {
    let streamer = Arc::new(FakeStreamer::with_script(vec![ScriptedStream::Stall]));

    let session = memory_session();
    let config = HarnessConfig {
        max_turn_secs: 1,
        ..HarnessConfig::default()
    };
    let mut agent = harness_agent(streamer.clone(), vec![], "", &session, auto_mode(), config);

    let (tx, _rx) = mpsc::unbounded_channel();
    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        agent.prompt_streaming("go", &tx, CancellationToken::new()),
    )
    .await
    .expect("stalled turn did not end within 5s");
    assert!(result.is_err());
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the wall clock must end the stalled stream near 1s"
    );

    let events = session_events(&session);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::Error {
            code: ErrorCode::Budget,
            message,
            retryable: false,
        } if message == "turn exceeded 1s"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::TurnEnded {
            reason: TurnEndReason::Failed { .. },
            ..
        }
    )));
    assert_eq!(streamer.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn provider_stream_with_usage_but_no_content_is_not_retried() {
    let end_with_usage = StreamEnd {
        captured_usage: Some(Usage {
            prompt_tokens: Some(10),
            completion_tokens: Some(5),
            total_tokens: Some(15),
            ..Default::default()
        }),
        ..Default::default()
    };
    let streamer = Arc::new(FakeStreamer::with_script(vec![ScriptedStream::Events(
        vec![ChatStreamEvent::End(end_with_usage)],
    )]));

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

    assert!(
        session_events(&session)
            .into_iter()
            .all(|event| !matches!(event, SessionEvent::AssistantAttempt { .. }))
    );
}

async fn scoped_workspace(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("meerai-{tag}-{}", SessionId::new()));
    tokio::fs::create_dir_all(root.join("src")).await.unwrap();
    tokio::fs::write(root.join("src/AGENTS.md"), "SCOPED RULE: keep tests green")
        .await
        .unwrap();
    root
}

#[tokio::test]
async fn scoped_agents_md_reaches_the_next_request_once() {
    let root = scoped_workspace("scoped").await;
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![path_call(
            "c1",
            "src/lib.rs",
        )]))],
        vec![ChatStreamEvent::End(tool_end(vec![path_call(
            "c2",
            "src/main.rs",
        )]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));

    let session = Session::memory(SessionId::new(), root.clone());
    let mut agent = MultiTurnAgent::with_streamer(
        streamer.clone(),
        "gpt-4o-mini",
        vec![Box::new(EchoToolset)],
        "",
        MultiTurnAgentConfig::default(),
    );
    agent.set_harness(harness(&session, auto_mode(), HarnessConfig::default()));

    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("look around", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "done");

    let events = session_events(&session);
    let injections = runtime_injections(&events);
    assert_eq!(injections.len(), 1);
    assert!(
        injections[0]
            .1
            .starts_with("Project instructions from src/AGENTS.md:")
    );
    assert!(injections[0].1.contains("SCOPED RULE: keep tests green"));
    let result_index = events
        .iter()
        .position(|event| matches!(event, SessionEvent::ToolResult { .. }))
        .unwrap();
    assert!(injections[0].0 > result_index);

    let reqs = streamer.requests.lock().unwrap();
    assert_eq!(reqs.len(), 3);
    assert!(!format!("{:?}", reqs[0].messages).contains("SCOPED RULE"));
    assert!(format!("{:?}", reqs[1].messages).contains("SCOPED RULE"));
    assert!(format!("{:?}", reqs[2].messages).contains("SCOPED RULE"));
    drop(reqs);

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn directory_path_injects_its_own_agents_md() {
    let root = scoped_workspace("scoped-dir").await;
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![path_call("c1", "src")]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));

    let session = Session::memory(SessionId::new(), root.clone());
    let mut agent = MultiTurnAgent::with_streamer(
        streamer.clone(),
        "gpt-4o-mini",
        vec![Box::new(EchoToolset)],
        "",
        MultiTurnAgentConfig::default(),
    );
    agent.set_harness(harness(&session, auto_mode(), HarnessConfig::default()));

    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("list src", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "done");

    let injections = runtime_injections(&session_events(&session));
    assert_eq!(injections.len(), 1);
    assert!(
        injections[0]
            .1
            .starts_with("Project instructions from src/AGENTS.md:")
    );

    let reqs = streamer.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    assert!(format!("{:?}", reqs[1].messages).contains("SCOPED RULE"));
    drop(reqs);

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn path_outside_workspace_injects_nothing() {
    let root = scoped_workspace("outside").await;
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![path_call(
            "c1",
            "../outside/src/lib.rs",
        )]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));

    let session = Session::memory(SessionId::new(), root.clone());
    let mut agent = MultiTurnAgent::with_streamer(
        streamer.clone(),
        "gpt-4o-mini",
        vec![Box::new(EchoToolset)],
        "",
        MultiTurnAgentConfig::default(),
    );
    agent.set_harness(harness(&session, auto_mode(), HarnessConfig::default()));

    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("escape", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "done");

    assert!(runtime_injections(&session_events(&session)).is_empty());
    let reqs = streamer.requests.lock().unwrap();
    assert!(!format!("{:?}", reqs[1].messages).contains("SCOPED RULE"));
    drop(reqs);

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn scoped_instructions_are_cleared_between_turns() {
    let root = scoped_workspace("scoped-turns").await;
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![path_call(
            "c1",
            "src/lib.rs",
        )]))],
        vec![ChatStreamEvent::End(text_end("first"))],
        vec![ChatStreamEvent::End(tool_end(vec![path_call(
            "c2",
            "src/lib.rs",
        )]))],
        vec![ChatStreamEvent::End(text_end("second"))],
    ]));

    let session = Session::memory(SessionId::new(), root.clone());
    let mut agent = MultiTurnAgent::with_streamer(
        streamer.clone(),
        "gpt-4o-mini",
        vec![Box::new(EchoToolset)],
        "",
        MultiTurnAgentConfig::default(),
    );
    agent.set_harness(harness(&session, auto_mode(), HarnessConfig::default()));

    let (tx, _rx) = mpsc::unbounded_channel();
    let first = agent
        .prompt_streaming("one", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(first, "first");
    let second = agent
        .prompt_streaming("two", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(second, "second");

    assert_eq!(runtime_injections(&session_events(&session)).len(), 2);

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn first_prompt_sets_a_single_line_session_title() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(text_end("first reply"))],
        vec![ChatStreamEvent::End(text_end("second reply"))],
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer,
        vec![],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    let prompt = format!("  {}   tail\nsecond line is ignored", "a".repeat(200));

    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming(&prompt, &tx, CancellationToken::new())
        .await
        .unwrap();

    let title = session.meta().title.expect("first prompt sets a title");
    assert_eq!(title, "a".repeat(80));
    assert!(!title.contains('\n'));
    assert!(!title.contains("second line"));

    agent
        .prompt_streaming("a later prompt", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(session.meta().title.as_deref(), Some(title.as_str()));
}

#[tokio::test]
async fn attaching_a_harness_seeds_cumulative_usage() {
    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("resumed reply"),
    )]]));
    let session = memory_session();
    let seeded: meerai_session::Usage = meerai_session::Usage {
        prompt_tokens: Some(100),
        completion_tokens: Some(40),
        total_tokens: Some(140),
        ..Default::default()
    };
    session
        .append(SessionEvent::UsageUpdated {
            usage: seeded,
            cumulative: seeded,
        })
        .await
        .unwrap();

    let mut agent = harness_agent(
        streamer,
        vec![],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("continue", &tx, CancellationToken::new())
        .await
        .unwrap();

    let mut latest = None;
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::UsageUpdated { cumulative, .. } = event {
            latest = Some(cumulative);
        }
    }
    assert_eq!(latest.unwrap().prompt_tokens, Some(110));
    assert_eq!(latest.unwrap().completion_tokens, Some(45));
    assert_eq!(latest.unwrap().total_tokens, Some(155));

    let logged = session_events(&session)
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::UsageUpdated { cumulative, .. } => Some(cumulative),
            _ => None,
        })
        .next_back()
        .expect("usage logged");
    assert_eq!(logged.prompt_tokens, Some(110));
    assert_eq!(logged.completion_tokens, Some(45));
    assert_eq!(logged.total_tokens, Some(155));
}

#[tokio::test]
async fn usage_reports_estimated_cost_when_pricing_is_configured() {
    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("done"),
    )]]));
    let session = memory_session();
    let config = HarnessConfig {
        token_pricing: Some(crate::harness::TokenPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
        }),
        ..HarnessConfig::default()
    };
    let mut agent = harness_agent(streamer, Vec::new(), "", &session, auto_mode(), config);

    let (tx, mut rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();

    let mut cost = None;
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::UsageUpdated { est_cost_usd, .. } = event {
            cost = est_cost_usd;
        }
    }
    let cost = cost.expect("cost reported");
    assert!((cost - 0.000105).abs() < 1e-12, "unexpected cost {cost}");
}

#[cfg(unix)]
#[tokio::test]
async fn user_prompt_submit_block_fails_the_turn_without_logging_user_message() {
    let streamer = Arc::new(FakeStreamer::new(Vec::new()));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer,
        Vec::new(),
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    agent.set_hooks(Arc::new(crate::hooks::HookEngine::new(vec![hook_entry(
        HookEvent::UserPromptSubmit,
        "printf 'blocked prompt' >&2; exit 2",
    )])));

    let (tx, _rx) = mpsc::unbounded_channel();
    let error = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("blocked prompt"));

    let events = session_events(&session);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::Error { code: ErrorCode::Config, message, .. }
            if message == "blocked prompt"
    )));
    assert!(!events.iter().any(|event| matches!(
        event,
        SessionEvent::UserMessage {
            source: InputSource::User,
            ..
        }
    )));
    assert!(session.meta().title.is_none());
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::TurnEnded { reason: TurnEndReason::Failed { error }, .. }
            if error == "blocked prompt"
    )));
}

#[cfg(unix)]
#[tokio::test]
async fn session_start_and_end_fire_once_through_the_worker() {
    let streamer = Arc::new(FakeStreamer::new(Vec::new()));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer,
        Vec::new(),
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    agent.set_hooks(Arc::new(crate::hooks::HookEngine::new(vec![
        hook_entry(HookEvent::SessionStart, "exit 0"),
        hook_entry(HookEvent::SessionEnd, "exit 0"),
    ])));

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx.send(AgentCommand::Quit).unwrap();
    while rx.recv().await.is_some() {}

    let events = session_events(&session);
    let count = |name: &str| {
        events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    SessionEvent::HookFired { event, .. } if event == name
                )
            })
            .count()
    };
    assert_eq!(count("SessionStart"), 1);
    assert_eq!(count("SessionEnd"), 1);
}

#[tokio::test]
async fn prompt_caching_disabled_sends_no_cache_control() {
    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("done"),
    )]]));
    let session = memory_session();
    let mut agent = MultiTurnAgent::with_streamer(
        streamer.clone(),
        "gemini::gemini-3.8-flash",
        Vec::new(),
        "",
        MultiTurnAgentConfig::default(),
    );
    let config = HarnessConfig {
        prompt_caching: false,
        ..HarnessConfig::default()
    };
    agent.set_harness(harness(&session, auto_mode(), config));

    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();

    let cached = streamer.options.lock().unwrap();
    assert_eq!(cached[0].cache_control, None);
}

#[tokio::test]
async fn plan_usage_omits_cache_control_and_temperature() {
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
    let config = HarnessConfig {
        plan_usage: true,
        prompt_caching: true,
        temperature: Some(0.7),
        ..HarnessConfig::default()
    };
    agent.set_harness(harness(&session, auto_mode(), config));

    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();

    let cached = streamer.options.lock().unwrap();
    assert_eq!(cached[0].cache_control, None);
    assert_eq!(cached[0].temperature, None);
}

#[tokio::test]
async fn api_key_profile_keeps_cache_control_and_temperature() {
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
    let config = HarnessConfig {
        plan_usage: false,
        prompt_caching: true,
        temperature: Some(0.7),
        ..HarnessConfig::default()
    };
    agent.set_harness(harness(&session, auto_mode(), config));

    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();

    let cached = streamer.options.lock().unwrap();
    assert_eq!(
        cached[0].cache_control,
        Some(genai::chat::CacheControl::Ephemeral)
    );
    assert_eq!(cached[0].temperature, Some(f64::from(0.7f32)));
}

#[test]
fn usage_copies_cache_token_details() {
    let usage = Usage {
        prompt_tokens: Some(100),
        completion_tokens: Some(5),
        total_tokens: Some(105),
        prompt_tokens_details: Some(genai::chat::PromptTokensDetails {
            cache_creation_tokens: Some(40),
            cached_tokens: Some(60),
            ..Default::default()
        }),
        ..Default::default()
    };
    let converted = meerai_session::Usage::from(usage);
    assert_eq!(converted.cached_tokens, Some(60));
    assert_eq!(converted.cache_creation_tokens, Some(40));
}

struct PlanningCancelToolset {
    cancel: CancellationToken,
}

#[async_trait]
impl Toolset for PlanningCancelToolset {
    fn name(&self) -> String {
        "echo".to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            r#type: "function".to_string(),
            name: "echo-say".to_string(),
            description: "Cancels the turn while planning".to_string(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
        }]
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == "echo-say"
    }

    fn effect_of(&self, _fn_name: &str) -> EffectClass {
        EffectClass::Read
    }

    fn effect_of_call(&self, _fn_name: &str, _args: &str) -> EffectClass {
        self.cancel.cancel();
        EffectClass::Read
    }

    async fn invoke(&self, _fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::Text("should not run".to_string()))
    }
}

#[tokio::test]
async fn turn_end_heals_calls_left_unexecuted_by_a_cancel() {
    let cancel = CancellationToken::new();
    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        tool_end(vec![tool_call("call-1", "one"), tool_call("call-2", "two")]),
    )]]));
    let session = memory_session();
    let mut agent = MultiTurnAgent::with_streamer(
        streamer,
        "gemini::gemini-3.8-flash",
        vec![Box::new(PlanningCancelToolset {
            cancel: cancel.clone(),
        })],
        "",
        MultiTurnAgentConfig::default(),
    );
    agent.set_harness(harness(&session, auto_mode(), HarnessConfig::default()));

    let (tx, _rx) = mpsc::unbounded_channel();
    agent.prompt_streaming("go", &tx, cancel).await.unwrap();

    let events = session_events(&session);
    let turn_ended = events
        .iter()
        .position(|event| matches!(event, SessionEvent::TurnEnded { .. }))
        .expect("turn must end");
    let before_end = &events[..turn_ended];
    assert_eq!(
        before_end
            .iter()
            .filter(|event| matches!(event, SessionEvent::ToolResult { .. }))
            .count(),
        0,
        "cancel during planning must leave results unlogged"
    );
    let repaired: Vec<CallId> = events[turn_ended..]
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ToolResult { call_id, output } => {
                assert!(
                    output.stderr.contains("interrupted before completion"),
                    "unexpected repair: {}",
                    output.stderr
                );
                Some(call_id.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        repaired,
        vec![CallId::from_raw("call-1"), CallId::from_raw("call-2")]
    );

    session.heal_dangling_tool_calls().await.unwrap();
    assert_eq!(
        session_events(&session)
            .iter()
            .filter(|event| matches!(event, SessionEvent::ToolResult { .. }))
            .count(),
        2,
        "repair must be idempotent"
    );
}
