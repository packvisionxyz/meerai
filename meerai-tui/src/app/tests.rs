use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use meerai_agents::events::{
    ApprovalDecision, ApprovalKind, ApprovalPreview, ApprovalRequest, GrantScope, RequestId,
};
use meerai_agents::{
    AbortCause, AgentEvent, ErrorCode, ItemDelta, ItemStatus, SessionItem, TurnEndReason, Usage,
};
use meerai_common::definitions::DefinitionSource;
use meerai_core::EffectClass;
use meerai_providers::providers::ProviderCatalog;
use meerai_providers::secrets::MemorySecretStore;

use super::commands::parse_mentions;
use super::input::encode_base64;
use super::*;
use crate::completion::CompletionItemKind;

fn test_app() -> (
    AppState,
    mpsc::UnboundedReceiver<AgentCommand>,
    Arc<MemorySecretStore>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    let secrets = Arc::new(MemorySecretStore::new());
    secrets.set_key("gemini", "test-gemini-key").unwrap();
    let settings = AppSettings::default();
    let runtime = Arc::new(RwLock::new(RuntimeProviderState::from_settings(
        &settings,
        ProviderCatalog::new(),
    )));
    let app = AppState::new(
        "gemini",
        "gemini-3.8-flash",
        PathBuf::from("."),
        tx,
        runtime,
        secrets.clone(),
        settings,
        None,
    );
    (app, rx, secrets)
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn key_mod(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

fn type_text(app: &mut AppState, text: &str) {
    for ch in text.chars() {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }
}

#[test]
fn opens_provider_select_on_startup_when_key_missing() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let secrets = Arc::new(MemorySecretStore::new());
    let settings = AppSettings::default();
    let catalog = ProviderCatalog::from_yaml("providers:\n  - id: custom-test-provider\n").unwrap();
    let runtime = Arc::new(RwLock::new(RuntimeProviderState::from_settings(
        &settings, catalog,
    )));
    let app = AppState::new(
        "custom-test-provider",
        "test-model",
        PathBuf::from("."),
        tx,
        runtime,
        secrets,
        settings,
        None,
    );
    assert!(matches!(
        app.modal,
        Some(ActiveModal::ProviderSelect { .. })
    ));
}

#[test]
fn connect_flow_saves_key_deletes_and_selects_model() {
    let (mut app, mut rx, secrets) = test_app();

    type_text(&mut app, "/connect");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::ProviderSelect { .. })
    ));

    type_text(&mut app, "opencode-go");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::ApiKeyInput { ref provider_id, .. }) if provider_id == "opencode-go"
    ));

    app.handle_terminal_event(Event::Paste("oc-secret-key\n".to_string()));
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        secrets.get_key("opencode-go").unwrap().as_deref(),
        Some("oc-secret-key")
    );
    assert!(matches!(app.modal, Some(ActiveModal::ModelInput { .. })));

    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::ModelInput { error: Some(_), .. })
    ));

    type_text(&mut app, "glm-5.3");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.modal.is_none());
    assert_eq!(app.tab().active_provider, "opencode-go");
    assert_eq!(app.tab().model, "glm-5.3");
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::SetModel("opencode_go::glm-5.3".to_string())
    );

    type_text(&mut app, "/provider");
    app.handle_key_event(key(KeyCode::Enter));
    type_text(&mut app, "opencode-go");
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key_mod(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert_eq!(secrets.get_key("opencode-go").unwrap(), None);
}

#[test]
fn model_input_accepts_manual_and_recent_models() {
    let (mut app, mut rx, _secrets) = test_app();

    type_text(&mut app, "/models");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(app.modal, Some(ActiveModal::ModelInput { .. })));

    type_text(&mut app, "custom-gemini-model");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.modal.is_none());
    assert_eq!(app.tab().model, "custom-gemini-model");
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::SetModel("gemini::custom-gemini-model".to_string())
    );

    type_text(&mut app, "/models");
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Down));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::ModelInput { ref input, .. }) if input == "custom-gemini-model"
    ));
}

#[test]
fn opens_model_input_on_startup_when_model_empty_and_blocks_empty_submit() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let secrets = Arc::new(MemorySecretStore::new());
    secrets.set_key("openai", "test-openai-key").unwrap();
    let settings = AppSettings::default();
    let runtime = Arc::new(RwLock::new(RuntimeProviderState::from_settings(
        &settings,
        ProviderCatalog::new(),
    )));
    let mut app = AppState::new(
        "openai",
        "",
        PathBuf::from("."),
        tx,
        runtime,
        secrets,
        settings,
        None,
    );
    assert!(matches!(app.modal, Some(ActiveModal::ModelInput { .. })));

    app.handle_key_event(key(KeyCode::Esc));
    assert!(app.modal.is_none());

    type_text(&mut app, "hello");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(!app.tab().is_running);
    assert!(rx.try_recv().is_err());
    assert!(matches!(app.modal, Some(ActiveModal::ModelInput { .. })));
}

#[test]
fn modal_switching_and_editing() {
    let (mut app, mut rx, _secrets) = test_app();
    assert_eq!(app.mode, InputMode::Insert);

    type_text(&mut app, "hello");
    app.handle_key_event(key_mod(KeyCode::Enter, KeyModifiers::SHIFT));
    type_text(&mut app, "world");
    assert_eq!(app.tab().input, "hello\nworld");

    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(app.mode, InputMode::Normal);

    app.handle_key_event(key(KeyCode::Char('i')));
    assert_eq!(app.mode, InputMode::Insert);

    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(app.mode, InputMode::Normal);
    app.handle_key_event(key(KeyCode::Char('a')));
    assert_eq!(app.mode, InputMode::Insert);

    app.handle_terminal_event(Event::Paste("!\r\nmore".to_string()));
    assert_eq!(app.tab().input, "hello\nworld!\nmore");

    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.tab().input.is_empty());
    assert!(app.tab().is_running);
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::SubmitPrompt("hello\nworld!\nmore".to_string())
    );
}

