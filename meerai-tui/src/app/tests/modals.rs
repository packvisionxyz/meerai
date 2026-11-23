use super::*;

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
fn opens_provider_select_on_startup_when_plan_credential_needs_relogin() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let secrets = Arc::new(MemorySecretStore::new());
    secrets.set_credential("openai", oauth_credential(true));
    let settings = AppSettings::default();
    let runtime = Arc::new(RwLock::new(RuntimeProviderState::from_settings(
        &settings,
        ProviderCatalog::new(),
    )));
    let app = AppState::new(
        "openai",
        "gpt-5",
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

    let (tx, _rx) = mpsc::unbounded_channel();
    let secrets = Arc::new(MemorySecretStore::new());
    secrets.set_credential("openai", oauth_credential(false));
    let settings = AppSettings::default();
    let runtime = Arc::new(RwLock::new(RuntimeProviderState::from_settings(
        &settings,
        ProviderCatalog::new(),
    )));
    let app = AppState::new(
        "openai",
        "gpt-5",
        PathBuf::from("."),
        tx,
        runtime,
        secrets,
        settings,
        None,
    );
    assert!(app.modal.is_none());
    assert!(app.active_key_status().is_configured());
}

#[test]
fn model_context_window_tracks_the_catalog_unless_pinned() {
    let (mut app, _rx, _secrets) = test_app();

    assert_eq!(
        app.model_context_window("gemini", "gemini-2.5-pro"),
        Some(1_048_576)
    );
    assert_eq!(
        app.model_context_window("gemini", "gemini::gemini-2.5-pro"),
        Some(1_048_576)
    );
    assert_eq!(app.model_context_window("gemini", "unknown-model"), None);

    app.explicit_context_window = Some(200_000);
    assert_eq!(app.model_context_window("gemini", "gemini-2.5-pro"), None);
}

#[test]
fn connect_flow_offers_auth_methods_for_openai() {
    let (mut app, _rx, _secrets) = test_app();

    type_text(&mut app, "/connect");
    app.handle_key_event(key(KeyCode::Enter));
    type_text(&mut app, "openai");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::AuthMethodSelect {
            ref provider_id,
            selected: 0,
            ..
        }) if provider_id == "openai"
    ));

    type_text(&mut app, "x");
    assert!(matches!(
        app.modal,
        Some(ActiveModal::AuthMethodSelect { selected: 0, .. })
    ));

    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::ChatGptLogin {
            ref provider_id,
            url: None,
            ..
        }) if provider_id == "openai"
    ));
    assert_eq!(app.pending_login_request.as_deref(), Some("openai"));

    app.handle_key_event(key(KeyCode::Esc));
    assert!(app.modal.is_none());
    assert!(app.pending_login_request.is_none());

    type_text(&mut app, "/connect");
    app.handle_key_event(key(KeyCode::Enter));
    type_text(&mut app, "openai");
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Down));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::AuthMethodSelect { selected: 1, .. })
    ));
    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::ApiKeyInput { ref provider_id, .. }) if provider_id == "openai"
    ));
}

#[test]
fn connect_flow_with_a_stored_plan_skips_auth_methods() {
    let (mut app, _rx, secrets) = test_app();
    secrets.set_credential("openai", oauth_credential(false));
    app.invalidate_key_status();

    type_text(&mut app, "/connect");
    app.handle_key_event(key(KeyCode::Enter));
    type_text(&mut app, "openai");
    app.handle_key_event(key(KeyCode::Enter));

    assert!(matches!(
        app.modal,
        Some(ActiveModal::ProviderConnection {
            ref provider_id,
            selected: 0,
            ..
        }) if provider_id == "openai"
    ));

    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(app.modal, Some(ActiveModal::ModelInput { .. })));
    assert_eq!(app.tab().active_provider, "openai");
}

