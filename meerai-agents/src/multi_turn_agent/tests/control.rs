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
async fn model_switch_updates_identity_in_request_and_log() {
    let streamer = Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(text_end("first"))],
        vec![ChatStreamEvent::End(text_end("second"))],
    ]));
    let session = memory_session();
    let mut agent = MultiTurnAgent::with_streamer(
        streamer.clone(),
        "openai::gpt-4o",
        vec![],
        "Be precise.",
        MultiTurnAgentConfig::default(),
    );
    agent.set_harness(harness(&session, auto_mode(), HarnessConfig::default()));

    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("one", &tx, CancellationToken::new())
        .await
        .unwrap();
    agent.set_model("gemini::gemini-3.8-flash");
    agent
        .prompt_streaming("two", &tx, CancellationToken::new())
        .await
        .unwrap();

    let requests = streamer.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let first = requests[0].system.clone().unwrap();
    let second = requests[1].system.clone().unwrap();
    assert!(first.contains("Active model: gpt-4o."));
    assert!(second.contains("Active model: gemini-3.8-flash."));
    assert!(!second.contains("openai::gpt-4o"));

    let system_messages: Vec<(String, u64)> = session_events(&session)
        .iter()
        .filter_map(|event| match event {
            SessionEvent::SystemMessage {
                sections,
                generation,
            } => Some((sections[0].content.clone(), *generation)),
            _ => None,
        })
        .collect();
    assert_eq!(system_messages.len(), 2);
    assert_eq!(system_messages[0].1, 0);
    assert_eq!(system_messages[1].1, 1);
    assert!(
        system_messages[1]
            .0
            .contains("Active model: gemini-3.8-flash.")
    );
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
    agent.set_harness(harness(&session, auto_mode(), HarnessConfig::default()));

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
        auto_mode(),
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
        auto_mode(),
        HarnessConfig::default(),
    );

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::ApplyAgent {
            name: "reviewer".to_string(),
            instructions: "Review every change.".to_string(),
            model: Some("gpt-5-mini".to_string()),
            context_window_tokens: None,
            effort_levels: None,
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
    assert_eq!(session.meta().model.as_deref(), Some("gpt-5-mini"));
    let reqs = streamer.requests.lock().unwrap();
    assert!(format!("{:?}", reqs[0].messages).contains("Agent instructions (reviewer):"));
    assert!(format!("{:?}", reqs[0].messages).contains("Review every change."));

    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn idle_set_model_persists_session_meta() {
    let streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("done"),
    )]]));

    let session = memory_session();
    let agent = harness_agent(
        streamer.clone(),
        vec![],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SetModel {
            model: "openai::gpt-5-mini".to_string(),
            provider: Some("openai".to_string()),
            context_window_tokens: None,
            effort_levels: None,
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

    assert_eq!(session.meta().model.as_deref(), Some("gpt-5-mini"));
    assert_eq!(session.meta().provider.as_deref(), Some("openai"));

    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn model_set_during_a_turn_persists_session_meta() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let streamer = Arc::new(GatedStreamer {
        turns: Mutex::new(VecDeque::from(vec![vec![ChatStreamEvent::End(text_end(
            "done",
        ))]])),
        requests: Mutex::new(Vec::new()),
        gate_index: 0,
        started: started.clone(),
        release: release.clone(),
    });

    let session = memory_session();
    let agent = harness_agent(
        streamer,
        vec![],
        "",
        &session,
        auto_mode(),
        HarnessConfig::default(),
    );

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SubmitPrompt("hello".to_string()))
        .unwrap();
    started.notified().await;
    cmd_tx
        .send(AgentCommand::SetModel {
            model: "openai::gpt-5-mini".to_string(),
            provider: None,
            context_window_tokens: None,
            effort_levels: None,
        })
        .unwrap();
    release.notify_one();

    loop {
        match rx.recv().await {
            Some(AgentEvent::TurnCompleted { .. }) => break,
            Some(_) => {}
            None => panic!("worker stopped mid-turn"),
        }
    }

    for _ in 0..50 {
        if session.meta().model.as_deref() == Some("gpt-5-mini") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(session.meta().model.as_deref(), Some("gpt-5-mini"));

    let _ = cmd_tx.send(AgentCommand::Quit);
}

fn model_switch_streamer() -> Arc<FakeStreamer> {
    let reply = "x".repeat(800);
    let summary = "Goal: ship the test";
    Arc::new(FakeStreamer::new(vec![
        vec![ChatStreamEvent::End(text_end(&reply))],
        vec![
            ChatStreamEvent::Chunk(StreamChunk {
                content: summary.to_string(),
            }),
            ChatStreamEvent::End(text_end(summary)),
        ],
        vec![ChatStreamEvent::End(text_end("second reply"))],
    ]))
}

async fn run_two_turns(
    cmd_tx: &mpsc::UnboundedSender<AgentCommand>,
    rx: &mut mpsc::UnboundedReceiver<AgentEvent>,
) {
    for prompt in ["first", "second"] {
        cmd_tx
            .send(AgentCommand::SubmitPrompt(prompt.to_string()))
            .unwrap();
        loop {
            match rx.recv().await {
                Some(AgentEvent::TurnCompleted { .. }) => break,
                Some(_) => {}
                None => panic!("worker stopped mid-turn"),
            }
        }
    }
}

fn pressure_compacted(session: &Session) -> bool {
    session_events(session).iter().any(|event| {
        matches!(
            event,
            SessionEvent::CompactionStarted {
                trigger: CompactionTrigger::Pressure,
                ..
            }
        )
    })
}

#[tokio::test]
async fn worker_set_model_applies_the_context_window() {
    let session = memory_session();
    let agent = harness_agent(
        model_switch_streamer(),
        vec![],
        "system",
        &session,
        auto_mode(),
        HarnessConfig {
            verbatim_window_turns: 1,
            ..HarnessConfig::default()
        },
    );

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SetModel {
            model: "openai::gpt-5-mini".to_string(),
            provider: None,
            context_window_tokens: Some(10),
            effort_levels: None,
        })
        .unwrap();
    run_two_turns(&cmd_tx, &mut rx).await;

    assert!(pressure_compacted(&session));

    let _ = cmd_tx.send(AgentCommand::Quit);
}

