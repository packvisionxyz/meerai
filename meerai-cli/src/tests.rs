use meerai_agents::events::{ApprovalDecision, ApprovalKind, ApprovalRequest, RequestId};
use meerai_agents::{AgentCommand, AgentEvent};
use meerai_common::definitions::AgentMode;
use meerai_common::settings::{AppSettings, ContextSettings};
use meerai_core::EffectClass;
use meerai_core::env::now_millis;
use meerai_session::{SessionSummary, latest_session};
use tokio::sync::mpsc;

use super::cli::*;
use super::headless::*;
use super::sessions_cmd::*;
use super::*;

#[test]
fn parses_provider_model_and_system_flags() {
    let parsed = parse_args(vec![
        "-p".to_string(),
        "opencode-go".to_string(),
        "-m".to_string(),
        "glm-5.3".to_string(),
        "--system".to_string(),
        "Be concise.".to_string(),
    ])
    .unwrap();
    assert_eq!(parsed.provider.as_deref(), Some("opencode-go"));
    assert_eq!(parsed.model.as_deref(), Some("glm-5.3"));
    assert_eq!(parsed.system, "Be concise.");
    assert!(!parsed.show_help);

    let equals_parsed = parse_args(vec![
        "--provider=openrouter".to_string(),
        "--model=claude-sonnet-4-5".to_string(),
        "--system=Test".to_string(),
    ])
    .unwrap();
    assert_eq!(equals_parsed.provider.as_deref(), Some("openrouter"));
    assert_eq!(equals_parsed.model.as_deref(), Some("claude-sonnet-4-5"));
    assert_eq!(equals_parsed.system, "Test");

    assert!(parse_args(vec!["--unknown".to_string()]).is_err());
}

#[test]
fn parses_headless_and_mode_flags() {
    let prompt = parse_args(vec!["--prompt".to_string(), "hi".to_string()]).unwrap();
    assert_eq!(prompt.prompt.as_deref(), Some("hi"));
    assert!(!prompt.json);
    assert_eq!(prompt.provider, None);

    let print_alias = parse_args(vec!["--print=hello".to_string()]).unwrap();
    assert_eq!(print_alias.prompt.as_deref(), Some("hello"));

    let json = parse_args(vec![
        "--prompt".to_string(),
        "hi".to_string(),
        "--json".to_string(),
    ])
    .unwrap();
    assert!(json.json);

    assert!(parse_args(vec!["--json".to_string()]).is_err());

    let mode = parse_args(vec![
        "--prompt".to_string(),
        "hi".to_string(),
        "--mode=auto".to_string(),
    ])
    .unwrap();
    assert_eq!(mode.mode.as_deref(), Some("auto"));

    let provider = parse_args(vec!["-p".to_string(), "gemini".to_string()]).unwrap();
    assert_eq!(provider.provider.as_deref(), Some("gemini"));

    assert!(
        parse_args(vec![
            "auth".to_string(),
            "list".to_string(),
            "--prompt".to_string(),
            "hi".to_string(),
        ])
        .is_err()
    );
    assert!(parse_args(vec!["--mode".to_string()]).is_err());
    assert!(parse_args(vec!["--prompt".to_string()]).is_err());
}

#[test]
fn parses_version_flag() {
    let long = parse_args(vec!["--version".to_string()]).unwrap();
    assert!(long.version);
    assert!(!long.show_help);
    assert_eq!(long.prompt, None);

    let short = parse_args(vec!["-V".to_string()]).unwrap();
    assert!(short.version);

    let prompt = parse_args(vec!["--prompt".to_string(), "hi".to_string()]).unwrap();
    assert!(!prompt.version);
}

#[test]
fn parses_env_file_flag_and_reports_missing_path() {
    let spaced = parse_args(vec!["--env-file".to_string(), ".env.local".to_string()]).unwrap();
    assert_eq!(spaced.env_file.as_deref(), Some(".env.local"));

    let equals = parse_args(vec!["--env-file=/tmp/meerai.env".to_string()]).unwrap();
    assert_eq!(equals.env_file.as_deref(), Some("/tmp/meerai.env"));

    assert!(parse_args(vec!["--env-file".to_string()]).is_err());

    let missing = std::env::temp_dir().join("meerai-r1-missing-env-file.env");
    let error = load_dotenv(Some(missing.as_path())).unwrap_err();
    assert!(error.to_string().contains("env file not found"));
}

#[test]
fn parses_agent_and_skill_flags() {
    let parsed = parse_args(vec![
        "--agent".to_string(),
        "reviewer".to_string(),
        "--skill".to_string(),
        "explore".to_string(),
        "--skill=fix".to_string(),
        "--skill=deploy".to_string(),
    ])
    .unwrap();
    assert_eq!(parsed.agents, vec!["reviewer"]);
    assert_eq!(parsed.skills, vec!["explore", "fix", "deploy"]);

    assert!(parse_args(vec!["--agent".to_string()]).is_err());
    assert!(
        parse_args(vec![
            "--agent".to_string(),
            "a".to_string(),
            "--agent".to_string(),
            "b".to_string(),
        ])
        .is_err()
    );
    assert!(parse_args(vec!["--skill".to_string()]).is_err());
}

fn test_agent(id: &str, model: Option<&str>, steps: Option<usize>) -> AgentDefinition {
    AgentDefinition {
        id: id.to_string(),
        source: meerai_common::definitions::DefinitionSource::Agents,
        description: None,
        model: model.map(str::to_string),
        steps,
        mode: AgentMode::All,
        permissions: Vec::new(),
        prompt: format!("Prompt for {id}."),
        path: std::path::PathBuf::from(format!(".agents/agents/{id}.md")),
        token_estimate: 0,
        warnings: Vec::new(),
        supporting_files: Vec::new(),
    }
}

fn test_skill(id: &str) -> SkillDefinition {
    SkillDefinition {
        id: id.to_string(),
        source: meerai_common::definitions::DefinitionSource::Claude,
        description: Some(format!("Skill {id}")),
        model_invocable: true,
        body: format!("Body for {id}."),
        path: std::path::PathBuf::from(format!(".claude/skills/{id}/SKILL.md")),
        directory: std::path::PathBuf::from(format!(".claude/skills/{id}")),
        token_estimate: 0,
        warnings: Vec::new(),
        supporting_files: Vec::new(),
    }
}

#[test]
fn subagent_only_agents_are_not_selectable_as_primary() {
    let mut subagent = test_agent("implementer", None, None);
    subagent.mode = AgentMode::Subagent;
    let agents = vec![test_agent("reviewer", None, None), subagent];

    let found = select_agent_definition(&agents, "reviewer").unwrap();
    assert_eq!(found.id, "reviewer");

    let error = select_agent_definition(&agents, "implementer").unwrap_err();
    let message = error.to_string();
    assert!(message.contains("mode: subagent"));
    assert!(message.contains("subagent-run"));

    let unknown = select_agent_definition(&agents, "missing").unwrap_err();
    assert!(unknown.to_string().contains("available agents: reviewer"));
}

#[test]
fn unknown_definition_ids_error_with_available_ids() {
    let agents = vec![
        test_agent("reviewer", None, None),
        test_agent("writer", None, None),
    ];
    let found = select_definition("--agent", "agents", "writer", &agents, |agent| {
        agent.id.as_str()
    })
    .unwrap();
    assert_eq!(found.id, "writer");

    let error = select_definition("--agent", "agents", "missing", &agents, |agent| {
        agent.id.as_str()
    })
    .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("unknown --agent id 'missing'"));
    assert!(message.contains("reviewer, writer"));

    let skills = vec![test_skill("explore")];
    let skill_error = select_definition("--skill", "skills", "nope", &skills, |skill| {
        skill.id.as_str()
    })
    .unwrap_err();
    assert!(
        skill_error
            .to_string()
            .contains("available skills: explore")
    );

    let empty: Vec<AgentDefinition> = Vec::new();
    let empty_error = select_definition("--agent", "agents", "missing", &empty, |agent| {
        agent.id.as_str()
    })
    .unwrap_err();
    assert!(empty_error.to_string().contains("available agents: (none)"));
}

#[test]
fn agent_model_precedence_and_colon_warning() {
    assert_eq!(
        pick_model(
            Some("cli-model"),
            Some("env-model"),
            Some("agent-model"),
            Some("settings-model"),
        )
        .as_deref(),
        Some("cli-model")
    );
    assert_eq!(
        pick_model(None, None, Some("agent-model"), Some("settings-model")).as_deref(),
        Some("agent-model")
    );
    assert_eq!(
        pick_model(None, Some("env-model"), Some("agent-model"), None).as_deref(),
        Some("env-model")
    );
    assert_eq!(
        pick_model(None, None, None, Some("settings-model")).as_deref(),
        Some("settings-model")
    );
    assert_eq!(pick_model(None, None, None, None), None);

    assert_eq!(
        agent_model_candidate(Some("gemini-3.1-pro")),
        (Some("gemini-3.1-pro".to_string()), None)
    );
    let (candidate, warning) = agent_model_candidate(Some("openrouter:deepseek-v4.1"));
    assert_eq!(candidate, None);
    assert!(warning.unwrap().contains("implies a provider change"));
    assert_eq!(agent_model_candidate(Some("   ")), (None, None));
    assert_eq!(agent_model_candidate(None), (None, None));
}

#[test]
fn composes_skill_then_agent_instructions() {
    let first = test_skill("explore");
    let second = test_skill("fix");
    let agent = test_agent("reviewer", None, None);

    let composed =
        compose_definition_prompt("BASE".to_string(), &[&first, &second], Some(&agent), &[]);
    assert_eq!(
        composed,
        "BASE\n\n## Skill instructions (explore)\n\nBody for explore.\n\n\
             ## Skill instructions (fix)\n\nBody for fix.\n\n\
             ## Agent instructions (reviewer)\n\nPrompt for reviewer."
    );

    assert_eq!(
        compose_definition_prompt("BASE".to_string(), &[], None, &[]),
        "BASE"
    );
}

#[test]
fn adds_a_skill_index_for_advertised_skills() {
    let skill = test_skill("explore");

    let composed = compose_definition_prompt("BASE".to_string(), &[], None, &[skill]);

    assert!(composed.starts_with("BASE\n\n## Skills\n\n"));
    assert!(composed.contains("- explore: Skill explore"));
    assert!(composed.contains("skill-load"));
    assert!(!composed.contains("Body for explore."));
}