#[test]
fn connection_screen_updates_and_removes_a_plan_credential() {
    let (mut app, _rx, secrets) = test_app();
    secrets.set_credential("openai", oauth_credential(false));
    app.invalidate_key_status();

    type_text(&mut app, "/connect");
    app.handle_key_event(key(KeyCode::Enter));
    type_text(&mut app, "openai");
    app.handle_key_event(key(KeyCode::Enter));

    app.handle_key_event(key(KeyCode::Down));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::ProviderConnection { selected: 1, .. })
    ));
    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::AuthMethodSelect { .. })
    ));

    app.handle_key_event(key(KeyCode::Esc));
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Down));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::ProviderConnection { selected: 2, .. })
    ));
    app.handle_key_event(key(KeyCode::Enter));

    assert!(secrets.read_credential("openai").unwrap().is_none());
    assert!(matches!(
        app.modal,
        Some(ActiveModal::ProviderSelect { .. })
    ));
}

#[test]
fn connection_screen_offers_next_and_update_for_an_api_key() {
    let (mut app, _rx, secrets) = test_app();
    secrets.set_key("opencode-go", "oc-secret-key").unwrap();
    app.invalidate_key_status();

    type_text(&mut app, "/connect");
    app.handle_key_event(key(KeyCode::Enter));
    type_text(&mut app, "opencode-go");
    app.handle_key_event(key(KeyCode::Enter));

    assert!(matches!(
        app.modal,
        Some(ActiveModal::ProviderConnection { selected: 0, .. })
    ));

    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::ApiKeyInput { ref provider_id, .. }) if provider_id == "opencode-go"
    ));
}

#[test]
fn chatgpt_login_retry_requests_a_fresh_flow() {
    let (mut app, _rx, _secrets) = test_app();
    app.modal = Some(ActiveModal::ChatGptLogin {
        provider_id: "openai".to_string(),
        provider_name: "OpenAI".to_string(),
        url: Some("https://auth.openai.com/example".to_string()),
        input: String::new(),
        error: Some("state mismatch".to_string()),
        busy: false,
    });

    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(app.pending_login_request.as_deref(), Some("openai"));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::ChatGptLogin {
            url: None,
            busy: true,
            error: None,
            ..
        })
    ));
}

#[test]
fn model_selection_on_a_plan_credential_sends_plan_usage() {
    let (mut app, mut rx, secrets) = test_app();
    secrets.set_credential("gemini", oauth_credential(false));
    app.invalidate_key_status();

    app.select_provider_and_model("gemini", "gemini-2.5-pro");

    let mut saw_plan = false;
    while let Ok(command) = rx.try_recv() {
        if matches!(
            command,
            meerai_agents::AgentCommand::SetPlanUsage { plan_usage: true }
        ) {
            saw_plan = true;
        }
    }
    assert!(saw_plan);
}

#[test]
fn auth_method_select_esc_returns_to_provider_select() {
    let (mut app, _rx, _secrets) = test_app();

    type_text(&mut app, "/connect");
    app.handle_key_event(key(KeyCode::Enter));
    type_text(&mut app, "openai");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::AuthMethodSelect { .. })
    ));

    app.handle_key_event(key(KeyCode::Esc));
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

    while rx.try_recv().is_ok() {}
    type_text(&mut app, "glm-5.3");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.modal.is_none());
    assert_eq!(app.tab().active_provider, "opencode-go");
    assert_eq!(app.tab().model, "glm-5.3");
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::SetModel {
            model: "opencode_go::glm-5.3".to_string(),
            context_window_tokens: None,
        }
    );
    assert!(matches!(
        rx.try_recv().unwrap(),
        AgentCommand::SetPlanUsage { .. }
    ));

    type_text(&mut app, "/provider");
    app.handle_key_event(key(KeyCode::Enter));
    type_text(&mut app, "opencode-go");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::ProviderConnection { selected: 0, .. })
    ));
    app.handle_key_event(key(KeyCode::Down));
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
        AgentCommand::SetModel {
            model: "gemini::custom-gemini-model".to_string(),
            context_window_tokens: None,
        }
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
fn clicking_a_user_message_opens_actions_and_copies_or_forks() {
    let (mut app, _rx, _secrets) = test_app();
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "hello world".to_string(),
    });
    app.transcript_inner_area = Rect::new(1, 1, 60, 10);
    app.thinking_hit_zones = vec![ThinkingHitZone {
        entry_idx: 0,
        start_line: 0,
        end_line: 2,
    }];
    let press = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 1,
        modifiers: KeyModifiers::NONE,
    };
    let release = MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 5,
        row: 1,
        modifiers: KeyModifiers::NONE,
    };
    assert!(app.handle_mouse_event(press));
    assert!(app.handle_mouse_event(release));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::UserMessageActions { entry_idx: 0, .. })
    ));

    app.handle_key_event(key(KeyCode::Char('c')));
    assert!(app.modal.is_none());
    assert_eq!(app.last_copied_text.as_deref(), Some("hello world"));

    app.session_opener = Some(std::sync::Arc::new(DummySessionOpener));
    assert!(app.handle_mouse_event(press));
    assert!(app.handle_mouse_event(release));
    app.handle_key_event(key(KeyCode::Char('f')));
    assert!(matches!(
        app.pending_session_actions.last(),
        Some(PendingSessionAction::ForkSession { draft, turn: None, .. })
            if draft == "hello world"
    ));
}

