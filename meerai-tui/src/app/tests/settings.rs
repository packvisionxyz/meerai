use super::*;

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

#[tokio::test]
async fn reload_keeps_the_session_and_sends_the_agent_command() {
    let (mut app, mut rx, _secrets) = test_app();
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "keep me".to_string(),
    });
    app.launch_system = "Be terse.".to_string();

    app.perform_reload().await;

    assert!(matches!(
        rx.try_recv().unwrap(),
        AgentCommand::Reload { .. }
    ));
    assert!(matches!(
        app.tab().transcript.first(),
        Some(TranscriptEntry::User { text }) if text == "keep me"
    ));
    assert!(app.tab().transcript.iter().any(|entry| matches!(
        entry,
        TranscriptEntry::System { text } if text.contains("Reloaded")
    )));
}

fn temp_hooks_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "meerai_tui_hooks_{name}_{}_{}",
        std::process::id(),
        nanos
    ))
}

#[tokio::test]
async fn reload_sends_hook_entries_from_the_config_dir() {
    let (mut app, mut rx, _secrets) = test_app();
    let dir = temp_hooks_dir("reload");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("hooks.json"),
        r#"[ { "event": "SessionStart", "command": "start.sh" } ]"#,
    )
    .unwrap();
    app.config_dir = Some(dir.clone());

    app.perform_reload().await;

    let mut reloaded = false;
    let mut hook_entries = None;
    while let Ok(command) = rx.try_recv() {
        match command {
            AgentCommand::Reload { .. } => reloaded = true,
            AgentCommand::SetHooks(entries) => hook_entries = Some(entries),
            _ => {}
        }
    }

    assert!(reloaded);
    let entries = hook_entries.expect("expected SetHooks");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].command, "start.sh");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reload_slash_command_defers_to_the_event_loop() {
    let (mut app, _rx, _secrets) = test_app();
    app.tab_mut().input = "/reload".to_string();
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.pending_reload);
}
