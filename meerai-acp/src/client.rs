use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    AuthMethod, CancelNotification, ContentBlock, InitializeRequest, InitializeResponse,
    LoadSessionRequest, NewSessionRequest, PromptRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome, SessionId,
    SessionModeState, SessionNotification, TextContent,
};
use agent_client_protocol::{AcpAgent, Agent, Client, LineDirection};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::events::{AcpEvent, event_from_update, permission_request, stop_reason};
use crate::{AcpPermissionRequest, AcpPreset};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(120);
const STDERR_TAIL_LINES: usize = 200;

type PendingPermission = (AcpPermissionRequest, oneshot::Sender<Option<String>>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcpHandshake {
    pub protocol_version: u16,
    pub agent_name: Option<String>,
    pub agent_title: Option<String>,
    pub agent_version: Option<String>,
    pub load_session: bool,
    pub auth_methods: Vec<String>,
    pub modes: Vec<String>,
    pub session_id: String,
    pub resumed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AcpStart {
    New,
    Load { session_id: String },
}

pub struct AcpSession {
    pub session_id: String,
    pub events: mpsc::UnboundedReceiver<AcpEvent>,
    handshake: AcpHandshake,
    commands: mpsc::UnboundedSender<Command>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    task: JoinHandle<Result<(), agent_client_protocol::Error>>,
}

impl AcpSession {
    pub fn handshake(&self) -> &AcpHandshake {
        &self.handshake
    }

    pub async fn prompt(&self, text: impl Into<String>) -> anyhow::Result<()> {
        self.send(Command::Prompt(text.into()))
    }

    pub async fn cancel(&self) -> anyhow::Result<()> {
        self.send(Command::Cancel)
    }

    pub async fn reply_permission(
        &self,
        request_id: u64,
        option_id: impl Into<String>,
    ) -> anyhow::Result<()> {
        self.send(Command::Reply {
            request_id,
            option_id: Some(option_id.into()),
        })
    }

    pub async fn cancel_permission(&self, request_id: u64) -> anyhow::Result<()> {
        self.send(Command::Reply {
            request_id,
            option_id: None,
        })
    }

    pub fn stderr_tail(&self) -> Vec<String> {
        let Ok(lines) = self.stderr.lock() else {
            return Vec::new();
        };
        lines.iter().cloned().collect()
    }

    pub async fn shutdown(self) -> anyhow::Result<()> {
        let AcpSession { commands, task, .. } = self;
        let _ = commands.send(Command::Shutdown);
        drop(commands);
        task.await??;
        Ok(())
    }

    fn send(&self, command: Command) -> anyhow::Result<()> {
        self.commands
            .send(command)
            .map_err(|_| anyhow::anyhow!("ACP session is closed"))
    }
}

enum Command {
    Prompt(String),
    Cancel,
    Reply {
        request_id: u64,
        option_id: Option<String>,
    },
    Shutdown,
}

pub async fn connect(preset: AcpPreset, cwd: PathBuf) -> anyhow::Result<AcpSession> {
    connect_with(preset, cwd, AcpStart::New).await
}

pub async fn connect_with(
    preset: AcpPreset,
    cwd: PathBuf,
    start: AcpStart,
) -> anyhow::Result<AcpSession> {
    let mut args = Vec::with_capacity(1 + preset.args.len());
    args.push(preset.command.to_string());
    args.extend(preset.args.iter().map(|arg| (*arg).to_string()));

    let stderr = Arc::new(Mutex::new(VecDeque::new()));
    let agent = AcpAgent::from_args(args)?.with_debug({
        let stderr = Arc::clone(&stderr);
        move |line, direction| {
            if direction != LineDirection::Stderr {
                return;
            }
            let Ok(mut lines) = stderr.lock() else {
                return;
            };
            if lines.len() == STDERR_TAIL_LINES {
                lines.pop_front();
            }
            lines.push_back(line.to_string());
        }
    });

    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (permission_tx, permission_rx) = mpsc::unbounded_channel();
    let (ready_tx, ready_rx) = oneshot::channel::<Result<AcpHandshake, String>>();
    let request_ids = Arc::new(AtomicU64::new(0));

    let connection = Client
        .v1()
        .name("meerai-acp")
        .on_receive_notification(
            {
                let events = events_tx.clone();
                async move |notification: SessionNotification, _connection| {
                    if let Some(event) = event_from_update(notification.update) {
                        let _ = events.send(event);
                    }
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            {
                let permissions = permission_tx.clone();
                let request_ids = Arc::clone(&request_ids);
                async move |request: RequestPermissionRequest, responder, _connection| {
                    let request_id = request_ids.fetch_add(1, Ordering::Relaxed) + 1;
                    let event = permission_request(request_id, request);
                    let (reply_tx, reply_rx) = oneshot::channel();
                    let reply = if permissions.send((event, reply_tx)).is_ok() {
                        reply_rx.await.ok().flatten()
                    } else {
                        None
                    };
                    let outcome = match reply {
                        Some(option_id) => RequestPermissionOutcome::Selected(
                            SelectedPermissionOutcome::new(option_id),
                        ),
                        None => RequestPermissionOutcome::Cancelled,
                    };
                    responder.respond(RequestPermissionResponse::new(outcome))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, move |connection: ConnectionTo<Agent>| async move {
            let initialize = connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await;
            let initialize = match initialize {
                Ok(response) => response,
                Err(error) => {
                    let _ = ready_tx.send(Err(error.to_string()));
                    return Err(error);
                }
            };

            let load_session = initialize.agent_capabilities.load_session;
            let (session_id, modes, resumed) = match plan_session(&start, load_session) {
                SessionPlan::Load(session_id) => {
                    let request = LoadSessionRequest::new(session_id.clone(), cwd.clone());
                    let response = connection.send_request(request).block_task().await;
                    match response {
                        Ok(response) => (session_id, response.modes, true),
                        Err(error) => {
                            let _ = ready_tx.send(Err(error.to_string()));
                            return Err(error);
                        }
                    }
                }
                SessionPlan::New => {
                    let request = NewSessionRequest::new(cwd);
                    let response = connection.send_request(request).block_task().await;
                    match response {
                        Ok(response) => (response.session_id.to_string(), response.modes, false),
                        Err(error) => {
                            let _ = ready_tx.send(Err(error.to_string()));
                            return Err(error);
                        }
                    }
                }
            };

            let handshake =
                handshake_from_initialize(&initialize, modes.as_ref(), session_id.clone(), resumed);
            let _ = events_tx.send(AcpEvent::Connected {
                agent_name: handshake.agent_name.clone(),
                agent_version: handshake.agent_version.clone(),
                session_id: session_id.clone(),
            });
            let _ = ready_tx.send(Ok(handshake.clone()));

            let session_id = SessionId::new(session_id);
            run_commands(connection, session_id, command_rx, permission_rx, events_tx).await;
            Ok(())
        });

    let task = tokio::spawn(connection);

    let handshake = match timeout(CONNECT_TIMEOUT, ready_rx).await {
        Ok(Ok(Ok(handshake))) => handshake,
        Ok(Ok(Err(message))) => {
            task.abort();
            return Err(anyhow::anyhow!(message));
        }
        Ok(Err(error)) => {
            task.abort();
            return Err(anyhow::anyhow!(
                "ACP connection closed during handshake: {error}"
            ));
        }
        Err(_) => {
            task.abort();
            return Err(anyhow::anyhow!(
                "ACP handshake timed out after {}s",
                CONNECT_TIMEOUT.as_secs()
            ));
        }
    };

    Ok(AcpSession {
        session_id: handshake.session_id.clone(),
        handshake,
        events: events_rx,
        commands: command_tx,
        stderr,
        task,
    })
}

enum SessionPlan {
    New,
    Load(String),
}

fn plan_session(start: &AcpStart, load_session: bool) -> SessionPlan {
    match start {
        AcpStart::Load { session_id } if load_session => SessionPlan::Load(session_id.clone()),
        _ => SessionPlan::New,
    }
}

fn handshake_from_initialize(
    initialize: &InitializeResponse,
    modes: Option<&SessionModeState>,
    session_id: String,
    resumed: bool,
) -> AcpHandshake {
    let (agent_name, agent_title, agent_version) = match &initialize.agent_info {
        Some(info) => (
            Some(info.name.clone()),
            info.title.clone(),
            Some(info.version.clone()),
        ),
        None => (None, None, None),
    };
    AcpHandshake {
        protocol_version: initialize.protocol_version.as_u16(),
        agent_name,
        agent_title,
        agent_version,
        load_session: initialize.agent_capabilities.load_session,
        auth_methods: initialize
            .auth_methods
            .iter()
            .filter_map(auth_method_name)
            .collect(),
        modes: modes
            .map(|state| {
                state
                    .available_modes
                    .iter()
                    .map(|mode| mode.name.clone())
                    .collect()
            })
            .unwrap_or_default(),
        session_id,
        resumed,
    }
}

fn auth_method_name(method: &AuthMethod) -> Option<String> {
    match method {
        AuthMethod::Terminal(method) => Some(method.name.clone()),
        AuthMethod::Agent(method) => Some(method.name.clone()),
        _ => None,
    }
}

async fn run_commands(
    connection: ConnectionTo<Agent>,
    session_id: SessionId,
    mut commands: mpsc::UnboundedReceiver<Command>,
    mut permissions: mpsc::UnboundedReceiver<PendingPermission>,
    events: mpsc::UnboundedSender<AcpEvent>,
) {
    let mut pending: HashMap<u64, oneshot::Sender<Option<String>>> = HashMap::new();

    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else {
                    break;
                };
                match command {
                    Command::Prompt(text) => {
                        let connection = connection.clone();
                        let session_id = session_id.clone();
                        let events = events.clone();
                        tokio::spawn(async move {
                            let request = PromptRequest::new(
                                session_id,
                                vec![ContentBlock::Text(TextContent::new(text))],
                            );
                            match connection.send_request(request).block_task().await {
                                Ok(response) => {
                                    let _ = events.send(AcpEvent::Finished {
                                        stop_reason: stop_reason(&response.stop_reason),
                                    });
                                }
                                Err(error) => {
                                    let _ = events.send(AcpEvent::Error(error.to_string()));
                                }
                            }
                        });
                    }
                    Command::Cancel => {
                        let notification = CancelNotification::new(session_id.clone());
                        if let Err(error) = connection.send_notification_to(Agent, notification) {
                            let _ = events.send(AcpEvent::Error(format!(
                                "session/cancel failed: {error}"
                            )));
                        }
                    }
                    Command::Reply { request_id, option_id } => {
                        if let Some(reply) = pending.remove(&request_id) {
                            let _ = reply.send(option_id);
                        }
                    }
                    Command::Shutdown => break,
                }
            }
            request = permissions.recv() => {
                let Some((event, reply)) = request else {
                    break;
                };
                pending.insert(event.request_id, reply);
                let _ = events.send(AcpEvent::PermissionRequest(event));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::v1::schema::{
        AgentCapabilities, AuthMethodAgent, AuthMethodTerminal, Implementation, SessionMode,
        SessionModeId,
    };

    fn load_start() -> AcpStart {
        AcpStart::Load {
            session_id: "remote-1".to_string(),
        }
    }

    #[test]
    fn load_plan_uses_session_load_when_supported() {
        match plan_session(&load_start(), true) {
            SessionPlan::Load(session_id) => assert_eq!(session_id, "remote-1"),
            SessionPlan::New => panic!("expected a session/load plan"),
        }
    }

    #[test]
    fn load_plan_falls_back_to_new_without_the_capability() {
        assert!(matches!(
            plan_session(&load_start(), false),
            SessionPlan::New
        ));
    }

    #[test]
    fn new_plan_ignores_the_load_capability() {
        assert!(matches!(
            plan_session(&AcpStart::New, true),
            SessionPlan::New
        ));
        assert!(matches!(
            plan_session(&AcpStart::New, false),
            SessionPlan::New
        ));
    }

    #[test]
    fn handshake_reports_initialize_and_session_metadata() {
        let mut capabilities = AgentCapabilities::default();
        capabilities.load_session = true;
        let mut initialize = InitializeResponse::new(ProtocolVersion::V1)
            .agent_capabilities(capabilities)
            .auth_methods(vec![
                AuthMethod::Agent(AuthMethodAgent::new("api-key", "API key")),
                AuthMethod::Terminal(AuthMethodTerminal::new("login", "Sign in")),
            ]);
        initialize.agent_info = Some(Implementation::new("agy", "1.5.9").title("Antigravity"));
        let modes = SessionModeState::new(
            SessionModeId::new("ask"),
            vec![
                SessionMode::new("ask", "Ask"),
                SessionMode::new("plan", "Plan"),
            ],
        );

        let handshake =
            handshake_from_initialize(&initialize, Some(&modes), "remote-1".to_string(), true);

        assert_eq!(handshake.protocol_version, 1);
        assert_eq!(handshake.agent_name.as_deref(), Some("agy"));
        assert_eq!(handshake.agent_title.as_deref(), Some("Antigravity"));
        assert_eq!(handshake.agent_version.as_deref(), Some("1.5.9"));
        assert!(handshake.load_session);
        assert_eq!(handshake.auth_methods, vec!["API key", "Sign in"]);
        assert_eq!(handshake.modes, vec!["Ask", "Plan"]);
        assert_eq!(handshake.session_id, "remote-1");
        assert!(handshake.resumed);
    }

    #[test]
    fn handshake_without_agent_info_or_modes_is_empty() {
        let initialize = InitializeResponse::new(ProtocolVersion::V1);
        let handshake = handshake_from_initialize(&initialize, None, "remote-2".to_string(), false);

        assert_eq!(handshake.protocol_version, 1);
        assert_eq!(handshake.agent_name, None);
        assert_eq!(handshake.agent_title, None);
        assert_eq!(handshake.agent_version, None);
        assert!(!handshake.load_session);
        assert!(handshake.auth_methods.is_empty());
        assert!(handshake.modes.is_empty());
        assert_eq!(handshake.session_id, "remote-2");
        assert!(!handshake.resumed);
    }
}
