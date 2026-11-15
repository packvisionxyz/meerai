use super::compaction::*;
use super::turn::*;
use super::*;
use crate::harness::HarnessConfig;
use futures::stream;
use genai::chat::{MessageContent, StreamChunk, Usage};
use meerai_core::{
    FileChangeKind, FileDiff, ObservationStatus, ToolDefinition, ToolError, async_trait,
};
use meerai_policy::{ActionSelector, ApprovalMode, Effect, PermissionRule, PolicyEngine};
use meerai_session::{COMPACTED_MARKER, Session, SessionConfig, SessionId};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{Barrier, Notify};

enum ScriptedStream {
    Events(Vec<ChatStreamEvent>),
    Error(String),
    Stall,
}

struct FakeStreamer {
    turns: Mutex<VecDeque<ScriptedStream>>,
    requests: Mutex<Vec<ChatRequest>>,
    options: Mutex<Vec<ChatOptions>>,
    models: Mutex<Vec<String>>,
}

impl FakeStreamer {
    fn new(turns: Vec<Vec<ChatStreamEvent>>) -> Self {
        Self::with_script(turns.into_iter().map(ScriptedStream::Events).collect())
    }

    fn with_script(turns: Vec<ScriptedStream>) -> Self {
        Self {
            turns: Mutex::new(turns.into()),
            requests: Mutex::new(Vec::new()),
            options: Mutex::new(Vec::new()),
            models: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl ChatStreamer for FakeStreamer {
    async fn stream_chat(
        &self,
        model: &str,
        request: ChatRequest,
        options: &ChatOptions,
    ) -> Result<BoxedChatStream> {
        self.models.lock().unwrap().push(model.to_string());
        self.requests.lock().unwrap().push(request);
        self.options.lock().unwrap().push(options.clone());
        let scripted = self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra LLM turn");
        match scripted {
            ScriptedStream::Events(events) => {
                Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
            }
            ScriptedStream::Error(message) => Err(anyhow!(message)),
            ScriptedStream::Stall => Ok(Box::pin(
                stream::once(async { Ok(ChatStreamEvent::Start) })
                    .chain(stream::pending::<Result<ChatStreamEvent>>()),
            )),
        }
    }
}

struct EchoToolset;

#[async_trait]
impl Toolset for EchoToolset {
    fn name(&self) -> String {
        "echo".to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            r#type: "function".to_string(),
            name: "echo-say".to_string(),
            description: "Echoes text or fails when asked".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "text": { "type": "string" } }
            }),
        }]
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == "echo-say"
    }

    async fn invoke(&self, _fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
        let v: serde_json::Value = serde_json::from_str(args)?;
        let text = v["text"].as_str().unwrap_or_default().to_string();
        if text == "fail" {
            Ok(ToolOutput::Fail("soft failure".to_string()))
        } else if text == "stop" {
            Ok(ToolOutput::Stop("done now".to_string()))
        } else {
            Ok(ToolOutput::Text(format!("echoed:{text}")))
        }
    }
}