#[test]
fn user_message_rewind_restores_the_message_to_the_composer() {
    let (mut app, mut rx, _secrets) = test_app();
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "first".to_string(),
    });
    app.tab_mut().transcript.push(TranscriptEntry::Assistant {
        text: "answer".to_string(),
        reasoning: String::new(),
        streaming: false,
        reasoning_expanded: false,
    });
    app.tab_mut()
        .user_turns
        .insert(0, TurnId::from_raw("turn-1"));
    app.tab_mut().turn_activity.insert(0, true);
    app.tab_mut().input = "draft text".to_string();
    app.tab_mut().cursor = "draft text".len();
    app.mode = InputMode::Normal;
    app.transcript_inner_area = Rect::new(1, 1, 60, 10);
    app.thinking_hit_zones = vec![ThinkingHitZone {
        entry_idx: 0,
        start_line: 0,
        end_line: 2,
    }];
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        app.handle_mouse_event(MouseEvent {
            kind,
            column: 5,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
    }
    app.handle_key_event(key(KeyCode::Char('w')));
    assert!(app.tab().transcript.is_empty());
    assert!(app.tab().user_turns.is_empty());
    assert!(app.tab().turn_activity.is_empty());
    assert_eq!(app.tab().input, "first");
    assert_eq!(app.tab().cursor, "first".len());
    assert_eq!(app.mode, InputMode::Insert);
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::Rewind {
            turn_id: TurnId::from_raw("turn-1"),
            scope: meerai_agents::RewindScope::Both,
        }
    );
}

#[test]
fn user_message_rewind_restores_an_empty_message_verbatim() {
    let (mut app, _rx, _secrets) = test_app();
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: String::new(),
    });
    app.tab_mut().user_turns.insert(0, TurnId::new());
    app.tab_mut().input = "draft".to_string();
    app.tab_mut().cursor = "draft".len();

    assert!(app.run_user_action(0, UserAction::Rewind));
    assert!(app.tab().transcript.is_empty());
    assert_eq!(app.tab().input, "");
    assert_eq!(app.tab().cursor, 0);
    assert_eq!(app.mode, InputMode::Insert);
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

    type_text(&mut app, "/sea");
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
fn question_overlay_submits_a_selected_label() {
    use meerai_agents::{QuestionAnswer, QuestionOption, QuestionPrompt};

    let (mut app, mut rx, _secrets) = test_app();
    app.handle_agent_event(AgentEvent::QuestionAsked {
        request: QuestionRequest {
            request_id: RequestId::from_raw("q-1"),
            turn_id: TurnId::new(),
            questions: vec![QuestionPrompt {
                header: "DB".to_string(),
                question: "Which database?".to_string(),
                options: vec![
                    QuestionOption {
                        label: "Postgres".to_string(),
                        description: None,
                    },
                    QuestionOption {
                        label: "SQLite".to_string(),
                        description: Some("local only".to_string()),
                    },
                ],
                multiple: false,
            }],
        },
    });
    assert!(matches!(app.modal, Some(ActiveModal::Question { .. })));

    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Char(' ')));
    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::AnswerQuestion {
            request_id: RequestId::from_raw("q-1"),
            answers: Some(vec![QuestionAnswer {
                header: "DB".to_string(),
                selected: vec!["SQLite".to_string()],
            }]),
        }
    );
    assert!(app.modal.is_none());
}