#[test]
fn normal_mode_scrolling_and_quit_controls() {
    let (mut app, mut rx, _secrets) = test_app();
    app.tab_mut().viewport_height = 10;
    app.tab_mut().total_lines = 40;
    app.tab_mut().auto_follow = true;
    app.mode = InputMode::Normal;

    assert_eq!(app.effective_scroll(), 30);

    app.handle_key_event(key(KeyCode::Char('k')));
    assert!(!app.tab().auto_follow);
    assert_eq!(app.effective_scroll(), 29);

    app.handle_key_event(key_mod(KeyCode::Char('u'), KeyModifiers::CONTROL));
    assert_eq!(app.effective_scroll(), 24);

    app.handle_key_event(key_mod(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert_eq!(app.effective_scroll(), 29);

    app.handle_key_event(key(KeyCode::Char('g')));
    app.handle_key_event(key(KeyCode::Char('g')));
    assert_eq!(app.effective_scroll(), 0);
    assert!(!app.tab().auto_follow);

    app.handle_key_event(key(KeyCode::Char('G')));
    assert_eq!(app.effective_scroll(), 30);
    assert!(app.tab().auto_follow);

    app.tab_mut().is_running = true;
    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(rx.try_recv().unwrap(), AgentCommand::Cancel);

    app.handle_key_event(key(KeyCode::Char('q')));
    assert!(app.should_quit);
    assert_eq!(rx.try_recv().unwrap(), AgentCommand::Quit);
}

#[test]
fn double_ctrl_c_exits_and_single_ctrl_c_cancels_when_running() {
    let (mut app, mut rx, _secrets) = test_app();
    app.tab_mut().is_running = true;
    app.handle_key_event(key_mod(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(rx.try_recv().unwrap(), AgentCommand::Cancel);
    assert!(!app.should_quit);

    app.tab_mut().is_running = false;
    app.handle_key_event(key_mod(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(!app.should_quit);
    assert!(app.status_notice.is_some());

    app.handle_key_event(key_mod(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(app.should_quit);
    assert_eq!(rx.try_recv().unwrap(), AgentCommand::Quit);
}

#[test]
fn completion_esc_dismisses_before_entering_normal_mode() {
    let (mut app, _rx, _secrets) = test_app();
    type_text(&mut app, "/");
    assert!(app.completion.is_active());

    app.handle_key_event(key_mod(KeyCode::Char('n'), KeyModifiers::CONTROL));
    assert_eq!(app.completion.selected(), 1);
    app.handle_key_event(key_mod(KeyCode::Char('p'), KeyModifiers::CONTROL));
    assert_eq!(app.completion.selected(), 0);

    app.handle_key_event(key(KeyCode::Esc));
    assert!(!app.completion.is_active());
    assert_eq!(app.mode, InputMode::Insert);

    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(app.mode, InputMode::Normal);
}

#[test]
fn slash_command_handling() {
    let (mut app, mut rx, _secrets) = test_app();

    type_text(&mut app, "/help");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::System { text }) if text.contains("/connect")
    ));

    type_text(&mut app, "/model gemini-3.1-pro-preview");
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(app.tab().model, "gemini-3.1-pro-preview");
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::SetModel("gemini::gemini-3.1-pro-preview".to_string())
    );

    type_text(&mut app, "/clear");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.tab().transcript.is_empty());

    app.tab_mut().is_running = true;
    type_text(&mut app, "/cancel");
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(rx.try_recv().unwrap(), AgentCommand::Cancel);

    type_text(&mut app, "/reset");
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(rx.try_recv().unwrap(), AgentCommand::Reset);
    assert!(!app.tab().is_running);

    type_text(&mut app, "/quit");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.should_quit);
    assert_eq!(rx.try_recv().unwrap(), AgentCommand::Quit);
}

fn sample_agent(
    id: &str,
    source: DefinitionSource,
    description: Option<&str>,
    model: Option<&str>,
    prompt: &str,
) -> AgentDefinition {
    AgentDefinition {
        id: id.to_string(),
        source,
        description: description.map(str::to_string),
        model: model.map(str::to_string),
        steps: None,
        mode: None,
        prompt: prompt.to_string(),
        path: PathBuf::from(format!(".agents/agents/{id}.md")),
    }
}

fn sample_skill(
    id: &str,
    source: DefinitionSource,
    description: Option<&str>,
    body: &str,
) -> SkillDefinition {
    SkillDefinition {
        id: id.to_string(),
        source,
        description: description.map(str::to_string),
        body: body.to_string(),
        path: PathBuf::from(format!(".agents/skills/{id}/SKILL.md")),
    }
}

fn submit(app: &mut AppState, command: &str) {
    type_text(app, command);
    if app.completion.is_active() {
        app.handle_key_event(key(KeyCode::Esc));
    }
    app.handle_key_event(key(KeyCode::Enter));
}

#[test]
fn mention_parser_applies_skills_then_agent_and_cleans_text() {
    let agents = vec![sample_agent(
        "reviewer",
        DefinitionSource::Agents,
        None,
        None,
        "Review.",
    )];
    let skills = vec![
        sample_skill("explore", DefinitionSource::Agents, None, "Explore."),
        sample_skill("fix", DefinitionSource::Claude, None, "Fix."),
    ];

    let plan = parse_mentions(
        "please @skill/explore and @skill/fix then @agent/reviewer do it",
        &agents,
        &skills,
    );
    assert_eq!(plan.skill_indices, vec![0, 1]);
    assert_eq!(plan.agent_indices, vec![0]);
    assert_eq!(plan.text.as_deref(), Some("please and then do it"));

    let deduped = parse_mentions(
        "@agent/reviewer @agent/reviewer @skill/fix @skill/fix",
        &agents,
        &skills,
    );
    assert_eq!(deduped.agent_indices, vec![0]);
    assert_eq!(deduped.skill_indices, vec![1]);
    assert_eq!(deduped.text.as_deref(), Some(""));

    let unknown = parse_mentions("@agent/nope @skill/nope keep", &agents, &skills);
    assert!(unknown.agent_indices.is_empty());
    assert!(unknown.skill_indices.is_empty());
    assert_eq!(unknown.text, None);
}

#[test]
fn submitting_mention_applies_agent_then_prompt() {
    let (mut app, mut rx, _secrets) = test_app();
    app.agents = vec![sample_agent(
        "reviewer",
        DefinitionSource::Agents,
        Some("Reviews changes"),
        Some("gemini-3.1-pro-preview"),
        "Review the diff.",
    )];

    submit(&mut app, "@agent/reviewer do the task");

    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::ApplyAgent {
            name: "reviewer".to_string(),
            instructions: "Review the diff.".to_string(),
            model: Some("gemini::gemini-3.1-pro-preview".to_string()),
        }
    );
    assert_eq!(app.tab().model, "gemini-3.1-pro-preview");
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::SubmitPrompt("do the task".to_string())
    );
    assert!(app.tab().is_running);
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::User {
            text: "do the task".to_string()
        })
    );
}

#[test]
fn mention_popup_arrows_and_tab_insert_without_submitting() {
    let (mut app, mut rx, _secrets) = test_app();
    app.agents = vec![sample_agent(
        "reviewer",
        DefinitionSource::Agents,
        Some("Reviews changes"),
        None,
        "Review.",
    )];
    app.skills = vec![sample_skill(
        "reviewer",
        DefinitionSource::Claude,
        Some("Review skill"),
        "Skill.",
    )];

    type_text(&mut app, "@reviewer");
    assert!(app.completion.is_active());
    let items = app.completion.items();
    assert_eq!(items[0].label, "reviewer");
    assert_eq!(items[0].item_kind, CompletionItemKind::Agent);
    assert_eq!(items[1].label, "reviewer");
    assert_eq!(items[1].item_kind, CompletionItemKind::Skill);

    app.handle_key_event(key(KeyCode::Down));
    assert_eq!(app.completion.selected(), 1);
    app.handle_key_event(key(KeyCode::Up));
    assert_eq!(app.completion.selected(), 0);
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Tab));

    assert_eq!(app.tab().input, "@skill/reviewer ");
    assert_eq!(app.mode, InputMode::Insert);
    assert!(!app.tab().is_running);
    assert!(rx.try_recv().is_err());
}

#[test]
fn enter_accepts_mention_without_submitting() {
    let (mut app, mut rx, _secrets) = test_app();
    app.agents = vec![sample_agent(
        "reviewer",
        DefinitionSource::Agents,
        Some("Reviews changes"),
        None,
        "Review.",
    )];

    type_text(&mut app, "@reviewer");
    assert!(app.completion.is_active());
    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(app.tab().input, "@agent/reviewer ");
    assert!(!app.tab().is_running);
    assert!(rx.try_recv().is_err());

    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::ApplyAgent {
            name: "reviewer".to_string(),
            instructions: "Review.".to_string(),
            model: None,
        }
    );
    assert!(rx.try_recv().is_err());
    assert!(!app.tab().is_running);
    assert!(app.tab().input.is_empty());
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::System { text })
            if text == "Applied agent reviewer (.agents)."
    ));
}

#[test]
fn only_mentions_apply_without_starting_a_turn() {
    let (mut app, mut rx, _secrets) = test_app();
    app.skills = vec![
        sample_skill("explore", DefinitionSource::Agents, None, "Explore."),
        sample_skill("fix", DefinitionSource::Claude, None, "Fix."),
    ];
    app.agents = vec![sample_agent(
        "reviewer",
        DefinitionSource::Claude,
        None,
        None,
        "Review.",
    )];

    submit(&mut app, "@skill/explore @skill/fix @agent/reviewer");

    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::ApplySkill {
            name: "explore".to_string(),
            instructions: "Explore.".to_string(),
        }
    );
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::ApplySkill {
            name: "fix".to_string(),
            instructions: "Fix.".to_string(),
        }
    );
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::ApplyAgent {
            name: "reviewer".to_string(),
            instructions: "Review.".to_string(),
            model: None,
        }
    );
    assert!(rx.try_recv().is_err());
    assert!(!app.tab().is_running);
    assert!(app.tab().input.is_empty());
    assert_eq!(
        app.tab().transcript,
        vec![
            TranscriptEntry::System {
                text: "Applied skill explore (.agents).".to_string()
            },
            TranscriptEntry::System {
                text: "Applied skill fix (.claude).".to_string()
            },
            TranscriptEntry::System {
                text: "Applied agent reviewer (.claude).".to_string()
            },
        ]
    );
}

#[test]
fn unknown_mention_stays_in_the_prompt() {
    let (mut app, mut rx, _secrets) = test_app();
    app.agents = vec![sample_agent(
        "reviewer",
        DefinitionSource::Agents,
        None,
        None,
        "Review.",
    )];

    submit(&mut app, "@agent/nope do it");

    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::SubmitPrompt("@agent/nope do it".to_string())
    );
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::User { text }) if text == "@agent/nope do it"
    ));
}

#[test]
fn repeated_mention_applies_once() {
    let (mut app, mut rx, _secrets) = test_app();
    app.skills = vec![sample_skill(
        "explore",
        DefinitionSource::Agents,
        None,
        "Explore.",
    )];

    submit(&mut app, "@skill/explore @skill/explore go");

    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::ApplySkill {
            name: "explore".to_string(),
            instructions: "Explore.".to_string(),
        }
    );
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::SubmitPrompt("go".to_string())
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn mention_agent_model_with_provider_prefix_is_ignored() {
    let (mut app, mut rx, _secrets) = test_app();
    app.agents = vec![sample_agent(
        "reviewer",
        DefinitionSource::Agents,
        None,
        Some("openrouter:deepseek-v4.1"),
        "Review.",
    )];

    submit(&mut app, "@agent/reviewer go");

    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::ApplyAgent {
            name: "reviewer".to_string(),
            instructions: "Review.".to_string(),
            model: None,
        }
    );
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::SubmitPrompt("go".to_string())
    );
    assert_eq!(app.tab().model, "gemini-3.8-flash");
    assert!(app.tab().transcript.iter().any(|entry| matches!(
        entry,
        TranscriptEntry::System { text } if text.contains("implies a provider change")
    )));
}

