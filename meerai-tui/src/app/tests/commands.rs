use super::*;

#[test]
fn ctrl_shortcuts_open_menus_and_reports_missing_diff() {
    let (mut app, _rx, _secrets) = test_app();

    app.handle_key_event(key_mod(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert!(matches!(app.modal, Some(ActiveModal::SessionSelect { .. })));
    app.modal = None;

    app.handle_key_event(key_mod(KeyCode::Char('o'), KeyModifiers::CONTROL));
    assert!(matches!(app.modal, Some(ActiveModal::ModelInput { .. })));
    app.modal = None;

    app.handle_key_event(key_mod(KeyCode::Char('a'), KeyModifiers::CONTROL));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::ProviderSelect { .. })
    ));
    app.modal = None;

    app.handle_key_event(key_mod(KeyCode::Char('e'), KeyModifiers::CONTROL));
    assert_eq!(
        app.status_notice.as_deref(),
        Some("No file changes to review")
    );

    app.file_changes.push(crate::FileChangeEntry {
        status: "M".to_string(),
        path: "src/lib.rs".to_string(),
        staged: false,
        unstaged: true,
    });
    app.handle_key_event(key_mod(KeyCode::Char('e'), KeyModifiers::CONTROL));
    assert!(matches!(app.modal, Some(ActiveModal::DiffView { .. })));
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
        AgentCommand::SetModel {
            model: "gemini::gemini-3.1-pro-preview".to_string(),
            provider: Some("gemini".to_string()),
            context_window_tokens: Some(1_048_576),
            effort_levels: Some(vec![
                meerai_common::settings::EffortLevel::Low,
                meerai_common::settings::EffortLevel::Medium,
                meerai_common::settings::EffortLevel::High,
            ]),
        }
    );
    assert!(matches!(
        rx.try_recv().unwrap(),
        AgentCommand::SetPlanUsage { .. }
    ));

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

#[test]
fn help_lists_the_agents_and_skills_pickers() {
    let (mut app, _rx, _secrets) = test_app();

    submit(&mut app, "/help");
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::System { text })
            if text.contains("/agent <id>")
                && text.contains("@agent/<id>")
                && text.contains("/agents")
                && text.contains("/skills")
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
            context_window_tokens: Some(1_048_576),
            effort_levels: Some(vec![
                meerai_common::settings::EffortLevel::Low,
                meerai_common::settings::EffortLevel::Medium,
                meerai_common::settings::EffortLevel::High,
            ]),
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
fn skills_and_agents_commands_open_filtered_mentions() {
    let (mut app, _rx, _secrets) = test_app();
    app.agents = vec![sample_agent(
        "reviewer",
        DefinitionSource::Agents,
        Some("Reviews changes"),
        None,
        "Review.",
    )];
    app.skills = vec![sample_skill(
        "orchestrate",
        DefinitionSource::Agents,
        Some("Plan a change"),
        "Plan.",
    )];

    submit(&mut app, "/skills");
    assert!(matches!(
        app.modal,
        Some(ActiveModal::Packs {
            tab: PackTab::Definitions,
            ..
        })
    ));

    app.modal = None;
    app.tab_mut().input.clear();
    app.tab_mut().cursor = 0;
    app.completion.reset();

    submit(&mut app, "/agents");
    assert_eq!(app.tab().input, "@agent/");
    assert!(app.completion.is_active());
    assert!(
        app.completion
            .items()
            .iter()
            .all(|item| item.item_kind == CompletionItemKind::Agent)
    );
    assert_eq!(app.completion.items()[0].label, "reviewer");
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
            context_window_tokens: None,
            effort_levels: None,
        }
    );
    assert_eq!(app.tab().model, "gemini-3.8-flash");
    assert!(app.tab().transcript.iter().any(|entry| matches!(
        entry,
        TranscriptEntry::System { text } if text.contains("implies a provider change")
    )));
}