fn text_end(text: &str) -> StreamEnd {
    StreamEnd {
        captured_content: Some(MessageContent::from_text(text)),
        captured_usage: Some(Usage {
            prompt_tokens: Some(10),
            completion_tokens: Some(5),
            total_tokens: Some(15),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn tool_end(calls: Vec<ToolCall>) -> StreamEnd {
    StreamEnd {
        captured_content: Some(MessageContent::from_tool_calls(calls)),
        ..Default::default()
    }
}

fn memory_session() -> Session {
    Session::memory(SessionId::new(), PathBuf::from("/tmp/workspace"))
}

async fn persistent_session(root: &Path) -> Session {
    let config = SessionConfig {
        root: root.to_path_buf(),
        workspace: root.to_path_buf(),
        project_local: false,
    };
    Session::open_or_create(&config, SessionId::new(), PathBuf::from("/tmp/workspace"))
        .await
        .unwrap()
}

struct StaticToolset {
    toolset: String,
    tool: String,
    output: String,
    effect: EffectClass,
    spill: bool,
}

impl StaticToolset {
    fn new(toolset: &str, tool: &str, output: impl Into<String>, effect: EffectClass) -> Self {
        Self {
            toolset: toolset.to_string(),
            tool: tool.to_string(),
            output: output.into(),
            effect,
            spill: true,
        }
    }

    fn without_spill(mut self) -> Self {
        self.spill = false;
        self
    }
}

#[async_trait]
impl Toolset for StaticToolset {
    fn name(&self) -> String {
        self.toolset.clone()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            r#type: "function".to_string(),
            name: self.tool.clone(),
            description: "Returns fixed output".to_string(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
        }]
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == self.tool
    }

    fn effect_of(&self, _fn_name: &str) -> EffectClass {
        self.effect
    }

    fn should_spill_output(&self, _fn_name: &str) -> bool {
        self.spill
    }

    async fn invoke(&self, _fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::Text(self.output.clone()))
    }
}

fn big_output(lines: usize) -> String {
    (0..lines)
        .map(|index| format!("row {index} with padded content for spilling\n"))
        .collect()
}

fn tool_end_counting(calls: Vec<ToolCall>, total_tokens: i32) -> StreamEnd {
    StreamEnd {
        captured_content: Some(MessageContent::from_tool_calls(calls)),
        captured_usage: Some(Usage {
            prompt_tokens: Some(total_tokens),
            total_tokens: Some(total_tokens),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn harness(session: &Session, policy: PolicyEngine, config: HarnessConfig) -> AgentHarness {
    AgentHarness {
        session: session.clone(),
        policy,
        config,
    }
}

fn yolo() -> PolicyEngine {
    PolicyEngine::new(ApprovalMode::Yolo)
}

fn harness_agent(
    streamer: Arc<dyn ChatStreamer>,
    tools: Vec<Box<dyn Toolset>>,
    system_prompt: &str,
    session: &Session,
    policy: PolicyEngine,
    config: HarnessConfig,
) -> MultiTurnAgent {
    let mut agent = MultiTurnAgent::with_streamer(
        streamer,
        "gpt-4o-mini",
        tools,
        system_prompt,
        MultiTurnAgentConfig::default(),
    );
    agent.set_harness(harness(session, policy, config));
    agent
}

fn any_rule(effect: Effect) -> PermissionRule {
    PermissionRule {
        action: ActionSelector::Any,
        resource: "*".to_string(),
        effect,
    }
}

fn session_events(session: &Session) -> Vec<SessionEvent> {
    session
        .events()
        .into_iter()
        .map(|logged| logged.event)
        .collect()
}

fn event_kind(event: &SessionEvent) -> &'static str {
    match event {
        SessionEvent::TurnStarted { .. } => "turn_started",
        SessionEvent::TurnEnded { .. } => "turn_ended",
        SessionEvent::StepStarted { .. } => "step_started",
        SessionEvent::StepEnded { .. } => "step_ended",
        SessionEvent::SystemMessage { .. } => "system_message",
        SessionEvent::UserMessage { .. } => "user_message",
        SessionEvent::AssistantMessage { .. } => "assistant_message",
        SessionEvent::AssistantAttempt { .. } => "assistant_attempt",
        SessionEvent::ToolCall { .. } => "tool_call",
        SessionEvent::ToolResult { .. } => "tool_result",
        SessionEvent::SpillApplied { .. } => "spill_applied",
        SessionEvent::CompactionStarted { .. } => "compaction_started",
        SessionEvent::CompactionSummary { .. } => "compaction_summary",
        SessionEvent::CompactionEnded { .. } => "compaction_ended",
        SessionEvent::RevertApplied { .. } => "revert_applied",
        SessionEvent::UsageUpdated { .. } => "usage_updated",
        SessionEvent::ApprovalRequested { .. } => "approval_requested",
        SessionEvent::ApprovalDecided { .. } => "approval_decided",
        SessionEvent::HookFired { .. } => "hook_fired",
        SessionEvent::CheckpointCreated { .. } => "checkpoint_created",
        SessionEvent::CircuitBreaker { .. } => "circuit_breaker",
        SessionEvent::Error { .. } => "error",
    }
}

fn tool_call(call_id: &str, text: &str) -> ToolCall {
    ToolCall {
        call_id: call_id.to_string(),
        fn_name: "echo-say".to_string(),
        fn_arguments: serde_json::json!({ "text": text }),
        thought_signatures: None,
    }
}

struct SlowToolset;

#[async_trait]
impl Toolset for SlowToolset {
    fn name(&self) -> String {
        "slow".to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            r#type: "function".to_string(),
            name: "slow-run".to_string(),
            description: "Sleeps past any reasonable timeout".to_string(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
        }]
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == "slow-run"
    }

    fn effect_of(&self, _fn_name: &str) -> EffectClass {
        EffectClass::Execute
    }

    async fn invoke(&self, _fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(ToolOutput::Text("finished".to_string()))
    }
}

struct DiffToolset;

#[async_trait]
impl Toolset for DiffToolset {
    fn name(&self) -> String {
        "diff".to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            r#type: "function".to_string(),
            name: "diff-write".to_string(),
            description: "Writes a file and reports a unified diff".to_string(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
        }]
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == "diff-write"
    }

    fn effect_of(&self, _fn_name: &str) -> EffectClass {
        EffectClass::Write
    }

    async fn invoke(&self, fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
        Ok(self.invoke_structured(fn_name, args).await?.output)
    }

    async fn invoke_structured(&self, _fn_name: &str, _args: &str) -> Result<ToolRun, ToolError> {
        Ok(ToolRun {
            output: ToolOutput::Text("wrote a.rs".to_string()),
            observation: ToolObservation::success("wrote a.rs"),
            file_changes: vec![FileDiff {
                path: "a.rs".to_string(),
                kind: FileChangeKind::Modified,
                unified_diff: "@@ -1 +1 @@\n-old\n+new".to_string(),
            }],
        })
    }
}

const LEAKED_TOKEN: &str = "sk-live-abcdefghijklmnopqrstuvwxyz012345";

struct LeakToolset;

#[async_trait]
impl Toolset for LeakToolset {
    fn name(&self) -> String {
        "leak".to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            r#type: "function".to_string(),
            name: "leak-echo".to_string(),
            description: "Returns text that contains a secret".to_string(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
        }]
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == "leak-echo"
    }

    async fn invoke(&self, _fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::Text(format!("OPENAI_API_KEY={LEAKED_TOKEN}")))
    }
}

struct FailingToolset;

#[async_trait]
impl Toolset for FailingToolset {
    fn name(&self) -> String {
        "failing".to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            r#type: "function".to_string(),
            name: "failing-run".to_string(),
            description: "Always fails with a secret in the error message".to_string(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
        }]
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == "failing-run"
    }

    async fn invoke(&self, _fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
        Err(ToolError::Unknown(anyhow!(
            "provider rejected key {LEAKED_TOKEN}"
        )))
    }
}