#[test]
fn list_commands_are_removed_and_agent_shortcuts_remain() {
    let (mut app, _rx, _secrets) = test_app();
    app.agents = vec![sample_agent(
        "reviewer",
        DefinitionSource::Agents,
        None,
        None,
        "Review.",
    )];

    submit(&mut app, "/agents");
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::Error { text, .. })
            if text.contains("Unknown command: /agents")
    ));

    app.tab_mut().transcript.clear();
    submit(&mut app, "/skills");
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::Error { text, .. })
            if text.contains("Unknown command: /skills")
    ));

    app.tab_mut().transcript.clear();
    submit(&mut app, "/help");
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::System { text })
            if text.contains("/agent <id>")
                && text.contains("@agent/<id>")
                && !text.contains("/agents")
                && !text.contains("/skills")
    ));
}

#[test]
fn agent_command_sends_instructions_and_syncs_model() {
    let (mut app, mut rx, _secrets) = test_app();
    app.agents = vec![sample_agent(
        "reviewer",
        DefinitionSource::Agents,
        Some("Reviews changes"),
        Some("gemini-3.1-pro-preview"),
        "Review the diff.",
    )];

    submit(&mut app, "/agent reviewer");
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::ApplyAgent {
            name: "reviewer".to_string(),
            instructions: "Review the diff.".to_string(),
            model: Some("gemini::gemini-3.1-pro-preview".to_string()),
        }
    );
    assert_eq!(app.tab().model, "gemini-3.1-pro-preview");
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::System { text })
            if text == "Applied agent reviewer (.agents)."
    ));
}

#[test]
fn skill_command_sends_body() {
    let (mut app, mut rx, _secrets) = test_app();
    app.skills = vec![sample_skill(
        "explore",
        DefinitionSource::Claude,
        Some("Search"),
        "Read-only exploration.",
    )];

    submit(&mut app, "/skill explore");
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::ApplySkill {
            name: "explore".to_string(),
            instructions: "Read-only exploration.".to_string(),
        }
    );
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::System { text })
            if text == "Applied skill explore (.claude)."
    ));
}

#[test]
fn unknown_definition_ids_error_and_send_nothing() {
    let (mut app, mut rx, _secrets) = test_app();
    app.agents = vec![sample_agent(
        "reviewer",
        DefinitionSource::Agents,
        None,
        None,
        "Review.",
    )];
    app.skills = vec![sample_skill(
        "explore",
        DefinitionSource::Agents,
        None,
        "Explore.",
    )];

    submit(&mut app, "/agent missing");
    assert!(rx.try_recv().is_err());
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::System { text })
            if text.contains("Unknown agent: missing") && text.contains("reviewer")
    ));

    submit(&mut app, "/agent");
    assert!(rx.try_recv().is_err());
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::System { text })
            if text.contains("Usage: /agent <id>") && text.contains("reviewer")
    ));

    submit(&mut app, "/skill missing");
    assert!(rx.try_recv().is_err());
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::System { text })
            if text.contains("Unknown skill: missing") && text.contains("explore")
    ));

    submit(&mut app, "/skill");
    assert!(rx.try_recv().is_err());
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::System { text })
            if text.contains("Usage: /skill <id>") && text.contains("explore")
    ));
}

#[test]
fn agent_model_with_provider_prefix_is_not_applied() {
    let (mut app, mut rx, _secrets) = test_app();
    app.agents = vec![sample_agent(
        "reviewer",
        DefinitionSource::Claude,
        None,
        Some("openrouter:deepseek-v4.1"),
        "Review.",
    )];

    submit(&mut app, "/agent reviewer");
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::ApplyAgent {
            name: "reviewer".to_string(),
            instructions: "Review.".to_string(),
            model: None,
        }
    );
    assert_eq!(app.tab().model, "gemini-3.8-flash");
    assert!(app.tab().transcript.iter().any(|entry| matches!(
        entry,
        TranscriptEntry::System { text } if text.contains("implies a provider change")
    )));
}

#[test]
fn agent_event_streaming_updates_transcript_and_status() {
    let (mut app, _rx, _secrets) = test_app();

    let turn_id = TurnId::from_raw("turn-1");
    app.handle_agent_event(AgentEvent::TurnStarted {
        turn_id: turn_id.clone(),
        model: "openai::gpt-4o-mini".to_string(),
    });
    assert!(app.tab().is_running);
    assert_eq!(app.tab().model, "gpt-4o-mini");
    assert_eq!(app.tab().turn_id, Some(turn_id));

    let reasoning_id = ItemId::from_raw("item-reasoning");
    app.handle_agent_event(AgentEvent::ItemStarted {
        item: SessionItem::Reasoning {
            id: reasoning_id.clone(),
            summary: String::new(),
        },
    });
    app.handle_agent_event(AgentEvent::ItemDelta {
        item_id: reasoning_id,
        delta: ItemDelta::ReasoningSummary("thinking...".to_string()),
    });

    let message_id = ItemId::from_raw("item-message");
    app.handle_agent_event(AgentEvent::ItemStarted {
        item: SessionItem::AgentMessage {
            id: message_id.clone(),
            text: String::new(),
        },
    });
    app.handle_agent_event(AgentEvent::ItemDelta {
        item_id: message_id.clone(),
        delta: ItemDelta::AgentText("Hello ".to_string()),
    });
    app.handle_agent_event(AgentEvent::ItemDelta {
        item_id: message_id.clone(),
        delta: ItemDelta::AgentText("world!".to_string()),
    });

    assert_eq!(
        app.tab().transcript,
        vec![TranscriptEntry::Assistant {
            text: "Hello world!".to_string(),
            reasoning: "thinking...".to_string(),
            streaming: true,
            reasoning_expanded: false,
        }]
    );

    assert!(app.toggle_latest_thinking());
    assert!(matches!(
        app.tab().transcript.first(),
        Some(TranscriptEntry::Assistant {
            reasoning_expanded: true,
            ..
        })
    ));

    let tool_item_id = ItemId::from_raw("item-tool");
    let call_id = meerai_agents::CallId::from_raw("c1");
    app.handle_agent_event(AgentEvent::ItemStarted {
        item: SessionItem::ToolCall {
            id: tool_item_id.clone(),
            call_id: call_id.clone(),
            name: "read_file".to_string(),
            args: "{\"path\":\"Cargo.toml\"}".to_string(),
            effect: EffectClass::Read,
            status: ItemStatus::Proposed,
            output: None,
        },
    });
    assert_eq!(app.tab().active_tool.as_deref(), Some("read_file"));

    app.handle_agent_event(AgentEvent::ItemDelta {
        item_id: tool_item_id.clone(),
        delta: ItemDelta::StatusChanged(ItemStatus::InProgress),
    });
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::ToolCall(ToolCallEntry {
            call_id: "c1".to_string(),
            name: "read_file".to_string(),
            args: "{\"path\":\"Cargo.toml\"}".to_string(),
            status: ToolCallStatus::Running,
            output: None,
        }))
    );

    app.handle_agent_event(AgentEvent::ItemCompleted {
        item: SessionItem::ToolCall {
            id: tool_item_id,
            call_id,
            name: "read_file".to_string(),
            args: "{\"path\":\"Cargo.toml\"}".to_string(),
            effect: EffectClass::Read,
            status: ItemStatus::Succeeded,
            output: Some("ok".to_string()),
        },
    });
    assert!(app.tab().active_tool.is_none());
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::ToolCall(ToolCallEntry {
            call_id: "c1".to_string(),
            name: "read_file".to_string(),
            args: "{\"path\":\"Cargo.toml\"}".to_string(),
            status: ToolCallStatus::Succeeded,
            output: Some("ok".to_string()),
        }))
    );

    app.handle_agent_event(AgentEvent::UsageUpdated {
        usage: Usage {
            prompt_tokens: Some(120),
            completion_tokens: Some(30),
            total_tokens: Some(150),
            cached_tokens: None,
        },
        cumulative: Usage {
            prompt_tokens: Some(120),
            completion_tokens: Some(30),
            total_tokens: Some(150),
            cached_tokens: None,
        },
        est_cost_usd: None,
    });
    assert_eq!(app.tab().usage.total_tokens, Some(150));
    assert_eq!(app.tab().metrics.session_total_tokens, 150);

    app.handle_agent_event(AgentEvent::ItemCompleted {
        item: SessionItem::AgentMessage {
            id: ItemId::from_raw("item-done"),
            text: "Done".to_string(),
        },
    });
    app.handle_agent_event(AgentEvent::TurnCompleted {
        turn_id: TurnId::from_raw("turn-1"),
        reason: TurnEndReason::Completed,
    });
    assert!(!app.tab().is_running);
    assert_eq!(app.tab().metrics.turn_count, 1);
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::Assistant {
            text: "Done".to_string(),
            reasoning: String::new(),
            streaming: false,
            reasoning_expanded: false,
        })
    );

    app.handle_agent_event(AgentEvent::Error {
        code: ErrorCode::Provider,
        message: "boom".to_string(),
    });
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::Error {
            text: "provider error: boom".to_string(),
            expanded: false,
        })
    );
    assert!(app.toggle_latest_thinking());
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::Error {
            text: "provider error: boom".to_string(),
            expanded: true,
        })
    );
}