#[test]
fn compact_and_mode_commands_emit_agent_commands() {
    let (mut app, mut rx, _secrets) = test_app();

    type_text(&mut app, "/compact");
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::Compact {
            focus: None,
            from_turn_id: None,
        }
    );
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

    type_text(&mut app, "/mode auto");
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::SetPermissionMode(ApprovalMode::Auto)
    );
    assert_eq!(app.tab().approval_mode, ApprovalMode::Auto);

    type_text(&mut app, "/mode nonsense");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(rx.try_recv().is_err());
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::Error { text, .. }) if text.contains("Invalid mode")
    ));
}

#[test]
fn shift_tab_cycles_approval_modes() {
    let (mut app, mut rx, _secrets) = test_app();
    assert_eq!(app.tab().approval_mode, ApprovalMode::Ask);

    app.handle_key_event(key(KeyCode::BackTab));
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::SetPermissionMode(ApprovalMode::Auto)
    );
    assert_eq!(app.tab().approval_mode, ApprovalMode::Auto);
    assert_eq!(app.status_notice.as_deref(), Some("Approval mode: auto"));

    app.handle_key_event(key(KeyCode::BackTab));
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::SetPermissionMode(ApprovalMode::Plan)
    );
    assert_eq!(app.tab().approval_mode, ApprovalMode::Plan);

    app.handle_key_event(key(KeyCode::BackTab));
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::SetPermissionMode(ApprovalMode::Ask)
    );
    assert_eq!(app.tab().approval_mode, ApprovalMode::Ask);
}

#[test]
fn acp_command_requests_an_acp_tab() {
    let (mut app, _rx, _secrets) = test_app();
    app.session_opener = Some(Arc::new(DummySessionOpener));

    submit(&mut app, "/acp");

    assert_eq!(
        app.take_pending_session_actions(),
        vec![PendingSessionAction::CreateAcpTab {
            preset: "agy".to_string()
        }]
    );
    assert_eq!(
        app.status_notice.as_deref(),
        Some("Opening ACP session (agy)...")
    );
}

#[test]
fn acp_command_requires_a_session_opener() {
    let (mut app, _rx, _secrets) = test_app();

    submit(&mut app, "/acp");

    assert!(app.take_pending_session_actions().is_empty());
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::Error { text, .. }) if text.contains("not available")
    ));
}

#[test]
fn fork_command_requests_a_fork_at_the_latest_turn() {
    let (mut app, _rx, _secrets) = test_app();
    app.session_opener = Some(Arc::new(DummySessionOpener));
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "hello".to_string(),
    });
    let turn = TurnId::new();
    app.tab_mut().user_turns.insert(0, turn.clone());
    let session_id = app.tab().session_id.clone();

    submit(&mut app, "/fork");

    assert_eq!(
        app.take_pending_session_actions(),
        vec![PendingSessionAction::ForkSession {
            session_id,
            turn: Some(turn),
            draft: String::new(),
        }]
    );
    assert_eq!(app.status_notice.as_deref(), Some("Forking session..."));
}

#[test]
fn btw_command_requests_a_transient_read_only_session() {
    let (mut app, _rx, _secrets) = test_app();
    app.session_opener = Some(Arc::new(DummySessionOpener));
    let session_id = app.tab().session_id.clone();

    submit(&mut app, "/btw what changed?");

    assert_eq!(
        app.take_pending_session_actions(),
        vec![PendingSessionAction::BtwSession {
            session_id,
            text: "what changed?".to_string(),
        }]
    );
    assert_eq!(app.status_notice.as_deref(), Some("Opening btw..."));
}

#[test]
fn ctrl_p_opens_the_command_palette_and_esc_closes_it() {
    let (mut app, _rx, _secrets) = test_app();
    app.handle_key_event(key_mod(KeyCode::Char('p'), KeyModifiers::CONTROL));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::CommandPalette { .. })
    ));

    app.handle_key_event(key(KeyCode::Esc));
    assert!(app.modal.is_none());
}

#[test]
fn command_palette_filters_and_runs_the_selected_command() {
    let (mut app, _rx, _secrets) = test_app();
    app.handle_key_event(key_mod(KeyCode::Char('p'), KeyModifiers::CONTROL));
    assert!(!app.tab().is_running);

    type_text(&mut app, "sidebar");
    app.handle_key_event(key(KeyCode::Enter));

    assert!(app.modal.is_none());
    assert!(!app.show_sidebar);
}