#[test]
fn question_overlay_esc_cancels_and_multiple_questions_advance() {
    use meerai_agents::{QuestionAnswer, QuestionOption, QuestionPrompt};

    let prompt = |header: &str| QuestionPrompt {
        header: header.to_string(),
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
    };

    let (mut app, mut rx, _secrets) = test_app();
    app.handle_agent_event(AgentEvent::QuestionAsked {
        request: QuestionRequest {
            request_id: RequestId::from_raw("q-2"),
            turn_id: TurnId::new(),
            questions: vec![prompt("One"), prompt("Two")],
        },
    });
    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::Question { current: 1, .. })
    ));
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::AnswerQuestion {
            request_id: RequestId::from_raw("q-2"),
            answers: Some(vec![
                QuestionAnswer {
                    header: "One".to_string(),
                    selected: vec!["a".to_string()],
                },
                QuestionAnswer {
                    header: "Two".to_string(),
                    selected: vec!["b".to_string()],
                },
            ]),
        }
    );

    app.handle_agent_event(AgentEvent::QuestionAsked {
        request: QuestionRequest {
            request_id: RequestId::from_raw("q-3"),
            turn_id: TurnId::new(),
            questions: vec![prompt("Three")],
        },
    });
    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::AnswerQuestion {
            request_id: RequestId::from_raw("q-3"),
            answers: None,
        }
    );
    assert!(app.modal.is_none());
}

#[test]
fn rewind_menu_code_choice_keeps_the_transcript_and_composer() {
    use meerai_agents::RewindScope;

    let (mut app, mut rx, _secrets) = test_app();
    let turn_id = TurnId::new();
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "hello".to_string(),
    });
    app.tab_mut().user_turns.insert(0, turn_id.clone());
    app.tab_mut().turn_activity.insert(0, true);

    type_text(&mut app, "/rewind");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(app.modal, Some(ActiveModal::RewindMenu { .. })));
    app.tab_mut().input = "draft".to_string();
    app.tab_mut().cursor = "draft".len();

    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::Rewind {
            turn_id,
            scope: RewindScope::Code,
        }
    );
    assert!(matches!(
        app.tab().transcript.as_slice(),
        [TranscriptEntry::User { text }] if text == "hello"
    ));
    assert_eq!(app.tab().user_turns.len(), 1);
    assert_eq!(app.tab().turn_activity.get(&0), Some(&true));
    assert_eq!(app.tab().input, "draft");
    assert_eq!(app.tab().cursor, "draft".len());
}

#[test]
fn rewind_menu_both_choice_restores_the_message_to_the_composer() {
    use meerai_agents::RewindScope;

    let (mut app, mut rx, _secrets) = test_app();
    let turn_id = TurnId::new();
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "hello".to_string(),
    });
    app.tab_mut().transcript.push(TranscriptEntry::Assistant {
        text: "answer".to_string(),
        reasoning: String::new(),
        streaming: false,
        reasoning_expanded: false,
    });
    app.tab_mut().user_turns.insert(0, turn_id.clone());
    app.tab_mut().turn_activity.insert(0, true);

    type_text(&mut app, "/rewind");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(app.modal, Some(ActiveModal::RewindMenu { .. })));
    app.tab_mut().input = "draft".to_string();
    app.tab_mut().cursor = "draft".len();
    app.mode = InputMode::Normal;

    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::Rewind {
            turn_id,
            scope: RewindScope::Both,
        }
    );
    assert!(app.tab().transcript.is_empty());
    assert!(app.tab().user_turns.is_empty());
    assert!(app.tab().turn_activity.is_empty());
    assert_eq!(app.tab().input, "hello");
    assert_eq!(app.tab().cursor, "hello".len());
    assert_eq!(app.mode, InputMode::Insert);
}

fn diff_list_app(label: &str) -> (AppState, std::path::PathBuf) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "meerai_app_diff_list_{label}_{}_{}",
        std::process::id(),
        nanos
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(root.join("b.rs"), "fn b() {}\n").unwrap();

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
    app.handle_terminal_event(Event::Resize(120, 30));
    app.file_changes = vec![
        FileChangeEntry {
            status: "??".to_string(),
            path: "a.rs".to_string(),
        },
        FileChangeEntry {
            status: "??".to_string(),
            path: "b.rs".to_string(),
        },
    ];
    (app, root)
}