#[test]
fn authoritative_agent_message_replaces_streamed_text() {
    let (mut app, _rx, _secrets) = test_app();
    app.handle_agent_event(AgentEvent::TurnStarted {
        turn_id: TurnId::from_raw("turn-authoritative"),
        model: "openai::gpt-4o-mini".to_string(),
    });

    let id = ItemId::from_raw("item-message-authoritative");
    app.handle_agent_event(AgentEvent::ItemStarted {
        item: SessionItem::AgentMessage {
            id: id.clone(),
            text: String::new(),
        },
    });
    app.handle_agent_event(AgentEvent::ItemDelta {
        item_id: id.clone(),
        delta: ItemDelta::AgentText("streamed draft".to_string()),
    });
    app.handle_agent_event(AgentEvent::ItemCompleted {
        item: SessionItem::AgentMessage {
            id: id.clone(),
            text: "final answer".to_string(),
        },
    });
    assert_eq!(
        app.tab().transcript,
        vec![TranscriptEntry::Assistant {
            text: "final answer".to_string(),
            reasoning: String::new(),
            streaming: false,
            reasoning_expanded: false,
        }]
    );

    let empty_id = ItemId::from_raw("item-message-empty");
    app.handle_agent_event(AgentEvent::ItemStarted {
        item: SessionItem::AgentMessage {
            id: empty_id.clone(),
            text: String::new(),
        },
    });
    app.handle_agent_event(AgentEvent::ItemDelta {
        item_id: empty_id.clone(),
        delta: ItemDelta::AgentText("kept stream".to_string()),
    });
    app.handle_agent_event(AgentEvent::ItemCompleted {
        item: SessionItem::AgentMessage {
            id: empty_id,
            text: String::new(),
        },
    });
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::Assistant {
            text: "kept stream".to_string(),
            reasoning: String::new(),
            streaming: false,
            reasoning_expanded: false,
        })
    );
}

#[test]
fn aborted_and_failed_turns_render_terminal_entries() {
    let (mut app, _rx, _secrets) = test_app();

    app.handle_agent_event(AgentEvent::TurnStarted {
        turn_id: TurnId::from_raw("turn-abort"),
        model: "openai::gpt-4o-mini".to_string(),
    });
    app.handle_agent_event(AgentEvent::TurnCompleted {
        turn_id: TurnId::from_raw("turn-abort"),
        reason: TurnEndReason::Aborted {
            cause: AbortCause::User,
        },
    });
    assert!(!app.tab().is_running);
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::System {
            text: "Turn cancelled.".to_string(),
        })
    );

    app.handle_agent_event(AgentEvent::TurnStarted {
        turn_id: TurnId::from_raw("turn-failed"),
        model: "openai::gpt-4o-mini".to_string(),
    });
    app.handle_agent_event(AgentEvent::Error {
        code: ErrorCode::Budget,
        message: "Exceeded maximum number of cycles (16)".to_string(),
    });
    app.handle_agent_event(AgentEvent::TurnCompleted {
        turn_id: TurnId::from_raw("turn-failed"),
        reason: TurnEndReason::Failed {
            error: "Exceeded maximum number of cycles (16)".to_string(),
        },
    });
    assert!(!app.tab().is_running);
    assert_eq!(app.tab().metrics.turn_count, 2);
    assert_eq!(app.tab().transcript.len(), 2);
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::Error {
            text: "budget error: Exceeded maximum number of cycles (16)".to_string(),
            expanded: false,
        })
    );
}

#[test]
fn compaction_items_render_system_entries() {
    let (mut app, _rx, _secrets) = test_app();
    app.handle_agent_event(AgentEvent::ItemCompleted {
        item: SessionItem::Compaction {
            id: ItemId::from_raw("item-compact"),
            trigger: meerai_agents::events::CompactionTrigger::Manual,
            freed_tokens: 2048,
        },
    });
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::System {
            text: "Compaction freed 2048 tokens".to_string(),
        })
    );
}

#[test]
fn approval_request_opens_overlay_and_accept_resolves() {
    let (mut app, mut rx, _secrets) = test_app();
    app.handle_agent_event(AgentEvent::ApprovalRequested {
        request: approval_request("req-1", "src/main.rs"),
    });
    assert!(matches!(
        app.modal,
        Some(ActiveModal::Approval { ref queue, .. }) if queue.len() == 1
    ));

    app.handle_key_event(key(KeyCode::Char('y')));
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::Approve {
            request_id: RequestId::from_raw("req-1"),
            decision: ApprovalDecision::Accept,
            scope: None,
        }
    );

    app.handle_agent_event(AgentEvent::ApprovalResolved {
        request_id: RequestId::from_raw("req-1"),
    });
    assert!(app.modal.is_none());
}

#[test]
fn approval_turn_scope_key_sends_turn_grant() {
    let (mut app, mut rx, _secrets) = test_app();
    app.handle_agent_event(AgentEvent::ApprovalRequested {
        request: approval_request("req-turn", "src/main.rs"),
    });

    app.handle_key_event(key(KeyCode::Char('t')));
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::Approve {
            request_id: RequestId::from_raw("req-turn"),
            decision: ApprovalDecision::Accept,
            scope: Some(GrantScope::Turn),
        }
    );
}

#[test]
fn transcript_from_events_replays_known_entries_and_skips_runtime_events() {
    use meerai_core::ToolObservation;
    use meerai_session::{
        CallId, CompactionId, ContentBlock, InputSource, LoggedEvent, Seq, SeqRange, SessionEvent,
    };

    let events = vec![
        LoggedEvent {
            seq: Seq(1),
            event: SessionEvent::SystemMessage {
                sections: Vec::new(),
                generation: 0,
            },
        },
        LoggedEvent {
            seq: Seq(2),
            event: SessionEvent::UserMessage {
                source: InputSource::RuntimeContext,
                content: vec![ContentBlock::Text("runtime context".to_string())],
            },
        },
        LoggedEvent {
            seq: Seq(3),
            event: SessionEvent::UserMessage {
                source: InputSource::User,
                content: vec![ContentBlock::Text("hello".to_string())],
            },
        },
        LoggedEvent {
            seq: Seq(4),
            event: SessionEvent::AssistantMessage {
                provider: "gemini".to_string(),
                model: "gemini-3.8-flash".to_string(),
                content: vec![
                    ContentBlock::Reasoning("thinking".to_string()),
                    ContentBlock::Text("answer".to_string()),
                ],
                tool_calls: Vec::new(),
                usage: Usage::default(),
                stream_digest: meerai_session::StreamDigest::default(),
            },
        },
        LoggedEvent {
            seq: Seq(5),
            event: SessionEvent::ToolCall {
                call_id: CallId::from_raw("call-1"),
                name: "code-read_file".to_string(),
                args: "{\"path\":\"src/lib.rs\"}".to_string(),
            },
        },
        LoggedEvent {
            seq: Seq(6),
            event: SessionEvent::ToolResult {
                call_id: CallId::from_raw("call-1"),
                output: ToolObservation::success("file contents"),
            },
        },
        LoggedEvent {
            seq: Seq(7),
            event: SessionEvent::ToolCall {
                call_id: CallId::from_raw("call-2"),
                name: "code-shell".to_string(),
                args: "{}".to_string(),
            },
        },
        LoggedEvent {
            seq: Seq(8),
            event: SessionEvent::CompactionSummary {
                compaction_id: CompactionId::from_raw("cmp-1"),
                replaced: SeqRange {
                    start: Seq(1),
                    end_inclusive: Seq(4),
                },
                summary: "summary".to_string(),
            },
        },
        LoggedEvent {
            seq: Seq(9),
            event: SessionEvent::StepStarted {
                step_id: meerai_session::StepId::from_raw("step-1"),
                turn_id: meerai_session::TurnId::from_raw("turn-1"),
            },
        },
    ];

    assert_eq!(
        transcript_from_events(&events),
        vec![
            TranscriptEntry::User {
                text: "hello".to_string(),
            },
            TranscriptEntry::Assistant {
                text: "answer".to_string(),
                reasoning: "thinking".to_string(),
                streaming: false,
                reasoning_expanded: false,
            },
            TranscriptEntry::ToolCall(ToolCallEntry {
                call_id: "call-1".to_string(),
                name: "code-read_file".to_string(),
                args: "{\"path\":\"src/lib.rs\"}".to_string(),
                status: ToolCallStatus::Succeeded,
                output: Some("file contents".to_string()),
            }),
            TranscriptEntry::ToolCall(ToolCallEntry {
                call_id: "call-2".to_string(),
                name: "code-shell".to_string(),
                args: "{}".to_string(),
                status: ToolCallStatus::Cancelled,
                output: None,
            }),
            TranscriptEntry::System {
                text: "Compacted history".to_string(),
            },
        ]
    );
}

