use super::*;
use crate::app::input::encode_base64;

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
        app.tab().transcript.first(),
        Some(TranscriptEntry::Assistant {
            reasoning_expanded: true,
            ..
        })
    ));
}

#[test]
fn dragging_over_expanded_content_does_not_collapse_it() {
    let (mut app, _rx, _secrets) = test_app();
    app.tab_mut().transcript.push(TranscriptEntry::Assistant {
        text: "Hello!".to_string(),
        reasoning: "line 1\nline 2".to_string(),
        streaming: false,
        reasoning_expanded: true,
    });
    app.transcript_inner_area = Rect::new(1, 1, 60, 10);
    app.thinking_hit_zones = vec![ThinkingHitZone {
        entry_idx: 0,
        start_line: 0,
        end_line: 3,
    }];

    let press = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 1,
        modifiers: KeyModifiers::NONE,
    };
    let drag = MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 20,
        row: 3,
        modifiers: KeyModifiers::NONE,
    };
    let release = MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 20,
        row: 3,
        modifiers: KeyModifiers::NONE,
    };
    assert!(app.handle_mouse_event(press));
    assert!(app.handle_mouse_event(drag));
    assert!(app.handle_mouse_event(release));
    assert!(matches!(
        app.tab().transcript.first(),
        Some(TranscriptEntry::Assistant {
            reasoning_expanded: true,
            ..
        })
    ));
}

#[test]
fn clicking_the_composer_moves_the_cursor() {
    let (mut app, _rx, _secrets) = test_app();
    app.tab_mut().input = "hello world".to_string();
    app.tab_mut().cursor = 0;
    app.composer_block_area = Rect::new(0, 19, 60, 5);
    app.composer_input_area = Rect::new(2, 20, 40, 3);
    app.composer_scroll = 0;
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 8,
        row: 20,
        modifiers: KeyModifiers::NONE,
    };
    assert!(app.handle_mouse_event(click));
    assert_eq!(app.tab().cursor, 6);
    assert_eq!(app.mode, InputMode::Insert);
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

    app.scroll_to_top();
    terminal
        .draw(|frame| crate::ui::render(frame, &mut app))
        .unwrap();
    let start_idx = app
        .transcript_wrapped_lines
        .iter()
        .position(|line| line.text.contains("Start marker"))
        .expect("start marker line");
    let inner = app.transcript_inner_area;
    let start_screen_row = (inner.y + (start_idx - app.tab().scroll_offset) as u16)
        .min(inner.y + inner.height.saturating_sub(1));
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
    app.handle_key_event(key(KeyCode::Esc));

    app.scroll_to_bottom();
    terminal
        .draw(|frame| crate::ui::render(frame, &mut app))
        .unwrap();
    let end_idx = app
        .transcript_wrapped_lines
        .iter()
        .position(|line| line.text.contains("OMEGA_END"))
        .expect("end marker line");
    let inner = app.transcript_inner_area;
    let end_screen_row = (inner.y + (end_idx - app.tab().scroll_offset) as u16)
        .min(inner.y + inner.height.saturating_sub(1));
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

#[test]
fn dragging_in_the_composer_selects_and_copies() {
    let (mut app, _rx, _secrets) = test_app();
    app.tab_mut().input = "hello world".to_string();
    app.tab_mut().cursor = 0;
    app.composer_block_area = Rect::new(0, 19, 60, 4);
    app.composer_input_area = Rect::new(2, 20, 50, 2);

    let down = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 2,
        row: 20,
        modifiers: KeyModifiers::NONE,
    };
    let drag = MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 8,
        row: 20,
        modifiers: KeyModifiers::NONE,
    };
    let up = MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 8,
        row: 20,
        modifiers: KeyModifiers::NONE,
    };
    assert!(app.handle_mouse_event(down));
    assert!(app.handle_mouse_event(drag));
    assert!(app.handle_mouse_event(up));

    assert_eq!(app.composer_selection, Some((0, 6)));
    assert_eq!(app.last_copied_text.as_deref(), Some("hello "));
    assert_eq!(app.tab().cursor, 6);

    app.handle_key_event(key(KeyCode::Backspace));
    assert_eq!(app.tab().input, "world");
    assert_eq!(app.tab().cursor, 0);
}

