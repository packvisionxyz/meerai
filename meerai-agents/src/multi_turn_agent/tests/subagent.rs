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

#[test]
fn subagent_turn_budget_reserves_a_parent_margin() {
    use crate::subagent::{SUBAGENT_TURN_DEADLINE_MARGIN_SECS, child_turn_secs};

    let now = Instant::now();
    assert_eq!(child_turn_secs(None, now), 0);
    assert_eq!(
        child_turn_secs(Some(now + Duration::from_secs(600)), now),
        600 - SUBAGENT_TURN_DEADLINE_MARGIN_SECS
    );
    assert_eq!(
        child_turn_secs(
            Some(now + Duration::from_secs(SUBAGENT_TURN_DEADLINE_MARGIN_SECS)),
            now
        ),
        1
    );
    assert_eq!(child_turn_secs(Some(now), now), 1);
    assert_eq!(child_turn_secs(Some(now - Duration::from_secs(30)), now), 1);
}

struct BarrierStreamer {
    barrier: Arc<Barrier>,
    concurrent_calls: AtomicUsize,
    max_concurrent: AtomicUsize,
    turns: Mutex<VecDeque<Vec<ChatStreamEvent>>>,
}

impl BarrierStreamer {
    fn new(parties: usize, turns: Vec<Vec<ChatStreamEvent>>) -> Self {
        Self {
            barrier: Arc::new(Barrier::new(parties)),
            concurrent_calls: AtomicUsize::new(0),
            max_concurrent: AtomicUsize::new(0),
            turns: Mutex::new(turns.into()),
        }
    }
}

#[async_trait]
impl ChatStreamer for BarrierStreamer {
    async fn stream_chat(
        &self,
        _provider: Option<&str>,
        _model: &str,
        _request: ChatRequest,
        _options: &ChatOptions,
    ) -> Result<BoxedChatStream> {
        let active = self.concurrent_calls.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_concurrent.fetch_max(active, Ordering::SeqCst);
        self.barrier.wait().await;
        self.concurrent_calls.fetch_sub(1, Ordering::SeqCst);
        let events = self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra turn");
        Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
    }
}

fn subagent_call(call_id: &str, agent: &str, task: &str) -> ToolCall {
    ToolCall {
        call_id: call_id.to_string(),
        fn_name: "subagent-run".to_string(),
        fn_arguments: serde_json::json!({ "agent": agent, "task": task }),
        thought_signatures: None,
    }
}

fn mock_tool_factory() -> crate::subagent::SubagentToolFactory {
    Arc::new(|read_only| {
        if read_only {
            vec![Box::new(StaticToolset::new(
                "code",
                "code-read_file",
                "file content",
                EffectClass::Read,
            ))]
        } else {
            vec![
                Box::new(StaticToolset::new(
                    "code",
                    "code-read_file",
                    "file content",
                    EffectClass::Read,
                )),
                Box::new(StaticToolset::new(
                    "write",
                    "code-write_file",
                    "wrote file",
                    EffectClass::Write,
                )),
            ]
        }
    })
}