struct LeakDiffToolset;

#[async_trait]
impl Toolset for LeakDiffToolset {
    fn name(&self) -> String {
        "leak_diff".to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            r#type: "function".to_string(),
            name: "leak-diff".to_string(),
            description: "Reports a diff that contains a secret".to_string(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
        }]
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == "leak-diff"
    }

    fn effect_of(&self, _fn_name: &str) -> EffectClass {
        EffectClass::Write
    }

    async fn invoke(&self, fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
        Ok(self.invoke_structured(fn_name, args).await?.output)
    }

    async fn invoke_structured(&self, _fn_name: &str, _args: &str) -> Result<ToolRun, ToolError> {
        Ok(ToolRun {
            output: ToolOutput::Text("wrote a.rs".to_string()),
            observation: ToolObservation::success("wrote a.rs"),
            file_changes: vec![FileDiff {
                path: "a.rs".to_string(),
                kind: FileChangeKind::Modified,
                unified_diff: format!("+token={LEAKED_TOKEN}"),
            }],
        })
    }
}

struct GatedStreamer {
    turns: Mutex<VecDeque<Vec<ChatStreamEvent>>>,
    requests: Mutex<Vec<ChatRequest>>,
    gate_index: usize,
    started: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl ChatStreamer for GatedStreamer {
    async fn stream_chat(
        &self,
        _model: &str,
        request: ChatRequest,
        _options: &ChatOptions,
    ) -> Result<BoxedChatStream> {
        let index = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request);
            requests.len() - 1
        };
        if index == self.gate_index {
            self.started.notify_one();
            self.release.notified().await;
        }
        let events = self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra LLM turn");
        Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
    }
}

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
async fn queues_steering_as_next_prompt() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![
            ChatStreamEvent::Chunk(StreamChunk {
                content: "one".to_string(),
            }),
            ChatStreamEvent::End(text_end("one")),
        ],
        vec![
            ChatStreamEvent::Chunk(StreamChunk {
                content: "two".to_string(),
            }),
            ChatStreamEvent::End(text_end("two")),
        ],
    ]));

    let agent = MultiTurnAgent::with_streamer(
        streamer.clone(),
        "gpt-4o-mini",
        vec![],
        "",
        MultiTurnAgentConfig::default(),
    );

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt("first".to_string()))
        .unwrap();
    cmd_tx
        .send(AgentCommand::Steer("second".to_string()))
        .unwrap();

    let mut completions = 0;
    while let Some(ev) = rx.recv().await {
        if matches!(ev, AgentEvent::TurnCompleted { .. }) {
            completions += 1;
            if completions == 2 {
                break;
            }
        }
    }
    assert_eq!(completions, 2);

    let reqs = streamer.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    assert!(format!("{:?}", reqs[1].messages).contains("second"));

    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn harness_dual_writes_session_events() {
    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        tool_end(vec![tool_call("c1", "stop")]),
    )]]));
    let session = memory_session();
    let mut agent = MultiTurnAgent::with_streamer(
        streamer,
        "gemini::gemini-3.8-flash",
        vec![Box::new(EchoToolset)],
        "Be precise.",
        MultiTurnAgentConfig::default(),
    );
    agent.set_harness(harness(&session, yolo(), HarnessConfig::default()));

    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("do it", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "done now");

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
            assert_eq!(sections[0].content, "Be precise.");
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
    agent.set_harness(harness(&session, yolo(), HarnessConfig::default()));

    let (tx, _rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("read it", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "final");

    let reqs = streamer.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[1].messages.len(), 3);
    assert_eq!(reqs[1].system.as_deref(), Some("Be precise."));
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
async fn steering_is_applied_at_the_step_boundary() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let streamer = Arc::new(GatedStreamer {
        turns: Mutex::new(
            vec![
                vec![
                    ChatStreamEvent::Chunk(StreamChunk {
                        content: "one".to_string(),
                    }),
                    ChatStreamEvent::End(text_end("one")),
                ],
                vec![
                    ChatStreamEvent::Chunk(StreamChunk {
                        content: "two".to_string(),
                    }),
                    ChatStreamEvent::End(text_end("two")),
                ],
            ]
            .into(),
        ),
        requests: Mutex::new(Vec::new()),
        gate_index: 0,
        started: started.clone(),
        release: release.clone(),
    });

    let session = memory_session();
    let mut agent = MultiTurnAgent::with_streamer(
        streamer.clone(),
        "gpt-4o-mini",
        vec![],
        "",
        MultiTurnAgentConfig::default(),
    );
    agent.set_harness(harness(&session, yolo(), HarnessConfig::default()));

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt("first".to_string()))
        .unwrap();
    started.notified().await;
    cmd_tx
        .send(AgentCommand::Steer("also add tests".to_string()))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(25)).await;
    release.notify_one();

    loop {
        match rx.recv().await {
            Some(AgentEvent::TurnCompleted { .. }) => break,
            Some(_) => {}
            None => panic!("worker stopped mid-turn"),
        }
    }

    let reqs = streamer.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    assert!(format!("{:?}", reqs[1].messages).contains("also add tests"));

    let steered = session_events(&session).iter().any(|event| {
        matches!(
            event,
            SessionEvent::UserMessage { source: InputSource::Steering, content }
                if content == &vec![ContentBlock::Text("also add tests".to_string())]
        )
    });
    assert!(steered);

    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn apply_commands_arrive_at_the_next_step_boundary_without_cancelling() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let streamer = Arc::new(GatedStreamer {
        turns: Mutex::new(
            vec![
                vec![ChatStreamEvent::End(tool_end(vec![tool_call("c1", "ok")]))],
                vec![ChatStreamEvent::End(text_end("done"))],
            ]
            .into(),
        ),
        requests: Mutex::new(Vec::new()),
        gate_index: 0,
        started: started.clone(),
        release: release.clone(),
    });

    let session = memory_session();
    let agent = harness_agent(
        streamer.clone(),
        vec![Box::new(EchoToolset)],
        "",
        &session,
        yolo(),
        HarnessConfig::default(),
    );

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt("first".to_string()))
        .unwrap();
    started.notified().await;
    cmd_tx
        .send(AgentCommand::ApplySkill {
            name: "tester".to_string(),
            instructions: "Add tests.".to_string(),
        })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(25)).await;
    release.notify_one();

    let reason = loop {
        match rx.recv().await {
            Some(AgentEvent::TurnCompleted { reason, .. }) => break reason,
            Some(_) => {}
            None => panic!("worker stopped mid-turn"),
        }
    };
    assert_eq!(reason, TurnEndReason::Completed);

    let reqs = streamer.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    assert!(
        !format!("{:?}", reqs[0].messages).contains("Skill instructions (tester):"),
        "the in-flight request must not see the skill"
    );
    assert!(format!("{:?}", reqs[1].messages).contains("Skill instructions (tester):"));
    assert!(format!("{:?}", reqs[1].messages).contains("Add tests."));

    let applied = session_events(&session)
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                SessionEvent::UserMessage { source: InputSource::System, content }
                    if content
                        == &vec![ContentBlock::Text(
                            "Skill instructions (tester):\n\nAdd tests.".to_string()
                        )]
            )
        })
        .count();
    assert_eq!(applied, 1);

    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn idle_apply_agent_logs_instruction_and_switches_model() {
    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("done"),
    )]]));

    let session = memory_session();
    let agent = harness_agent(
        streamer.clone(),
        vec![],
        "",
        &session,
        yolo(),
        HarnessConfig::default(),
    );

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::ApplyAgent {
            name: "reviewer".to_string(),
            instructions: "Review every change.".to_string(),
            model: Some("gpt-5-mini".to_string()),
        })
        .unwrap();
    cmd_tx
        .send(AgentCommand::SubmitPrompt("hello".to_string()))
        .unwrap();

    loop {
        match rx.recv().await {
            Some(AgentEvent::TurnCompleted { .. }) => break,
            Some(_) => {}
            None => panic!("worker stopped mid-turn"),
        }
    }

    let logged = session_events(&session);
    let applied = logged
        .iter()
        .filter(|event| {
            matches!(
                event,
                SessionEvent::UserMessage { source: InputSource::System, content }
                    if content
                        == &vec![ContentBlock::Text(
                            "Agent instructions (reviewer):\n\nReview every change.".to_string()
                        )]
            )
        })
        .count();
    assert_eq!(applied, 1);

    let models = streamer.models.lock().unwrap().clone();
    assert_eq!(models, vec!["gpt-5-mini".to_string()]);
    let reqs = streamer.requests.lock().unwrap();
    assert!(format!("{:?}", reqs[0].messages).contains("Agent instructions (reviewer):"));
    assert!(format!("{:?}", reqs[0].messages).contains("Review every change."));

    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn idle_apply_skill_logs_instruction_without_changing_model() {
    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("done"),
    )]]));

    let session = memory_session();
    let agent = harness_agent(
        streamer.clone(),
        vec![],
        "",
        &session,
        yolo(),
        HarnessConfig::default(),
    );

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::ApplySkill {
            name: "tester".to_string(),
            instructions: "Add tests.".to_string(),
        })
        .unwrap();
    cmd_tx
        .send(AgentCommand::SubmitPrompt("hello".to_string()))
        .unwrap();

    loop {
        match rx.recv().await {
            Some(AgentEvent::TurnCompleted { .. }) => break,
            Some(_) => {}
            None => panic!("worker stopped mid-turn"),
        }
    }

    let logged = session_events(&session);
    let applied = logged
        .iter()
        .filter(|event| {
            matches!(
                event,
                SessionEvent::UserMessage { source: InputSource::System, content }
                    if content
                        == &vec![ContentBlock::Text(
                            "Skill instructions (tester):\n\nAdd tests.".to_string()
                        )]
            )
        })
        .count();
    assert_eq!(applied, 1);

    let models = streamer.models.lock().unwrap().clone();
    assert_eq!(models, vec!["gpt-4o-mini".to_string()]);
    let reqs = streamer.requests.lock().unwrap();
    assert!(format!("{:?}", reqs[0].messages).contains("Skill instructions (tester):"));
    assert!(format!("{:?}", reqs[0].messages).contains("Add tests."));

    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn apply_agent_without_harness_updates_history_and_model() {
    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("done"),
    )]]));

    let agent = MultiTurnAgent::with_streamer(
        streamer.clone(),
        "gpt-4o-mini",
        vec![],
        "",
        MultiTurnAgentConfig::default(),
    );

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::ApplyAgent {
            name: "reviewer".to_string(),
            instructions: "Review every change.".to_string(),
            model: Some("gpt-5-mini".to_string()),
        })
        .unwrap();
    cmd_tx
        .send(AgentCommand::SubmitPrompt("hello".to_string()))
        .unwrap();

    loop {
        match rx.recv().await {
            Some(AgentEvent::TurnCompleted { .. }) => break,
            Some(_) => {}
            None => panic!("worker stopped mid-turn"),
        }
    }

    let models = streamer.models.lock().unwrap().clone();
    assert_eq!(models, vec!["gpt-5-mini".to_string()]);
    let reqs = streamer.requests.lock().unwrap();
    assert!(format!("{:?}", reqs[0].messages).contains("Agent instructions (reviewer):"));
    assert!(format!("{:?}", reqs[0].messages).contains("Review every change."));

    let _ = cmd_tx.send(AgentCommand::Quit);
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
    agent.set_harness(harness(&session, yolo(), config));

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
    agent.set_harness(harness(&session, yolo(), HarnessConfig::default()));

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
        yolo(),
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
    assert!(completed.contains("Full result stored at:"));

    let events = session_events(&session);
    let (bytes_omitted, blob_path) = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::SpillApplied {
                bytes_omitted,
                blob_path,
                ..
            } => Some((*bytes_omitted, blob_path.clone())),
            _ => None,
        })
        .expect("spill applied");
    assert!(bytes_omitted > 0);
    assert!(blob_path.exists());
    assert!(
        blob_path.starts_with(root.join(session.id().as_str()).join("blobs")),
        "blob written under the session dir"
    );

    let observation = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ToolResult { output, .. } => Some(output.clone()),
            _ => None,
        })
        .expect("tool result logged");
    assert!(observation.stdout.starts_with("row 0 with"));
    assert!(observation.stdout.contains("Full result stored at:"));

    let reqs = streamer.requests.lock().unwrap();
    assert!(format!("{:?}", reqs[1].messages).contains("Full result stored at:"));

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
    let mut agent = harness_agent(streamer, vec![Box::new(tool)], "", &session, yolo(), config);

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
    let mut agent = harness_agent(streamer.clone(), vec![], "system", &session, yolo(), config);

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
        include_system_messages: false,
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
            .system
            .as_deref()
            .is_some_and(|system| system.contains("Goal"))
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
    let mut agent = harness_agent(streamer.clone(), vec![], "system", &session, yolo(), config);

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
        yolo(),
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
    cmd_tx.send(AgentCommand::Compact).unwrap();
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
    let mut agent = harness_agent(streamer.clone(), vec![], "", &session, yolo(), config);

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
    let agent = harness_agent(streamer.clone(), vec![], "", &session, yolo(), config);

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
async fn final_step_strips_tools_and_requests_summary() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![tool_call("c1", "ok")]))],
        vec![ChatStreamEvent::End(text_end("wrapped up"))],
    ]));

    let session = memory_session();
    let config = HarnessConfig {
        max_steps_per_turn: 2,
        ..HarnessConfig::default()
    };
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(EchoToolset)],
        "",
        &session,
        yolo(),
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
        yolo(),
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
        yolo(),
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
    ]));

    let session = memory_session();
    let agent = harness_agent(
        streamer.clone(),
        vec![Box::new(EchoToolset)],
        "",
        &session,
        yolo(),
        HarnessConfig::default(),
    );

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt("go".to_string()))
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
    assert_eq!(code, Some(ErrorCode::LoopDetected));
    assert_eq!(failure.as_deref(), Some(LOOP_DETECTED));

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
        } if error == LOOP_DETECTED
    )));

    let reqs = streamer.requests.lock().unwrap();
    assert_eq!(reqs.len(), 4);
    let next_request = format!("{:?}", reqs[3].messages);
    assert!(
        next_request.contains("failed identically 3 times"),
        "the breaker diagnostic must reach the next request: {next_request}"
    );

    let _ = cmd_tx.send(AgentCommand::Quit);
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