#[test]
fn ctrl_p_moves_the_mention_popup_instead_of_opening_the_palette() {
    let (mut app, _rx, _secrets) = test_app();
    app.agents = vec![
        test_agent_definition("reviewer"),
        test_agent_definition("planner"),
    ];
    type_text(&mut app, "@");
    assert!(app.completion.is_active());
    let before = app.completion.selected();

    app.handle_key_event(key_mod(KeyCode::Char('p'), KeyModifiers::CONTROL));

    assert!(app.modal.is_none());
    assert!(app.completion.is_active());
    assert_ne!(app.completion.selected(), before);
}

fn test_agent_definition(id: &str) -> AgentDefinition {
    AgentDefinition {
        id: id.to_string(),
        source: DefinitionSource::Agents,
        description: None,
        model: None,
        steps: None,
        mode: meerai_common::definitions::AgentMode::All,
        permissions: Vec::new(),
        prompt: String::new(),
        path: PathBuf::from(format!(".agents/agents/{id}.md")),
        token_estimate: 0,
        warnings: Vec::new(),
        supporting_files: Vec::new(),
    }
}

#[test]
fn update_command_reports_the_available_version() {
    let (mut app, _rx, _secrets) = test_app();
    app.update_available = Some("9.9.9".to_string());
    app.tab_mut().input = "/update".to_string();
    app.handle_key_event(key(KeyCode::Enter));

    let text = app
        .tab()
        .transcript
        .iter()
        .find_map(|entry| match entry {
            TranscriptEntry::System { text } => Some(text.clone()),
            _ => None,
        })
        .expect("system message");
    assert!(text.contains("9.9.9"), "{text}");
    assert!(text.contains("install.sh"), "{text}");
}

struct DummyWebHandle;

#[meerai_core::async_trait]
impl crate::WebServerHandle for DummyWebHandle {
    fn port(&self) -> u16 {
        8765
    }

    fn url(&self) -> &str {
        "http://127.0.0.1:8765"
    }

    async fn stop(&self) {}
}

struct DummyWebLauncher;

#[meerai_core::async_trait]
impl crate::WebServerLauncher for DummyWebLauncher {
    async fn start(
        &self,
        _registry: Arc<crate::SessionRegistry>,
        _opener: Arc<dyn crate::SessionOpener>,
        _port: Option<u16>,
        _secret: Option<String>,
    ) -> anyhow::Result<(Arc<dyn crate::WebServerHandle>, crate::WebServerStart)> {
        anyhow::bail!("unused in unit test")
    }
}

#[test]
fn serve_command_validates_dialog_stops_and_clicks_statusline() {
    let (mut app, _rx, _secrets) = test_app();

    submit(&mut app, "/serve");
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::Error { text, .. }) if text.contains("not enabled")
    ));

    app.web_launcher = Some(Arc::new(DummyWebLauncher));
    submit(&mut app, "/serve");
    assert!(matches!(app.modal, Some(ActiveModal::ServeDialog { .. })));

    type_text(&mut app, "4040");
    app.handle_key_event(key(KeyCode::Tab));
    type_text(&mut app, "short");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::ServeDialog {
            error: Some(_),
            busy: false,
            ..
        })
    ));

    app.handle_key_event(key_mod(KeyCode::Char('u'), KeyModifiers::CONTROL));
    type_text(&mut app, "valid-pass-123");
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        app.take_pending_session_actions(),
        vec![PendingSessionAction::StartWebServer {
            port: Some(4040),
            secret: Some("valid-pass-123".to_string()),
        }]
    );

    app.modal = None;
    app.web_server = Some(crate::app::RunningWebServer {
        port: 8765,
        url: "http://127.0.0.1:8765".to_string(),
        handle: Arc::new(DummyWebHandle),
    });
    app.web_server_hit = Some(ratatui::layout::Rect::new(10, 20, 12, 1));
    app.handle_mouse_event(crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: 12,
        row: 20,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.last_copied_text.as_deref(),
        Some("http://127.0.0.1:8765")
    );

    submit(&mut app, "/serve stop");
    assert_eq!(
        app.take_pending_session_actions(),
        vec![PendingSessionAction::StopWebServer]
    );
}