#[tokio::test]
async fn worker_set_model_without_a_window_keeps_the_current_window() {
    let session = memory_session();
    let agent = harness_agent(
        model_switch_streamer(),
        vec![],
        "system",
        &session,
        auto_mode(),
        HarnessConfig {
            context_window_tokens: 10,
            verbatim_window_turns: 1,
            ..HarnessConfig::default()
        },
    );

    let (cmd_tx, mut rx) = spawn_worker(agent);
    cmd_tx
        .send(AgentCommand::SetModel {
            model: "openai::gpt-5-mini".to_string(),
            provider: None,
            context_window_tokens: None,
            effort_levels: None,
        })
        .unwrap();
    run_two_turns(&cmd_tx, &mut rx).await;

    assert!(pressure_compacted(&session));

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
        auto_mode(),
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
            context_window_tokens: None,
            effort_levels: None,
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
async fn reload_preserves_token_pricing() {
    let pricing = crate::harness::TokenPricing {
        input_per_mtok: 1.5,
        output_per_mtok: 3.0,
    };
    let streamer = Arc::new(FakeStreamer::new(Vec::new()));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer,
        Vec::new(),
        "system",
        &session,
        auto_mode(),
        HarnessConfig {
            token_pricing: Some(pricing),
            ..HarnessConfig::default()
        },
    );

    let context = ContextSettings {
        max_steps_per_turn: Some(7),
        ..ContextSettings::default()
    };
    agent.reload_config(None, Some(context), None, "reloaded system".to_string());

    let harness = agent.harness().expect("harness");
    assert_eq!(harness.config.token_pricing, Some(pricing));
    assert_eq!(harness.config.max_steps_per_turn, 7);
}

#[tokio::test]
async fn reload_preserves_plan_usage() {
    let streamer = Arc::new(FakeStreamer::new(Vec::new()));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer,
        Vec::new(),
        "system",
        &session,
        auto_mode(),
        HarnessConfig {
            plan_usage: true,
            ..HarnessConfig::default()
        },
    );

    let context = ContextSettings {
        max_steps_per_turn: Some(7),
        ..ContextSettings::default()
    };
    agent.reload_config(None, Some(context), None, "reloaded system".to_string());

    let harness = agent.harness().expect("harness");
    assert!(harness.config.plan_usage);
    assert_eq!(harness.config.max_steps_per_turn, 7);

    agent.reload_config(None, None, None, "prompt".to_string());
    assert!(agent.harness().expect("harness").config.plan_usage);
}

#[tokio::test]
async fn reload_preserves_resolved_context_window_when_unset() {
    let streamer = Arc::new(FakeStreamer::new(Vec::new()));
    let session = memory_session();
    let mut agent = harness_agent(
        streamer,
        Vec::new(),
        "system",
        &session,
        auto_mode(),
        HarnessConfig {
            context_window_tokens: 200_000,
            ..HarnessConfig::default()
        },
    );

    agent.reload_config(None, None, None, "prompt".to_string());
    assert_eq!(
        agent
            .harness()
            .expect("harness")
            .config
            .context_window_tokens,
        200_000
    );

    agent.reload_config(
        None,
        Some(ContextSettings::default()),
        None,
        "prompt".to_string(),
    );
    assert_eq!(
        agent
            .harness()
            .expect("harness")
            .config
            .context_window_tokens,
        200_000
    );

    let narrow = ContextSettings {
        context_window_tokens: Some(50_000),
        ..ContextSettings::default()
    };
    agent.reload_config(None, Some(narrow), None, "prompt".to_string());
    assert_eq!(
        agent
            .harness()
            .expect("harness")
            .config
            .context_window_tokens,
        50_000
    );

    agent.reload_config(None, None, None, "prompt".to_string());
    assert_eq!(
        agent
            .harness()
            .expect("harness")
            .config
            .context_window_tokens,
        50_000
    );
}

