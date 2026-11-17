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

    app.handle_key_event(key(KeyCode::Tab));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::DiffView {
            file_index: 0,
            list: DiffListState::Open { selected: 0 },
            ..
        })
    ));
    app.handle_key_event(key(KeyCode::BackTab));
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