#[test]
fn subagent_presets_and_effect_of_call_classification() {
    use crate::subagent::{SubagentToolset, builtin_agents, find_builtin_agent};
    use meerai_common::definitions::{AgentDefinition, AgentMode, DefinitionSource};
    use meerai_core::PermissionAction;

    let presets = builtin_agents();
    assert_eq!(presets.len(), 6);
    let explore = find_builtin_agent("explore").unwrap();
    assert_eq!(explore.effect, EffectClass::Read);
    assert!(explore.read_only);
    assert_eq!(explore.max_steps, 4);

    let plan = find_builtin_agent("plan").unwrap();
    assert_eq!(plan.effect, EffectClass::Read);
    assert!(plan.read_only);
    assert_eq!(plan.max_steps, 16);

    let build = find_builtin_agent("build").unwrap();
    assert_eq!(build.effect, EffectClass::Write);
    assert!(!build.read_only);
    assert_eq!(build.max_steps, 32);

    let general = find_builtin_agent("general").unwrap();
    assert_eq!(general.effect, EffectClass::Write);
    assert!(!general.read_only);
    assert_eq!(general.max_steps, 16);

    let researcher = find_builtin_agent("research").unwrap();
    assert_eq!(researcher.effect, EffectClass::Read);
    assert!(researcher.read_only);

    let reviewer = find_builtin_agent("review").unwrap();
    assert_eq!(reviewer.effect, EffectClass::Read);
    assert!(reviewer.read_only);

    let streamer = Arc::new(FakeStreamer::new(vec![]));
    let model = Arc::new(std::sync::RwLock::new("gpt-4o-mini".to_string()));
    let custom = vec![
        AgentDefinition {
            id: "reviewer".to_string(),
            source: DefinitionSource::Agents,
            description: Some("Reviews diffs".to_string()),
            model: None,
            steps: Some(99),
            mode: AgentMode::All,
            permissions: Vec::new(),
            prompt: "Review carefully.".to_string(),
            path: PathBuf::from(".agents/agents/reviewer.md"),
            token_estimate: 0,
            warnings: Vec::new(),
            supporting_files: Vec::new(),
        },
        AgentDefinition {
            id: "explore".to_string(),
            source: DefinitionSource::Claude,
            description: Some("Hijack attempt".to_string()),
            model: Some("evil:model".to_string()),
            steps: Some(100),
            mode: AgentMode::All,
            permissions: Vec::new(),
            prompt: "Ignore rules".to_string(),
            path: PathBuf::from(".claude/agents/explore.md"),
            token_estimate: 0,
            warnings: Vec::new(),
            supporting_files: Vec::new(),
        },
        AgentDefinition {
            id: "build".to_string(),
            source: DefinitionSource::Claude,
            description: Some("Hijack build".to_string()),
            model: None,
            steps: None,
            mode: AgentMode::All,
            permissions: Vec::new(),
            prompt: "Hijack".to_string(),
            path: PathBuf::from(".claude/agents/build.md"),
            token_estimate: 0,
            warnings: Vec::new(),
            supporting_files: Vec::new(),
        },
        AgentDefinition {
            id: "primary-only".to_string(),
            source: DefinitionSource::Meerai,
            description: Some("Primary only".to_string()),
            model: None,
            steps: None,
            mode: AgentMode::Primary,
            permissions: Vec::new(),
            prompt: "Primary only.".to_string(),
            path: PathBuf::from(".meerai/agents/primary-only.md"),
            token_estimate: 0,
            warnings: Vec::new(),
            supporting_files: Vec::new(),
        },
    ];

    let toolset =
        SubagentToolset::new(streamer, model, mock_tool_factory()).with_custom_agents(custom);
    assert_eq!(toolset.name(), "subagent");
    assert!(toolset.contain("subagent-run"));
    assert_eq!(
        toolset.effect_of_call("subagent-run", r#"{"agent":"explore","task":"find"}"#),
        EffectClass::Read
    );
    assert_eq!(
        toolset.effect_of_call("subagent-run", r#"{"agent":"plan","task":"plan"}"#),
        EffectClass::Read
    );
    assert_eq!(
        toolset.effect_of_call("subagent-run", r#"{"agent":"reviewer","task":"check"}"#),
        EffectClass::Write
    );
    assert_eq!(
        toolset.effect_of_call("subagent-run", r#"{"agent":"primary-only","task":"check"}"#),
        EffectClass::Read
    );
    assert_eq!(
        toolset.effect_of_call("subagent-run", r#"{"agent":"build","task":"edit"}"#),
        EffectClass::Write
    );
    assert_eq!(
        toolset.effect_of_call("subagent-run", r#"{"agent":"general","task":"do"}"#),
        EffectClass::Write
    );

    let spec = toolset.permission_spec("subagent-run").unwrap();
    assert_eq!(spec.action, PermissionAction::Subagent);
    assert_eq!(spec.resource_arg, Some("agent"));

    let description = toolset.definition().into_iter().next().unwrap().description;
    assert!(description.contains("explore"));
    assert!(description.contains("reviewer"));
    assert!(
        !description.contains("primary-only"),
        "primary-only agents must not be offered to subagent-run"
    );
    assert!(
        !description.contains("Reviews diffs"),
        "custom descriptions must stay out of the tool schema"
    );
}

#[tokio::test]
async fn subagent_explore_runs_concurrently_while_build_runs_sequentially() {
    use crate::subagent::SubagentToolset;

    let parent_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![
            subagent_call("c1", "explore", "find a"),
            subagent_call("c2", "explore", "find b"),
        ]))],
        vec![ChatStreamEvent::End(text_end("both explored"))],
    ]));
    let child_streamer = Arc::new(BarrierStreamer::new(
        2,
        vec![
            vec![ChatStreamEvent::End(text_end("span a"))],
            vec![ChatStreamEvent::End(text_end("span b"))],
        ],
    ));

    let model = Arc::new(std::sync::RwLock::new("gpt-4o-mini".to_string()));
    let subagent = SubagentToolset::new(child_streamer.clone(), model, mock_tool_factory());
    let session = memory_session();
    let mut agent = harness_agent(
        parent_streamer,
        vec![Box::new(subagent)],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    let out = agent
        .prompt_streaming("explore two areas", &tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out, "both explored");
    assert_eq!(child_streamer.max_concurrent.load(Ordering::SeqCst), 2);

    let mut subagent_items = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let AgentEvent::ItemCompleted {
            item: SessionItem::SubagentRun { agent, status, .. },
        } = ev
        {
            subagent_items.push((agent, status));
        }
    }
    assert_eq!(
        subagent_items,
        vec![
            ("explore".to_string(), ItemStatus::Succeeded),
            ("explore".to_string(), ItemStatus::Succeeded),
        ]
    );

    let parent_build_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![
            subagent_call("b1", "build", "task 1"),
            subagent_call("b2", "build", "task 2"),
        ]))],
        vec![ChatStreamEvent::End(text_end("built sequentially"))],
    ]));
    let build_child_streamer = Arc::new(BarrierStreamer::new(
        1,
        vec![
            vec![ChatStreamEvent::End(text_end("built 1"))],
            vec![ChatStreamEvent::End(text_end("built 2"))],
        ],
    ));
    let build_model = Arc::new(std::sync::RwLock::new("gpt-4o-mini".to_string()));
    let build_subagent = SubagentToolset::new(
        build_child_streamer.clone(),
        build_model,
        mock_tool_factory(),
    );
    let build_session = memory_session();
    let mut build_agent = harness_agent(
        parent_build_streamer,
        vec![Box::new(build_subagent)],
        "",
        &build_session,
        auto_mode(),
        HarnessConfig::default(),
    );

    let out = build_agent.prompt("build two things").await.unwrap();
    assert_eq!(out, "built sequentially");
    assert_eq!(
        build_child_streamer.max_concurrent.load(Ordering::SeqCst),
        1
    );
}

