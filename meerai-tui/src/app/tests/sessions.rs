use super::*;

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
        backend: Backend::Native,
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

fn make_opened_acp_session(
    id: &str,
    preset: &str,
) -> (OpenedSession, mpsc::UnboundedReceiver<AgentCommand>) {
    let (mut opened, rx) = make_opened_session(id, Some("ACP session"), Vec::new());
    opened.provider = String::new();
    opened.model = String::new();
    opened.backend = Backend::Acp {
        preset: preset.to_string(),
        remote_session_id: None,
    };
    (opened, rx)
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
fn clicking_the_tab_close_button_closes_without_deleting_the_session() {
    let (mut app, _rx, _secrets) = test_app();
    let first = app.tab().session_id.clone();
    let (opened, _event_rx) = make_opened_session("ses-close-two", Some("two"), Vec::new());
    let _ = app.apply_opened_session(opened, false);
    assert_eq!(app.tabs().len(), 2);

    app.tab_hit_zones = vec![TabHitZone {
        rect: Rect::new(10, 0, 1, 1),
        kind: TabHitKind::CloseTab(1),
    }];
    assert!(app.click_tab_at(10, 0));
    assert_eq!(app.tabs().len(), 1);
    assert_eq!(app.tab().session_id, first);

    app.pending_session_actions.clear();
}

#[test]
fn session_picker_deletes_after_a_second_ctrl_d() {
    let (mut app, _rx, _secrets) = test_app();
    let summary = make_session_summary("ses-delete", Some("Old task"), 10, None);
    app.modal = Some(ActiveModal::SessionSelect {
        filter: String::new(),
        selected: 0,
        sessions: vec![summary.clone()],
        loading: false,
        error: None,
    });

    app.handle_key_event(key_mod(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert_eq!(app.session_delete_confirm.as_ref(), Some(&summary.id));
    assert!(app.pending_session_actions.is_empty());

    app.handle_key_event(key_mod(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert!(app.session_delete_confirm.is_none());
    assert!(matches!(
        app.pending_session_actions.last(),
        Some(PendingSessionAction::DeleteSession(id)) if id == &summary.id
    ));
}

#[test]
fn session_picker_refuses_to_delete_an_open_session() {
    let (mut app, _rx, _secrets) = test_app();
    let (opened, _event_rx) = make_opened_session("ses-open", Some("Open"), Vec::new());
    let _ = app.apply_opened_session(opened, false);
    let summary = make_session_summary("ses-open", Some("Open"), 10, None);
    app.modal = Some(ActiveModal::SessionSelect {
        filter: String::new(),
        selected: 0,
        sessions: vec![summary.clone()],
        loading: false,
        error: None,
    });

    app.handle_key_event(key_mod(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert!(app.session_delete_confirm.is_none());
    assert!(app.pending_session_actions.is_empty());
    assert!(matches!(
        &app.modal,
        Some(ActiveModal::SessionSelect { error: Some(message), .. })
            if message.contains("Close the tab")
    ));
}

#[test]
fn acp_tabs_keep_the_provider_runtime_untouched_on_switch() {
    let (mut app, _rx, _secrets) = test_app();
    let (opened, _event_rx) = make_opened_acp_session("ses-acp", "agy");
    assert!(app.apply_opened_session(opened, false).is_some());

    assert_eq!(app.active_tab_index(), 1);
    assert!(app.tab().is_acp());
    assert_eq!(app.tab().acp_preset(), Some("agy"));
    assert_eq!(
        app.tab().backend,
        Backend::Acp {
            preset: "agy".to_string(),
            remote_session_id: None,
        }
    );

    assert!(app.switch_to_tab(0));
    app.runtime.write().unwrap().active_provider = "openrouter".to_string();
    assert!(app.switch_to_tab(1));
    assert_eq!(app.runtime.read().unwrap().active_provider, "openrouter");
}

#[tokio::test]
async fn default_acp_session_creation_reports_unsupported() {
    let opener = DummySessionOpener;
    let err = match opener.create_acp_session("agy").await {
        Ok(_) => panic!("default create_acp_session must fail"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("not supported"));
}