#[test]
fn transcript_from_events_marks_unmatched_tool_calls_cancelled() {
    use meerai_session::{CallId, LoggedEvent, Seq, SessionEvent};

    let events = vec![LoggedEvent {
        seq: Seq(1),
        event: SessionEvent::ToolCall {
            call_id: CallId::from_raw("call-crashed"),
            name: "code-shell".to_string(),
            args: "{}".to_string(),
        },
    }];

    assert_eq!(
        transcript_from_events(&events),
        vec![TranscriptEntry::ToolCall(ToolCallEntry {
            call_id: "call-crashed".to_string(),
            name: "code-shell".to_string(),
            args: "{}".to_string(),
            status: ToolCallStatus::Cancelled,
            output: None,
        })]
    );
}

#[test]
fn transcript_from_events_excludes_reverted_turns() {
    use meerai_core::ToolObservation;
    use meerai_session::{
        CallId, CompactionId, ContentBlock, InputSource, LoggedEvent, Seq, SeqRange, SessionEvent,
        TurnTrigger,
    };

    let events = vec![
        LoggedEvent {
            seq: Seq(1),
            event: SessionEvent::UserMessage {
                source: InputSource::User,
                content: vec![ContentBlock::Text("keep prompt".to_string())],
            },
        },
        LoggedEvent {
            seq: Seq(2),
            event: SessionEvent::AssistantMessage {
                provider: "gemini".to_string(),
                model: "gemini-3.8-flash".to_string(),
                content: vec![ContentBlock::Text("keep reply".to_string())],
                tool_calls: Vec::new(),
                usage: Usage::default(),
                stream_digest: meerai_session::StreamDigest::default(),
            },
        },
        LoggedEvent {
            seq: Seq(3),
            event: SessionEvent::TurnStarted {
                turn_id: TurnId::from_raw("turn-reverted"),
                trigger: TurnTrigger::UserPrompt,
            },
        },
        LoggedEvent {
            seq: Seq(4),
            event: SessionEvent::UserMessage {
                source: InputSource::User,
                content: vec![ContentBlock::Text("reverted prompt".to_string())],
            },
        },
        LoggedEvent {
            seq: Seq(5),
            event: SessionEvent::ToolCall {
                call_id: CallId::from_raw("call-reverted"),
                name: "code-write_file".to_string(),
                args: "{}".to_string(),
            },
        },
        LoggedEvent {
            seq: Seq(6),
            event: SessionEvent::ToolResult {
                call_id: CallId::from_raw("call-reverted"),
                output: ToolObservation::success("written"),
            },
        },
        LoggedEvent {
            seq: Seq(7),
            event: SessionEvent::AssistantMessage {
                provider: "gemini".to_string(),
                model: "gemini-3.8-flash".to_string(),
                content: vec![ContentBlock::Text("reverted reply".to_string())],
                tool_calls: Vec::new(),
                usage: Usage::default(),
                stream_digest: meerai_session::StreamDigest::default(),
            },
        },
        LoggedEvent {
            seq: Seq(8),
            event: SessionEvent::RevertApplied {
                turn_id: TurnId::from_raw("turn-reverted"),
                replaced: SeqRange {
                    start: Seq(3),
                    end_inclusive: Seq(7),
                },
            },
        },
        LoggedEvent {
            seq: Seq(9),
            event: SessionEvent::CompactionSummary {
                compaction_id: CompactionId::from_raw("cmp-keep"),
                replaced: SeqRange {
                    start: Seq(1),
                    end_inclusive: Seq(2),
                },
                summary: "summary".to_string(),
            },
        },
        LoggedEvent {
            seq: Seq(10),
            event: SessionEvent::UserMessage {
                source: InputSource::User,
                content: vec![ContentBlock::Text("after revert".to_string())],
            },
        },
    ];

    assert_eq!(
        transcript_from_events(&events),
        vec![
            TranscriptEntry::User {
                text: "keep prompt".to_string(),
            },
            TranscriptEntry::Assistant {
                text: "keep reply".to_string(),
                reasoning: String::new(),
                streaming: false,
                reasoning_expanded: false,
            },
            TranscriptEntry::System {
                text: "Compacted history".to_string(),
            },
            TranscriptEntry::User {
                text: "after revert".to_string(),
            },
        ]
    );
}

#[test]
fn session_usage_from_events_returns_last_cumulative_usage() {
    use meerai_session::{LoggedEvent, Seq, SessionEvent};

    let events = vec![
        LoggedEvent {
            seq: Seq(1),
            event: SessionEvent::UsageUpdated {
                usage: Usage::default(),
                cumulative: Usage {
                    prompt_tokens: Some(10),
                    completion_tokens: Some(5),
                    total_tokens: Some(15),
                    cached_tokens: None,
                },
            },
        },
        LoggedEvent {
            seq: Seq(2),
            event: SessionEvent::UsageUpdated {
                usage: Usage::default(),
                cumulative: Usage {
                    prompt_tokens: Some(20),
                    completion_tokens: Some(7),
                    total_tokens: Some(27),
                    cached_tokens: None,
                },
            },
        },
    ];

    assert_eq!(
        session_usage_from_events(&events),
        Some(Usage {
            prompt_tokens: Some(20),
            completion_tokens: Some(7),
            total_tokens: Some(27),
            cached_tokens: None,
        })
    );
    assert_eq!(session_usage_from_events(&[]), None);
}

#[test]
fn approval_queue_keeps_order_and_closes_after_all_resolved() {
    let (mut app, mut rx, _secrets) = test_app();
    app.handle_agent_event(AgentEvent::ApprovalRequested {
        request: approval_request("req-1", "src/a.rs"),
    });
    app.handle_agent_event(AgentEvent::ApprovalRequested {
        request: approval_request("req-2", "src/b.rs"),
    });
    let Some(ActiveModal::Approval { queue, .. }) = &app.modal else {
        panic!("expected approval overlay");
    };
    assert_eq!(queue.len(), 2);
    assert_eq!(queue[0].request_id, RequestId::from_raw("req-1"));
    assert_eq!(queue[0].preview.title, "edit: src/a.rs");

    app.handle_key_event(key(KeyCode::Char('a')));
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::Approve {
            request_id: RequestId::from_raw("req-1"),
            decision: ApprovalDecision::AcceptForSession,
            scope: None,
        }
    );

    app.handle_agent_event(AgentEvent::ApprovalResolved {
        request_id: RequestId::from_raw("req-1"),
    });
    assert!(matches!(
        app.modal,
        Some(ActiveModal::Approval { ref queue, .. }) if queue.len() == 1
    ));

    app.handle_agent_event(AgentEvent::ApprovalResolved {
        request_id: RequestId::from_raw("req-2"),
    });
    assert!(app.modal.is_none());
}

#[test]
fn approval_decline_with_reason_and_cancel_keys() {
    let (mut app, mut rx, _secrets) = test_app();
    app.handle_agent_event(AgentEvent::ApprovalRequested {
        request: approval_request("req-1", "src/main.rs"),
    });

    app.handle_key_event(key(KeyCode::Char('r')));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::Approval {
            editing_reason: true,
            ..
        })
    ));
    type_text(&mut app, "risky change");
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::Approve {
            request_id: RequestId::from_raw("req-1"),
            decision: ApprovalDecision::DeclineWithReason("risky change".to_string()),
            scope: None,
        }
    );

    app.handle_agent_event(AgentEvent::ApprovalResolved {
        request_id: RequestId::from_raw("req-1"),
    });
    app.handle_agent_event(AgentEvent::ApprovalRequested {
        request: approval_request("req-2", "src/lib.rs"),
    });
    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::Approve {
            request_id: RequestId::from_raw("req-2"),
            decision: ApprovalDecision::Cancel,
            scope: None,
        }
    );
}

#[test]
fn compact_and_mode_commands_emit_agent_commands() {
    let (mut app, mut rx, _secrets) = test_app();

    type_text(&mut app, "/compact");
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(rx.try_recv().unwrap(), AgentCommand::Compact);
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::System { text }) if text.contains("Compaction requested")
    ));

    type_text(&mut app, "/mode ");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(rx.try_recv().is_err());
    assert_eq!(app.tab().approval_mode, ApprovalMode::Ask);
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::System { text }) if text == "Approval mode: ask"
    ));

    type_text(&mut app, "/mode auto-edit");
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::SetPermissionMode(ApprovalMode::AutoEdit)
    );
    assert_eq!(app.tab().approval_mode, ApprovalMode::AutoEdit);

    type_text(&mut app, "/mode nonsense");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(rx.try_recv().is_err());
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::Error { text, .. }) if text.contains("Invalid mode")
    ));
}

