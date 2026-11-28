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

pub(super) enum ScriptedStream {
    Events(Vec<ChatStreamEvent>),
    EventsThenError(Vec<ChatStreamEvent>, String),
    Error(String),
    Stall,
}

pub(super) struct FakeStreamer {
    pub(super) turns: Mutex<VecDeque<ScriptedStream>>,
    pub(super) requests: Mutex<Vec<ChatRequest>>,
    pub(super) options: Mutex<Vec<ChatOptions>>,
    pub(super) models: Mutex<Vec<String>>,
}

impl FakeStreamer {
    pub(super) fn new(turns: Vec<Vec<ChatStreamEvent>>) -> Self {
        Self::with_script(turns.into_iter().map(ScriptedStream::Events).collect())
    }

    pub(super) fn with_script(turns: Vec<ScriptedStream>) -> Self {
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
        _provider: Option<&str>,
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
            ScriptedStream::EventsThenError(events, message) => {
                let failure = anyhow!(message);
                Ok(Box::pin(
                    stream::iter(events.into_iter().map(Ok))
                        .chain(stream::once(async move { Err(failure) })),
                ))
            }
            ScriptedStream::Error(message) => Err(anyhow!(message)),
            ScriptedStream::Stall => Ok(Box::pin(
                stream::once(async { Ok(ChatStreamEvent::Start) })
                    .chain(stream::pending::<Result<ChatStreamEvent>>()),
            )),
        }
    }
}

pub(super) struct EchoToolset;

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

pub(super) fn text_end(text: &str) -> StreamEnd {
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

pub(super) fn tool_end(calls: Vec<ToolCall>) -> StreamEnd {
    StreamEnd {
        captured_content: Some(MessageContent::from_tool_calls(calls)),
        ..Default::default()
    }
}

pub(super) fn memory_session() -> Session {
    Session::memory(SessionId::new(), PathBuf::from("/tmp/workspace"))
}

pub(super) async fn persistent_session(root: &Path) -> Session {
    let config = SessionConfig {
        root: root.to_path_buf(),
        workspace: root.to_path_buf(),
    };
    Session::open_or_create(&config, SessionId::new(), PathBuf::from("/tmp/workspace"))
        .await
        .unwrap()
}

pub(super) struct StaticToolset {
    pub(super) toolset: String,
    pub(super) tool: String,
    pub(super) output: String,
    pub(super) effect: EffectClass,
    pub(super) spill: bool,
}

impl StaticToolset {
    pub(super) fn new(
        toolset: &str,
        tool: &str,
        output: impl Into<String>,
        effect: EffectClass,
    ) -> Self {
        Self {
            toolset: toolset.to_string(),
            tool: tool.to_string(),
            output: output.into(),
            effect,
            spill: true,
        }
    }

    pub(super) fn without_spill(mut self) -> Self {
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

pub(super) struct SandboxEscapeToolset {
    pub(super) calls: Mutex<Vec<String>>,
}

impl Default for SandboxEscapeToolset {
    fn default() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl Toolset for SandboxEscapeToolset {
    fn name(&self) -> String {
        "code".to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            r#type: "function".to_string(),
            name: "code-shell".to_string(),
            description: "Runs a shell command".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "command": { "type": "string" } }
            }),
        }]
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == "code-shell"
    }

    fn effect_of(&self, _fn_name: &str) -> EffectClass {
        EffectClass::External
    }

    async fn invoke(&self, _fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::Text("escaped".to_string()))
    }

    async fn invoke_structured(&self, _fn_name: &str, args: &str) -> Result<ToolRun, ToolError> {
        self.calls.lock().unwrap().push(args.to_string());
        if args.contains("\"unsandboxed\":true") {
            return Ok(ToolRun::from_output(ToolOutput::Text(
                "escaped".to_string(),
            )));
        }
        let mut run = ToolRun::from_output(ToolOutput::Text(
            "Flutter failed to open a file at \"/Users/u/flutter/bin/cache/lockfile\".\n\
             Please ensure that the SDK is installed in a location that has read/write \
             permissions for the current user."
                .to_string(),
        ));
        run.observation.hint = Some(meerai_core::SANDBOX_DENIED_HINT.to_string());
        Ok(run)
    }
}