#[test]
fn hidden_skills_stay_out_of_the_index() {
    let mut skill = test_skill("seed");
    skill.model_invocable = false;

    let composed = compose_definition_prompt("BASE".to_string(), &[], None, &[skill]);

    assert_eq!(composed, "BASE");
}

#[test]
fn discovers_and_indexes_a_project_skill() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "meerai_skill_index_{}_{}",
        std::process::id(),
        nanos
    ));
    let skill_dir = dir.join(".agents/skills/review");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: review\ndescription: Review a diff for regressions\n---\nCheck the diff.\n",
    )
    .unwrap();
    std::fs::write(skill_dir.join("notes.md"), "Details").unwrap();

    let catalog = meerai_common::definitions::load_definitions(&dir);
    let composed = compose_definition_prompt("BASE".to_string(), &[], None, &catalog.skills);

    assert!(composed.contains("- review: Review a diff for regressions"));
    assert!(!composed.contains("Check the diff."));
    assert_eq!(
        catalog.skills[0].supporting_files,
        vec!["notes.md".to_string()]
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn caps_large_definition_instructions() {
    let mut huge = test_skill("huge");
    huge.body = "x".repeat(20_000);
    let composed = compose_definition_prompt("BASE".to_string(), &[&huge], None, &[]);
    assert!(composed.len() < 17 * 1024);
    assert!(composed.contains("definition truncated"));

    let mut first = test_skill("first");
    first.body = "a".repeat(16_400);
    let mut second = test_skill("second");
    second.body = "b".repeat(16_400);
    let composed = compose_definition_prompt("BASE".to_string(), &[&first, &second], None, &[]);
    assert!(composed.len() < 33 * 1024);
    assert!(composed.contains("definition block truncated"));
}

#[test]
fn prune_message_names_count_and_window() {
    assert_eq!(
        format_prune_message(3, 30),
        "Pruned 3 sessions (older than 30 days)"
    );
    assert_eq!(
        format_prune_message(1, 7),
        "Pruned 1 sessions (older than 7 days)"
    );
}

#[test]
fn help_text_lists_agent_and_skill_surfaces() {
    for needle in [
        "--agent <ID>",
        "--skill <ID>",
        "/agent <id>",
        "/skill <id>",
        "@agent/<id>",
        "@skill/<id>",
    ] {
        assert!(help_text().contains(needle), "help is missing {needle}");
    }
    for removed in ["/agents", "/skills"] {
        assert!(!help_text().contains(removed), "help still lists {removed}");
    }
}

#[test]
fn dotenv_loads_only_when_explicitly_enabled() {
    assert!(!dotenv_flag_enabled(None));
    for value in ["1", "true", "TRUE", "True", "yes", " Yes "] {
        assert!(dotenv_flag_enabled(Some(value)), "{value} should opt in");
    }
    for value in ["", "0", "no", "false", "off"] {
        assert!(
            !dotenv_flag_enabled(Some(value)),
            "{value} should not opt in"
        );
    }

    let path = std::env::temp_dir().join("meerai-r1-dotenv-probe.env");
    std::fs::write(&path, "MEERAI_R1_DOTENV_PROBE=loaded\n").unwrap();
    load_dotenv(Some(path.as_path())).unwrap();
    assert_eq!(
        std::env::var("MEERAI_R1_DOTENV_PROBE").as_deref(),
        Ok("loaded")
    );
    let _ = std::fs::remove_file(&path);
}

fn sample_approval_event() -> AgentEvent {
    use meerai_agents::TurnId;
    use meerai_agents::events::ApprovalPreview;
    use meerai_core::EffectClass;

    AgentEvent::ApprovalRequested {
        request: ApprovalRequest {
            request_id: RequestId::from_raw("req-headless"),
            turn_id: TurnId::from_raw("turn-headless"),
            kind: ApprovalKind::ToolCall,
            preview: ApprovalPreview {
                title: "edit: src/main.rs".to_string(),
                detail: "tool: code-edit_file\neffect: write".to_string(),
            },
            effect: Some(EffectClass::Write),
        },
    }
}

fn sample_spend_approval_event() -> AgentEvent {
    use meerai_agents::TurnId;
    use meerai_agents::events::ApprovalPreview;

    AgentEvent::ApprovalRequested {
        request: ApprovalRequest {
            request_id: RequestId::from_raw("req-spend"),
            turn_id: TurnId::from_raw("turn-headless"),
            kind: ApprovalKind::Spend,
            preview: ApprovalPreview {
                title: "Session token budget reached".to_string(),
                detail: "session used 1200 of 1000 tokens".to_string(),
            },
            effect: None,
        },
    }
}

fn approval_request(event: &AgentEvent) -> &ApprovalRequest {
    match event {
        AgentEvent::ApprovalRequested { request } => request,
        other => panic!("expected approval request, got {other:?}"),
    }
}

#[test]
fn headless_tool_call_approval_cancels_and_exits_2() {
    let event = sample_approval_event();
    assert_eq!(headless_outcome(&event), HeadlessOutcome::ApprovalRequired);

    let action = HeadlessApprovalAction::for_request(approval_request(&event));
    assert_eq!(action, HeadlessApprovalAction::Cancel);
    assert_eq!(action.command(), AgentCommand::Cancel);

    let mut exit = HeadlessExit::new();
    exit.observe(headless_outcome(&event));
    assert_eq!(exit.code, EXIT_APPROVAL_REQUIRED);
    assert!(exit.approval_cancelled);
}

#[test]
fn headless_spend_approval_declines_and_exits_1() {
    let event = sample_spend_approval_event();
    assert_eq!(headless_outcome(&event), HeadlessOutcome::SpendDeclined);

    let action = HeadlessApprovalAction::for_request(approval_request(&event));
    assert_eq!(
        action,
        HeadlessApprovalAction::Decline {
            request_id: RequestId::from_raw("req-spend"),
        }
    );
    assert_eq!(
        action.command(),
        AgentCommand::Approve {
            request_id: RequestId::from_raw("req-spend"),
            decision: ApprovalDecision::Decline,
            scope: None,
        }
    );

    let mut exit = HeadlessExit::new();
    exit.observe(headless_outcome(&event));
    assert_eq!(exit.code, 1);

    use meerai_agents::{TurnEndReason, TurnId};
    exit.observe(headless_outcome(&AgentEvent::TurnCompleted {
        turn_id: TurnId::from_raw("turn-headless"),
        reason: TurnEndReason::Completed,
    }));
    assert_eq!(exit.code, 1);
}

#[test]
fn headless_exit_codes_follow_turn_outcome() {
    use meerai_agents::{AbortCause, TurnEndReason, TurnId};

    let completed = AgentEvent::TurnCompleted {
        turn_id: TurnId::from_raw("turn-headless"),
        reason: TurnEndReason::Completed,
    };
    let failed = AgentEvent::TurnCompleted {
        turn_id: TurnId::from_raw("turn-headless"),
        reason: TurnEndReason::Failed {
            error: "boom".to_string(),
        },
    };
    let aborted = AgentEvent::TurnCompleted {
        turn_id: TurnId::from_raw("turn-headless"),
        reason: TurnEndReason::Aborted {
            cause: AbortCause::User,
        },
    };

    let mut exit = HeadlessExit::new();
    exit.observe(headless_outcome(&completed));
    assert_eq!(exit.code, 0);

    let mut exit = HeadlessExit::new();
    exit.observe(headless_outcome(&failed));
    assert_eq!(exit.code, 1);

    let mut exit = HeadlessExit::new();
    exit.observe(headless_outcome(&aborted));
    assert_eq!(exit.code, 1);

    let mut exit = HeadlessExit::new();
    exit.observe(headless_outcome(&AgentEvent::Error {
        code: meerai_agents::ErrorCode::Provider,
        message: "provider down".to_string(),
    }));
    assert_eq!(exit.code, 1);

    let mut exit = HeadlessExit::new();
    exit.observe(headless_outcome(&sample_approval_event()));
    exit.observe(headless_outcome(&aborted));
    exit.observe(headless_outcome(&completed));
    exit.mark_failed();
    assert_eq!(exit.code, EXIT_APPROVAL_REQUIRED);
}

#[test]
fn resolves_approval_mode_from_flag_then_settings_then_default() {
    assert_eq!(
        resolve_approval_mode(None, None).unwrap(),
        ApprovalMode::Ask
    );
    assert_eq!(
        resolve_approval_mode(Some("plan"), Some("auto")).unwrap(),
        ApprovalMode::Plan
    );
    assert_eq!(
        resolve_approval_mode(None, Some("auto")).unwrap(),
        ApprovalMode::Auto
    );
    assert_eq!(
        resolve_approval_mode(None, Some("AUTO")).unwrap(),
        ApprovalMode::Auto
    );

    let error = resolve_approval_mode(Some("wild"), None).unwrap_err();
    assert!(error.to_string().contains("invalid --mode value"));
    let settings_error = resolve_approval_mode(None, Some("wild")).unwrap_err();
    assert!(settings_error.to_string().contains("settings.yml"));
}

#[test]
fn harness_config_uses_defaults_and_context_overrides() {
    let defaults = HarnessConfig::from_context_settings(None);
    assert_eq!(defaults, HarnessConfig::default());

    let context = ContextSettings {
        context_window_tokens: Some(200_000),
        high_watermark_pct: Some(80),
        spill_max_inline_tokens: Some(8_000),
        verbatim_window_turns: Some(6),
        reasoning_pruning: Some(false),
        prompt_caching: Some(false),
        rollback_on_verify_failure: Some(true),
        max_steps_per_turn: Some(48),
        no_progress_fail_steps: Some(24),
        tool_timeout_ms: Some(60_000),
        max_session_tokens: Some(2_000_000),
        max_turn_secs: Some(600),
        summarizer_model: Some("openrouter:small-model".to_string()),
        recovery_model: Some("openrouter:fallback-model".to_string()),
        step_limit_continuations: None,
        post_edit_diagnostics: None,
        auto_memory: None,
        memory_prompt_bytes: None,
        temperature: None,
        effort: None,
    };
    let config = HarnessConfig::from_context_settings(Some(&context));
    assert_eq!(config.context_window_tokens, 200_000);
    assert_eq!(config.high_watermark_pct, 80);
    assert_eq!(config.spill_max_inline_tokens, 8_000);
    assert_eq!(config.verbatim_window_turns, 6);
    assert!(!config.reasoning_pruning);
    assert!(!config.prompt_caching);
    assert!(config.rollback_on_verify_failure);
    assert_eq!(
        config.recovery_model.as_deref(),
        Some("openrouter:fallback-model")
    );
    assert_eq!(config.max_steps_per_turn, 48);
    assert_eq!(config.tool_timeout_ms, 60_000);
    assert_eq!(config.max_session_tokens, Some(2_000_000));
}

#[test]
fn agent_events_serialize_to_single_line_ndjson_and_roundtrip() {
    use meerai_agents::events::{ApprovalKind, ApprovalPreview, ApprovalRequest};
    use meerai_agents::{CallId, ItemId, ItemStatus, SessionItem, TurnEndReason, TurnId};
    use meerai_core::EffectClass;

    let turn_id = TurnId::from_raw("turn-ndjson");
    let events = vec![
        AgentEvent::TurnStarted {
            turn_id: turn_id.clone(),
            model: "gemini::gemini-3.8-flash".to_string(),
        },
        AgentEvent::ItemStarted {
            item: SessionItem::ToolCall {
                id: ItemId::from_raw("item-ndjson"),
                call_id: CallId::from_raw("call-ndjson"),
                name: "code-write_file".to_string(),
                args: "{\"path\":\"src/lib.rs\"}".to_string(),
                effect: EffectClass::Write,
                status: ItemStatus::Proposed,
                output: None,
            },
        },
        AgentEvent::ApprovalRequested {
            request: ApprovalRequest {
                request_id: meerai_agents::events::RequestId::from_raw("req-ndjson"),
                turn_id: turn_id.clone(),
                kind: ApprovalKind::ToolCall,
                preview: ApprovalPreview {
                    title: "edit: src/lib.rs".to_string(),
                    detail: "tool: code-write_file\neffect: write".to_string(),
                },
                effect: Some(EffectClass::Write),
            },
        },
        AgentEvent::TurnCompleted {
            turn_id: turn_id.clone(),
            reason: TurnEndReason::Completed,
        },
    ];

    for event in events {
        let line = serde_json::to_string(&event).unwrap();
        assert!(!line.contains('\n'));
        assert_eq!(serde_json::from_str::<AgentEvent>(&line).unwrap(), event);
    }
}

#[test]
fn hook_completed_serializes_to_ndjson_and_keeps_headless_running() {
    let event = AgentEvent::HookCompleted {
        event: "PreToolUse".to_string(),
        decision: "deny".to_string(),
        reason: Some("blocked".to_string()),
    };

    let line = serde_json::to_string(&event).unwrap();
    assert!(!line.contains('\n'));
    assert!(line.contains("\"type\":\"hookCompleted\""));
    assert!(line.contains("\"event\":\"PreToolUse\""));
    assert!(line.contains("\"decision\":\"deny\""));
    assert!(line.contains("\"reason\":\"blocked\""));
    assert_eq!(headless_outcome(&event), HeadlessOutcome::Continue);
}

#[test]
fn parses_auth_subcommands() {
    let list = parse_args(vec!["auth".to_string(), "list".to_string()]).unwrap();
    assert_eq!(
        list.subcommand,
        Some(CliSubcommand::Auth(AuthCommand::List))
    );

    let ls = parse_args(vec!["auth".to_string(), "ls".to_string()]).unwrap();
    assert_eq!(ls.subcommand, Some(CliSubcommand::Auth(AuthCommand::List)));

    let default_auth = parse_args(vec!["auth".to_string()]).unwrap();
    assert_eq!(
        default_auth.subcommand,
        Some(CliSubcommand::Auth(AuthCommand::List))
    );

    let login = parse_args(vec![
        "auth".to_string(),
        "login".to_string(),
        "openai".to_string(),
        "sk-test".to_string(),
    ])
    .unwrap();
    assert_eq!(
        login.subcommand,
        Some(CliSubcommand::Auth(AuthCommand::Login {
            provider: "openai".to_string(),
            key: Some("sk-test".to_string()),
            chatgpt: false,
        }))
    );

    let login_prompt = parse_args(vec![
        "auth".to_string(),
        "login".to_string(),
        "anthropic".to_string(),
    ])
    .unwrap();
    assert_eq!(
        login_prompt.subcommand,
        Some(CliSubcommand::Auth(AuthCommand::Login {
            provider: "anthropic".to_string(),
            key: None,
            chatgpt: false,
        }))
    );

    let login_chatgpt = parse_args(vec![
        "auth".to_string(),
        "login".to_string(),
        "openai".to_string(),
        "--chatgpt".to_string(),
    ])
    .unwrap();
    assert_eq!(
        login_chatgpt.subcommand,
        Some(CliSubcommand::Auth(AuthCommand::Login {
            provider: "openai".to_string(),
            key: None,
            chatgpt: true,
        }))
    );

    assert!(
        parse_args(vec![
            "auth".to_string(),
            "login".to_string(),
            "openai".to_string(),
            "sk-test".to_string(),
            "--chatgpt".to_string(),
        ])
        .is_err()
    );

    let logout = parse_args(vec![
        "auth".to_string(),
        "logout".to_string(),
        "openai".to_string(),
    ])
    .unwrap();
    assert_eq!(
        logout.subcommand,
        Some(CliSubcommand::Auth(AuthCommand::Logout {
            provider: "openai".to_string(),
        }))
    );

    assert!(parse_args(vec!["auth".to_string(), "bad".to_string()]).is_err());
    assert!(parse_args(vec!["auth".to_string(), "login".to_string()]).is_err());
    assert!(parse_args(vec!["auth".to_string(), "logout".to_string()]).is_err());
}

#[test]
fn parses_sessions_subcommands() {
    let sessions = |args: &[&str]| -> Vec<String> {
        std::iter::once("sessions".to_string())
            .chain(args.iter().map(|arg| arg.to_string()))
            .collect()
    };

    assert_eq!(
        parse_args(sessions(&["list"])).unwrap().subcommand,
        Some(CliSubcommand::Sessions(SessionsCommand::List))
    );
    assert_eq!(
        parse_args(sessions(&[])).unwrap().subcommand,
        Some(CliSubcommand::Sessions(SessionsCommand::List))
    );
    assert_eq!(
        parse_args(sessions(&["rm", "abc"])).unwrap().subcommand,
        Some(CliSubcommand::Sessions(SessionsCommand::Remove {
            target: SessionTarget::Id("abc".to_string()),
        }))
    );
    assert_eq!(
        parse_args(sessions(&["rm", "--all"])).unwrap().subcommand,
        Some(CliSubcommand::Sessions(SessionsCommand::Remove {
            target: SessionTarget::All,
        }))
    );

    let missing = parse_args(sessions(&["rm"])).unwrap_err();
    assert!(missing.to_string().contains("session id or --all"));

    let both = parse_args(sessions(&["rm", "abc", "--all"])).unwrap_err();
    assert!(both.to_string().contains("not both"));

    assert!(parse_args(sessions(&["rm", "abc", "def"])).is_err());
    assert!(parse_args(sessions(&["rm", "--bogus"])).is_err());
    assert!(parse_args(sessions(&["list", "extra"])).is_err());

    let unknown = parse_args(sessions(&["bad"])).unwrap_err();
    assert!(unknown.to_string().contains("list, rm"));
}

#[test]
fn parses_continue_and_resume_flags() {
    let resume = parse_args(vec![
        "--resume".to_string(),
        "ses-abc".to_string(),
        "--prompt".to_string(),
        "hi".to_string(),
    ])
    .unwrap();
    assert_eq!(resume.resume.as_deref(), Some("ses-abc"));
    assert!(!resume.continue_session);

    let equals = parse_args(vec![
        "--resume=ses-xyz".to_string(),
        "--prompt=hi".to_string(),
    ])
    .unwrap();
    assert_eq!(equals.resume.as_deref(), Some("ses-xyz"));

    let continued = parse_args(vec![
        "--continue".to_string(),
        "--prompt".to_string(),
        "hi".to_string(),
    ])
    .unwrap();
    assert!(continued.continue_session);
    assert_eq!(continued.resume, None);

    let tui_continue = parse_args(vec!["--continue".to_string()]).unwrap();
    assert!(tui_continue.continue_session);
    assert_eq!(tui_continue.prompt, None);

    let tui_resume = parse_args(vec!["--resume".to_string(), "ses-abc".to_string()]).unwrap();
    assert_eq!(tui_resume.resume.as_deref(), Some("ses-abc"));
    assert_eq!(tui_resume.prompt, None);

    let conflict = parse_args(vec![
        "--continue".to_string(),
        "--resume".to_string(),
        "ses-abc".to_string(),
        "--prompt".to_string(),
        "hi".to_string(),
    ])
    .unwrap_err();
    assert!(conflict.to_string().contains("cannot be combined"));

    let conflict_tui = parse_args(vec![
        "--continue".to_string(),
        "--resume".to_string(),
        "ses-abc".to_string(),
    ])
    .unwrap_err();
    assert!(conflict_tui.to_string().contains("cannot be combined"));

    assert!(parse_args(vec!["--resume".to_string()]).is_err());
}

#[test]
fn session_rows_prefer_title_then_model() {
    let base = SessionSummary {
        id: SessionId::from_raw("ses-1"),
        dir: std::path::PathBuf::from("/tmp/ses-1"),
        title: Some("Fix build".to_string()),
        model: Some("glm-5.3".to_string()),
        created_at_ms: 1,
        updated_at_ms: 2,
        bytes: 3,
        parent_session_id: None,
    };
    assert_eq!(format_session_row(&base), "ses-1\t2\t3\tFix build");

    let model_only = SessionSummary {
        title: None,
        ..base.clone()
    };
    assert_eq!(format_session_row(&model_only), "ses-1\t2\t3\tglm-5.3");

    let empty = SessionSummary {
        title: Some(String::new()),
        model: None,
        ..base
    };
    assert_eq!(format_session_row(&empty), "ses-1\t2\t3\t");
}

#[tokio::test]
async fn continue_with_empty_session_root_reports_no_sessions() {
    let root = std::env::temp_dir().join(format!("meerai-r2b-continue-{}", SessionId::new()));
    std::fs::create_dir_all(&root).unwrap();
    assert!(latest_session(&root).unwrap().is_none());
    let error = continue_session_id(&root).await.unwrap_err();
    assert!(error.to_string().contains("no sessions found to continue"));
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn resume_missing_session_errors_without_creating_it() {
    let root = std::env::temp_dir().join(format!("meerai-r2b-resume-{}", SessionId::new()));
    let config = SessionConfig {
        root: root.clone(),
        workspace: root.clone(),
    };
    let id = SessionId::new();
    let error = Session::open(&config, id.clone(), root.clone())
        .await
        .err()
        .expect("missing session must not open");
    assert!(error.to_string().contains("session not found"));
    assert!(!root.join(id.as_str()).exists());
    std::fs::remove_dir_all(&root).ok();
}

fn session_summary(id: &str, updated_at_ms: u64) -> SessionSummary {
    SessionSummary {
        id: SessionId::from_raw(id),
        dir: std::path::PathBuf::from(format!("/tmp/{id}")),
        title: None,
        model: None,
        created_at_ms: updated_at_ms,
        updated_at_ms,
        bytes: 0,
        parent_session_id: None,
    }
}

#[test]
fn parses_sessions_prune_flags() {
    let sessions = |args: &[&str]| -> Vec<String> {
        std::iter::once("sessions".to_string())
            .chain(args.iter().map(|arg| arg.to_string()))
            .collect()
    };

    assert_eq!(
        parse_args(sessions(&["prune"])).unwrap().subcommand,
        Some(CliSubcommand::Sessions(SessionsCommand::Prune {
            days: 30,
            keep: 0,
        }))
    );
    assert_eq!(
        parse_args(sessions(&["prune", "--days", "7", "--keep", "3"]))
            .unwrap()
            .subcommand,
        Some(CliSubcommand::Sessions(SessionsCommand::Prune {
            days: 7,
            keep: 3,
        }))
    );
    assert_eq!(
        parse_args(sessions(&["prune", "--days=14", "--keep=2"]))
            .unwrap()
            .subcommand,
        Some(CliSubcommand::Sessions(SessionsCommand::Prune {
            days: 14,
            keep: 2,
        }))
    );

    assert!(parse_args(sessions(&["prune", "--days"])).is_err());
    assert!(parse_args(sessions(&["prune", "--days", "soon"])).is_err());
    assert!(parse_args(sessions(&["prune", "--keep", "-1"])).is_err());
    assert!(parse_args(sessions(&["prune", "extra"])).is_err());
    assert!(parse_args(sessions(&["prune", "--bogus"])).is_err());
}

#[test]
fn select_prunable_uses_cutoff_and_keeps_newest_sessions() {
    let day = 86_400_000;
    let now = 1_000 * day;
    let sessions = vec![
        session_summary("ses-old", now - 40 * day),
        session_summary("ses-mid", now - 31 * day),
        session_summary("ses-edge", now - 30 * day),
        session_summary("ses-new", now - day),
    ];

    assert_eq!(
        select_prunable(&sessions, now, 30, 0, &[]),
        vec![
            SessionId::from_raw("ses-mid"),
            SessionId::from_raw("ses-old")
        ]
    );
    assert_eq!(
        select_prunable(&sessions, now, 30, 2, &[]),
        vec![
            SessionId::from_raw("ses-mid"),
            SessionId::from_raw("ses-old")
        ]
    );
    assert_eq!(
        select_prunable(&sessions, now, 30, 3, &[]),
        vec![SessionId::from_raw("ses-old")]
    );
    assert!(select_prunable(&sessions, now, 30, 4, &[]).is_empty());
}

#[test]
fn select_prunable_never_selects_the_excluded_session() {
    let day = 86_400_000;
    let now = 1_000 * day;
    let sessions = vec![
        session_summary("ses-old", now - 40 * day),
        session_summary("ses-mid", now - 31 * day),
        session_summary("ses-new", now - day),
    ];

    let excluded = SessionId::from_raw("ses-old");
    assert_eq!(
        select_prunable(&sessions, now, 30, 0, std::slice::from_ref(&excluded)),
        vec![SessionId::from_raw("ses-mid")]
    );

    let active_but_recent = SessionId::from_raw("ses-new");
    assert_eq!(
        select_prunable(
            &sessions,
            now,
            30,
            0,
            std::slice::from_ref(&active_but_recent)
        ),
        vec![
            SessionId::from_raw("ses-mid"),
            SessionId::from_raw("ses-old")
        ]
    );
}

fn write_fake_session(root: &Path, id: &str, updated_at_ms: u64) {
    let store = meerai_store::Store::open(root.join("meerai.db")).unwrap();
    store
        .immediate(|tx| {
            tx.execute(
                "INSERT INTO sessions (id, project_root, cwd, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    id,
                    root.to_string_lossy(),
                    root.to_string_lossy(),
                    updated_at_ms as i64,
                    updated_at_ms as i64
                ],
            )?;
            Ok(())
        })
        .unwrap();
    std::fs::create_dir_all(root.join(id)).unwrap();
}

#[test]
fn auto_prune_is_opt_in_via_settings() {
    let manual: AppSettings = serde_yaml::from_str("active_provider: openrouter\n").unwrap();
    assert_eq!(auto_prune_config(&manual), None);

    let keep_only: AppSettings = serde_yaml::from_str("sessions:\n  keep: 3\n").unwrap();
    assert_eq!(auto_prune_config(&keep_only), None);

    let enabled: AppSettings = serde_yaml::from_str("sessions:\n  auto_prune_days: 7\n").unwrap();
    assert_eq!(auto_prune_config(&enabled), Some((7, 0)));

    let with_keep: AppSettings =
        serde_yaml::from_str("sessions:\n  auto_prune_days: 14\n  keep: 2\n").unwrap();
    assert_eq!(auto_prune_config(&with_keep), Some((14, 2)));
}

#[tokio::test]
async fn prune_old_sessions_respects_days_keep_and_exclusion() {
    let day: u64 = 86_400_000;
    let now = now_millis();
    let root = std::env::temp_dir().join(format!("meerai-cfg1-prune-{}", SessionId::new()));
    std::fs::create_dir_all(&root).unwrap();
    write_fake_session(&root, "ses-oldest", now.saturating_sub(40 * day));
    write_fake_session(&root, "ses-old", now.saturating_sub(31 * day));
    write_fake_session(&root, "ses-recent", now.saturating_sub(29 * day));
    write_fake_session(&root, "ses-new", now.saturating_sub(day));

    let excluded = SessionId::from_raw("ses-old");
    let count = prune_old_sessions(root.clone(), 30, 0, std::slice::from_ref(&excluded))
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert!(!root.join("ses-oldest").exists());
    assert!(root.join("ses-old").exists(), "excluded session must stay");
    assert!(root.join("ses-recent").exists());
    assert!(root.join("ses-new").exists());

    let none = SessionId::from_raw("ses-none");
    let count = prune_old_sessions(root.clone(), 30, 3, std::slice::from_ref(&none))
        .await
        .unwrap();
    assert_eq!(count, 0);

    let count = prune_old_sessions(root.clone(), 30, 0, std::slice::from_ref(&none))
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert!(!root.join("ses-old").exists());
    assert!(root.join("ses-recent").exists());
    assert!(root.join("ses-new").exists());

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn prune_old_sessions_missing_root_is_not_an_error() {
    let root = std::env::temp_dir().join(format!("meerai-cfg1-missing-{}", SessionId::new()));
    let count = prune_old_sessions(root, 7, 0, &[]).await.unwrap();
    assert_eq!(count, 0);
}

#[test]
fn formats_token_summary_and_omits_missing_fields() {
    assert_eq!(format_token_summary(&meerai_agents::Usage::default()), None);

    let full = meerai_agents::Usage {
        prompt_tokens: Some(10),
        completion_tokens: Some(5),
        total_tokens: Some(15),
        cached_tokens: Some(6),
        cache_creation_tokens: None,
    };
    assert_eq!(
        format_token_summary(&full).as_deref(),
        Some("tokens: prompt=10 completion=5 total=15 cached=6")
    );

    let partial = meerai_agents::Usage {
        prompt_tokens: None,
        completion_tokens: Some(4),
        total_tokens: None,
        cached_tokens: None,
        cache_creation_tokens: None,
    };
    assert_eq!(
        format_token_summary(&partial).as_deref(),
        Some("tokens: completion=4")
    );
}

#[test]
fn parses_sessions_fork_and_top_level_fork_flags() {
    let sessions = |args: &[&str]| -> Vec<String> {
        std::iter::once("sessions".to_string())
            .chain(args.iter().map(|arg| arg.to_string()))
            .collect()
    };

    assert_eq!(
        parse_args(sessions(&["fork", "ses-123"]))
            .unwrap()
            .subcommand,
        Some(CliSubcommand::Sessions(SessionsCommand::Fork {
            id: "ses-123".to_string(),
            turn: None,
        }))
    );
    assert_eq!(
        parse_args(sessions(&["fork", "ses-123", "--turn", "turn-2"]))
            .unwrap()
            .subcommand,
        Some(CliSubcommand::Sessions(SessionsCommand::Fork {
            id: "ses-123".to_string(),
            turn: Some("turn-2".to_string()),
        }))
    );
    assert_eq!(
        parse_args(sessions(&["fork", "ses-123", "--turn=turn-3"]))
            .unwrap()
            .subcommand,
        Some(CliSubcommand::Sessions(SessionsCommand::Fork {
            id: "ses-123".to_string(),
            turn: Some("turn-3".to_string()),
        }))
    );
    assert!(parse_args(sessions(&["fork", "--help"])).unwrap().show_help);
    assert!(parse_args(sessions(&["fork"])).is_err());
    assert!(parse_args(sessions(&["fork", "ses-123", "--turn"])).is_err());
    assert!(parse_args(sessions(&["fork", "ses-123", "--bogus"])).is_err());
    assert!(parse_args(sessions(&["fork", "ses-123", "extra"])).is_err());

    let spaced = parse_args(vec!["--fork".to_string(), "ses-abc".to_string()]).unwrap();
    assert_eq!(spaced.fork.as_deref(), Some("ses-abc"));
    assert!(!spaced.continue_session);
    assert_eq!(spaced.resume, None);

    let equals = parse_args(vec!["--fork=ses-xyz".to_string()]).unwrap();
    assert_eq!(equals.fork.as_deref(), Some("ses-xyz"));

    assert!(parse_args(vec!["--fork".to_string()]).is_err());

    let fork_continue = parse_args(vec![
        "--fork".to_string(),
        "ses-abc".to_string(),
        "--continue".to_string(),
    ])
    .unwrap_err();
    assert!(fork_continue.to_string().contains("cannot be combined"));

    let fork_resume = parse_args(vec![
        "--fork=ses-abc".to_string(),
        "--resume".to_string(),
        "ses-def".to_string(),
    ])
    .unwrap_err();
    assert!(fork_resume.to_string().contains("cannot be combined"));

    assert!(help_text().contains("sessions fork <ID> [--turn <TURN_ID>]"));
    assert!(help_text().contains("--fork <ID>"));
}

#[tokio::test]
async fn fork_session_helper_forks_latest_or_explicit_turn_and_rejects_missing() {
    use meerai_session::{
        ContentBlock, InputSource, SessionEvent, TurnEndReason, TurnId, TurnTrigger,
    };

    let root = std::env::temp_dir().join(format!("meerai-m9-fork-{}", SessionId::new()));
    let config = SessionConfig {
        root: root.clone(),
        workspace: root.clone(),
    };

    let empty_id = SessionId::new();
    let _empty = Session::open_or_create(&config, empty_id.clone(), root.clone())
        .await
        .unwrap();
    let no_closed = fork_session(&root, &root, empty_id.as_str(), None)
        .await
        .err()
        .expect("empty session cannot fork");
    assert!(no_closed.to_string().contains("no closed turns"));

    let source_id = SessionId::new();
    let source = Session::open_or_create(&config, source_id.clone(), root.clone())
        .await
        .unwrap();
    let turn_1 = TurnId::from_raw("turn-1");
    let turn_2 = TurnId::from_raw("turn-2");

    source
        .append(SessionEvent::TurnStarted {
            turn_id: turn_1.clone(),
            trigger: TurnTrigger::UserPrompt,
        })
        .await
        .unwrap();
    source
        .append(SessionEvent::UserMessage {
            source: InputSource::User,
            content: vec![ContentBlock::Text("first".to_string())],
        })
        .await
        .unwrap();
    source
        .append(SessionEvent::TurnEnded {
            turn_id: turn_1.clone(),
            reason: TurnEndReason::Completed,
        })
        .await
        .unwrap();

    source
        .append(SessionEvent::TurnStarted {
            turn_id: turn_2.clone(),
            trigger: TurnTrigger::UserPrompt,
        })
        .await
        .unwrap();
    source
        .append(SessionEvent::UserMessage {
            source: InputSource::User,
            content: vec![ContentBlock::Text("second".to_string())],
        })
        .await
        .unwrap();
    source
        .append(SessionEvent::TurnEnded {
            turn_id: turn_2.clone(),
            reason: TurnEndReason::Completed,
        })
        .await
        .unwrap();

    let latest = fork_session(&root, &root, source_id.as_str(), None)
        .await
        .unwrap();
    assert_ne!(latest.id(), &source_id);
    assert_eq!(latest.meta().forked_from.as_ref(), Some(&source_id));
    assert_eq!(latest.meta().inherited_event_count, Some(6));
    assert_eq!(latest.events().len(), 6);

    let explicit = fork_session(&root, &root, source_id.as_str(), Some("turn-1"))
        .await
        .unwrap();
    assert_ne!(explicit.id(), &source_id);
    assert_ne!(explicit.id(), latest.id());
    assert_eq!(explicit.meta().forked_from.as_ref(), Some(&source_id));
    assert_eq!(explicit.meta().inherited_event_count, Some(3));
    assert_eq!(explicit.events().len(), 3);

    let missing_turn = fork_session(&root, &root, source_id.as_str(), Some("turn-missing"))
        .await
        .err()
        .expect("unknown turn cannot fork");
    assert!(missing_turn.to_string().contains("unknown turn"));

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn plain_event_printer_renders_subagent_run_lifecycle() {
    use meerai_agents::{ItemDelta, ItemId, ItemStatus, SessionItem};

    let mut printer = PlainEventPrinter::new();
    let mut out = Vec::new();
    let item_id = ItemId::from_raw("item-subagent");

    printer
        .handle(
            &AgentEvent::ItemDelta {
                item_id: ItemId::from_raw("item-msg"),
                delta: ItemDelta::AgentText("Working".to_string()),
            },
            &mut out,
        )
        .unwrap();
    printer
        .handle(
            &AgentEvent::ItemStarted {
                item: SessionItem::SubagentRun {
                    id: item_id.clone(),
                    name: None,
                    agent: "explore".to_string(),
                    status: ItemStatus::InProgress,
                    report: None,
                    child_session_id: None,
                    model: None,
                    approval_mode: meerai_policy::ApprovalMode::Ask,
                    steps_used: 0,
                    max_steps: 0,
                    usage: Default::default(),
                    est_cost_usd: None,
                    active_tool: None,
                },
            },
            &mut out,
        )
        .unwrap();
    printer
        .handle(
            &AgentEvent::ItemDelta {
                item_id: item_id.clone(),
                delta: ItemDelta::StatusChanged(ItemStatus::Pending),
            },
            &mut out,
        )
        .unwrap();
    printer
        .handle(
            &AgentEvent::ItemCompleted {
                item: SessionItem::SubagentRun {
                    id: item_id,
                    name: None,
                    agent: "explore".to_string(),
                    status: ItemStatus::Succeeded,
                    report: Some("done".to_string()),
                    child_session_id: None,
                    model: None,
                    approval_mode: meerai_policy::ApprovalMode::Ask,
                    steps_used: 0,
                    max_steps: 0,
                    usage: Default::default(),
                    est_cost_usd: None,
                    active_tool: None,
                },
            },
            &mut out,
        )
        .unwrap();
    printer.finish(&mut out).unwrap();

    let rendered = String::from_utf8(out).unwrap();
    assert_eq!(
        rendered,
        "Working\n▸ subagent explore in-progress\n▸ subagent explore pending\n▸ subagent explore succeeded\n"
    );
}

#[test]
fn build_workspace_toolsets_exposes_all_and_filters_read_only() {
    let root = std::env::temp_dir().join(format!("meerai-toolsets-{}", SessionId::new()));
    let code_tools = CodeTools::new(root.clone());
    let repo_map = RepoMapToolset::new(root.clone());
    let lsp = LspToolset::new(root.clone());
    let sac = SearchAsCodeToolset::new(root.clone())
        .with_runtime(meerai_tools::search_as_code::ScriptRuntime::Python);

    let full_names: Vec<String> =
        build_workspace_toolsets(&code_tools, &repo_map, &lsp, &sac, false)
            .into_iter()
            .flat_map(|toolset| {
                toolset
                    .definition()
                    .into_iter()
                    .map(|def| def.name)
                    .collect::<Vec<_>>()
            })
            .collect();
    assert!(full_names.contains(&"code-read_file".to_string()));
    assert!(full_names.contains(&"code-write_file".to_string()));
    assert!(full_names.contains(&"code-shell".to_string()));
    assert!(full_names.contains(&"retrieval-repo_map".to_string()));
    assert!(full_names.contains(&"lsp-go_to_definition".to_string()));
    assert!(full_names.contains(&"lsp-find_references".to_string()));
    assert!(full_names.contains(&"lsp-get_diagnostics".to_string()));
    assert!(full_names.contains(&"sac-run_script".to_string()));

    let ro_names: Vec<String> = build_workspace_toolsets(&code_tools, &repo_map, &lsp, &sac, true)
        .into_iter()
        .flat_map(|toolset| {
            toolset
                .definition()
                .into_iter()
                .map(|def| def.name)
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(ro_names.contains(&"code-read_file".to_string()));
    assert!(ro_names.contains(&"retrieval-repo_map".to_string()));
    assert!(ro_names.contains(&"lsp-go_to_definition".to_string()));
    assert!(!ro_names.contains(&"code-write_file".to_string()));
    assert!(!ro_names.contains(&"code-edit_file".to_string()));
    assert!(!ro_names.contains(&"code-shell".to_string()));
    assert!(!ro_names.contains(&"sac-run_script".to_string()));

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn build_policy_denies_sensitive_reads_and_allows_source_files_in_ask_and_auto() {
    use meerai_core::EffectClass;
    use meerai_policy::{Decision, PermissionAction, PermissionCheck};

    let root = std::env::temp_dir().join(format!("meerai-policy-{}", SessionId::new()));
    std::fs::create_dir_all(&root).unwrap();
    let minimal_rules: serde_yaml::Value = serde_yaml::from_str("[]").unwrap();

    for permissions in [None, Some(&minimal_rules)] {
        for mode in [ApprovalMode::Ask, ApprovalMode::Auto] {
            let policy = build_policy(mode, permissions, &root).unwrap();

            for path in [".env", "secrets.yml", "credentials.json"] {
                let decision = policy.decide(&PermissionCheck {
                    action: PermissionAction::Read,
                    resource: path.to_string(),
                    effect: EffectClass::Read,
                    tool_name: "code-read_file".to_string(),
                });
                assert!(
                    matches!(decision, Decision::Deny { .. }),
                    "expected deny for {path} in {mode:?}, got {decision:?}"
                );
            }

            let source_decision = policy.decide(&PermissionCheck {
                action: PermissionAction::Read,
                resource: "src/main.rs".to_string(),
                effect: EffectClass::Read,
                tool_name: "code-read_file".to_string(),
            });
            assert_eq!(source_decision, Decision::Allow);
        }
    }

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn build_workspace_toolsets_blocks_sensitive_files_and_attaches_execution_world() {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use meerai_core::{ObservationStatus, async_trait};
    use meerai_tools::{ExecutionWorld, SandboxPolicy, WorldCommand, WorldExecOutcome};

    #[derive(Debug, Default)]
    struct RecordingWorld {
        programs: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ExecutionWorld for RecordingWorld {
        fn kind(&self) -> &'static str {
            "recording"
        }

        fn is_sandboxed(&self) -> bool {
            true
        }

        async fn spawn_command(
            &self,
            command: &WorldCommand,
            _policy: &SandboxPolicy,
            _timeout: Duration,
        ) -> std::io::Result<WorldExecOutcome> {
            self.programs.lock().unwrap().push(command.program.clone());
            Ok(WorldExecOutcome::TimedOut)
        }
    }

    let root = std::env::temp_dir().join(format!("meerai-world-toolsets-{}", SessionId::new()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join(".env"), "API_KEY=secret\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();

    let world = Arc::new(RecordingWorld::default());
    let world_dyn: Arc<dyn ExecutionWorld> = world.clone();
    let code_tools = CodeTools::new(root.clone()).with_execution_world(world_dyn.clone());
    let repo_map = RepoMapToolset::new(root.clone());
    let lsp = LspToolset::new(root.clone());
    let sac = SearchAsCodeToolset::new(root.clone())
        .with_execution_world(world_dyn.clone())
        .with_runtime(meerai_tools::search_as_code::ScriptRuntime::Python);

    assert_eq!(code_tools.execution_world().kind(), "recording");
    assert_eq!(sac.execution_world().kind(), "recording");

    let toolsets = build_workspace_toolsets(&code_tools, &repo_map, &lsp, &sac, false);
    let code_toolset = toolsets
        .iter()
        .find(|toolset| toolset.contain("code-read_file"))
        .expect("code toolset present");
    let sac_toolset = toolsets
        .iter()
        .find(|toolset| toolset.contain("sac-run_script"))
        .expect("sac toolset present");

    let env_read = code_toolset
        .invoke_structured(
            "code-read_file",
            &serde_json::json!({ "path": ".env" }).to_string(),
        )
        .await
        .unwrap();
    assert_eq!(env_read.observation.status, ObservationStatus::Refused);

    let src_read = code_toolset
        .invoke_structured(
            "code-read_file",
            &serde_json::json!({ "path": "src/main.rs" }).to_string(),
        )
        .await
        .unwrap();
    assert_eq!(src_read.observation.status, ObservationStatus::Success);

    let _ = code_toolset
        .invoke_structured(
            "code-shell",
            &serde_json::json!({ "command": "echo hi" }).to_string(),
        )
        .await
        .unwrap();
    let _ = sac_toolset
        .invoke_structured(
            "sac-run_script",
            &serde_json::json!({ "language": "python", "script": "print('ok')" }).to_string(),
        )
        .await
        .unwrap();

    {
        let programs = world.programs.lock().unwrap();
        assert_eq!(programs.len(), 2);
        assert_eq!(programs[1], "python3");
    }

    let detected = detect_execution_world(true).await;
    assert!(!detected.kind().is_empty());

    std::fs::remove_dir_all(&root).ok();
}

#[cfg(any(feature = "tui", feature = "web"))]
#[tokio::test]
async fn cli_session_host_lists_creates_and_opens_sessions() {
    let root = std::env::temp_dir().join(format!("meerai-cli-host-{}", SessionId::new()));
    let config_root = root.join("config");
    let sessions_root = root.join("sessions");
    let workspace_root = root.join("workspace");
    std::fs::create_dir_all(&config_root).unwrap();
    std::fs::create_dir_all(&workspace_root).unwrap();

    let settings = AppSettings::default();
    let runtime = Arc::new(RwLock::new(RuntimeProviderState::from_settings(
        &settings,
        ProviderCatalog::new(),
    )));
    let expected_provider = runtime.read().unwrap().active_provider.clone();
    let expected_model = "test-model".to_string();
    let secrets: Arc<dyn meerai_client::secrets::SecretStore> =
        Arc::new(meerai_client::secrets::MemorySecretStore::new());
    let client = build_genai_client(runtime.clone(), secrets.clone()).unwrap();

    let mut session_config = SessionConfig::new(workspace_root.clone());
    session_config.root = sessions_root.clone();

    let host = CliSessionHost {
        script_runtime: None,
        workspace_root: workspace_root.clone(),
        session_config,
        runtime,
        secrets,
        user_settings: std::sync::Mutex::new(settings.clone()),
        settings,
        config_dir: Some(config_root.clone()),
        data_dir: None,
        client,
        approval_mode: ApprovalMode::Ask,
        execution_world: Arc::new(meerai_tools::HostWorld::new()),
        sensitive_env_vars: Vec::new(),
        hooks: Arc::new(HookEngine::default()),
        custom_agents: Vec::new(),
        skill_catalog: Arc::new(vec![test_skill("review")]),
        base_system: "Test system prompt".to_string(),
        launch_system: String::new(),
        launch_definitions: String::new(),
        default_provider: expected_provider.clone(),
        default_model: expected_model.clone(),
    };

    assert!(host.list_sessions().await.unwrap().is_empty());
    assert!(
        host.enabled_tool_names()
            .contains(&"skill-load".to_string())
    );

    let created = host.create_session().await.unwrap();
    assert_eq!(created.provider, expected_provider);
    assert_eq!(created.model, expected_model);
    let _ = created.cmd_tx.send(AgentCommand::Quit);

    let listed = host.list_sessions().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, created.session_id);

    let reopened = host.open_session(&created.session_id).await.unwrap();
    assert_eq!(reopened.session_id, created.session_id);
    assert_eq!(reopened.provider, expected_provider);
    assert_eq!(reopened.model, expected_model);
    let _ = reopened.cmd_tx.send(AgentCommand::Quit);

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn empty_session_cleanup_removes_only_old_unopened_sessions() {
    use meerai_session::{
        ContentBlock, InputSource, SessionConfig, SessionEvent, TurnId, TurnTrigger,
    };

    let root = std::env::temp_dir().join(format!("meerai-empty-sessions-{}", SessionId::new()));
    let config = SessionConfig {
        root: root.clone(),
        workspace: root.clone(),
    };

    let unused_id = SessionId::new();
    let unused = Session::open_or_create(&config, unused_id.clone(), root.clone())
        .await
        .unwrap();
    drop(unused);

    let open_id = SessionId::new();
    let _open = Session::open_or_create(&config, open_id.clone(), root.clone())
        .await
        .unwrap();

    let young_id = SessionId::new();
    let _young = Session::open_or_create(&config, young_id.clone(), root.clone())
        .await
        .unwrap();

    let active_id = SessionId::new();
    let _active = Session::open_or_create(&config, active_id.clone(), root.clone())
        .await
        .unwrap();

    let used_id = SessionId::new();
    let used = Session::open_or_create(&config, used_id.clone(), root.clone())
        .await
        .unwrap();
    used.append(SessionEvent::TurnStarted {
        turn_id: TurnId::from_raw("turn-1"),
        trigger: TurnTrigger::UserPrompt,
    })
    .await
    .unwrap();
    used.append(SessionEvent::UserMessage {
        source: InputSource::User,
        content: vec![ContentBlock::Text("keep me".to_string())],
    })
    .await
    .unwrap();

    age_session(&root, &unused_id, EMPTY_SESSION_MAX_AGE_MS);
    age_session(&root, &open_id, EMPTY_SESSION_MAX_AGE_MS);

    let removed = crate::sessions_cmd::remove_empty_sessions(
        &root,
        EMPTY_SESSION_MAX_AGE_MS,
        &[active_id.clone(), open_id.clone()],
    )
    .await
    .unwrap();
    assert_eq!(removed, 1);

    let remaining: Vec<String> = list_sessions(&root)
        .unwrap()
        .into_iter()
        .map(|summary| summary.id.to_string())
        .collect();
    assert!(remaining.contains(&active_id.to_string()));
    assert!(remaining.contains(&open_id.to_string()));
    assert!(remaining.contains(&young_id.to_string()));
    assert!(remaining.contains(&used_id.to_string()));
    assert!(!remaining.contains(&unused_id.to_string()));

    std::fs::remove_dir_all(&root).ok();
}

fn age_session(root: &std::path::Path, id: &SessionId, age_ms: u64) {
    let store = meerai_store::Store::open(root.join("meerai.db")).unwrap();
    store
        .immediate(|tx| {
            tx.execute(
                "UPDATE sessions SET updated_at_ms = ?2 WHERE id = ?1",
                (id.as_str(), now_millis().saturating_sub(age_ms)),
            )?;
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn empty_session_ids_keep_order_and_skip_titled_or_used_sessions() {
    use meerai_session::{ContentBlock, InputSource, SessionConfig, SessionEvent};

    let root = std::env::temp_dir().join(format!("meerai-empty-ids-{}", SessionId::new()));
    let config = SessionConfig {
        root: root.clone(),
        workspace: root.clone(),
    };

    let first = SessionId::new();
    let _first = Session::open_or_create(&config, first.clone(), root.clone())
        .await
        .unwrap();
    let titled = SessionId::new();
    let titled_session = Session::open_or_create(&config, titled.clone(), root.clone())
        .await
        .unwrap();
    titled_session.set_title("Named task").await.unwrap();
    let used = SessionId::new();
    let used_session = Session::open_or_create(&config, used.clone(), root.clone())
        .await
        .unwrap();
    used_session
        .append(SessionEvent::UserMessage {
            source: InputSource::User,
            content: vec![ContentBlock::Text("hello".to_string())],
        })
        .await
        .unwrap();

    let ids = empty_session_ids(&root, &[titled.clone(), first.clone(), used.clone()]).await;
    assert_eq!(ids, vec![first]);

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn parses_hooks_subcommands() {
    let hooks = |args: &[&str]| -> Vec<String> {
        std::iter::once("hooks".to_string())
            .chain(args.iter().map(|arg| arg.to_string()))
            .collect()
    };

    assert_eq!(
        parse_args(hooks(&[])).unwrap().subcommand,
        Some(CliSubcommand::Hooks(HooksCommand::Status))
    );
    assert_eq!(
        parse_args(hooks(&["status"])).unwrap().subcommand,
        Some(CliSubcommand::Hooks(HooksCommand::Status))
    );
    assert_eq!(
        parse_args(hooks(&["trust"])).unwrap().subcommand,
        Some(CliSubcommand::Hooks(HooksCommand::Trust))
    );
    assert_eq!(
        parse_args(hooks(&["untrust"])).unwrap().subcommand,
        Some(CliSubcommand::Hooks(HooksCommand::Untrust))
    );
    assert_eq!(
        parse_args(hooks(&["verify"])).unwrap().subcommand,
        Some(CliSubcommand::Hooks(HooksCommand::Verify {
            command: None,
            reset_baseline: false
        }))
    );
    assert_eq!(
        parse_args(hooks(&["verify", "--command", "cargo test"]))
            .unwrap()
            .subcommand,
        Some(CliSubcommand::Hooks(HooksCommand::Verify {
            command: Some("cargo test".to_string()),
            reset_baseline: false
        }))
    );
    assert_eq!(
        parse_args(hooks(&["verify", "--reset-baseline"]))
            .unwrap()
            .subcommand,
        Some(CliSubcommand::Hooks(HooksCommand::Verify {
            command: None,
            reset_baseline: true
        }))
    );
    assert!(parse_args(hooks(&["-h"])).unwrap().show_help);
    assert!(parse_args(hooks(&["--help"])).unwrap().show_help);
    assert!(parse_args(hooks(&["trust", "--help"])).unwrap().show_help);
    assert!(parse_args(hooks(&["untrust", "-h"])).unwrap().show_help);
    assert!(parse_args(hooks(&["verify", "--help"])).unwrap().show_help);
    assert!(parse_args(hooks(&["status", "extra"])).is_err());
    assert!(parse_args(hooks(&["trust", "extra"])).is_err());
    assert!(parse_args(hooks(&["verify", "--command"])).is_err());
    assert!(parse_args(hooks(&["verify", "extra"])).is_err());
    assert!(parse_args(hooks(&["bogus"])).is_err());
    assert!(help_text().contains("hooks <trust|untrust|status|verify>"));
}

#[test]
fn hooks_trust_command_persists_and_removes_project() {
    let config_dir = std::env::temp_dir().join(format!("meerai-hooks-cfg-{}", SessionId::new()));
    let project_root =
        std::env::temp_dir().join(format!("meerai-hooks-project-{}", SessionId::new()));
    std::fs::create_dir_all(&project_root).unwrap();
    let mut settings = AppSettings::default();

    crate::hooks_cmd::run_hooks(
        HooksCommand::Trust,
        Some(&config_dir),
        &project_root,
        &mut settings,
    )
    .unwrap();
    let saved = meerai_common::settings::load_settings_from(&config_dir).unwrap();
    assert!(
        meerai_common::hooks::project_is_trusted(
            &saved.hooks.as_ref().unwrap().trusted_projects,
            &project_root,
        ),
        "trust must persist to settings.yml"
    );

    crate::hooks_cmd::run_hooks(
        HooksCommand::Untrust,
        Some(&config_dir),
        &project_root,
        &mut settings,
    )
    .unwrap();
    let saved = meerai_common::settings::load_settings_from(&config_dir).unwrap();
    assert!(
        !meerai_common::hooks::project_is_trusted(
            &saved.hooks.unwrap().trusted_projects,
            &project_root,
        ),
        "untrust must remove the project from settings.yml"
    );

    let error =
        crate::hooks_cmd::run_hooks(HooksCommand::Trust, None, &project_root, &mut settings)
            .unwrap_err();
    assert!(error.to_string().contains("no config directory"));

    std::fs::remove_dir_all(&config_dir).ok();
    std::fs::remove_dir_all(&project_root).ok();
}

#[test]
fn headless_question_cancel_builds_answer_command() {
    use meerai_agents::{QuestionOption, QuestionPrompt, QuestionRequest};

    let request = QuestionRequest {
        request_id: RequestId::from_raw("q-1"),
        turn_id: TurnId::new(),
        questions: vec![QuestionPrompt {
            header: "DB".to_string(),
            question: "Which?".to_string(),
            options: vec![
                QuestionOption {
                    label: "a".to_string(),
                    description: None,
                },
                QuestionOption {
                    label: "b".to_string(),
                    description: None,
                },
            ],
            multiple: false,
        }],
    };
    assert_eq!(
        question_cancel_command(&request),
        AgentCommand::AnswerQuestion {
            request_id: RequestId::from_raw("q-1"),
            answers: None,
        }
    );
}

#[test]
fn parses_acp_flag_and_subcommand() {
    let flag = parse_args(vec![
        "--acp".to_string(),
        "--prompt".to_string(),
        "hi".to_string(),
    ])
    .unwrap();
    assert!(flag.acp);
    assert_eq!(flag.subcommand, None);

    let with_mode = parse_args(vec![
        "--acp".to_string(),
        "--mode".to_string(),
        "plan".to_string(),
        "--prompt".to_string(),
        "hi".to_string(),
    ])
    .unwrap();
    assert!(with_mode.acp);
    assert_eq!(with_mode.mode.as_deref(), Some("plan"));

    let doctor = parse_args(vec!["acp".to_string(), "doctor".to_string()]).unwrap();
    assert_eq!(
        doctor.subcommand,
        Some(CliSubcommand::Acp(AcpCommand::Doctor))
    );

    let default = parse_args(vec!["acp".to_string()]).unwrap();
    assert_eq!(
        default.subcommand,
        Some(CliSubcommand::Acp(AcpCommand::Doctor))
    );

    assert!(
        parse_args(vec!["acp".to_string(), "-h".to_string()])
            .unwrap()
            .show_help
    );
    assert!(
        parse_args(vec![
            "acp".to_string(),
            "doctor".to_string(),
            "--help".to_string()
        ])
        .unwrap()
        .show_help
    );
    assert!(parse_args(vec!["acp".to_string(), "bogus".to_string()]).is_err());
    assert!(
        parse_args(vec![
            "acp".to_string(),
            "doctor".to_string(),
            "extra".to_string()
        ])
        .is_err()
    );

    assert!(help_text().contains("meerai acp doctor"));
    assert!(help_text().contains("--acp"));
}

#[test]
fn acp_rejects_agent_and_skill_flags() {
    for args in [
        vec!["--acp", "--agent", "reviewer"],
        vec!["--acp", "--skill", "explore"],
    ] {
        let error = parse_args(args.iter().map(|arg| arg.to_string())).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("--acp cannot be combined with --agent or --skill"),
            "unexpected error: {error}"
        );
    }
}

#[test]
fn cli_acp_spawner_backs_the_acp_run_tool() {
    let toolset = AcpRunToolset::new(Arc::new(CliAcpSpawner));
    assert_eq!(toolset.name(), "acp");
    assert!(toolset.contain("acp-run"));
    assert!(!toolset.contain("subagent-run"));
    assert_eq!(toolset.effect_of("acp-run"), EffectClass::External);
    let names: Vec<String> = toolset
        .definition()
        .into_iter()
        .map(|definition| definition.name)
        .collect();
    assert_eq!(names, vec!["acp-run".to_string()]);
}

fn sample_external_agent_approval_event() -> AgentEvent {
    use meerai_agents::events::ApprovalPreview;

    AgentEvent::ApprovalRequested {
        request: ApprovalRequest {
            request_id: RequestId::from_raw("req-external"),
            turn_id: meerai_agents::TurnId::from_raw("turn-headless"),
            kind: ApprovalKind::ExternalAgent,
            preview: ApprovalPreview {
                title: "External agent permission: Edit src/lib.rs".to_string(),
                detail: "external agent".to_string(),
            },
            effect: None,
        },
    }
}

#[test]
fn headless_external_agent_approval_declines_and_exits_2() {
    let event = sample_external_agent_approval_event();
    assert_eq!(headless_outcome(&event), HeadlessOutcome::ApprovalRequired);

    let action = HeadlessApprovalAction::for_request(approval_request(&event));
    assert_eq!(
        action,
        HeadlessApprovalAction::Decline {
            request_id: RequestId::from_raw("req-external"),
        }
    );

    let mut exit = HeadlessExit::new();
    exit.observe(headless_outcome(&event));
    assert_eq!(exit.code, EXIT_APPROVAL_REQUIRED);
    assert!(exit.approval_cancelled);
}

#[tokio::test]
async fn run_headless_channels_completes_on_turn_end() {
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();

    let driver = tokio::spawn(async move {
        assert_eq!(
            cmd_rx.recv().await,
            Some(AgentCommand::SubmitPrompt("hi".to_string()))
        );
        let _ = event_tx.send(AgentEvent::TurnCompleted {
            turn_id: meerai_agents::TurnId::from_raw("turn-headless"),
            reason: meerai_agents::TurnEndReason::Completed,
        });
        loop {
            match cmd_rx.recv().await {
                Some(AgentCommand::Quit) | None => break,
                _ => {}
            }
        }
    });

    let code = run_headless_channels(cmd_tx, event_rx, "hi".to_string(), false)
        .await
        .unwrap();
    assert_eq!(code, 0);
    driver.await.unwrap();
}

#[cfg(feature = "tui")]
#[test]
fn exit_summary_shows_logo_title_and_resume_command() {
    let summary = exit_summary("ses_abc123", Some("Readiness review check"));
    assert!(summary.contains("\x1b[1m"));
    assert!(summary.contains("\x1b[0m"));
    let mut plain = String::new();
    let mut rest = summary.as_str();
    while let Some(idx) = rest.find("\x1b[") {
        plain.push_str(&rest[..idx]);
        let after = &rest[idx + 2..];
        match after.find('m') {
            Some(end) => rest = &after[end + 1..],
            None => break,
        }
    }
    plain.push_str(rest);
    assert!(plain.contains("  ██      ██\n"));
    assert!(plain.contains("  ████  ████\n"));
    assert!(plain.contains("Session   Readiness review check"));
    assert!(plain.contains("Continue  meerai --resume ses_abc123"));
}

#[cfg(feature = "tui")]
#[test]
fn exit_summary_falls_back_to_the_session_id_without_a_title() {
    for title in [None, Some("   ")] {
        let summary = exit_summary("ses_abc123", title);
        assert!(summary.contains("Session   ses_abc123"));
        assert!(summary.contains("Continue  meerai --resume ses_abc123"));
    }
}

#[test]
fn session_model_specs_with_an_adapter_prefix_keep_the_current_provider() {
    let catalog = ProviderCatalog::new();
    assert_eq!(
        split_session_model(&catalog, "openai", "openai_resp::gpt-6.1-sol"),
        ("openai".to_string(), "gpt-6.1-sol".to_string())
    );
    assert_eq!(
        split_session_model(&catalog, "openai", "anthropic::claude-sonnet-4-5"),
        ("anthropic".to_string(), "claude-sonnet-4-5".to_string())
    );
    assert_eq!(
        split_session_model(&catalog, "openai", "gpt-6.1-sol"),
        ("openai".to_string(), "gpt-6.1-sol".to_string())
    );
}

#[test]
fn parses_serve_subcommand_and_flags() {
    let bare = parse_args(["serve".to_string()]).unwrap();
    assert_eq!(
        bare.subcommand,
        Some(CliSubcommand::Serve(ServeCommand {
            port: None,
            username: None,
            password: None,
            auth: false,
        }))
    );

    let with_port = parse_args(["serve", "--port", "8080"].map(str::to_string)).unwrap();
    assert_eq!(
        with_port.subcommand,
        Some(CliSubcommand::Serve(ServeCommand {
            port: Some(8080),
            username: None,
            password: None,
            auth: false,
        }))
    );

    let with_resume_and_port = parse_args(
        [
            "serve",
            "--resume",
            "id",
            "--port=9000",
            "--user",
            "alice",
            "--password",
            "custompass",
            "--auth",
        ]
        .map(str::to_string),
    )
    .unwrap();
    assert_eq!(
        with_resume_and_port.subcommand,
        Some(CliSubcommand::Serve(ServeCommand {
            port: Some(9000),
            username: Some("alice".to_string()),
            password: Some("custompass".to_string()),
            auth: true,
        }))
    );
    assert_eq!(with_resume_and_port.resume.as_deref(), Some("id"));

    let with_flags = parse_args(
        [
            "serve",
            "--continue",
            "-p",
            "openai",
            "-m",
            "gpt-5",
            "--mode",
            "ask",
            "--agent",
            "reviewer",
            "--skill",
            "explore",
            "--env-file",
            ".env.local",
            "--system",
            "Be brief.",
        ]
        .map(str::to_string),
    )
    .unwrap();
    assert_eq!(
        with_flags.subcommand,
        Some(CliSubcommand::Serve(ServeCommand {
            port: None,
            username: None,
            password: None,
            auth: false,
        }))
    );
    assert!(with_flags.continue_session);
    assert_eq!(with_flags.provider.as_deref(), Some("openai"));
    assert_eq!(with_flags.model.as_deref(), Some("gpt-5"));
    assert_eq!(with_flags.mode.as_deref(), Some("ask"));
    assert_eq!(with_flags.agents, vec!["reviewer"]);
    assert_eq!(with_flags.skills, vec!["explore"]);
    assert_eq!(with_flags.env_file.as_deref(), Some(".env.local"));
    assert_eq!(with_flags.system, "Be brief.");

    let with_fork_and_acp =
        parse_args(["serve", "--fork", "ses-1", "--acp"].map(str::to_string)).unwrap();
    assert_eq!(
        with_fork_and_acp.subcommand,
        Some(CliSubcommand::Serve(ServeCommand {
            port: None,
            username: None,
            password: None,
            auth: false,
        }))
    );
    assert_eq!(with_fork_and_acp.fork.as_deref(), Some("ses-1"));
    assert!(with_fork_and_acp.acp);

    assert!(
        parse_args(["serve", "--help"].map(str::to_string))
            .unwrap()
            .show_help
    );
    assert!(
        parse_args(["serve", "-h"].map(str::to_string))
            .unwrap()
            .show_help
    );

    assert!(parse_args(["serve", "--port", "70000"].map(str::to_string)).is_err());
    assert!(parse_args(["serve", "--port=abc"].map(str::to_string)).is_err());
    assert!(parse_args(["serve", "--port"].map(str::to_string)).is_err());
    assert!(parse_args(["serve", "--port="].map(str::to_string)).is_err());
    assert!(parse_args(["serve", "--bogus"].map(str::to_string)).is_err());
    assert!(parse_args(["serve", "extra"].map(str::to_string)).is_err());
    assert!(parse_args(["--port", "8080"].map(str::to_string)).is_err());

    let prompt_conflict = parse_args(["serve", "--prompt", "hi"].map(str::to_string)).unwrap_err();
    assert!(prompt_conflict.to_string().contains("cannot be combined"));
    let prompt_before = parse_args(["--prompt", "hi", "serve"].map(str::to_string)).unwrap_err();
    assert!(prompt_before.to_string().contains("cannot be combined"));
}

#[cfg(feature = "web")]
#[test]
fn resolve_serve_auth_defaults_local_credentials_and_allows_overrides() {
    let default_creds = resolve_serve_auth_with(None, None, None, None, false, || {
        panic!("prompt should not run unless requested")
    })
    .unwrap();
    assert!(default_creds.auth.is_enabled());
    assert_eq!(default_creds.default_secret.as_deref(), Some("12345678"));
    assert!(matches!(
        default_creds.auth,
        meerai_web::ServeAuth::Basic {
            username: Some(ref u),
            secret: Some(_)
        } if u == "meerai"
    ));

    let from_cli = resolve_serve_auth_with(
        Some("bob".to_string()),
        Some("cli-secret-1".to_string()),
        None,
        Some("env-ignored".to_string()),
        false,
        || panic!("prompt should not run when cli password is set"),
    )
    .unwrap();
    assert!(from_cli.default_secret.is_none());
    assert!(matches!(
        from_cli.auth,
        meerai_web::ServeAuth::Basic {
            username: Some(ref u),
            secret: Some(_)
        } if u == "bob"
    ));

    let from_env = resolve_serve_auth_with(
        None,
        None,
        None,
        Some("8-chars!".to_string()),
        false,
        || panic!("prompt should not run when env secret is set"),
    )
    .unwrap();
    assert!(from_env.default_secret.is_none());
    assert!(matches!(
        from_env.auth,
        meerai_web::ServeAuth::Basic {
            username: Some(ref u),
            secret: Some(_)
        } if u == "meerai"
    ));

    let short_env =
        resolve_serve_auth_with(None, None, None, Some("7-chars".to_string()), false, || {
            Ok(String::new())
        })
        .unwrap_err();
    assert!(format!("{short_env:#}").contains("8 characters"));

    let prompt_default =
        resolve_serve_auth_with(None, None, None, Some(String::new()), true, || {
            Ok(String::new())
        })
        .unwrap();
    assert!(prompt_default.auth.is_enabled());
    assert_eq!(prompt_default.default_secret.as_deref(), Some("12345678"));

    let prompt_custom =
        resolve_serve_auth_with(Some("alice".to_string()), None, None, None, true, || {
            Ok("pass1234".to_string())
        })
        .unwrap();
    assert!(prompt_custom.default_secret.is_none());
    assert!(matches!(
        prompt_custom.auth,
        meerai_web::ServeAuth::Basic {
            username: Some(ref u),
            secret: Some(_)
        } if u == "alice"
    ));

    let prompt_short =
        resolve_serve_auth_with(None, None, None, None, true, || Ok("short".to_string()))
            .unwrap_err();
    assert!(format!("{prompt_short:#}").contains("8 characters"));
}

#[test]
fn parses_auth_reset_vault_flags() {
    assert_eq!(
        parse_args(vec!["auth".to_string(), "reset-vault".to_string()])
            .unwrap()
            .subcommand,
        Some(CliSubcommand::Auth(AuthCommand::ResetVault { yes: false }))
    );
    assert_eq!(
        parse_args(vec![
            "auth".to_string(),
            "reset-vault".to_string(),
            "--yes".to_string()
        ])
        .unwrap()
        .subcommand,
        Some(CliSubcommand::Auth(AuthCommand::ResetVault { yes: true }))
    );
}

#[test]
fn parses_memory_route_pack_and_export_commands() {
    let args =
        |prefix: &[&str]| -> Vec<String> { prefix.iter().map(|arg| arg.to_string()).collect() };

    assert_eq!(
        parse_args(args(&["memory", "list", "parser"]))
            .unwrap()
            .subcommand,
        Some(CliSubcommand::Memory(MemoryCommand::List {
            project: None,
            query: Some("parser".to_string()),
        }))
    );
    assert_eq!(
        parse_args(args(&[
            "memory",
            "save",
            "decision",
            "Use SQLite",
            "shared",
            "store",
            "--pin",
        ]))
        .unwrap()
        .subcommand,
        Some(CliSubcommand::Memory(MemoryCommand::Save {
            project: None,
            kind: "decision".to_string(),
            title: "Use SQLite".to_string(),
            body: Some("shared store".to_string()),
            pinned: true,
        }))
    );
    assert_eq!(
        parse_args(args(&["memory", "pin", "mem-1", "--off"]))
            .unwrap()
            .subcommand,
        Some(CliSubcommand::Memory(MemoryCommand::Pin {
            project: None,
            id: "mem-1".to_string(),
            pinned: false,
        }))
    );

    assert_eq!(
        parse_args(args(&[
            "routes",
            "set",
            "build",
            "openai::o3",
            "1.5",
            "20000",
            "2",
        ]))
        .unwrap()
        .subcommand,
        Some(CliSubcommand::Routes(RoutesCommand::Set {
            task: "build".to_string(),
            spec: "openai::o3".to_string(),
            effort: None,
            max_cost_usd: Some(1.5),
            max_tokens_per_turn: Some(20_000),
            max_concurrency: Some(2),
        }))
    );
    assert_eq!(
        parse_args(args(&["routes", "clear", "explore"]))
            .unwrap()
            .subcommand,
        Some(CliSubcommand::Routes(RoutesCommand::Clear {
            task: "explore".to_string(),
        }))
    );

    assert_eq!(
        parse_args(args(&["pack", "trust", "review"]))
            .unwrap()
            .subcommand,
        Some(CliSubcommand::Pack(PackCommand::SetTrusted {
            id: "review".to_string(),
            trusted: true,
        }))
    );
    assert_eq!(
        parse_args(args(&["pack", "pin", "review", "abc123"]))
            .unwrap()
            .subcommand,
        Some(CliSubcommand::Pack(PackCommand::Pin {
            id: "review".to_string(),
            rev: "abc123".to_string(),
        }))
    );

    assert_eq!(
        parse_args(args(&["sessions", "export", "ses-1"]))
            .unwrap()
            .subcommand,
        Some(CliSubcommand::Sessions(SessionsCommand::Export {
            id: "ses-1".to_string(),
        }))
    );
}