#[test]
fn steer_while_running_sends_steer_and_keeps_running() {
    let (mut app, mut rx, _secrets) = test_app();
    app.tab_mut().is_running = true;
    app.tab_mut().turn_id = Some(TurnId::from_raw("turn-steer"));

    type_text(&mut app, "focus on tests");
    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::Steer("focus on tests".to_string())
    );
    assert!(app.tab().is_running);
    assert_eq!(app.tab().turn_id, Some(TurnId::from_raw("turn-steer")));
    assert!(app.tab().input.is_empty());
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::User { text }) if text == "focus on tests"
    ));
}

fn approval_request(id: &str, path: &str) -> ApprovalRequest {
    ApprovalRequest {
        request_id: RequestId::from_raw(id),
        turn_id: TurnId::from_raw("turn-approval"),
        kind: ApprovalKind::ToolCall,
        preview: ApprovalPreview {
            title: format!("edit: {path}"),
            detail: format!("tool: code-write_file\neffect: write\nresource: {path}"),
        },
        effect: Some(EffectClass::Write),
    }
}

#[test]
fn handles_mouse_thinking_toggle() {
    let (mut app, _rx, _secrets) = test_app();
    app.tab_mut().transcript.push(TranscriptEntry::Assistant {
        text: "Hello!".to_string(),
        reasoning: "line 1\nline 2".to_string(),
        streaming: false,
        reasoning_expanded: false,
    });
    app.transcript_inner_area = Rect::new(1, 1, 60, 10);
    app.thinking_hit_zones = vec![ThinkingHitZone {
        entry_idx: 0,
        start_line: 0,
        end_line: 1,
    }];

    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 1,
        modifiers: KeyModifiers::NONE,
    };
    assert!(app.handle_mouse_event(click));
    assert!(matches!(
        app.tab().transcript.first(),
        Some(TranscriptEntry::Assistant {
            reasoning_expanded: true,
            ..
        })
    ));
}

#[test]
fn clicking_file_change_opens_diff_modal_and_search_attaches_file() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "meerai_app_diff_search_{}_{}",
        std::process::id(),
        nanos
    ));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/demo.rs"),
        "fn demo() {\n    let target = 1;\n}\n",
    )
    .unwrap();

    let (tx, _rx) = mpsc::unbounded_channel();
    let secrets = Arc::new(MemorySecretStore::new());
    secrets.set_key("gemini", "test-key").unwrap();
    let settings = AppSettings::default();
    let runtime = Arc::new(RwLock::new(RuntimeProviderState::from_settings(
        &settings,
        ProviderCatalog::new(),
    )));
    let mut app = AppState::new(
        "gemini",
        "gemini-3.8-flash",
        root.clone(),
        tx,
        runtime,
        secrets,
        settings,
        None,
    );

    app.handle_terminal_event(Event::Resize(90, 30));
    assert!(!app.is_sidebar_visible());
    app.handle_key_event(key_mod(KeyCode::Char('b'), KeyModifiers::CONTROL));
    app.handle_key_event(key_mod(KeyCode::Char('b'), KeyModifiers::CONTROL));
    assert!(app.status_notice.is_some());

    app.handle_terminal_event(Event::Resize(120, 30));
    assert!(app.is_sidebar_visible());

    app.file_changes.push(FileChangeEntry {
        status: "??".to_string(),
        path: "src/demo.rs".to_string(),
    });
    app.sidebar_changes_inner_area = Rect::new(85, 16, 32, 10);
    app.visible_file_change_rows = 1;

    let file_click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 90,
        row: 16,
        modifiers: KeyModifiers::NONE,
    };
    assert!(app.handle_mouse_event(file_click));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::DiffView {
            file_index: 0,
            ref diff,
            ..
        }) if diff.path == "src/demo.rs" && diff.additions == 3
    ));

    app.handle_key_event(key(KeyCode::Esc));
    assert!(app.modal.is_none());

    app.handle_key_event(key_mod(KeyCode::Char('f'), KeyModifiers::CONTROL));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::Search {
            mode: SearchMode::Files,
            ..
        })
    ));

    app.handle_key_event(key(KeyCode::Tab));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::Search {
            mode: SearchMode::Content,
            ..
        })
    ));
    type_text(&mut app, "target");
    assert!(matches!(
        app.modal,
        Some(ActiveModal::Search { ref results, .. }) if results.len() == 1 && results[0].line == Some(2)
    ));

    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.modal.is_none());
    assert_eq!(app.tab().input, "@src/demo.rs#L2 ");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn detached_search_opens_immediately_from_slash_space_and_normal_mode() {
    let (mut app, _rx, _secrets) = test_app();

    type_text(&mut app, "/se");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::Search {
            mode: SearchMode::Files,
            ..
        })
    ));
    assert!(app.tab().input.is_empty());

    app.handle_key_event(key(KeyCode::Esc));
    assert!(app.modal.is_none());

    type_text(&mut app, "/grep ");
    assert!(matches!(
        app.modal,
        Some(ActiveModal::Search {
            mode: SearchMode::Content,
            ..
        })
    ));
    assert!(app.tab().input.is_empty());

    app.handle_key_event(key(KeyCode::Esc));
    type_text(&mut app, "draft prompt");
    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(app.mode, InputMode::Normal);

    app.handle_key_event(key(KeyCode::Char('/')));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::Search {
            mode: SearchMode::Files,
            ..
        })
    ));
    assert_eq!(app.tab().input, "draft prompt");
}

#[test]
fn slash_auth_lists_configured_credential_sources() {
    let (mut app, _rx, secrets) = test_app();
    secrets.set_key("openai", "sk-stored-12345678").unwrap();
    let before_len = app.tab().transcript.len();

    type_text(&mut app, "/auth");
    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(app.tab().transcript.len(), before_len + 1);
    let Some(TranscriptEntry::System { text }) = app.tab().transcript.last() else {
        panic!("expected system transcript entry for /auth");
    };
    assert!(text.contains("Credentials ("));
    assert!(text.contains("openai"));
    assert!(text.contains("sk-s...5678"));
}

#[test]
fn drag_selection_auto_copies_within_pane_and_normal_yank_works() {
    let (mut app, _rx, _secrets) = test_app();
    app.terminal_width = 30;
    app.transcript_inner_area = Rect::new(1, 1, 20, 4);
    let mk_row = |s: &str| -> Vec<String> {
        let mut row: Vec<String> = s.chars().map(|c| c.to_string()).collect();
        row.resize(30, " ".to_string());
        row
    };
    app.screen_lines = vec![
        mk_row("┌────────────────────┐┌──────┐"),
        mk_row("│┃ user prompt       ││side1 │"),
        mk_row("│  assistant reply   ││side2 │"),
        mk_row("└────────────────────┘└──────┘"),
    ];

    assert!(app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 1,
        row: 1,
        modifiers: KeyModifiers::NONE,
    }));
    assert!(app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 25,
        row: 2,
        modifiers: KeyModifiers::NONE,
    }));
    assert!(matches!(
        app.selection,
        Some(TextSelection { active: true, .. })
    ));

    assert!(app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 25,
        row: 2,
        modifiers: KeyModifiers::NONE,
    }));
    assert!(app.selection.is_none());
    let copied = app.last_copied_text.as_deref().unwrap_or("");
    assert_eq!(copied, "user prompt\n  assistant reply");
    assert!(!copied.contains("side1"));
    assert!(!copied.contains("│"));
    assert!(
        app.status_notice
            .as_deref()
            .is_some_and(|n| n.contains("Copied"))
    );

    app.tab_mut().transcript.push(TranscriptEntry::Assistant {
        text: "Latest assistant answer".to_string(),
        reasoning: String::new(),
        streaming: false,
        reasoning_expanded: false,
    });
    app.mode = InputMode::Normal;
    assert!(app.handle_key_event(key(KeyCode::Char('y'))));
    assert_eq!(
        app.last_copied_text.as_deref(),
        Some("Latest assistant answer")
    );

    assert_eq!(encode_base64(b"foo"), "Zm9v");
    assert_eq!(encode_base64(b"fo"), "Zm8=");
    assert_eq!(encode_base64(b"f"), "Zg==");
}