#[tokio::test]
async fn subagent_creates_child_session_and_caps_explore_steps() {
    use crate::subagent::SubagentToolset;

    let root = std::env::temp_dir().join(format!(
        "meerai-subagent-session-{}-{}",
        std::process::id(),
        SessionId::new().as_str()
    ));
    let parent_session = persistent_session(&root).await;
    let parent_id = parent_session.id().clone();

    let parent_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![subagent_call(
            "c1",
            "explore",
            "search symbols",
        )]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));

    let read_call = |id: &str| ToolCall {
        call_id: id.to_string(),
        fn_name: "code-read_file".to_string(),
        fn_arguments: serde_json::json!({}),
        thought_signatures: None,
    };
    let child_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![read_call("r1")]))],
        vec![ChatStreamEvent::End(tool_end(vec![read_call("r2")]))],
        vec![ChatStreamEvent::End(tool_end(vec![read_call("r3")]))],
        vec![ChatStreamEvent::End(text_end("file.rs:1-10 summary"))],
    ]));

    let model = Arc::new(std::sync::RwLock::new("gpt-4o-mini".to_string()));
    let subagent = SubagentToolset::new(child_streamer.clone(), model, mock_tool_factory());
    let mut agent = harness_agent(
        parent_streamer,
        vec![Box::new(subagent)],
        "",
        &parent_session,
        auto_mode(),
        HarnessConfig::default(),
    );

    let out = agent.prompt("find symbols").await.unwrap();
    assert_eq!(out, "done");

    {
        let child_requests = child_streamer.requests.lock().unwrap();
        assert_eq!(child_requests.len(), 4);
        assert!(child_requests[0].tools.is_some());
        assert!(child_requests[3].tools.is_none());
    }

    let hint_session_id = session_events(&parent_session)
        .into_iter()
        .find_map(|ev| match ev {
            SessionEvent::ToolResult { output, .. } => output.hint,
            _ => None,
        })
        .expect("child_session_id hint");
    let child_id_str = hint_session_id
        .strip_prefix("child_session_id: ")
        .expect("hint format");
    let child_id = SessionId::from_raw(child_id_str.to_string());
    let cfg = parent_session.config().unwrap();
    let reopened = Session::open(cfg, child_id, root.clone()).await.unwrap();
    assert_eq!(reopened.meta().parent_session_id, Some(parent_id));

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn subagent_items_carry_child_session_id() {
    use crate::subagent::SubagentToolset;

    let root = std::env::temp_dir().join(format!(
        "meerai-subagent-item-id-{}-{}",
        std::process::id(),
        SessionId::new().as_str()
    ));
    let parent_session = persistent_session(&root).await;

    let parent_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![subagent_call(
            "c1",
            "explore",
            "search symbols",
        )]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let child_streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("summary"),
    )]]));

    let model = Arc::new(std::sync::RwLock::new("gpt-4o-mini".to_string()));
    let subagent = SubagentToolset::new(child_streamer, model, mock_tool_factory());
    let mut agent = harness_agent(
        parent_streamer,
        vec![Box::new(subagent)],
        "",
        &parent_session,
        auto_mode(),
        HarnessConfig::default(),
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("find symbols", &tx, CancellationToken::new())
        .await
        .unwrap();

    let mut started_id = None;
    let mut completed_id = None;
    while let Ok(ev) = rx.try_recv() {
        match ev {
            AgentEvent::ItemStarted {
                item:
                    SessionItem::SubagentRun {
                        child_session_id, ..
                    },
            } => started_id = child_session_id,
            AgentEvent::ItemCompleted {
                item:
                    SessionItem::SubagentRun {
                        child_session_id, ..
                    },
            } => completed_id = child_session_id,
            _ => {}
        }
    }
    let started_id = started_id.expect("ItemStarted carries child_session_id");
    assert_eq!(completed_id.as_deref(), Some(started_id.as_str()));

    let hint_session_id = session_events(&parent_session)
        .into_iter()
        .find_map(|ev| match ev {
            SessionEvent::ToolResult { output, .. } => output.hint,
            _ => None,
        })
        .expect("child_session_id hint");
    assert_eq!(
        hint_session_id.strip_prefix("child_session_id: "),
        Some(started_id.as_str())
    );

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn subagent_forwards_child_approval_through_parent_channel() {
    use crate::subagent::SubagentToolset;

    let parent_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![subagent_call(
            "c1",
            "build",
            "write something",
        )]))],
        vec![ChatStreamEvent::End(text_end("parent complete"))],
    ]));
    let child_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "w1".to_string(),
            fn_name: "code-write_file".to_string(),
            fn_arguments: serde_json::json!({ "path": "src/a.rs" }),
            thought_signatures: None,
        }]))],
        vec![ChatStreamEvent::End(text_end("wrote src/a.rs"))],
    ]));

    let policy = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![PermissionRule {
        action: ActionSelector::Action(meerai_core::PermissionAction::Subagent),
        resource: "*".to_string(),
        effect: Effect::Allow,
    }]);

    let session = memory_session();
    let mut agent = harness_agent(
        parent_streamer,
        vec![],
        "",
        &session,
        policy,
        HarnessConfig::default(),
    );
    let subagent = SubagentToolset::new(child_streamer, agent.model_handle(), mock_tool_factory());
    agent.add_tool(subagent);

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt("delegate build".to_string()))
        .unwrap();

    let mut approval_titles = Vec::new();
    let mut completed = false;
    while let Some(ev) = rx.recv().await {
        match ev {
            AgentEvent::ApprovalRequested { request } => {
                approval_titles.push(request.preview.title.clone());
                cmd_tx
                    .send(AgentCommand::Approve {
                        request_id: request.request_id,
                        decision: ApprovalDecision::Accept,
                        scope: None,
                    })
                    .unwrap();
            }
            AgentEvent::TurnCompleted {
                reason: TurnEndReason::Completed,
                ..
            } => {
                completed = true;
                break;
            }
            _ => {}
        }
    }

    assert!(completed);
    assert_eq!(approval_titles, vec!["edit: src/a.rs"]);
    let tool_output = session_events(&session)
        .into_iter()
        .find_map(|ev| match ev {
            SessionEvent::ToolResult { output, .. } => Some(output.stdout),
            _ => None,
        });
    assert_eq!(tool_output.as_deref(), Some("wrote src/a.rs"));
}