struct BarrierToolset {
    barrier: Arc<Barrier>,
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

struct StopBatchToolset {
    effect: EffectClass,
    calls: Arc<AtomicUsize>,
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

struct OrderToolset {
    order: Arc<Mutex<Vec<String>>>,
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

fn bare_call(call_id: &str, fn_name: &str) -> ToolCall {
    ToolCall {
        call_id: call_id.to_string(),
        fn_name: fn_name.to_string(),
        fn_arguments: serde_json::json!({}),
        thought_signatures: None,
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
        yolo(),
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
        yolo(),
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
        yolo(),
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
        yolo(),
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
    let mut agent = harness_agent(streamer.clone(), vec![], "", &session, yolo(), config);

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
    let policy = PolicyEngine::new(ApprovalMode::AutoEdit);
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
    let agent = harness_agent(streamer.clone(), vec![], "", &session, yolo(), config);

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
    cmd_tx.send(AgentCommand::Compact).unwrap();
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
        yolo(),
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
        yolo(),
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
        yolo(),
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

fn path_call(call_id: &str, path: &str) -> ToolCall {
    ToolCall {
        call_id: call_id.to_string(),
        fn_name: "echo-say".to_string(),
        fn_arguments: serde_json::json!({ "path": path }),
        thought_signatures: None,
    }
}

async fn scoped_workspace(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("meerai-{tag}-{}", SessionId::new()));
    tokio::fs::create_dir_all(root.join("src")).await.unwrap();
    tokio::fs::write(root.join("src/AGENTS.md"), "SCOPED RULE: keep tests green")
        .await
        .unwrap();
    root
}

fn runtime_injections(events: &[SessionEvent]) -> Vec<(usize, String)> {
    events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| match event {
            SessionEvent::UserMessage {
                source: InputSource::RuntimeContext,
                content,
            } => content.iter().find_map(|block| match block {
                ContentBlock::Text(text) if text.contains("Project instructions from") => {
                    Some((index, text.clone()))
                }
                _ => None,
            }),
            _ => None,
        })
        .collect()
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
    agent.set_harness(harness(&session, yolo(), HarnessConfig::default()));

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
    agent.set_harness(harness(&session, yolo(), HarnessConfig::default()));

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
    agent.set_harness(harness(&session, yolo(), HarnessConfig::default()));

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
    agent.set_harness(harness(&session, yolo(), HarnessConfig::default()));

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
        yolo(),
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
    let mut agent = harness_agent(streamer.clone(), vec![], "system", &session, yolo(), config);

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
async fn redacts_tool_output_in_session_log_and_items() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "c1".to_string(),
            fn_name: "leak-echo".to_string(),
            fn_arguments: serde_json::json!({}),
            thought_signatures: None,
        }]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(LeakToolset)],
        "",
        &session,
        yolo(),
        HarnessConfig::default(),
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();