pub(super) struct SandboxEscapeTool {
    pub(super) inner: Arc<SandboxEscapeToolset>,
}

#[async_trait]
impl Toolset for SandboxEscapeTool {
    fn name(&self) -> String {
        self.inner.name()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        self.inner.definition()
    }

    fn contain(&self, fn_name: &str) -> bool {
        self.inner.contain(fn_name)
    }

    fn effect_of(&self, fn_name: &str) -> EffectClass {
        self.inner.effect_of(fn_name)
    }

    async fn invoke(&self, fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
        self.inner.invoke(fn_name, args).await
    }

    async fn invoke_structured(&self, fn_name: &str, args: &str) -> Result<ToolRun, ToolError> {
        self.inner.invoke_structured(fn_name, args).await
    }
}

pub(super) fn big_output(lines: usize) -> String {
    (0..lines)
        .map(|index| format!("row {index} with padded content for spilling\n"))
        .collect()
}

pub(super) struct LargeSchemaToolset;

#[async_trait]
impl Toolset for LargeSchemaToolset {
    fn name(&self) -> String {
        "large".to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            r#type: "function".to_string(),
            name: "large-run".to_string(),
            description: "x".repeat(100_000),
            parameters: serde_json::json!({ "type": "object" }),
        }]
    }

    fn contain(&self, _fn_name: &str) -> bool {
        false
    }

    async fn invoke(&self, _fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::Text("never".to_string()))
    }
}

pub(super) struct NoteWriteToolset {
    pub(super) root: PathBuf,
}

#[async_trait]
impl Toolset for NoteWriteToolset {
    fn name(&self) -> String {
        "note".to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            r#type: "function".to_string(),
            name: "note-write".to_string(),
            description: "Writes the note file".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } }
            }),
        }]
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == "note-write"
    }

    fn effect_of(&self, _fn_name: &str) -> EffectClass {
        EffectClass::Write
    }

    async fn invoke(&self, _fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
        let args: serde_json::Value = serde_json::from_str(args)?;
        let path = args["path"].as_str().unwrap_or("note.txt");
        std::fs::write(self.root.join(path), "changed")
            .map_err(|error| ToolError::Unknown(anyhow::Error::from(error)))?;
        Ok(ToolOutput::Text("written".to_string()))
    }
}

pub(super) struct CoerceToolset {
    pub(super) seen: Arc<Mutex<serde_json::Value>>,
}

#[async_trait]
impl Toolset for CoerceToolset {
    fn name(&self) -> String {
        "coerce".to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            r#type: "function".to_string(),
            name: "coerce-run".to_string(),
            description: "Records its arguments".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "count": { "type": "integer" } }
            }),
        }]
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == "coerce-run"
    }

    async fn invoke(&self, _fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
        let value: serde_json::Value = serde_json::from_str(args)?;
        *self.seen.lock().unwrap() = value;
        Ok(ToolOutput::Text("ok".to_string()))
    }
}

pub(super) struct InvalidArgsToolset;

#[async_trait]
impl Toolset for InvalidArgsToolset {
    fn name(&self) -> String {
        "invalid".to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            r#type: "function".to_string(),
            name: "invalid-run".to_string(),
            description: "Always rejects its arguments".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "count": { "type": "integer" } }
            }),
        }]
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == "invalid-run"
    }

    async fn invoke(&self, _fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
        Err(ToolError::WrongArguments(
            serde_json::from_str::<serde_json::Value>("{").unwrap_err(),
        ))
    }
}