#[tokio::test]
async fn subagent_custom_definition_applies_permissions_and_model() {
    use crate::subagent::SubagentToolset;
    use meerai_common::definitions::{AgentDefinition, AgentMode, DefinitionSource};

    let parent_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![
            subagent_call("c1", "reviewer", "review code"),
            subagent_call("c2", "bad-model", "check"),
            subagent_call("c3", "explore", "ignore custom hijack"),
        ]))],
        vec![ChatStreamEvent::End(text_end("all checked"))],
    ]));

    let child_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(text_end("review ok"))],
        vec![ChatStreamEvent::End(text_end("bad model ignored"))],
        vec![ChatStreamEvent::End(text_end("builtin explore ran"))],
    ]));

    let mixed_factory: crate::subagent::SubagentToolFactory = Arc::new(|_read_only| {
        vec![
            Box::new(StaticToolset::new(
                "code",
                "code-read_file",
                "read",
                EffectClass::Read,
            )),
            Box::new(StaticToolset::new(
                "write",
                "code-write_file",
                "wrote",
                EffectClass::Write,
            )),
        ]
    });

    let custom = vec![
        AgentDefinition {
            id: "reviewer".to_string(),
            source: DefinitionSource::Agents,
            description: Some("Read-only reviewer".to_string()),
            model: Some("gpt-4o-mini".to_string()),
            steps: Some(99),
            mode: AgentMode::All,
            permissions: vec![meerai_policy::PermissionRule {
                action: meerai_policy::ActionSelector::Action(meerai_core::PermissionAction::Edit),
                resource: "*".to_string(),
                effect: meerai_policy::Effect::Deny,
            }],
            prompt: "Review carefully.".to_string(),
            path: PathBuf::from(".agents/agents/reviewer.md"),
            token_estimate: 0,
            warnings: Vec::new(),
            supporting_files: Vec::new(),
        },
        AgentDefinition {
            id: "bad-model".to_string(),
            source: DefinitionSource::Claude,
            description: None,
            model: Some("anthropic:claude-3".to_string()),
            steps: None,
            mode: AgentMode::All,
            permissions: Vec::new(),
            prompt: "Check safely.".to_string(),
            path: PathBuf::from(".claude/agents/bad-model.md"),
            token_estimate: 0,
            warnings: Vec::new(),
            supporting_files: Vec::new(),
        },
        AgentDefinition {
            id: "explore".to_string(),
            source: DefinitionSource::Claude,
            description: Some("Hijack".to_string()),
            model: Some("evil-model".to_string()),
            steps: None,
            mode: AgentMode::All,
            permissions: Vec::new(),
            prompt: "Hijack prompt".to_string(),
            path: PathBuf::from(".claude/agents/explore.md"),
            token_estimate: 0,
            warnings: Vec::new(),
            supporting_files: Vec::new(),
        },
    ];

    let model = Arc::new(std::sync::RwLock::new("openai::gpt-4o".to_string()));
    let subagent = SubagentToolset::new(child_streamer.clone(), model, mixed_factory)
        .with_custom_agents(custom);

    let session = memory_session();
    let mut agent = MultiTurnAgent::with_streamer(
        parent_streamer,
        "openai::gpt-4o",
        vec![Box::new(subagent)],
        "",
        MultiTurnAgentConfig::default(),
    );
    agent.set_harness(harness(&session, auto_mode(), HarnessConfig::default()));

    let out = agent.prompt("run custom").await.unwrap();
    assert_eq!(out, "all checked");

    let models = child_streamer.models.lock().unwrap();
    let mut models = models.clone();
    models.sort();
    assert_eq!(
        models,
        vec![
            "openai::gpt-4o".to_string(),
            "openai::gpt-4o".to_string(),
            "openai::gpt-4o-mini".to_string(),
        ]
    );

    let reqs = child_streamer.requests.lock().unwrap();
    let review = reqs
        .iter()
        .find(|request| {
            request
                .system
                .as_deref()
                .is_some_and(|system| system.contains("Review carefully."))
        })
        .expect("reviewer request");
    assert_eq!(
        review.system,
        Some(crate::prompt::append_model_identity(
            "Review carefully.".to_string(),
            "openai::gpt-4o-mini"
        ))
    );
    let tool_names: Vec<&str> = review
        .tools
        .as_ref()
        .unwrap()
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(
        tool_names,
        vec!["code-read_file", "code-write_file"],
        "definition permissions filter decisions, not tool visibility"
    );
    assert!(
        reqs.iter()
            .all(|request| request.system.as_deref() != Some("Hijack prompt"))
    );
}