#[test]
fn dragging_below_the_composer_keeps_the_selection() {
    let (mut app, _rx, _secrets) = test_app();
    app.tab_mut().input = "hello world".to_string();
    app.tab_mut().cursor = 0;
    app.composer_block_area = Rect::new(0, 19, 60, 4);
    app.composer_input_area = Rect::new(2, 20, 50, 2);

    assert!(app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 2,
        row: 20,
        modifiers: KeyModifiers::NONE,
    }));
    assert!(app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 70,
        row: 26,
        modifiers: KeyModifiers::NONE,
    }));
    assert!(app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 70,
        row: 26,
        modifiers: KeyModifiers::NONE,
    }));

    assert_eq!(app.composer_selection, Some((0, "hello world".len())));
    assert_eq!(app.last_copied_text.as_deref(), Some("hello world"));
}

#[test]
fn double_clicking_the_composer_selects_a_word() {
    let (mut app, _rx, _secrets) = test_app();
    app.tab_mut().input = "hello world".to_string();
    app.tab_mut().cursor = 0;
    app.composer_block_area = Rect::new(0, 19, 60, 4);
    app.composer_input_area = Rect::new(2, 20, 50, 2);

    for _ in 0..2 {
        assert!(app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 8,
            row: 20,
            modifiers: KeyModifiers::NONE,
        }));
        assert!(app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 8,
            row: 20,
            modifiers: KeyModifiers::NONE,
        }));
    }

    assert_eq!(app.composer_selection, Some((6, 11)));
    assert_eq!(app.last_copied_text.as_deref(), Some("world"));
    assert_eq!(app.tab().cursor, 11);
}

#[test]
fn alt_shift_left_right_moves_the_active_tab() {
    let (mut app, _rx, _secrets) = test_app();
    let first = app.tab().session_id.as_str().to_string();
    for id in ["sess-b", "sess-c"] {
        let (opened, _cmd_rx) = super::sessions::make_opened_session(id, Some(id), Vec::new());
        let _ = app.apply_opened_session(opened, SessionOpenMode::NewTab);
    }
    let order = |app: &AppState| -> Vec<String> {
        app.tabs()
            .iter()
            .map(|tab| tab.session_id.as_str().to_string())
            .collect()
    };
    assert_eq!(order(&app), [first.as_str(), "sess-b", "sess-c"]);
    assert_eq!(app.active_tab_index(), 2);

    let alt_shift = KeyModifiers::ALT | KeyModifiers::SHIFT;
    app.handle_key_event(key_mod(KeyCode::Left, alt_shift));
    assert_eq!(app.active_tab_index(), 1);
    assert_eq!(app.tab().session_id.as_str(), "sess-c");
    assert_eq!(order(&app), [first.as_str(), "sess-c", "sess-b"]);

    app.handle_key_event(key_mod(KeyCode::Right, alt_shift));
    assert_eq!(app.active_tab_index(), 2);
    assert_eq!(order(&app), [first.as_str(), "sess-b", "sess-c"]);

    app.handle_key_event(key_mod(KeyCode::Right, alt_shift));
    assert_eq!(app.active_tab_index(), 2);
    assert_eq!(order(&app), [first.as_str(), "sess-b", "sess-c"]);

    assert!(app.switch_to_tab(0));
    app.handle_key_event(key_mod(KeyCode::Left, alt_shift));
    assert_eq!(app.active_tab_index(), 0);
    assert_eq!(order(&app), [first.as_str(), "sess-b", "sess-c"]);

    app.handle_key_event(key_mod(KeyCode::Left, KeyModifiers::ALT));
    assert_eq!(app.active_tab_index(), 2);
    assert_eq!(order(&app), [first.as_str(), "sess-b", "sess-c"]);
}