#[test]
fn diff_modal_file_list_toggles_moves_opens_and_closes() {
    let (mut app, root) = diff_list_app("keys");
    app.open_diff_modal(0);
    assert!(matches!(
        app.modal,
        Some(ActiveModal::DiffView {
            file_index: 0,
            scroll: 0,
            list: DiffListState::Closed,
            ..
        })
    ));

    app.handle_key_event(key(KeyCode::Char('f')));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::DiffView {
            file_index: 0,
            list: DiffListState::Open { selected: 0 },
            ..
        })
    ));

    app.handle_key_event(key(KeyCode::Char('f')));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::DiffView {
            list: DiffListState::Closed,
            ..
        })
    ));

    app.handle_key_event(key(KeyCode::Char('f')));
    app.handle_key_event(key(KeyCode::Char('j')));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::DiffView {
            list: DiffListState::Open { selected: 1 },
            ..
        })
    ));
    app.handle_key_event(key(KeyCode::Char('j')));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::DiffView {
            list: DiffListState::Open { selected: 1 },
            ..
        })
    ));
    app.handle_key_event(key(KeyCode::Char('k')));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::DiffView {
            list: DiffListState::Open { selected: 0 },
            ..
        })
    ));
    app.handle_key_event(key(KeyCode::Up));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::DiffView {
            list: DiffListState::Open { selected: 0 },
            ..
        })
    ));
    app.handle_key_event(key(KeyCode::Down));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::DiffView {
            list: DiffListState::Open { selected: 1 },
            ..
        })
    ));

    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::DiffView {
            file_index: 1,
            scroll: 0,
            list: DiffListState::Open { selected: 1 },
            ref diff,
            ..
        }) if diff.path == "b.rs"
    ));

    app.handle_key_event(key(KeyCode::Right));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::DiffView {
            file_index: 0,
            list: DiffListState::Open { selected: 0 },
            ..
        })
    ));
    app.handle_key_event(key(KeyCode::Left));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::DiffView {
            file_index: 1,
            list: DiffListState::Open { selected: 1 },
            ..
        })
    ));

    app.handle_key_event(key(KeyCode::Esc));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::DiffView {
            file_index: 1,
            list: DiffListState::Closed,
            ..
        })
    ));
    app.handle_key_event(key(KeyCode::Char('f')));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::DiffView {
            file_index: 1,
            list: DiffListState::Open { selected: 1 },
            ..
        })
    ));
    app.handle_key_event(key(KeyCode::Esc));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::DiffView {
            list: DiffListState::Closed,
            ..
        })
    ));
    app.handle_key_event(key(KeyCode::Esc));
    assert!(app.modal.is_none());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn diff_modal_file_list_click_opens_the_clicked_file() {
    let (mut app, root) = diff_list_app("click");
    app.open_diff_modal(0);
    app.handle_key_event(key(KeyCode::Char('f')));

    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).expect("terminal");
    terminal
        .draw(|frame| crate::ui::render(frame, &mut app))
        .expect("draw diff list");
    let zone = app
        .modal_hit_zones
        .iter()
        .find(|zone| zone.kind == ModalHitKind::ListItem(1))
        .copied()
        .expect("file list row zone");

    assert!(app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: zone.rect.x,
        row: zone.rect.y,
        modifiers: KeyModifiers::NONE,
    }));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::DiffView {
            file_index: 1,
            scroll: 0,
            list: DiffListState::Open { selected: 1 },
            ref diff,
            ..
        }) if diff.path == "b.rs"
    ));

    let _ = std::fs::remove_dir_all(&root);
}

fn question_request(id: &str, multiple: bool) -> QuestionRequest {
    use meerai_agents::{QuestionOption, QuestionPrompt};
    QuestionRequest {
        request_id: RequestId::from_raw(id),
        turn_id: TurnId::new(),
        questions: vec![QuestionPrompt {
            header: "DB".to_string(),
            question: "Which database?".to_string(),
            options: vec![
                QuestionOption {
                    label: "Postgres".to_string(),
                    description: None,
                },
                QuestionOption {
                    label: "SQLite".to_string(),
                    description: None,
                },
            ],
            multiple,
        }],
    }
}