#[test]
fn subagent_run_lifecycle_folds_and_renders() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let (mut app, _rx, _secrets) = test_app();
    app.handle_agent_event(AgentEvent::TurnStarted {
        turn_id: TurnId::from_raw("turn-subagent"),
        model: "gemini::gemini-3.8-flash".to_string(),
    });

    let subagent_id = ItemId::from_raw("item-sub-1");
    app.handle_agent_event(AgentEvent::ItemStarted {
        item: SessionItem::SubagentRun {
            id: subagent_id.clone(),
            agent: "reviewer".to_string(),
            status: ItemStatus::Proposed,
            report: None,
        },
    });
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::Subagent(SubagentEntry {
            id: "item-sub-1".to_string(),
            agent: "reviewer".to_string(),
            status: ToolCallStatus::Proposed,
            report: None,
        }))
    );

    app.handle_agent_event(AgentEvent::ItemDelta {
        item_id: subagent_id.clone(),
        delta: ItemDelta::StatusChanged(ItemStatus::InProgress),
    });
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::Subagent(SubagentEntry {
            id: "item-sub-1".to_string(),
            agent: "reviewer".to_string(),
            status: ToolCallStatus::Running,
            report: None,
        }))
    );

    app.handle_agent_event(AgentEvent::ItemCompleted {
        item: SessionItem::SubagentRun {
            id: subagent_id,
            agent: "reviewer".to_string(),
            status: ItemStatus::Succeeded,
            report: Some("Line one\nLine two".to_string()),
        },
    });
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::Subagent(SubagentEntry {
            id: "item-sub-1".to_string(),
            agent: "reviewer".to_string(),
            status: ToolCallStatus::Succeeded,
            report: Some("Line one\nLine two".to_string()),
        }))
    );

    let backend = TestBackend::new(120, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| crate::ui::render(frame, &mut app))
        .unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(rendered.contains("▸ [subagent] reviewer [ok]"));
    assert!(rendered.contains("Line one Line two"));
}

#[test]
fn subagent_entry_finalizes_on_parent_turn_failure_and_abort() {
    let (mut app, _rx, _secrets) = test_app();

    app.handle_agent_event(AgentEvent::TurnStarted {
        turn_id: TurnId::from_raw("t-abort"),
        model: "gemini::gemini-3.8-flash".to_string(),
    });
    app.handle_agent_event(AgentEvent::ItemStarted {
        item: SessionItem::SubagentRun {
            id: ItemId::from_raw("sub-abort"),
            agent: "explore".to_string(),
            status: ItemStatus::InProgress,
            report: None,
        },
    });
    app.handle_agent_event(AgentEvent::TurnCompleted {
        turn_id: TurnId::from_raw("t-abort"),
        reason: TurnEndReason::Aborted {
            cause: AbortCause::User,
        },
    });
    assert!(app.tab().transcript.iter().any(|entry| {
        matches!(
            entry,
            TranscriptEntry::Subagent(s) if s.id == "sub-abort" && s.status == ToolCallStatus::Cancelled
        )
    }));

    app.handle_agent_event(AgentEvent::TurnStarted {
        turn_id: TurnId::from_raw("t-fail"),
        model: "gemini::gemini-3.8-flash".to_string(),
    });
    app.handle_agent_event(AgentEvent::ItemStarted {
        item: SessionItem::SubagentRun {
            id: ItemId::from_raw("sub-fail"),
            agent: "build".to_string(),
            status: ItemStatus::InProgress,
            report: None,
        },
    });
    app.handle_agent_event(AgentEvent::TurnCompleted {
        turn_id: TurnId::from_raw("t-fail"),
        reason: TurnEndReason::Failed {
            error: "budget exceeded".to_string(),
        },
    });
    assert!(app.tab().transcript.iter().any(|entry| {
        matches!(
            entry,
            TranscriptEntry::Subagent(s) if s.id == "sub-fail" && s.status == ToolCallStatus::Failed
        )
    }));
}

struct DummySessionOpener;

#[meerai_core::async_trait]
impl SessionOpener for DummySessionOpener {
    async fn list_sessions(&self) -> anyhow::Result<Vec<meerai_session::SessionSummary>> {
        Ok(Vec::new())
    }

    async fn open_session(&self, id: &SessionId) -> anyhow::Result<OpenedSession> {
        anyhow::bail!("unexpected open_session call for {id}")
    }

    async fn create_session(&self) -> anyhow::Result<OpenedSession> {
        anyhow::bail!("unexpected create_session call")
    }
}

fn make_opened_session(
    id: &str,
    title: Option<&str>,
    transcript: Vec<TranscriptEntry>,
) -> (OpenedSession, mpsc::UnboundedReceiver<AgentCommand>) {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (_ev_tx, event_rx) = mpsc::unbounded_channel();
    let opened = OpenedSession {
        session_id: SessionId::from_raw(id),
        title: title.map(str::to_string),
        provider: "gemini".to_string(),
        model: "gemini-3.8-flash".to_string(),
        approval_mode: ApprovalMode::Ask,
        transcript,
        events: Vec::new(),
        cmd_tx,
        event_rx,
    };
    (opened, cmd_rx)
}

fn make_session_summary(
    id: &str,
    title: Option<&str>,
    updated_at_ms: u64,
    parent: Option<&str>,
) -> meerai_session::SessionSummary {
    meerai_session::SessionSummary {
        id: SessionId::from_raw(id),
        dir: PathBuf::from(format!("/tmp/{id}")),
        title: title.map(str::to_string),
        model: Some("gemini-3.8-flash".to_string()),
        created_at_ms: updated_at_ms.saturating_sub(1_000),
        updated_at_ms,
        bytes: 128,
        parent_session_id: parent.map(SessionId::from_raw),
    }
}

#[test]
fn session_tabs_open_replace_empty_dedup_switch_and_close() {
    let (mut app, mut initial_rx, _secrets) = test_app();
    assert_eq!(app.tabs().len(), 1);
    assert!(app.should_replace_active_tab());

    let (opened_first, _rx_first) = make_opened_session(
        "sess-1",
        Some("Refactor parser"),
        vec![TranscriptEntry::User {
            text: "Hello".to_string(),
        }],
    );
    assert!(app.apply_opened_session(opened_first, true).is_some());
    assert_eq!(initial_rx.try_recv().unwrap(), AgentCommand::Quit);
    assert_eq!(app.tabs().len(), 1);
    assert_eq!(app.active_tab_index(), 0);
    assert_eq!(app.tab().session_id, SessionId::from_raw("sess-1"));
    assert_eq!(app.tab().title.as_deref(), Some("Refactor parser"));

    let (opened_second, _rx_second) = make_opened_session("sess-2", Some("Fix tests"), Vec::new());
    assert!(app.apply_opened_session(opened_second, true).is_some());
    assert_eq!(app.tabs().len(), 2);
    assert_eq!(app.active_tab_index(), 1);
    assert_eq!(app.tab().session_id, SessionId::from_raw("sess-2"));

    let (dup_first, mut dup_rx) =
        make_opened_session("sess-1", Some("Refactor parser"), Vec::new());
    assert!(app.apply_opened_session(dup_first, false).is_none());
    assert_eq!(dup_rx.try_recv().unwrap(), AgentCommand::Quit);
    assert_eq!(app.tabs().len(), 2);
    assert_eq!(app.active_tab_index(), 0);

    assert!(app.next_tab());
    assert_eq!(app.active_tab_index(), 1);
    assert!(app.prev_tab());
    assert_eq!(app.active_tab_index(), 0);

    app.handle_key_event(key_mod(KeyCode::Char('2'), KeyModifiers::ALT));
    assert_eq!(app.active_tab_index(), 1);

    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(app.mode, InputMode::Normal);
    app.handle_key_event(key(KeyCode::Char('g')));
    app.handle_key_event(key(KeyCode::Char('T')));
    assert_eq!(app.active_tab_index(), 0);
    app.handle_key_event(key(KeyCode::Char(']')));
    assert_eq!(app.active_tab_index(), 1);
    app.handle_key_event(key(KeyCode::Char('[')));
    assert_eq!(app.active_tab_index(), 0);

    app.handle_key_event(key_mod(KeyCode::Char('w'), KeyModifiers::CONTROL));
    assert_eq!(app.tabs().len(), 1);
    assert_eq!(app.tab().session_id, SessionId::from_raw("sess-2"));

    app.handle_key_event(key(KeyCode::Char('i')));
    assert_eq!(app.mode, InputMode::Insert);
    type_text(&mut app, "/close");
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(app.tabs().len(), 1);
    assert_eq!(
        app.status_notice.as_deref(),
        Some("Cannot close the last tab (use /quit to exit)")
    );
}

