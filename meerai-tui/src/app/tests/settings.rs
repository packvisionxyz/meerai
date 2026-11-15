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

#[test]
fn reload_slash_command_defers_to_the_event_loop() {
    let (mut app, _rx, _secrets) = test_app();
    app.tab_mut().input = "/reload".to_string();
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.pending_reload);
}