pub(super) fn tool_end_counting(calls: Vec<ToolCall>, total_tokens: i32) -> StreamEnd {
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

pub(super) fn harness(
    session: &Session,
    policy: PolicyEngine,
    config: HarnessConfig,
) -> AgentHarness {
    AgentHarness {
        session: session.clone(),
        policy,
        config,
        routing: None,
    }
}

pub(super) fn auto_mode() -> PolicyEngine {
    PolicyEngine::new(ApprovalMode::Auto)
}

pub(super) fn harness_agent(
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

pub(super) fn any_rule(effect: Effect) -> PermissionRule {
    PermissionRule {
        action: ActionSelector::Any,
        resource: "*".to_string(),
        effect,
    }
}

pub(super) fn session_events(session: &Session) -> Vec<SessionEvent> {
    session
        .events()
        .into_iter()
        .map(|logged| logged.event)
        .collect()
}

pub(super) fn event_kind(event: &SessionEvent) -> &'static str {
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
        SessionEvent::UsageUpdated { .. } => "usage_updated",
        SessionEvent::ApprovalRequested { .. } => "approval_requested",
        SessionEvent::ApprovalDecided { .. } => "approval_decided",
        SessionEvent::QuestionAsked { .. } => "question_asked",
        SessionEvent::QuestionAnswered { .. } => "question_answered",
        SessionEvent::RewindApplied { .. } => "rewind_applied",
        SessionEvent::HookFired { .. } => "hook_fired",
        SessionEvent::ContextTurnPinned { .. } => "context_turn_pinned",
        SessionEvent::LoopUpdated { .. } => "loop_updated",
        SessionEvent::CheckpointCreated { .. } => "checkpoint_created",
        SessionEvent::CircuitBreaker { .. } => "circuit_breaker",
        SessionEvent::Error { .. } => "error",
    }
}

pub(super) fn tool_call(call_id: &str, text: &str) -> ToolCall {
    ToolCall {
        call_id: call_id.to_string(),
        fn_name: "echo-say".to_string(),
        fn_arguments: serde_json::json!({ "text": text }),
        thought_signatures: None,
    }
}

pub(super) struct SlowToolset;

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

pub(super) struct DiffToolset;

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

pub(super) const LEAKED_TOKEN: &str = "sk-live-abcdefghijklmnopqrstuvwxyz012345";

pub(super) struct LeakToolset;

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

pub(super) struct FailingToolset;

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

pub(super) struct LeakDiffToolset;

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

pub(super) struct GatedStreamer {
    pub(super) turns: Mutex<VecDeque<Vec<ChatStreamEvent>>>,
    pub(super) requests: Mutex<Vec<ChatRequest>>,
    pub(super) gate_index: usize,
    pub(super) started: Arc<Notify>,
    pub(super) release: Arc<Notify>,
}

#[async_trait]
impl ChatStreamer for GatedStreamer {
    async fn stream_chat(
        &self,
        _provider: Option<&str>,
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

pub(super) struct BarrierToolset {
    pub(super) barrier: Arc<Barrier>,
}
pub(super) struct StopBatchToolset {
    pub(super) effect: EffectClass,
    pub(super) calls: Arc<AtomicUsize>,
}
pub(super) struct OrderToolset {
    pub(super) order: Arc<Mutex<Vec<String>>>,
}
pub(super) fn bare_call(call_id: &str, fn_name: &str) -> ToolCall {
    ToolCall {
        call_id: call_id.to_string(),
        fn_name: fn_name.to_string(),
        fn_arguments: serde_json::json!({}),
        thought_signatures: None,
    }
}
pub(super) fn path_call(call_id: &str, path: &str) -> ToolCall {
    ToolCall {
        call_id: call_id.to_string(),
        fn_name: "echo-say".to_string(),
        fn_arguments: serde_json::json!({ "path": path }),
        thought_signatures: None,
    }
}
pub(super) fn runtime_injections(events: &[SessionEvent]) -> Vec<(usize, String)> {
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
#[cfg(unix)]
pub(super) fn hook_entry(event: HookEvent, command: &str) -> meerai_common::hooks::HookEntry {
    meerai_common::hooks::HookEntry {
        event,
        matcher: None,
        if_pattern: None,
        command: command.to_string(),
        baseline_file: None,
        timeout_ms: None,
        fail_closed: None,
    }
}
