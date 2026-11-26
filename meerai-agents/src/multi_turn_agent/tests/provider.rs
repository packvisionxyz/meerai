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
async fn transient_provider_error_is_retried_and_logged() {
    let streamer = Arc::new(FakeStreamer::with_script(vec![
        ScriptedStream::Error("connection reset by peer".to_string()),
        ScriptedStream::Events(vec![ChatStreamEvent::End(text_end("recovered"))]),
    ]));

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
    assert_eq!(out, "recovered");
    assert_eq!(streamer.requests.lock().unwrap().len(), 2);

    let attempts = session_events(&session)
        .into_iter()
        .filter(|event| matches!(event, SessionEvent::AssistantAttempt { .. }))
        .count();
    assert_eq!(attempts, 1);
}

#[tokio::test]
async fn partial_stream_failure_discards_output_and_retries() {
    let streamer = Arc::new(FakeStreamer::with_script(vec![
        ScriptedStream::EventsThenError(
            vec![ChatStreamEvent::Chunk(StreamChunk {
                content: "partial answer".to_string(),
            })],
            "LLM stream error: Web stream error for model 'openai::m'.\nCause: error decoding response body"
                .to_string(),
        ),
        ScriptedStream::Events(vec![ChatStreamEvent::End(text_end("recovered"))]),
    ]));

    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        vec![],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "recovered");
    assert_eq!(streamer.requests.lock().unwrap().len(), 2);

    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    let discarded: Vec<ItemId> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ItemDiscarded { item_id } => Some(item_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(discarded.len(), 1);

    let messages: Vec<String> = session_events(&session)
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::AssistantMessage { content, .. } => Some(
                content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text(text) => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect();
    assert_eq!(messages, vec!["recovered".to_string()]);
}

#[tokio::test]
async fn partial_stream_auth_failure_is_not_retried() {
    let streamer = Arc::new(FakeStreamer::with_script(vec![
        ScriptedStream::EventsThenError(
            vec![ChatStreamEvent::Chunk(StreamChunk {
                content: "partial answer".to_string(),
            })],
            "LLM stream error: HTTP error.\nStatus: 401 Unauthorized".to_string(),
        ),
    ]));

    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        vec![],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    let error = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("401"));
    assert_eq!(streamer.requests.lock().unwrap().len(), 1);

    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::ItemDiscarded { .. }))
    );
}

#[tokio::test]
async fn provider_auth_error_is_not_retried() {
    let streamer = Arc::new(FakeStreamer::with_script(vec![ScriptedStream::Error(
        "401 Unauthorized: invalid API key".to_string(),
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
    let error = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("invalid API key"));
    assert_eq!(streamer.requests.lock().unwrap().len(), 1);

    let events = session_events(&session);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::AssistantAttempt { .. }))
            .count(),
        1
    );
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::Error {
            code: ErrorCode::Provider,
            retryable: false,
            ..
        }
    )));
}

#[tokio::test]
async fn transient_provider_failure_marks_the_turn_retryable() {
    let streamer = Arc::new(FakeStreamer::with_script(vec![
        ScriptedStream::Error("network timeout".to_string()),
        ScriptedStream::Error("network timeout".to_string()),
        ScriptedStream::Error("network timeout".to_string()),
    ]));

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
    let result = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await;
    assert!(result.is_err());
    assert_eq!(
        streamer.requests.lock().unwrap().len(),
        MAX_PROVIDER_ATTEMPTS
    );

    let events = session_events(&session);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::AssistantAttempt { .. }))
            .count(),
        MAX_PROVIDER_ATTEMPTS
    );
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::Error {
            code: ErrorCode::Provider,
            retryable: true,
            ..
        }
    )));
}

#[tokio::test]
async fn empty_provider_stream_is_retried_and_recovers() {
    let streamer = Arc::new(FakeStreamer::with_script(vec![
        ScriptedStream::Events(vec![ChatStreamEvent::End(StreamEnd::default())]),
        ScriptedStream::Events(vec![ChatStreamEvent::End(text_end("recovered"))]),
    ]));

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
    assert_eq!(out, "recovered");
    assert_eq!(streamer.requests.lock().unwrap().len(), 2);

    let attempts = session_events(&session)
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::AssistantAttempt { error, .. } => Some(error),
            _ => None,
        })
        .collect::<Vec<String>>();
    assert_eq!(attempts.len(), 1);
    assert!(attempts[0].starts_with(EMPTY_STREAM_ERROR));
    assert!(attempts[0].contains("stop_reason=none"));
    assert!(attempts[0].contains("events=1"));
}

#[tokio::test]
async fn empty_provider_stream_fails_as_transient_after_max_attempts() {
    let streamer = Arc::new(FakeStreamer::with_script(vec![
        ScriptedStream::Events(vec![ChatStreamEvent::End(StreamEnd::default())]),
        ScriptedStream::Events(vec![ChatStreamEvent::End(StreamEnd::default())]),
        ScriptedStream::Events(vec![ChatStreamEvent::End(StreamEnd::default())]),
    ]));

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
    let result = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await;
    assert!(result.is_err());
    assert_eq!(
        streamer.requests.lock().unwrap().len(),
        MAX_PROVIDER_ATTEMPTS
    );

    let events = session_events(&session);
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    SessionEvent::AssistantAttempt { error, .. } if error.starts_with(EMPTY_STREAM_ERROR)
                )
            })
            .count(),
        MAX_PROVIDER_ATTEMPTS
    );
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::Error {
            code: ErrorCode::Provider,
            retryable: true,
            ..
        }
    )));
}

#[tokio::test]
async fn empty_provider_stream_reports_provider_and_stop_reason() {
    let end = || StreamEnd {
        captured_stop_reason: Some(StopReason::Completed("STOP".to_string())),
        ..Default::default()
    };
    let streamer = Arc::new(FakeStreamer::with_script(vec![
        ScriptedStream::Events(vec![ChatStreamEvent::Start, ChatStreamEvent::End(end())]),
        ScriptedStream::Events(vec![ChatStreamEvent::Start, ChatStreamEvent::End(end())]),
        ScriptedStream::Events(vec![ChatStreamEvent::Start, ChatStreamEvent::End(end())]),
    ]));

    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        vec![],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    agent.set_model("gemini::gemini-2.5-flash");

    let (tx, _rx) = mpsc::unbounded_channel();
    let error = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap_err();
    let message = format!("{error:#}");

    assert!(message.starts_with(EMPTY_STREAM_ERROR));
    assert!(message.contains("provider=gemini"));
    assert!(message.contains("model=gemini::gemini-2.5-flash"));
    assert!(message.contains("stop_reason=completed(STOP)"));
    assert!(message.contains("events=2"));
    assert!(message.contains("hint:"));
}

#[tokio::test]
async fn empty_provider_stream_with_content_filter_is_not_retried() {
    let end = StreamEnd {
        captured_stop_reason: Some(StopReason::ContentFilter("SAFETY".to_string())),
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
    let error = agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap_err();
    let message = format!("{error:#}");

    assert_eq!(streamer.requests.lock().unwrap().len(), 1);
    assert!(message.contains("content filter"));
    assert!(message.contains("stop_reason=content_filter(SAFETY)"));
    assert!(session_events(&session).iter().any(|event| matches!(
        event,
        SessionEvent::Error {
            code: ErrorCode::Provider,
            retryable: false,
            ..
        }
    )));
}