    let logged = serde_json::to_string(&session_events(&session)).unwrap();
    assert!(logged.contains("[REDACTED:openai_key]"));
    assert!(!logged.contains(LEAKED_TOKEN));

    let mut outputs = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::ItemCompleted {
            item: SessionItem::ToolCall { output, .. },
        } = event
            && let Some(output) = output
        {
            outputs.push(output);
        }
    }
    assert_eq!(outputs.len(), 1);
    assert!(outputs[0].contains("[REDACTED:openai_key]"));
    assert!(!outputs[0].contains(LEAKED_TOKEN));

    let reqs = streamer.requests.lock().unwrap();
    let model_view = format!("{:?}", reqs[1].messages);
    assert!(model_view.contains("[REDACTED:openai_key]"));
    assert!(!model_view.contains(LEAKED_TOKEN));
}

#[tokio::test]
async fn redacts_file_change_diff_text() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "c1".to_string(),
            fn_name: "leak-diff".to_string(),
            fn_arguments: serde_json::json!({}),
            thought_signatures: None,
        }]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer,
        vec![Box::new(LeakDiffToolset)],
        "",
        &session,
        yolo(),
        HarnessConfig::default(),
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("write", &tx, CancellationToken::new())
        .await
        .unwrap();

    let mut changes = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::ItemCompleted {
            item: SessionItem::FileChange { changes: diffs, .. },
        } = event
        {
            changes.extend(diffs);
        }
    }
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, "a.rs");
    assert!(changes[0].unified_diff.contains("[REDACTED:openai_key]"));
    assert!(!changes[0].unified_diff.contains(LEAKED_TOKEN));
}

#[tokio::test]
async fn redacts_failed_tool_dispatch_errors() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "c1".to_string(),
            fn_name: "failing-run".to_string(),
            fn_arguments: serde_json::json!({}),
            thought_signatures: None,
        }]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer.clone(),
        vec![Box::new(FailingToolset)],
        "",
        &session,
        yolo(),
        HarnessConfig::default(),
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();

    let logged = serde_json::to_string(&session_events(&session)).unwrap();
    assert!(logged.contains("[REDACTED:openai_key]"));
    assert!(!logged.contains(LEAKED_TOKEN));

    let mut outputs = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::ItemCompleted {
            item: SessionItem::ToolCall { output, .. },
        } = event
            && let Some(output) = output
        {
            outputs.push(output);
        }
    }
    assert_eq!(outputs.len(), 1);
    assert!(outputs[0].contains("[REDACTED:openai_key]"));
    assert!(!outputs[0].contains(LEAKED_TOKEN));

    let reqs = streamer.requests.lock().unwrap();
    let model_view = format!("{:?}", reqs[1].messages);
    assert!(model_view.contains("[REDACTED:openai_key]"));
    assert!(!model_view.contains(LEAKED_TOKEN));
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
        yolo(),
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