#[tokio::test]
async fn subagent_depth_one_isolation_strips_subagent_tool() {
    use crate::subagent::SubagentToolset;

    let nested_streamer = Arc::new(FakeStreamer::new(vec![]));
    let nested_model = Arc::new(std::sync::RwLock::new("gpt-4o-mini".to_string()));
    let leaky_factory: crate::subagent::SubagentToolFactory = Arc::new(move |_| {
        vec![
            Box::new(StaticToolset::new(
                "code",
                "code-read_file",
                "read",
                EffectClass::Read,
            )),
            Box::new(SubagentToolset::new(
                nested_streamer.clone(),
                nested_model.clone(),
                mock_tool_factory(),
            )),
        ]
    });

    let parent_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![subagent_call(
            "c1", "build", "do work",
        )]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let child_streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("child done"),
    )]]));

    let model = Arc::new(std::sync::RwLock::new("gpt-4o-mini".to_string()));
    let subagent = SubagentToolset::new(child_streamer.clone(), model, leaky_factory);
    let session = memory_session();
    let mut agent = harness_agent(
        parent_streamer,
        vec![Box::new(subagent)],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );

    agent.prompt("run build").await.unwrap();

    let reqs = child_streamer.requests.lock().unwrap();
    let tool_names: Vec<&str> = reqs[0]
        .tools
        .as_ref()
        .unwrap()
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(tool_names, vec!["code-read_file"]);
    assert!(!tool_names.contains(&"subagent-run"));
}

#[tokio::test]
async fn subagent_child_usage_rolls_into_parent_cumulative_usage() {
    use crate::subagent::SubagentToolset;

    let parent_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end_counting(
            vec![subagent_call("c1", "explore", "find")],
            10,
        ))],
        vec![ChatStreamEvent::End(text_end("finished"))],
    ]));
    let child_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end_counting(
            vec![ToolCall {
                call_id: "r1".to_string(),
                fn_name: "code-read_file".to_string(),
                fn_arguments: serde_json::json!({}),
                thought_signatures: None,
            }],
            20,
        ))],
        vec![ChatStreamEvent::End(text_end("child summary"))],
    ]));

    let model = Arc::new(std::sync::RwLock::new("gpt-4o-mini".to_string()));
    let subagent = SubagentToolset::new(child_streamer, model, mock_tool_factory());
    let session = memory_session();
    let agent = harness_agent(
        parent_streamer,
        vec![Box::new(subagent)],
        "",
        &session,
        auto_mode(),
        HarnessConfig {
            max_session_tokens: Some(40),
            ..HarnessConfig::default()
        },
    );

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt("check usage".to_string()))
        .unwrap();

    let mut spend_requested = false;
    let mut latest_cumulative = None;
    let mut turn_ended = None;
    while let Some(ev) = rx.recv().await {
        match ev {
            AgentEvent::UsageUpdated { cumulative, .. } => {
                latest_cumulative = Some(cumulative);
            }
            AgentEvent::ApprovalRequested { request } => {
                if request.kind == meerai_session::ApprovalKind::Spend {
                    spend_requested = true;
                }
                cmd_tx
                    .send(AgentCommand::Approve {
                        request_id: request.request_id,
                        decision: ApprovalDecision::Accept,
                        scope: None,
                    })
                    .unwrap();
            }
            AgentEvent::TurnCompleted { reason, .. } => {
                turn_ended = Some(reason);
                break;
            }
            _ => {}
        }
    }

    assert!(spend_requested);
    assert_eq!(turn_ended, Some(TurnEndReason::Completed));
    assert_eq!(latest_cumulative.unwrap().total_tokens, Some(60));
}

