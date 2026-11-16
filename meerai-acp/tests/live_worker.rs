use std::time::Duration;

use meerai_acp::{AGY, AcpStart, AcpWorkerConfig, spawn_acp_worker};
use meerai_agents::events::ApprovalDecision;
use meerai_agents::{AgentCommand, AgentEvent, ItemDelta};
use meerai_session::{InputSource, Session, SessionEvent, SessionId};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::time::timeout;

const TURN_TIMEOUT: Duration = Duration::from_secs(120);

#[tokio::test]
#[ignore = "live agy"]
async fn live_worker_mirrors_a_turn_into_the_session_log() {
    let cwd = std::env::current_dir().expect("working directory");
    let session = Session::memory(SessionId::new(), cwd.clone());
    let resumed = Arc::new(AtomicBool::new(true));
    let (commands, mut events) = spawn_acp_worker(AcpWorkerConfig {
        session: session.clone(),
        preset: AGY,
        cwd,
        start: AcpStart::New,
        resumed: Arc::clone(&resumed),
    });

    commands
        .send(AgentCommand::SubmitPrompt("Reply with OK".to_string()))
        .expect("prompt sent");

    let (text, reason) = timeout(TURN_TIMEOUT, async {
        let mut text = String::new();
        loop {
            let event = events.recv().await.expect("event stream open");
            match event {
                AgentEvent::ItemDelta {
                    delta: ItemDelta::AgentText(chunk),
                    ..
                } => text.push_str(&chunk),
                AgentEvent::ApprovalRequested { request } => {
                    commands
                        .send(AgentCommand::Approve {
                            request_id: request.request_id,
                            decision: ApprovalDecision::Accept,
                            scope: None,
                        })
                        .expect("approval sent");
                }
                AgentEvent::TurnCompleted { reason, .. } => return (text, reason),
                AgentEvent::Error { message, .. } => panic!("worker error: {message}"),
                _ => {}
            }
        }
    })
    .await
    .expect("turn completed within 120 s");

    assert!(text.contains("OK"), "assistant text: {text:?}");
    assert_eq!(reason, meerai_agents::TurnEndReason::Completed);

    let log = session.events();
    assert!(log.iter().any(|logged| matches!(
        &logged.event,
        SessionEvent::UserMessage {
            source: InputSource::User,
            content,
        } if content == &vec![meerai_session::ContentBlock::Text("Reply with OK".to_string())]
    )));
    assert!(log.iter().any(|logged| matches!(
        &logged.event,
        SessionEvent::AssistantMessage { provider, model, .. }
            if provider == "acp:agy" && model == "agy"
    )));
    assert!(
        log.iter()
            .any(|logged| matches!(&logged.event, SessionEvent::TurnEnded { .. }))
    );
    assert!(!resumed.load(Ordering::Relaxed));

    commands.send(AgentCommand::Quit).expect("quit sent");
}