fn type_text(app: &mut AppState, text: &str) {
    for ch in text.chars() {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }
}

#[test]
fn question_overlay_custom_answer_accepts_typed_text() {
    let (mut app, mut rx, _secrets) = test_app();
    app.handle_agent_event(AgentEvent::QuestionAsked {
        request: question_request("q-other", false),
    });
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Down));
    type_text(&mut app, "DuckDB please");
    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::AnswerQuestion {
            request_id: RequestId::from_raw("q-other"),
            answers: Some(vec![QuestionAnswer {
                header: "DB".to_string(),
                selected: vec!["DuckDB please".to_string()],
            }]),
        }
    );
}

#[test]
fn question_overlay_selecting_an_option_clears_the_custom_answer() {
    let (mut app, mut rx, _secrets) = test_app();
    app.handle_agent_event(AgentEvent::QuestionAsked {
        request: question_request("q-clear", false),
    });
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Down));
    type_text(&mut app, "custom");
    app.handle_key_event(key(KeyCode::Up));
    app.handle_key_event(key(KeyCode::Up));
    app.handle_key_event(key(KeyCode::Char(' ')));
    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::AnswerQuestion {
            request_id: RequestId::from_raw("q-clear"),
            answers: Some(vec![QuestionAnswer {
                header: "DB".to_string(),
                selected: vec!["Postgres".to_string()],
            }]),
        }
    );
}

#[test]
fn question_overlay_multiple_keeps_options_and_custom_text() {
    let (mut app, mut rx, _secrets) = test_app();
    app.handle_agent_event(AgentEvent::QuestionAsked {
        request: question_request("q-multi", true),
    });
    app.handle_key_event(key(KeyCode::Char(' ')));
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Down));
    type_text(&mut app, "and DuckDB");
    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::AnswerQuestion {
            request_id: RequestId::from_raw("q-multi"),
            answers: Some(vec![QuestionAnswer {
                header: "DB".to_string(),
                selected: vec!["Postgres".to_string(), "and DuckDB".to_string()],
            }]),
        }
    );
}