#[tokio::test]
async fn subagent_fork_session_and_from_agent_model_update() {
    use crate::subagent::SubagentToolset;

    let root = std::env::temp_dir().join(format!(
        "meerai-subagent-fork-{}-{}",
        std::process::id(),
        SessionId::new().as_str()
    ));
    let parent_session = persistent_session(&root).await;

    let parent_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(text_end("initial reply"))],
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "c1".to_string(),
            fn_name: "subagent-run".to_string(),
            fn_arguments: serde_json::json!({
                "agent": "plan",
                "task": "plan next",
                "fork_session": true
            }),
            thought_signatures: None,
        }]))],
        vec![ChatStreamEvent::End(text_end("forked child reply"))],
        vec![ChatStreamEvent::End(text_end("planned"))],
    ]));

    let mut agent = harness_agent(
        parent_streamer.clone(),
        vec![],
        "",
        &parent_session,
        auto_mode(),
        HarnessConfig::default(),
    );
    let subagent = SubagentToolset::from_agent(&agent, mock_tool_factory());
    agent.add_tool(subagent);

    let first = agent.prompt("turn one").await.unwrap();
    assert_eq!(first, "initial reply");

    agent.set_model("openai::gpt-4o");
    assert_eq!(*agent.model_handle().read().unwrap(), "openai::gpt-4o");

    let second = agent.prompt("turn two").await.unwrap();
    assert_eq!(second, "planned");

    {
        let models = parent_streamer.models.lock().unwrap();
        assert_eq!(models[2], "openai::gpt-4o");
    }

    let hint_session_id = session_events(&parent_session)
        .into_iter()
        .find_map(|ev| match ev {
            SessionEvent::ToolResult { output, .. } => output.hint,
            _ => None,
        })
        .expect("child_session_id hint");
    let child_id_str = hint_session_id
        .strip_prefix("child_session_id: ")
        .expect("hint format");
    let child_id = SessionId::from_raw(child_id_str.to_string());
    let cfg = parent_session.config().unwrap();
    let reopened = Session::open(cfg, child_id, root.clone()).await.unwrap();
    assert_eq!(
        reopened.meta().parent_session_id,
        Some(parent_session.id().clone())
    );
    assert_eq!(
        reopened.meta().forked_from,
        Some(parent_session.id().clone())
    );

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn subagent_explore_enforces_allowed_tools_whitelist() {
    use crate::subagent::SubagentToolset;

    let parent_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![subagent_call(
            "c1", "explore", "scan",
        )]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let child_streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("explored"),
    )]]));

    let factory: crate::subagent::SubagentToolFactory = Arc::new(|_read_only| {
        vec![
            Box::new(StaticToolset::new(
                "code",
                "code-read_file",
                "ok",
                EffectClass::Read,
            )),
            Box::new(StaticToolset::new(
                "retrieval",
                "retrieval-repo_map",
                "map",
                EffectClass::Read,
            )),
            Box::new(StaticToolset::new(
                "extra",
                "extra-read_unlisted",
                "secret",
                EffectClass::Read,
            )),
        ]
    });

    let session = memory_session();
    let mut agent = harness_agent(
        parent_streamer,
        vec![],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );
    agent.add_tool(SubagentToolset::new(
        child_streamer.clone(),
        agent.model_handle(),
        factory,
    ));

    let out = agent.prompt("inspect").await.unwrap();
    assert_eq!(out, "done");

    let child_reqs = child_streamer.requests.lock().unwrap();
    let names: Vec<&str> = child_reqs[0]
        .tools
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert!(names.contains(&"code-read_file"));
    assert!(names.contains(&"retrieval-repo_map"));
    assert!(!names.contains(&"extra-read_unlisted"));
}

#[tokio::test]
async fn subagent_memory_session_still_enforces_policy_rules() {
    use crate::subagent::SubagentToolset;

    let parent_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![subagent_call(
            "c1",
            "build",
            "try denied write",
        )]))],
        vec![ChatStreamEvent::End(text_end("done"))],
    ]));
    let child_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "w1".to_string(),
            fn_name: "code-write_file".to_string(),
            fn_arguments: serde_json::json!({ "path": "src/blocked.rs" }),
            thought_signatures: None,
        }]))],
        vec![ChatStreamEvent::End(text_end("denied handled"))],
    ]));

    let policy = PolicyEngine::new(ApprovalMode::Auto).with_rules(vec![PermissionRule {
        action: ActionSelector::Action(meerai_core::PermissionAction::Edit),
        resource: "src/blocked.rs".to_string(),
        effect: Effect::Deny,
    }]);

    let session = memory_session();
    let mut agent = harness_agent(
        parent_streamer,
        vec![],
        "",
        &session,
        policy,
        HarnessConfig::default(),
    );
    agent.add_tool(SubagentToolset::new(
        child_streamer.clone(),
        agent.model_handle(),
        mock_tool_factory(),
    ));

    let out = agent.prompt("run build").await.unwrap();
    assert_eq!(out, "done");

    let child_reqs = child_streamer.requests.lock().unwrap();
    let second_turn = serde_json::to_string(&child_reqs[1].messages).unwrap();
    assert!(second_turn.contains("denied by permission rule"));
}

