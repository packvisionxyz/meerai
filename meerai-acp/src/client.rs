use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    CancelNotification, ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionId, SessionNotification, TextContent,
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

pub struct AcpSession {
    pub session_id: String,
    pub events: mpsc::UnboundedReceiver<AcpEvent>,
    commands: mpsc::UnboundedSender<Command>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    task: JoinHandle<Result<(), agent_client_protocol::Error>>,
}

impl AcpSession {
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
        let lines = self.stderr.lock().expect("stderr tail lock poisoned");
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
            let mut lines = stderr.lock().expect("stderr tail lock poisoned");
            if lines.len() == STDERR_TAIL_LINES {
                lines.pop_front();
            }
            lines.push_back(line.to_string());
        }
    });

    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (permission_tx, permission_rx) = mpsc::unbounded_channel();
    let (ready_tx, ready_rx) = oneshot::channel();
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

            let new_session = connection
                .send_request(NewSessionRequest::new(cwd))
                .block_task()
                .await;
            let new_session = match new_session {
                Ok(response) => response,
                Err(error) => {
                    let _ = ready_tx.send(Err(error.to_string()));
                    return Err(error);
                }
            };

            let session_id = new_session.session_id;
            let (agent_name, agent_version) = match initialize.agent_info {
                Some(info) => (Some(info.name), Some(info.version)),
                None => (None, None),
            };
            let _ = events_tx.send(AcpEvent::Connected {
                agent_name,
                agent_version,
                session_id: session_id.to_string(),
            });
            let _ = ready_tx.send(Ok(session_id.to_string()));

            run_commands(connection, session_id, command_rx, permission_rx, events_tx).await;
            Ok(())
        });

    let task = tokio::spawn(connection);

    let session_id = match timeout(CONNECT_TIMEOUT, ready_rx).await {
        Ok(Ok(Ok(session_id))) => session_id,
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
        session_id,
        events: events_rx,
        commands: command_tx,
        stderr,
        task,
    })
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