#[tokio::test]
async fn set_context_window_tokens_updates_the_harness() {
    let session = memory_session();
    let mut agent = harness_agent(
        Arc::new(FakeStreamer::new(Vec::new())),
        Vec::new(),
        "system",
        &session,
        auto_mode(),
        HarnessConfig {
            context_window_tokens: 128_000,
            ..HarnessConfig::default()
        },
    );

    agent.set_context_window_tokens(200_000);
    assert_eq!(
        agent
            .harness()
            .expect("harness")
            .config
            .context_window_tokens,
        200_000
    );

    let mut bare = MultiTurnAgent::with_streamer(
        Arc::new(FakeStreamer::new(Vec::new())),
        "gpt-4o-mini",
        Vec::new(),
        "system",
        MultiTurnAgentConfig::default(),
    );
    bare.set_context_window_tokens(1_000);
    assert!(bare.harness().is_none());
}

#[tokio::test]
async fn reload_applies_post_edit_diagnostics_reset_to_default() {
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
    agent.set_post_edit_diagnostics(true);

    agent.reload_config(None, None, None, "prompt".to_string());
    assert!(!agent.post_edit_diagnostics);

    let enabled = ContextSettings {
        post_edit_diagnostics: Some(true),
        ..ContextSettings::default()
    };
    agent.reload_config(None, Some(enabled), None, "prompt".to_string());
    assert!(agent.post_edit_diagnostics);

    let unset = ContextSettings {
        post_edit_diagnostics: None,
        ..ContextSettings::default()
    };
    agent.reload_config(None, Some(unset), None, "prompt".to_string());
    assert!(!agent.post_edit_diagnostics);
}

#[tokio::test]
async fn extra_high_effort_maps_to_xhigh_and_clamps_to_the_model_set() {
    use meerai_common::settings::EffortLevel;

    let wanted = EffortLevel::ExtraHigh;
    let open_streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("done"),
    )]]));
    let session = memory_session();
    let mut agent = MultiTurnAgent::with_streamer(
        open_streamer.clone(),
        "openai_resp::gpt-6.2-sol",
        Vec::new(),
        "",
        MultiTurnAgentConfig::default(),
    );
    let config = HarnessConfig {
        effort: Some(wanted),
        effort_levels: Some(EffortLevel::ALL.to_vec()),
        ..HarnessConfig::default()
    };
    agent.set_harness(harness(&session, auto_mode(), config));
    let (tx, _rx) = mpsc::unbounded_channel();
    agent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    {
        let cached = open_streamer.options.lock().unwrap();
        assert!(matches!(
            cached[0].reasoning_effort,
            Some(genai::chat::ReasoningEffort::XHigh)
        ));
    }

    let capped_streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("done"),
    )]]));
    let capped_session = memory_session();
    let mut capped = MultiTurnAgent::with_streamer(
        capped_streamer.clone(),
        "gemini::gemini-3.8-flash",
        Vec::new(),
        "",
        MultiTurnAgentConfig::default(),
    );
    let capped_config = HarnessConfig {
        effort: Some(wanted),
        effort_levels: Some(vec![
            EffortLevel::Low,
            EffortLevel::Medium,
            EffortLevel::High,
        ]),
        ..HarnessConfig::default()
    };
    capped.set_harness(harness(&capped_session, auto_mode(), capped_config));
    capped
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    {
        let cached = capped_streamer.options.lock().unwrap();
        assert!(matches!(
            cached[0].reasoning_effort,
            Some(genai::chat::ReasoningEffort::High)
        ));
    }

    let silent_streamer = Arc::new(FakeStreamer::new(vec![vec![ChatStreamEvent::End(
        text_end("done"),
    )]]));
    let silent_session = memory_session();
    let mut silent = MultiTurnAgent::with_streamer(
        silent_streamer.clone(),
        "openai::plain-model",
        Vec::new(),
        "",
        MultiTurnAgentConfig::default(),
    );
    let silent_config = HarnessConfig {
        effort: Some(wanted),
        effort_levels: Some(Vec::new()),
        ..HarnessConfig::default()
    };
    silent.set_harness(harness(&silent_session, auto_mode(), silent_config));
    silent
        .prompt_streaming("go", &tx, CancellationToken::new())
        .await
        .unwrap();
    let cached = silent_streamer.options.lock().unwrap();
    assert!(
        cached[0].reasoning_effort.is_none(),
        "a model without effort support receives no reasoning hint"
    );
}