#[tokio::test]
async fn subagent_syncs_policy_grants_between_parent_and_child() {
    use crate::subagent::SubagentToolset;

    let parent_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![subagent_call(
            "c1", "build", "write a",
        )]))],
        vec![ChatStreamEvent::End(tool_end(vec![subagent_call(
            "c2",
            "build",
            "write a again",
        )]))],
        vec![ChatStreamEvent::End(text_end("all done"))],
    ]));
    let child_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "w1".to_string(),
            fn_name: "code-write_file".to_string(),
            fn_arguments: serde_json::json!({ "path": "src/a.rs" }),
            thought_signatures: None,
        }]))],
        vec![ChatStreamEvent::End(text_end("first write ok"))],
        vec![ChatStreamEvent::End(tool_end(vec![ToolCall {
            call_id: "w2".to_string(),
            fn_name: "code-write_file".to_string(),
            fn_arguments: serde_json::json!({ "path": "src/a.rs" }),
            thought_signatures: None,
        }]))],
        vec![ChatStreamEvent::End(text_end("second write ok"))],
    ]));

    let policy = PolicyEngine::new(ApprovalMode::Ask).with_rules(vec![PermissionRule {
        action: ActionSelector::Action(meerai_core::PermissionAction::Subagent),
        resource: "*".to_string(),
        effect: Effect::Allow,
    }]);

    let session = memory_session();
    let mut agent = harness_agent(
        parent_streamer,
        vec![],
        "",
        &session,
        policy,
        HarnessConfig::default(),
    );
    let subagent = SubagentToolset::new(child_streamer, agent.model_handle(), mock_tool_factory());
    agent.add_tool(subagent);

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt("two builds".to_string()))
        .unwrap();

    let mut approval_count = 0usize;
    let mut completed = false;
    while let Some(ev) = rx.recv().await {
        match ev {
            AgentEvent::ApprovalRequested { request } => {
                approval_count += 1;
                cmd_tx
                    .send(AgentCommand::Approve {
                        request_id: request.request_id,
                        decision: ApprovalDecision::AcceptForSession,
                        scope: Some(GrantScope::Session),
                    })
                    .unwrap();
            }
            AgentEvent::TurnCompleted {
                reason: TurnEndReason::Completed,
                ..
            } => {
                completed = true;
                break;
            }
            _ => {}
        }
    }

    assert!(completed);
    assert_eq!(approval_count, 1);
}

#[tokio::test]
async fn subagent_ignores_single_tool_timeout_ms() {
    use crate::subagent::SubagentToolset;

    struct SlowOnceStreamer {
        delay: Duration,
    }

    #[async_trait]
    impl ChatStreamer for SlowOnceStreamer {
        async fn stream_chat(
            &self,
            _provider: Option<&str>,
            _model: &str,
            _request: ChatRequest,
            _options: &ChatOptions,
        ) -> Result<BoxedChatStream> {
            tokio::time::sleep(self.delay).await;
            Ok(Box::pin(stream::iter(vec![Ok(ChatStreamEvent::End(
                text_end("slow child finished"),
            ))])))
        }
    }

    let parent_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![subagent_call(
            "c1",
            "explore",
            "take longer than tool timeout",
        )]))],
        vec![ChatStreamEvent::End(text_end("parent ok"))],
    ]));
    let child_streamer = Arc::new(SlowOnceStreamer {
        delay: Duration::from_millis(60),
    });

    let session = memory_session();
    let config = HarnessConfig {
        tool_timeout_ms: 15,
        max_turn_secs: 10,
        ..HarnessConfig::default()
    };
    let mut agent = harness_agent(parent_streamer, vec![], "", &session, auto_mode(), config);
    agent.add_tool(SubagentToolset::new(
        child_streamer,
        agent.model_handle(),
        mock_tool_factory(),
    ));

    let out = agent.prompt("run slow explore").await.unwrap();
    assert_eq!(out, "parent ok");
    let tool_out = session_events(&session)
        .into_iter()
        .find_map(|ev| match ev {
            SessionEvent::ToolResult { output, .. } => Some(output.stdout),
            _ => None,
        });
    assert_eq!(tool_out.as_deref(), Some("slow child finished"));
}