#[test]
fn memory_modal_pins_and_deletes_the_selection() {
    let (mut app, _rx, _secrets) = test_app();
    let memory = meerai_host::ProjectMemory {
        id: "mem-1".to_string(),
        project_root: "/repo".to_string(),
        kind: meerai_host::ProjectMemoryKind::Decision,
        title: "Use SQLite".to_string(),
        body: "All state lives in meerai.db".to_string(),
        source_session_id: None,
        pinned: false,
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    app.modal = Some(ActiveModal::Memory {
        entries: vec![memory],
        query: String::new(),
        selected: 0,
        loading: false,
        error: None,
    });

    app.handle_key_event(key(KeyCode::Char('p')));
    app.modal = Some(ActiveModal::Memory {
        entries: vec![meerai_host::ProjectMemory {
            id: "mem-1".to_string(),
            project_root: "/repo".to_string(),
            kind: meerai_host::ProjectMemoryKind::Decision,
            title: "Use SQLite".to_string(),
            body: "All state lives in meerai.db".to_string(),
            source_session_id: None,
            pinned: false,
            created_at_ms: 1,
            updated_at_ms: 1,
        }],
        query: String::new(),
        selected: 0,
        loading: false,
        error: None,
    });
    app.handle_key_event(key(KeyCode::Char('d')));

    let actions = app.take_pending_session_actions();
    assert!(matches!(
        actions.first(),
        Some(PendingSessionAction::MemoryAction {
            action: MemoryModalAction::TogglePin,
            id
        }) if id == "mem-1"
    ));
    assert!(matches!(
        actions.get(1),
        Some(PendingSessionAction::MemoryAction {
            action: MemoryModalAction::Delete,
            id
        }) if id == "mem-1"
    ));
}

#[test]
fn packs_modal_toggles_state_and_subagent_form_spawns() {
    let (mut app, mut rx, _secrets) = test_app();
    app.modal = Some(ActiveModal::Packs {
        packs: vec![meerai_host::InstalledPack {
            id: "review".to_string(),
            source_kind: meerai_host::PackSourceKind::Local,
            source_uri: "./review".to_string(),
            pinned_rev: "local".to_string(),
            manifest_json: "{}".to_string(),
            enabled: true,
            trusted: false,
            installed_at_ms: 1,
            updated_at_ms: 1,
        }],
        definitions: meerai_host::HostDefinitionsDiagnostics::default(),
        tab: PackTab::Packs,
        selected: 0,
        loading: false,
        error: None,
    });
    app.handle_key_event(key(KeyCode::Char('e')));
    let actions = app.take_pending_session_actions();
    assert!(matches!(
        actions.first(),
        Some(PendingSessionAction::PackAction {
            action: PackModalAction::ToggleEnabled,
            id
        }) if id == "review"
    ));
    app.modal = None;

    app.modal = Some(ActiveModal::SubagentSpawn(SubagentSpawnPanel {
        name: "parser scan".to_string(),
        agent: "explore".to_string(),
        field: 2,
        ..SubagentSpawnPanel::default()
    }));
    for ch in "find the parser".chars() {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.modal.is_none());
    assert!(matches!(
        rx.try_recv().unwrap(),
        AgentCommand::SpawnSubagent { spec }
            if spec.agent == "explore"
                && spec.task == "find the parser"
                && spec.name.as_deref() == Some("parser scan")
    ));
}

#[test]
fn context_modal_scrolls_and_packs_switches_tabs() {
    let (mut app, _rx, _secrets) = test_app();
    app.modal = Some(ActiveModal::Context(ContextPanel {
        breakdown: Some(meerai_session::ContextBreakdown::default()),
        ..ContextPanel::default()
    }));
    app.handle_key_event(key(KeyCode::Char('f')));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::Context(ContextPanel {
            focus_input: Some(_),
            ..
        }))
    ));

    app.modal = Some(ActiveModal::Packs {
        packs: Vec::new(),
        definitions: meerai_host::HostDefinitionsDiagnostics::default(),
        tab: PackTab::Packs,
        selected: 0,
        loading: false,
        error: None,
    });
    app.handle_key_event(key(KeyCode::Tab));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::Packs {
            tab: PackTab::Definitions,
            ..
        })
    ));
}

#[test]
fn vault_reset_requires_the_word_reset_and_clears_credentials() {
    let (mut app, _rx, secrets) = test_app();
    secrets.set_key("openai", "sk-secret").unwrap();
    submit(&mut app, "/vault-reset");
    assert!(matches!(app.modal, Some(ActiveModal::VaultReset { .. })));

    for ch in "nope".chars() {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }
    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::VaultReset { error: Some(_), .. })
    ));
    assert!(secrets.get_key("openai").unwrap().is_some());

    app.modal = Some(ActiveModal::VaultReset {
        input: "reset".to_string(),
        error: None,
    });
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.modal.is_none());
    assert!(secrets.get_key("openai").unwrap().is_none());
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::System { text }) if text.contains("Vault reset")
    ));
}

#[test]
fn sampling_effort_keys_work_from_the_temperature_field() {
    let (mut app, _rx, _secrets) = test_app();
    submit(&mut app, "/temperature");
    assert!(matches!(
        app.modal,
        Some(ActiveModal::Sampling { field: 1, .. })
    ));

    app.handle_key_event(key(KeyCode::Char('h')));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::Sampling {
            effort: Some(EffortLevel::High),
            ..
        })
    ));
    app.handle_key_event(key(KeyCode::Char('m')));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::Sampling {
            effort: Some(EffortLevel::Medium),
            ..
        })
    ));
    app.handle_key_event(key(KeyCode::Enter));

    assert!(app.modal.is_none());
    let context = app.settings.context.as_ref().unwrap();
    assert_eq!(context.effort, Some(EffortLevel::Medium));
    assert_eq!(app.context_effort, Some(EffortLevel::Medium));
}

