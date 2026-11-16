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
        mode: None,
        prompt: String::new(),
        path: PathBuf::from(format!(".agents/agents/{id}.md")),
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
