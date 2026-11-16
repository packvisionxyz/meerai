use std::time::Duration;

use meerai_acp::{AGY, AcpEvent, AcpPermissionKind, connect};
use tokio::time::{Instant, timeout};

const STEP_TIMEOUT: Duration = Duration::from_secs(60);

#[tokio::test]
#[ignore = "live agy"]
async fn live_agy_replies_with_ok() {
    let cwd = std::env::current_dir().expect("working directory");
    let mut session = connect(AGY, cwd).await.expect("connect to agy acp");
    assert!(!session.session_id.is_empty());

    let handshake = session.handshake();
    println!(
        "handshake: protocol={} agent={:?} title={:?} version={:?} load_session={} auth_methods={:?} modes={:?} session={} resumed={}",
        handshake.protocol_version,
        handshake.agent_name,
        handshake.agent_title,
        handshake.agent_version,
        handshake.load_session,
        handshake.auth_methods,
        handshake.modes,
        handshake.session_id,
        handshake.resumed,
    );
    assert_eq!(handshake.protocol_version, 1);
    assert_eq!(handshake.session_id, session.session_id);
    assert!(!handshake.resumed);

    timeout(STEP_TIMEOUT, async {
        while let Some(event) = session.events.recv().await {
            if matches!(event, AcpEvent::Connected { .. }) {
                return;
            }
        }
        panic!("event stream closed before Connected");
    })
    .await
    .expect("Connected within 60 s");

    session.prompt("Reply with OK").await.expect("prompt sent");

    let deadline = Instant::now() + STEP_TIMEOUT;
    let stop_reason = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let event = timeout(remaining, session.events.recv())
            .await
            .expect("Finished within 60 s")
            .expect("event stream open");
        match event {
            AcpEvent::PermissionRequest(request) => {
                let option = request
                    .options
                    .iter()
                    .find(|option| option.kind == AcpPermissionKind::AllowOnce)
                    .map(|option| option.option_id.clone());
                match option {
                    Some(option_id) => session
                        .reply_permission(request.request_id, option_id)
                        .await
                        .expect("permission reply sent"),
                    None => session
                        .cancel_permission(request.request_id)
                        .await
                        .expect("permission cancel sent"),
                }
            }
            AcpEvent::Finished { stop_reason } => break stop_reason,
            AcpEvent::Error(message) => panic!("ACP error: {message}"),
            _ => {}
        }
    };

    assert_eq!(stop_reason, "end_turn");
    session.shutdown().await.expect("shutdown");
}