#[test]
fn sampling_temperature_field_accepts_the_default_keyword() {
    let (mut app, _rx, _secrets) = test_app();
    submit(&mut app, "/temperature");
    for ch in "default".chars() {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }
    app.handle_key_event(key(KeyCode::Enter));

    assert!(app.modal.is_none());
    let context = app.settings.context.as_ref().unwrap();
    assert_eq!(context.temperature, None, "default must clear the override");
    assert_eq!(app.context_effort, Some(EffortLevel::Medium));
}

#[test]
fn sampling_panel_sets_effort_and_temperature_together() {
    let (mut app, _rx, _secrets) = test_app();
    submit(&mut app, "/sampling");
    assert!(matches!(app.modal, Some(ActiveModal::Sampling { .. })));

    app.handle_key_event(key(KeyCode::Char('h')));
    app.handle_key_event(key(KeyCode::Tab));
    for ch in "0.9".chars() {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }
    app.handle_key_event(key(KeyCode::Enter));

    assert!(app.modal.is_none());
    let context = app.settings.context.as_ref().unwrap();
    assert_eq!(context.effort, Some(EffortLevel::High));
    assert_eq!(context.temperature.map(|t| t.value()), Some(0.9));
    assert_eq!(app.context_temperature, Some(0.9));
    assert!(
        app.take_pending_session_actions()
            .iter()
            .any(|action| { matches!(action, PendingSessionAction::SyncContext) })
    );
}

#[test]
fn queued_edit_arrows_move_across_wrapped_rows() {
    let (mut app, _rx, _secrets) = test_app();
    app.queued_editor_width = 5;
    app.tab_mut().queued = vec!["abcdefghijkl".to_string()];
    app.modal = Some(ActiveModal::QueuedEdit {
        index: 0,
        input: "abcdefghijkl".to_string(),
        cursor: 7,
        error: None,
    });

    app.handle_key_event(key(KeyCode::Up));
    let Some(ActiveModal::QueuedEdit { cursor, .. }) = &app.modal else {
        panic!("queued edit modal should stay open");
    };
    assert_eq!(*cursor, 2);

    app.handle_key_event(key(KeyCode::Down));
    let Some(ActiveModal::QueuedEdit { cursor, .. }) = &app.modal else {
        panic!("queued edit modal should stay open");
    };
    assert_eq!(*cursor, 7);
}

#[test]
fn queued_edit_arrows_keep_the_goal_column() {
    let (mut app, _rx, _secrets) = test_app();
    app.queued_editor_width = 5;
    app.tab_mut().queued = vec!["abcdefghij\nxy\nklmnopqrst".to_string()];
    app.modal = Some(ActiveModal::QueuedEdit {
        index: 0,
        input: "abcdefghij\nxy\nklmnopqrst".to_string(),
        cursor: 8,
        error: None,
    });

    app.handle_key_event(key(KeyCode::Down));
    let Some(ActiveModal::QueuedEdit { cursor, .. }) = &app.modal else {
        panic!("queued edit modal should stay open");
    };
    assert_eq!(*cursor, 13);

    app.handle_key_event(key(KeyCode::Down));
    let Some(ActiveModal::QueuedEdit { cursor, .. }) = &app.modal else {
        panic!("queued edit modal should stay open");
    };
    assert_eq!(*cursor, 17);

    app.handle_key_event(key(KeyCode::Up));
    app.handle_key_event(key(KeyCode::Up));
    let Some(ActiveModal::QueuedEdit { cursor, .. }) = &app.modal else {
        panic!("queued edit modal should stay open");
    };
    assert_eq!(*cursor, 8);
}

#[test]
fn shell_runs_track_concurrent_commands_and_output() {
    let (mut app, _rx, _secrets) = test_app();
    let (tab_id, first) = app.start_shell_run("sleep 1".to_string());
    let (_, second) = app.start_shell_run("cargo test".to_string());
    assert_eq!(app.run_count(RunKind::Shells), 2);

    app.complete_shell_run(tab_id, first, Ok("done".to_string()));
    assert_eq!(app.run_count(RunKind::Shells), 1);
    assert_eq!(
        app.shell_run(first).and_then(|run| run.output.as_deref()),
        Some("done")
    );
    assert_eq!(
        app.shell_run(second).map(|run| run.status),
        Some(ShellRunStatus::Running)
    );
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::System { text }) if text == "done"
    ));
}