#[test]
fn effort_command_updates_state_queues_sync_and_appears_in_help() {
    let (mut app, _rx, _secrets) = test_app();

    submit(&mut app, "/effort");
    assert!(matches!(app.modal, Some(ActiveModal::Sampling { .. })));
    app.modal = None;

    submit(&mut app, "/effort high");
    assert_eq!(
        app.context_effort,
        Some(meerai_common::settings::EffortLevel::High)
    );
    assert_eq!(app.context_temperature, Some(0.5));
    assert_eq!(
        app.settings.context.as_ref().and_then(|c| c.effort),
        Some(meerai_common::settings::EffortLevel::High)
    );
    assert_eq!(
        app.settings.context.as_ref().and_then(|c| c.temperature),
        None
    );
    assert_eq!(
        app.take_pending_session_actions(),
        vec![PendingSessionAction::SyncContext]
    );

    submit(&mut app, "/effort low");
    assert_eq!(
        app.context_effort,
        Some(meerai_common::settings::EffortLevel::Low)
    );
    assert_eq!(app.context_temperature, Some(0.0));
    assert_eq!(
        app.take_pending_session_actions(),
        vec![PendingSessionAction::SyncContext]
    );

    submit(&mut app, "/effort bogus");
    assert_eq!(
        app.context_effort,
        Some(meerai_common::settings::EffortLevel::Low)
    );
    assert!(
        app.status_notice
            .as_deref()
            .is_some_and(|n| n.contains("Unknown effort 'bogus'"))
    );
    assert!(app.take_pending_session_actions().is_empty());

    submit(&mut app, "/help");
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::System { text }) if text.contains("/effort [low|medium|high|xhigh|max]")
    ));
}

#[test]
fn loop_command_opens_the_read_only_panel() {
    let (mut app, _rx, _secrets) = test_app();
    app.tab_mut().loop_state = Some(meerai_session::LoopState {
        objective: Some("Ship the change".to_string()),
        items: vec![meerai_session::LoopItem {
            id: 1,
            text: "Fix it".to_string(),
            status: meerai_session::LoopItemStatus::Open,
            ..meerai_session::LoopItem::default()
        }],
        ..meerai_session::LoopState::default()
    });

    type_text(&mut app, "/loop");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(app.modal, Some(ActiveModal::Loop { .. })));

    app.handle_key_event(key(KeyCode::Esc));
    assert!(app.modal.is_none());
}

#[test]
fn host_action_commands_queue_and_context_is_local() {
    let (mut app, mut rx, _secrets) = test_app();

    type_text(&mut app, "/context");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(
        rx.try_recv().unwrap(),
        AgentCommand::InspectContext
    ));
    assert!(matches!(app.modal, Some(ActiveModal::Context(_))));
    app.modal = None;

    type_text(&mut app, "/memory parser");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(app.modal, Some(ActiveModal::Memory { .. })));
    app.modal = None;

    type_text(&mut app, "/routing set build openai::o3 1.5 20000 2");
    app.handle_key_event(key(KeyCode::Enter));
    type_text(&mut app, "/subagent explore find the parser");
    app.handle_key_event(key(KeyCode::Enter));
    type_text(&mut app, "/packs");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(app.modal, Some(ActiveModal::Packs { .. })));
    app.modal = None;

    let actions = app.take_pending_session_actions();
    assert!(matches!(
        actions.first(),
        Some(PendingSessionAction::LoadMemories {
            query: Some(query)
        }) if query == "parser"
    ));
    assert!(matches!(
        actions.get(1),
        Some(PendingSessionAction::HostAction {
            action: meerai_host::HostAction::SetRoute { task_type, provider, model, .. },
            ..
        }) if task_type == "build" && provider == "openai" && model == "o3"
    ));
    assert!(matches!(
        actions.get(2),
        Some(PendingSessionAction::HostAction {
            action: meerai_host::HostAction::SpawnSubagent { spec },
            ..
        }) if spec.agent == "explore" && spec.task == "find the parser"
    ));
    assert!(matches!(
        actions.get(3),
        Some(PendingSessionAction::LoadPacks)
    ));
}