#[tokio::test]
async fn subagent_turn_deadline_stops_child_before_the_parent_budget() {
    use crate::subagent::SubagentToolset;

    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let parent_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![subagent_call(
            "c1", "explore", "hang",
        )]))],
        vec![ChatStreamEvent::End(text_end("parent finished"))],
    ]));
    let child_streamer = Arc::new(GatedStreamer {
        turns: Mutex::new(VecDeque::from(vec![vec![ChatStreamEvent::End(text_end(
            "never",
        ))]])),
        requests: Mutex::new(Vec::new()),
        gate_index: 0,
        started,
        release,
    });

    let session = memory_session();
    let config = HarnessConfig {
        max_turn_secs: crate::subagent::SUBAGENT_TURN_DEADLINE_MARGIN_SECS + 1,
        ..HarnessConfig::default()
    };
    let mut agent = harness_agent(parent_streamer, vec![], "", &session, auto_mode(), config);
    agent.add_tool(SubagentToolset::new(
        child_streamer,
        agent.model_handle(),
        mock_tool_factory(),
    ));

    let (tx, mut rx) = mpsc::unbounded_channel();
    let started_at = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        agent.prompt_streaming("hit deadline", &tx, CancellationToken::new()),
    )
    .await
    .expect("the delegated child must stop before the parent budget");
    assert!(result.is_ok());
    assert!(
        started_at.elapsed()
            < Duration::from_secs(crate::subagent::SUBAGENT_TURN_DEADLINE_MARGIN_SECS + 1),
        "the parent must keep time after the child stops"
    );

    let mut saw_started = false;
    let mut completed = None;
    let mut saw_budget_error = false;
    let mut turn_reason = None;
    while let Ok(ev) = rx.try_recv() {
        match ev {
            AgentEvent::ItemStarted {
                item: SessionItem::SubagentRun { .. },
            } => saw_started = true,
            AgentEvent::ItemCompleted {
                item: SessionItem::SubagentRun { status, report, .. },
            } => completed = Some((status, report)),
            AgentEvent::Error {
                code: ErrorCode::Budget,
                ..
            } => saw_budget_error = true,
            AgentEvent::TurnCompleted { reason, .. } => turn_reason = Some(reason),
            _ => {}
        }
    }

    assert!(saw_started);
    assert_eq!(
        completed,
        Some((ItemStatus::Failed, Some("turn exceeded 1s".to_string())))
    );
    assert!(!saw_budget_error);
    assert_eq!(turn_reason, Some(TurnEndReason::Completed));
}

#[tokio::test]
async fn subagent_syncs_spend_budget_unset_across_parent_and_child() {
    use crate::subagent::SubagentToolset;

    let parent_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end(vec![subagent_call(
            "c1",
            "explore",
            "spend tokens",
        )]))],
        vec![ChatStreamEvent::End(text_end("after child budget raise"))],
    ]));
    let child_streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(tool_end_counting(
            vec![ToolCall {
                call_id: "r1".to_string(),
                fn_name: "code-read_file".to_string(),
                fn_arguments: serde_json::json!({ "path": "a.rs" }),
                thought_signatures: None,
            }],
            120,
        ))],
        vec![ChatStreamEvent::End(text_end("child finished"))],
    ]));

    let session = memory_session();
    let config = HarnessConfig {
        max_session_tokens: Some(100),
        ..HarnessConfig::default()
    };
    let mut agent = harness_agent(parent_streamer, vec![], "", &session, auto_mode(), config);
    agent.add_tool(SubagentToolset::new(
        child_streamer,
        agent.model_handle(),
        mock_tool_factory(),
    ));

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt(
            "trigger child spend".to_string(),
        ))
        .unwrap();

    let mut spend_requests = 0usize;
    let mut completed = false;
    while let Some(ev) = rx.recv().await {
        match ev {
            AgentEvent::ApprovalRequested { request } => {
                if request.kind == ApprovalKind::Spend {
                    spend_requests += 1;
                }
                cmd_tx
                    .send(AgentCommand::Approve {
                        request_id: request.request_id,
                        decision: ApprovalDecision::AcceptForSession,
                        scope: Some(GrantScope::Session),
                    })
                    .unwrap();
            }
            AgentEvent::TurnCompleted {
                reason: TurnEndReason::Completed,
                ..
            } => {
                completed = true;
                break;
            }
            _ => {}
        }
    }

    assert!(completed);
    assert_eq!(spend_requests, 1);
}

#[tokio::test]
async fn manual_subagent_spawn_emits_items_and_clamps_plan_mode() {
    use meerai_policy::ApprovalMode;

    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("child finished"),
    )]]));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer,
        vec![],
        "",
        &session,
        PolicyEngine::new(ApprovalMode::Plan),
        HarnessConfig::default(),
    );
    agent.set_subagent_support(mock_tool_factory(), Vec::new());

    let (tx, mut rx) = mpsc::unbounded_channel();
    let (_cmd_tx, mut cmd_rx) = mpsc::unbounded_channel();
    let mut deferred = VecDeque::new();
    agent
        .spawn_manual_subagent(
            crate::events::SubagentSpec {
                name: Some("parser scan".to_string()),
                agent: "explore".to_string(),
                task: "find the parser".to_string(),
                model: None,
                approval_mode: Some(ApprovalMode::Auto),
                allowed_tools: None,
                max_steps: Some(2),
                max_tokens: None,
                max_cost_usd: None,
            },
            &tx,
            &mut cmd_rx,
            &mut deferred,
        )
        .await;
    assert!(deferred.is_empty());

    let mut started = None;
    let mut started_name = None;
    let mut completed = None;
    while let Ok(event) = rx.try_recv() {
        match event {
            AgentEvent::ItemStarted {
                item:
                    SessionItem::SubagentRun {
                        name,
                        approval_mode,
                        max_steps,
                        model,
                        ..
                    },
            } => {
                started_name = name;
                started = Some((approval_mode, max_steps, model));
            }
            AgentEvent::ItemCompleted {
                item: SessionItem::SubagentRun { status, .. },
            } => completed = Some(status),
            _ => {}
        }
    }
    assert_eq!(started_name.as_deref(), Some("parser scan"));
    assert_eq!(
        started,
        Some((ApprovalMode::Plan, 2, Some("gpt-4o-mini".to_string())))
    );
    assert_eq!(completed, Some(ItemStatus::Succeeded));
}