#[test]
fn background_tab_events_track_unread_and_restore_pending_approvals() {
    let (mut app, _rx1, _secrets) = test_app();
    let tab1_id = app.tab().tab_id;

    let (opened_second, mut rx2) = make_opened_session("sess-bg", Some("Background"), Vec::new());
    let (tab2_id, _) = app.apply_opened_session(opened_second, false).unwrap();
    assert_eq!(app.active_tab_index(), 1);

    assert!(app.switch_to_tab(0));
    assert_eq!(app.tab().tab_id, tab1_id);
    assert!(!app.tabs()[1].has_unread);

    app.handle_tab_agent_event(
        tab2_id,
        AgentEvent::TurnStarted {
            turn_id: TurnId::from_raw("t-bg"),
            model: "gemini::gemini-3.8-flash".to_string(),
        },
    );
    app.handle_tab_agent_event(
        tab2_id,
        AgentEvent::ItemDelta {
            item_id: ItemId::from_raw("item-bg"),
            delta: ItemDelta::AgentText("Done in background".to_string()),
        },
    );
    app.handle_tab_agent_event(
        tab2_id,
        AgentEvent::ItemCompleted {
            item: SessionItem::AgentMessage {
                id: ItemId::from_raw("item-bg"),
                text: "Done in background".to_string(),
            },
        },
    );
    app.handle_tab_agent_event(
        tab2_id,
        AgentEvent::ApprovalRequested {
            request: approval_request("req-bg-1", "src/lib.rs"),
        },
    );

    assert!(app.tabs()[1].has_unread);
    assert!(app.tabs()[1].is_running);
    assert_eq!(app.tabs()[1].pending_approvals.len(), 1);
    assert!(app.modal.is_none());

    assert!(app.switch_to_tab(1));
    assert!(!app.tabs()[1].has_unread);
    assert!(matches!(
        app.modal,
        Some(ActiveModal::Approval { ref queue, .. })
            if queue.len() == 1 && queue[0].request_id == RequestId::from_raw("req-bg-1")
    ));

    app.handle_key_event(key(KeyCode::Char('y')));
    assert_eq!(
        rx2.try_recv().unwrap(),
        AgentCommand::Approve {
            request_id: RequestId::from_raw("req-bg-1"),
            decision: ApprovalDecision::Accept,
            scope: None,
        }
    );
}

#[test]
fn sessions_command_and_modal_filter_select_and_new() {
    let (mut app, _rx, _secrets) = test_app();
    app.session_opener = Some(Arc::new(DummySessionOpener));

    type_text(&mut app, "/sessions");
    app.handle_key_event(key(KeyCode::Enter));

    assert!(matches!(
        app.modal,
        Some(ActiveModal::SessionSelect { loading: true, .. })
    ));
    assert_eq!(
        app.take_pending_session_actions(),
        vec![PendingSessionAction::RefreshSessions]
    );

    app.apply_listed_sessions(Ok(vec![
        make_session_summary("sess-alpha", Some("Alpha task"), 300, None),
        make_session_summary("sess-child", Some("Subagent"), 250, Some("sess-alpha")),
        make_session_summary("sess-beta", Some("Beta bugfix"), 200, None),
    ]));

    let filtered_len = match &app.modal {
        Some(ActiveModal::SessionSelect {
            filter, sessions, ..
        }) => {
            assert_eq!(sessions.len(), 3);
            app.filtered_sessions(sessions, filter).len()
        }
        other => panic!("expected SessionSelect modal, got {other:?}"),
    };
    assert_eq!(filtered_len, 2);

    type_text(&mut app, "beta");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.modal.is_none());
    assert_eq!(
        app.take_pending_session_actions(),
        vec![PendingSessionAction::OpenSessionId(SessionId::from_raw(
            "sess-beta"
        ))]
    );

    type_text(&mut app, "/sessions alpha");
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        app.take_pending_session_actions(),
        vec![PendingSessionAction::OpenSessionQuery("alpha".to_string())]
    );

    type_text(&mut app, "/new");
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        app.take_pending_session_actions(),
        vec![PendingSessionAction::CreateNewTab]
    );
}

#[test]
fn tab_bar_mouse_clicks_switch_tabs_and_request_new_tab() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let (mut app, _rx, _secrets) = test_app();
    app.session_opener = Some(Arc::new(DummySessionOpener));
    let (opened_second, _rx2) = make_opened_session("sess-2", Some("Second"), Vec::new());
    app.apply_opened_session(opened_second, false);

    let backend = TestBackend::new(120, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| crate::ui::render(frame, &mut app))
        .unwrap();

    let tab1_zone = app
        .tab_hit_zones
        .iter()
        .find(|zone| zone.kind == TabHitKind::Tab(0))
        .copied()
        .expect("tab 1 hit zone");
    assert_eq!(app.active_tab_index(), 1);
    assert!(app.click_tab_at(tab1_zone.rect.x, tab1_zone.rect.y));
    assert_eq!(app.active_tab_index(), 0);

    let new_zone = app
        .tab_hit_zones
        .iter()
        .find(|zone| zone.kind == TabHitKind::NewTab)
        .copied()
        .expect("new tab hit zone");
    assert!(app.click_tab_at(new_zone.rect.x, new_zone.rect.y));
    assert_eq!(
        app.take_pending_session_actions(),
        vec![PendingSessionAction::CreateNewTab]
    );
}

#[test]
fn multiline_input_supports_ctrl_j_ctrl_enter_and_backslash_continuation() {
    let (mut app, mut rx, _secrets) = test_app();

    type_text(&mut app, "line1");
    app.handle_key_event(key_mod(KeyCode::Char('j'), KeyModifiers::CONTROL));
    type_text(&mut app, "line2");
    app.handle_key_event(key_mod(KeyCode::Enter, KeyModifiers::CONTROL));
    type_text(&mut app, "line3\\");
    app.handle_key_event(key(KeyCode::Enter));
    type_text(&mut app, "line4");
    assert_eq!(app.tab().input, "line1\nline2\nline3\nline4");

    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::SubmitPrompt("line1\nline2\nline3\nline4".to_string())
    );
}

#[test]
fn doc_selection_copies_while_scrolling_across_long_and_wrapped_content() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let (mut app, _rx, _secrets) = test_app();
    let long_sentence = "Alpha ".repeat(20) + "OMEGA_END";
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "Start marker".to_string(),
    });
    for i in 0..25 {
        app.tab_mut().transcript.push(TranscriptEntry::System {
            text: format!("Middle filler line {i}"),
        });
    }
    app.tab_mut().transcript.push(TranscriptEntry::Assistant {
        text: long_sentence.clone(),
        reasoning: String::new(),
        streaming: false,
        reasoning_expanded: false,
    });

    let backend = TestBackend::new(90, 14);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| crate::ui::render(frame, &mut app))
        .unwrap();

    let inner = app.transcript_inner_area;
    assert!(app.transcript_wrapped_lines.len() > inner.height as usize);
    assert!(app.tab().scroll_offset > 0);

    // Drag across top-most visible line and scroll up with wheel while holding Left button.
    let initial_scroll = app.tab().scroll_offset;
    assert!(app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: inner.x + 2,
        row: inner.y + inner.height - 1,
        modifiers: KeyModifiers::NONE,
    }));
    assert!(app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: inner.x + 2,
        row: inner.y,
        modifiers: KeyModifiers::NONE,
    }));
    assert!(app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: inner.x + 2,
        row: inner.y,
        modifiers: KeyModifiers::NONE,
    }));
    assert!(app.tab().scroll_offset < initial_scroll);
    assert!(app.doc_selection.as_ref().is_some_and(|s| s.active));

    assert!(app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: inner.x,
        row: inner.y,
        modifiers: KeyModifiers::NONE,
    }));
    assert!(app.doc_selection.is_none());
    let wheel_copied = app.last_copied_text.as_deref().unwrap_or("");
    assert!(!wheel_copied.is_empty());

    // Now scroll to top, click at "Start marker", scroll back to bottom, and Shift+Click on OMEGA_END.
    app.scroll_to_top();
    terminal
        .draw(|frame| crate::ui::render(frame, &mut app))
        .unwrap();
    let start_idx = app
        .transcript_wrapped_lines
        .iter()
        .position(|line| line.text.contains("Start marker"))
        .expect("start marker line");
    let start_screen_row = inner.y + (start_idx - app.tab().scroll_offset) as u16;
    assert!(app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: inner.x,
        row: start_screen_row,
        modifiers: KeyModifiers::NONE,
    }));
    assert!(app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: inner.x,
        row: start_screen_row,
        modifiers: KeyModifiers::NONE,
    }));

    app.scroll_to_bottom();
    terminal
        .draw(|frame| crate::ui::render(frame, &mut app))
        .unwrap();
    let end_idx = app
        .transcript_wrapped_lines
        .iter()
        .position(|line| line.text.contains("OMEGA_END"))
        .expect("end marker line");
    let end_screen_row = inner.y + (end_idx - app.tab().scroll_offset) as u16;
    assert!(app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: inner.x + inner.width - 1,
        row: end_screen_row,
        modifiers: KeyModifiers::SHIFT,
    }));
    let full_copied = app.last_copied_text.as_deref().unwrap_or("");
    assert!(full_copied.contains("Start marker"));
    assert!(full_copied.contains("Middle filler line 0"));
    assert!(full_copied.contains(&long_sentence));
}
