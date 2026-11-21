use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use crossterm::event::EventStream;
use futures::StreamExt;
use meerai_agents::{AgentEvent, SessionItem};
use meerai_host::{
    HostActionContext, LiveEntry, LiveSessionDetail, RegistryEvent, RegistryEventKind,
    SubscribeResult, execute_host_action,
};
use meerai_session::{LoggedEvent, SessionId, SessionSummary};
use tokio::sync::{broadcast, mpsc};
use tokio::time::{self, MissedTickBehavior};

use crate::app::{
    ActiveModal, AppState, FileChangeEntry, PendingSessionAction, RunningWebServer,
    SessionOpenMode, ToolCallEntry, ToolCallStatus, TranscriptEntry,
};
use crate::diff::list_git_changes;
use crate::terminal::{self, TerminalGuard};
use crate::{
    ChatGptLoginOutcome, OpenedSession, SessionOpener, WebServerHandle, WebServerStart, ui,
};

const TICK_INTERVAL: Duration = Duration::from_millis(80);
const GIT_POLL_TICKS: usize = 40;
const CREDENTIAL_POLL_TICKS: usize = 750;

enum SessionResolveOutcome {
    AlreadyOpen(SessionId),
    Opened {
        opened: Box<OpenedSession>,
        draft: Option<String>,
    },
    FromRegistry(Box<LiveSessionDetail>),
}

enum LoopMsg {
    Agent {
        tab_id: u64,
        event: AgentEvent,
    },
    RegistryAgent {
        session_id: SessionId,
        event: AgentEvent,
    },
    RegistryEvent(Box<RegistryEvent>),
    StreamClosed {
        tab_id: u64,
    },
    SessionsListed(Result<Vec<SessionSummary>>),
    SessionDeleted(Result<bool>),
    SessionCreatedForDelete {
        session_id: SessionId,
        created: Result<OpenedSession>,
    },
    SessionCreatedForReplace {
        replaced_tab: u64,
        created: Result<OpenedSession>,
    },
    SessionResolved {
        mode: SessionOpenMode,
        result: Result<SessionResolveOutcome>,
    },
    AcpResolved(Result<OpenedSession>),
    ChildSessionRead {
        tab_id: u64,
        item_id: String,
        result: Result<Vec<LoggedEvent>>,
    },
    UserShellDone {
        tab_id: u64,
        command: String,
        result: Result<String>,
    },
    LoginDone {
        result: Result<ChatGptLoginOutcome>,
    },
    WebServerStarted(Result<(Arc<dyn WebServerHandle>, WebServerStart)>),
    WebServerStopped,
    HostActionDone {
        tab_id: u64,
        result: Result<String>,
    },
    BlobLoaded {
        key: String,
        result: Result<String>,
    },
    MemoriesLoaded(Result<Vec<meerai_host::ProjectMemory>>),
    RoutesLoaded(Result<meerai_host::RoutingSettings>),
    PacksLoaded(Result<Vec<meerai_host::InstalledPack>>),
    DefinitionsLoaded(Result<meerai_host::HostDefinitionsDiagnostics>),
    ModalError(String),
}

pub async fn run_event_loop(
    terminal: &mut TerminalGuard,
    app: &mut AppState,
    event_rx: mpsc::UnboundedReceiver<AgentEvent>,
) -> Result<()> {
    let mut term_events = EventStream::new();
    let mut ticker = time::interval(TICK_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let (loop_tx, mut loop_rx) = mpsc::unbounded_channel::<LoopMsg>();
    if let Some(reg) = app.registry.clone() {
        spawn_registry_forwarder(reg, loop_tx.clone());
        drop(event_rx);
    } else {
        spawn_tab_forwarder(app.tab().tab_id, event_rx, loop_tx.clone());
    }

    let mut last_status: Option<terminal::TerminalStatus> = None;
    let mut last_title: Option<String> = None;
    let (git_tx, mut git_rx) = mpsc::unbounded_channel::<Vec<FileChangeEntry>>();
    let mut git_in_flight = false;
    let mut ticks_since_git = 0usize;
    let mut ticks_since_credentials = 0usize;

    spawn_git_status_refresh(
        app.workspace_root.clone(),
        git_tx.clone(),
        &mut git_in_flight,
    );

    let _ = dispatch_pending_actions(app, &loop_tx);
    terminal.draw(|frame| ui::render(frame, app))?;

    while !app.should_quit {
        let mut needs_redraw = false;

        tokio::select! {
            maybe_term = term_events.next() => {
                match maybe_term {
                    Some(Ok(ev)) => {
                        if app.handle_terminal_event(ev) {
                            needs_redraw = true;
                        }
                        if dispatch_pending_actions(app, &loop_tx) {
                            needs_redraw = true;
                        }
                    }
                    Some(Err(err)) => return Err(err.into()),
                    None => app.quit(),
                }
            }
            maybe_msg = loop_rx.recv() => {
                match maybe_msg {
                    Some(msg) => {
                        let mut should_refresh_git = handle_loop_msg(app, msg, &loop_tx);
                        while let Ok(next_msg) = loop_rx.try_recv() {
                            if handle_loop_msg(app, next_msg, &loop_tx) {
                                should_refresh_git = true;
                            }
                        }
                        if dispatch_pending_actions(app, &loop_tx) {
                            should_refresh_git = true;
                        }
                        if should_refresh_git {
                            spawn_git_status_refresh(
                                app.workspace_root.clone(),
                                git_tx.clone(),
                                &mut git_in_flight,
                            );
                            ticks_since_git = 0;
                        }
                        needs_redraw = true;
                    }
                    None => app.quit(),
                }
            }
            Some(changes) = git_rx.recv() => {
                git_in_flight = false;
                if app.file_changes != changes {
                    app.file_changes = changes;
                    needs_redraw = true;
                }
            }
            _ = ticker.tick() => {
                let any_running = app.tabs.iter().any(|tab| tab.is_running);
                let had_notice = app.status_notice.is_some();
                app.on_tick();
                if any_running || (had_notice && app.status_notice.is_none()) {
                    needs_redraw = true;
                }
                ticks_since_git += 1;
                if ticks_since_git >= GIT_POLL_TICKS {
                    ticks_since_git = 0;
                    spawn_git_status_refresh(
                        app.workspace_root.clone(),
                        git_tx.clone(),
                        &mut git_in_flight,
                    );
                }
                ticks_since_credentials += 1;
                if ticks_since_credentials >= CREDENTIAL_POLL_TICKS {
                    ticks_since_credentials = 0;
                    if let Some(opener) = app.session_opener.clone() {
                        tokio::spawn(async move {
                            let _ = opener.ensure_credentials().await;
                        });
                    }
                }
            }
        }

        if let Some(provider_id) = app.pending_login_request.take() {
            let result = match app.session_opener.clone() {
                Some(opener) => opener.start_chatgpt_login(&provider_id).await,
                None => Err(anyhow::anyhow!(
                    "ChatGPT sign-in is not available in this client"
                )),
            };
            match result {
                Ok(login) => {
                    app.login_input = Some(login.paste_tx);
                    app.login_cancel = Some(login.cancel_tx);
                    app.apply_login_started(login.url);
                    let tx = loop_tx.clone();
                    tokio::spawn(async move {
                        let result = login
                            .result_rx
                            .await
                            .unwrap_or_else(|_| Err(anyhow::anyhow!("ChatGPT sign-in task ended")));
                        let _ = tx.send(LoopMsg::LoginDone { result });
                    });
                }
                Err(err) => app.apply_login_failed(format!("{err:#}")),
            }
            needs_redraw = true;
        }

        let desired_status = if app.tabs.iter().any(|tab| !tab.pending_approvals.is_empty()) {
            terminal::TerminalStatus::Approval
        } else if app.tabs.iter().any(|tab| tab.is_running) {
            terminal::TerminalStatus::Working
        } else {
            terminal::TerminalStatus::Idle
        };
        let desired_title = app.tab().title.clone();
        if last_status != Some(desired_status) || last_title != desired_title {
            terminal::set_status(desired_status, desired_title.as_deref());
            last_status = Some(desired_status);
            last_title = desired_title;
        }

        if app.pending_reload {
            app.pending_reload = false;
            app.pending_quiet_reload = false;
            app.perform_reload().await;
            needs_redraw = true;
        } else if app.pending_quiet_reload {
            app.pending_quiet_reload = false;
            app.perform_reload_quiet().await;
            needs_redraw = true;
        }

        if app.should_quit {
            break;
        }

        if needs_redraw {
            terminal.draw(|frame| ui::render(frame, app))?;
        }
    }

    Ok(())
}

fn spawn_registry_forwarder(
    registry: Arc<crate::SessionRegistry>,
    tx: mpsc::UnboundedSender<LoopMsg>,
) {
    let mut agent_rx = registry.subscribe_agent_events();
    let tx_agent = tx.clone();
    tokio::spawn(async move {
        loop {
            match agent_rx.recv().await {
                Ok((session_id, event)) => {
                    if tx_agent
                        .send(LoopMsg::RegistryAgent { session_id, event })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    if let SubscribeResult::Replay { mut rx, .. } = registry.subscribe(None) {
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        if tx.send(LoopMsg::RegistryEvent(Box::new(ev))).is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

fn spawn_tab_forwarder(
    tab_id: u64,
    mut rx: mpsc::UnboundedReceiver<AgentEvent>,
    tx: mpsc::UnboundedSender<LoopMsg>,
) {
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if tx.send(LoopMsg::Agent { tab_id, event }).is_err() {
                return;
            }
        }
        let _ = tx.send(LoopMsg::StreamClosed { tab_id });
    });
}

fn dispatch_pending_actions(app: &mut AppState, loop_tx: &mpsc::UnboundedSender<LoopMsg>) -> bool {
    let actions = app.take_pending_session_actions();
    if actions.is_empty() {
        return false;
    }
    let opener = app.session_opener.clone();
    let mut changed = false;
    for action in actions {
        match action {
            PendingSessionAction::StartWebServer { port, secret } => {
                let Some(reg) = app.registry.clone() else {
                    app.modal = Some(ActiveModal::ServeDialog {
                        port_input: port.map(|p| p.to_string()).unwrap_or_default(),
                        secret_input: String::new(),
                        editing_secret: false,
                        error: Some("Session registry is unavailable".to_string()),
                        busy: false,
                    });
                    changed = true;
                    continue;
                };
                let (Some(launcher), Some(opener_ref)) = (app.web_launcher.clone(), opener.clone())
                else {
                    app.modal = None;
                    app.status_notice = Some("Web server launcher unavailable".to_string());
                    changed = true;
                    continue;
                };
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let result = launcher.start(reg, opener_ref, port, secret).await;
                    let _ = tx.send(LoopMsg::WebServerStarted(result));
                });
            }
            PendingSessionAction::StopWebServer => {
                if let Some(server) = app.web_server.take() {
                    let tx = loop_tx.clone();
                    tokio::spawn(async move {
                        server.handle.stop().await;
                        let _ = tx.send(LoopMsg::WebServerStopped);
                    });
                    changed = true;
                }
            }
            PendingSessionAction::CloseSession { session_id } => {
                if let Some(reg) = app.registry.clone() {
                    tokio::spawn(async move {
                        let _ = reg.close_and_wait_session(&session_id).await;
                    });
                }
            }
            PendingSessionAction::RefreshSessions => {
                let Some(opener) = opener.clone() else {
                    continue;
                };
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let result = opener.list_sessions().await;
                    let _ = tx.send(LoopMsg::SessionsListed(result));
                });
            }
            PendingSessionAction::OpenSessionId(id) => {
                if let Some(idx) = app.tab_index_for_session(&id) {
                    app.switch_to_tab(idx);
                    app.status_notice = Some(format!("Switched to tab {}", idx + 1));
                    changed = true;
                    continue;
                }
                let mode = if app.should_replace_active_tab() {
                    SessionOpenMode::ReplaceEmpty
                } else {
                    SessionOpenMode::NewTab
                };
                if let Some(detail) = app.registry.as_ref().and_then(|r| r.session_detail(&id)) {
                    let _ = loop_tx.send(LoopMsg::SessionResolved {
                        mode,
                        result: Ok(SessionResolveOutcome::FromRegistry(Box::new(detail))),
                    });
                    continue;
                }
                let Some(opener) = opener.clone() else {
                    continue;
                };
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let result = opener.open_session(&id).await.map(|opened| {
                        SessionResolveOutcome::Opened {
                            opened: Box::new(opened),
                            draft: None,
                        }
                    });
                    let _ = tx.send(LoopMsg::SessionResolved { mode, result });
                });
            }
            PendingSessionAction::RestoreSessionId(id) => {
                if app.tab_index_for_session(&id).is_some() {
                    continue;
                }
                if let Some(detail) = app.registry.as_ref().and_then(|r| r.session_detail(&id)) {
                    let _ = loop_tx.send(LoopMsg::SessionResolved {
                        mode: SessionOpenMode::Background,
                        result: Ok(SessionResolveOutcome::FromRegistry(Box::new(detail))),
                    });
                    continue;
                }
                let Some(opener) = opener.clone() else {
                    continue;
                };
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let result = opener.open_session(&id).await.map(|opened| {
                        SessionResolveOutcome::Opened {
                            opened: Box::new(opened),
                            draft: None,
                        }
                    });
                    let _ = tx.send(LoopMsg::SessionResolved {
                        mode: SessionOpenMode::Background,
                        result,
                    });
                });
            }
            PendingSessionAction::OpenSessionQuery(query) => {
                let Some(opener) = opener.clone() else {
                    continue;
                };
                let mode = if app.should_replace_active_tab() {
                    SessionOpenMode::ReplaceEmpty
                } else {
                    SessionOpenMode::NewTab
                };
                let open_ids: HashSet<SessionId> =
                    app.tabs.iter().map(|tab| tab.session_id.clone()).collect();
                let reg = app.registry.clone();
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let result =
                        resolve_and_open_query(&opener, reg.as_ref(), &open_ids, &query).await;
                    let _ = tx.send(LoopMsg::SessionResolved { mode, result });
                });
            }
            PendingSessionAction::CreateNewTab => {
                let Some(opener) = opener.clone() else {
                    continue;
                };
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let result =
                        opener
                            .create_session()
                            .await
                            .map(|opened| SessionResolveOutcome::Opened {
                                opened: Box::new(opened),
                                draft: None,
                            });
                    let _ = tx.send(LoopMsg::SessionResolved {
                        mode: SessionOpenMode::NewTab,
                        result,
                    });
                });
            }
            PendingSessionAction::CreateAcpTab { preset } => {
                let Some(opener) = opener.clone() else {
                    continue;
                };
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let result = opener.create_acp_session(&preset).await;
                    let _ = tx.send(LoopMsg::AcpResolved(result));
                });
            }
            PendingSessionAction::DeleteSession { session_id } => {
                let Some(opener) = opener.clone() else {
                    continue;
                };
                let close_tab = app.tab_index_for_session(&session_id);
                let create_replacement = close_tab == Some(app.active_tab) && app.tabs.len() == 1;
                if create_replacement {
                    let tx = loop_tx.clone();
                    tokio::spawn(async move {
                        let created = opener.create_session().await;
                        let _ = tx.send(LoopMsg::SessionCreatedForDelete {
                            session_id,
                            created,
                        });
                    });
                } else {
                    if let Some(idx) = close_tab {
                        changed |= app.close_tab(idx);
                    }
                    let reg = app.registry.clone();
                    let tx = loop_tx.clone();
                    tokio::spawn(async move {
                        if let Some(reg) = reg {
                            let _ = reg.close_and_wait_session(&session_id).await;
                        }
                        let result = opener.delete_session(&session_id).await;
                        let _ = tx.send(LoopMsg::SessionDeleted(result));
                    });
                }
            }
            PendingSessionAction::ReadChildSession {
                tab_id,
                item_id,
                session_id,
            } => {
                let Some(opener) = opener.clone() else {
                    continue;
                };
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let result = opener.read_session_events(&session_id).await;
                    let _ = tx.send(LoopMsg::ChildSessionRead {
                        tab_id,
                        item_id,
                        result,
                    });
                });
            }
            PendingSessionAction::RunUserShell { tab_id, command } => {
                let Some(opener) = opener.clone() else {
                    continue;
                };
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let result = opener.run_user_shell(&command).await;
                    let _ = tx.send(LoopMsg::UserShellDone {
                        tab_id,
                        command,
                        result,
                    });
                });
            }
            PendingSessionAction::ForkSession {
                session_id,
                turn,
                draft,
            } => {
                let Some(opener) = opener.clone() else {
                    continue;
                };
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let result = async {
                        let opened = match turn {
                            Some(turn) => opener.fork_session(&session_id, &turn).await?,
                            None => opener.create_session().await?,
                        };
                        Ok(SessionResolveOutcome::Opened {
                            opened: Box::new(opened),
                            draft: Some(draft),
                        })
                    }
                    .await;
                    let _ = tx.send(LoopMsg::SessionResolved {
                        mode: SessionOpenMode::NewTab,
                        result,
                    });
                });
            }
            PendingSessionAction::ReplaceLastTab => {
                let Some(opener) = opener.clone() else {
                    continue;
                };
                let replaced_tab = app.tab().tab_id;
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let created = opener.create_session().await;
                    let _ = tx.send(LoopMsg::SessionCreatedForReplace {
                        replaced_tab,
                        created,
                    });
                });
            }
            PendingSessionAction::LoadMemories { query } => {
                let Some(opener) = opener.clone() else {
                    let _ = loop_tx.send(LoopMsg::ModalError(
                        "Host actions need a session host.".to_string(),
                    ));
                    changed = true;
                    continue;
                };
                let workspace = app.workspace_root.clone();
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let project = workspace.display().to_string();
                    let result = match query {
                        Some(query) => opener.search_memories(&project, &query).await,
                        None => opener.list_memories(&project).await,
                    };
                    let _ = tx.send(LoopMsg::MemoriesLoaded(result));
                });
                changed = true;
            }
            PendingSessionAction::LoadRoutes => {
                let Some(opener) = opener.clone() else {
                    let _ = loop_tx.send(LoopMsg::ModalError(
                        "Host actions need a session host.".to_string(),
                    ));
                    changed = true;
                    continue;
                };
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let _ = tx.send(LoopMsg::RoutesLoaded(opener.routes().await));
                });
                changed = true;
            }
            PendingSessionAction::LoadPacks => {
                let Some(opener) = opener.clone() else {
                    let _ = loop_tx.send(LoopMsg::ModalError(
                        "Host actions need a session host.".to_string(),
                    ));
                    changed = true;
                    continue;
                };
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let _ = tx.send(LoopMsg::PacksLoaded(opener.list_packs().await));
                });
                changed = true;
            }
            PendingSessionAction::LoadDefinitions => {
                let Some(opener) = opener.clone() else {
                    let _ = loop_tx.send(LoopMsg::ModalError(
                        "Host actions need a session host.".to_string(),
                    ));
                    changed = true;
                    continue;
                };
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let _ = tx.send(LoopMsg::DefinitionsLoaded(
                        opener.definitions_diagnostics().await,
                    ));
                });
                changed = true;
            }
            PendingSessionAction::MemoryAction { action, id } => {
                let Some(opener) = opener.clone() else {
                    let _ = loop_tx.send(LoopMsg::ModalError(
                        "Host actions need a session host.".to_string(),
                    ));
                    changed = true;
                    continue;
                };
                let workspace = app.workspace_root.clone();
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let project = workspace.display().to_string();
                    let result = match action {
                        crate::app::MemoryModalAction::Delete => {
                            opener.delete_memory(&project, &id).await.map(|_| ())
                        }
                        crate::app::MemoryModalAction::TogglePin => {
                            match opener.list_memories(&project).await {
                                Ok(memories) => {
                                    let pinned = memories
                                        .iter()
                                        .find(|memory| memory.id == id)
                                        .map(|memory| memory.pinned)
                                        .unwrap_or(false);
                                    opener
                                        .set_memory_pinned(&project, &id, !pinned)
                                        .await
                                        .map(|_| ())
                                }
                                Err(error) => Err(error),
                            }
                        }
                    };
                    match result {
                        Ok(()) => {
                            let _ = tx.send(LoopMsg::MemoriesLoaded(
                                opener.list_memories(&project).await,
                            ));
                        }
                        Err(error) => {
                            let _ = tx.send(LoopMsg::ModalError(format!("{error:#}")));
                        }
                    }
                });
                changed = true;
            }
            PendingSessionAction::RouteAction { task_type } => {
                let Some(opener) = opener.clone() else {
                    let _ = loop_tx.send(LoopMsg::ModalError(
                        "Host actions need a session host.".to_string(),
                    ));
                    changed = true;
                    continue;
                };
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    match opener.delete_route(&task_type).await {
                        Ok(_) => {
                            let _ = tx.send(LoopMsg::RoutesLoaded(opener.routes().await));
                        }
                        Err(error) => {
                            let _ = tx.send(LoopMsg::ModalError(format!("{error:#}")));
                        }
                    }
                });
                changed = true;
            }
            PendingSessionAction::PackAction { action, id } => {
                let Some(opener) = opener.clone() else {
                    let _ = loop_tx.send(LoopMsg::ModalError(
                        "Host actions need a session host.".to_string(),
                    ));
                    changed = true;
                    continue;
                };
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let result = match opener.list_packs().await {
                        Ok(packs) => {
                            let pack = packs.iter().find(|pack| pack.id == id);
                            match (action, pack) {
                                (crate::app::PackModalAction::ToggleEnabled, Some(pack)) => {
                                    opener.set_pack_enabled(&id, !pack.enabled).await
                                }
                                (crate::app::PackModalAction::ToggleTrusted, Some(pack)) => {
                                    opener.set_pack_trusted(&id, !pack.trusted).await
                                }
                                (crate::app::PackModalAction::Remove, _) => {
                                    opener.remove_pack(&id).await
                                }
                                (_, None) => Ok(false),
                            }
                        }
                        Err(error) => Err(error),
                    };
                    match result {
                        Ok(_) => {
                            let _ = tx.send(LoopMsg::PacksLoaded(opener.list_packs().await));
                        }
                        Err(error) => {
                            let _ = tx.send(LoopMsg::ModalError(format!("{error:#}")));
                        }
                    }
                });
                changed = true;
            }
            PendingSessionAction::HostAction { tab_id, action } => {
                let Some(opener) = app.session_opener.clone() else {
                    if let Some(tab) = app.tabs.iter_mut().find(|tab| tab.tab_id == tab_id) {
                        tab.transcript.push(TranscriptEntry::Error {
                            text: "Host actions need a session host.".to_string(),
                            expanded: false,
                        });
                    }
                    changed = true;
                    continue;
                };
                let workspace = app.workspace_root.clone();
                let cmd_tx = app
                    .tabs
                    .iter()
                    .find(|tab| tab.tab_id == tab_id)
                    .map(|tab| tab.cmd_tx.clone());
                let session_id = app
                    .tabs
                    .iter()
                    .find(|tab| tab.tab_id == tab_id)
                    .map(|tab| tab.session_id.clone());
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let result = execute_host_action(
                        HostActionContext {
                            opener: opener.as_ref(),
                            project_root: &workspace,
                            session_id: session_id.as_ref(),
                            cmd_tx: cmd_tx.as_ref(),
                        },
                        action,
                    )
                    .await;
                    let _ = tx.send(LoopMsg::HostActionDone { tab_id, result });
                });
                changed = true;
            }
            PendingSessionAction::LoadBlob { tab_id, key } => {
                let session_id = app
                    .tabs
                    .iter()
                    .find(|tab| tab.tab_id == tab_id)
                    .map(|tab| tab.session_id.clone());
                let opener = opener.clone();
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let result = match (opener, session_id) {
                        (Some(opener), Some(session_id)) => {
                            opener.read_session_blob(&session_id, &key).await
                        }
                        _ => Err(anyhow::anyhow!("no active session for the blob viewer")),
                    };
                    let _ = tx.send(LoopMsg::BlobLoaded { key, result });
                });
                changed = true;
            }
            PendingSessionAction::SyncContext => {
                if let (Some(opener), Some(reg)) = (opener.clone(), app.registry.clone()) {
                    let context = app.settings.context.clone();
                    tokio::spawn(async move {
                        let update = meerai_host::HostSettingsUpdate {
                            context,
                            ..Default::default()
                        };
                        if let Ok(outcome) = opener.update_settings(update).await {
                            reg.emit_reload(&outcome);
                        }
                    });
                } else {
                    app.pending_quiet_reload = true;
                }
            }
        }
    }
    changed
}

async fn resolve_and_open_query(
    opener: &Arc<dyn SessionOpener>,
    registry: Option<&Arc<crate::SessionRegistry>>,
    open_ids: &HashSet<SessionId>,
    query: &str,
) -> Result<SessionResolveOutcome> {
    let sessions = opener.list_sessions().await?;
    let trimmed = query.trim();
    if let Some(exact) = sessions.iter().find(|s| s.id.as_str() == trimmed) {
        if open_ids.contains(&exact.id) {
            return Ok(SessionResolveOutcome::AlreadyOpen(exact.id.clone()));
        }
        if let Some(detail) = registry.and_then(|r| r.session_detail(&exact.id)) {
            return Ok(SessionResolveOutcome::FromRegistry(Box::new(detail)));
        }
        return opener
            .open_session(&exact.id)
            .await
            .map(|opened| SessionResolveOutcome::Opened {
                opened: Box::new(opened),
                draft: None,
            });
    }

    let prefix_matches: Vec<&SessionSummary> = sessions
        .iter()
        .filter(|s| s.id.as_str().starts_with(trimmed))
        .collect();
    match prefix_matches.as_slice() {
        [single] => {
            if open_ids.contains(&single.id) {
                return Ok(SessionResolveOutcome::AlreadyOpen(single.id.clone()));
            }
            if let Some(detail) = registry.and_then(|r| r.session_detail(&single.id)) {
                return Ok(SessionResolveOutcome::FromRegistry(Box::new(detail)));
            }
            opener
                .open_session(&single.id)
                .await
                .map(|opened| SessionResolveOutcome::Opened {
                    opened: Box::new(opened),
                    draft: None,
                })
        }
        [] => bail!("no session matching '{trimmed}'"),
        many => {
            let ids = many
                .iter()
                .take(4)
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            bail!("ambiguous session prefix '{trimmed}' ({ids})")
        }
    }
}

fn handle_loop_msg(
    app: &mut AppState,
    msg: LoopMsg,
    loop_tx: &mpsc::UnboundedSender<LoopMsg>,
) -> bool {
    match msg {
        LoopMsg::Agent { tab_id, event } => {
            if matches!(event, AgentEvent::ApprovalRequested { .. }) {
                crate::terminal::notify_approval();
            }
            let touches = event_touches_workspace(&event);
            app.handle_tab_agent_event(tab_id, event);
            touches
        }
        LoopMsg::RegistryAgent { session_id, event } => {
            if matches!(event, AgentEvent::ApprovalRequested { .. }) {
                crate::terminal::notify_approval();
            }
            let touches = event_touches_workspace(&event);
            if let Some(idx) = app.tab_index_for_session(&session_id) {
                let tab_id = app.tabs[idx].tab_id;
                app.handle_tab_agent_event(tab_id, event);
            }
            touches
        }
        LoopMsg::RegistryEvent(ev) => handle_registry_event(app, *ev),
        LoopMsg::StreamClosed { tab_id } => {
            if let Some(idx) = app.tab_index_by_id(tab_id) {
                if app.tabs.len() == 1 {
                    app.quit();
                } else {
                    let tab = &mut app.tabs[idx];
                    tab.is_running = false;
                    tab.active_tool = None;
                    app.status_notice = Some(format!("Tab {} worker disconnected", idx + 1));
                }
            }
            false
        }
        LoopMsg::LoginDone { result } => {
            match result {
                Ok(outcome) => app.apply_login_succeeded(&outcome.provider_id, outcome.account),
                Err(err) => app.apply_login_failed(format!("{err:#}")),
            }
            true
        }
        LoopMsg::SessionsListed(result) => {
            app.apply_listed_sessions(result);
            false
        }
        LoopMsg::ChildSessionRead {
            tab_id,
            item_id,
            result,
        } => {
            app.apply_child_session_read(tab_id, &item_id, result);
            false
        }
        LoopMsg::UserShellDone {
            tab_id,
            command,
            result,
        } => {
            if app.user_shell_running.as_deref() == Some(command.as_str()) {
                app.user_shell_running = None;
            }
            if let Some(idx) = app.tab_index_by_id(tab_id) {
                let text = match result {
                    Ok(output) if output.trim().is_empty() => {
                        format!("$ {command}\n(no output)")
                    }
                    Ok(output) => output,
                    Err(error) => format!("$ {command}\nfailed: {error:#}"),
                };
                let updated_modal = match &mut app.modal {
                    Some(ActiveModal::Terminal {
                        command: active,
                        output,
                        running,
                        scroll,
                    }) if *active == command => {
                        *output = Some(text.clone());
                        *running = false;
                        *scroll = 0;
                        true
                    }
                    _ => false,
                };
                if !updated_modal {
                    app.tabs[idx]
                        .transcript
                        .push(TranscriptEntry::System { text });
                    app.tabs[idx].transcript_version =
                        app.tabs[idx].transcript_version.wrapping_add(1);
                }
                app.tabs[idx].auto_follow = true;
                true
            } else {
                false
            }
        }
        LoopMsg::SessionDeleted(result) => {
            match result {
                Ok(true) => {
                    app.status_notice = Some("Session deleted".to_string());
                }
                Ok(false) => {
                    app.status_notice = Some("Session was already gone".to_string());
                }
                Err(err) => {
                    app.status_notice = Some(format!("Delete failed: {err:#}"));
                }
            }
            app.pending_session_actions
                .push(PendingSessionAction::RefreshSessions);
            true
        }
        LoopMsg::SessionCreatedForDelete {
            session_id,
            created,
        } => {
            match created {
                Ok(opened) => {
                    let _ = mount_opened_session(app, opened, SessionOpenMode::NewTab, loop_tx);
                    if let Some(idx) = app.tab_index_for_session(&session_id) {
                        app.close_tab(idx);
                    }
                    app.pending_session_actions
                        .push(PendingSessionAction::DeleteSession { session_id });
                }
                Err(err) => {
                    let msg = format!("Could not create a new session: {err:#}");
                    app.status_notice = Some(truncate_notice(&msg));
                    app.pending_session_actions
                        .push(PendingSessionAction::RefreshSessions);
                }
            }
            true
        }
        LoopMsg::SessionCreatedForReplace {
            replaced_tab,
            created,
        } => {
            match created {
                Ok(opened) => {
                    let _ = mount_opened_session(app, opened, SessionOpenMode::NewTab, loop_tx);
                    if let Some(idx) = app.tab_index_by_id(replaced_tab) {
                        app.close_tab(idx);
                    }
                    app.status_notice = Some(format!("Session {}", app.tab().session_id));
                }
                Err(err) => {
                    let msg = format!("Could not create a new session: {err:#}");
                    app.status_notice = Some(truncate_notice(&msg));
                }
            }
            true
        }
        LoopMsg::SessionResolved { mode, result } => {
            match result {
                Ok(SessionResolveOutcome::AlreadyOpen(id)) => {
                    if let Some(idx) = app.tab_index_for_session(&id) {
                        app.switch_to_tab(idx);
                        app.status_notice = Some(format!("Switched to tab {}", idx + 1));
                    }
                }
                Ok(SessionResolveOutcome::Opened { opened, draft }) => {
                    if let Some(new_tab_id) = mount_opened_session(app, *opened, mode, loop_tx)
                        && let Some(draft) = draft
                        && let Some(idx) = app.tab_index_by_id(new_tab_id)
                    {
                        let tab = &mut app.tabs[idx];
                        tab.cursor = draft.len();
                        tab.input = draft;
                    }
                }
                Ok(SessionResolveOutcome::FromRegistry(detail)) => {
                    mount_registry_detail(app, *detail, mode);
                }
                Err(err) => {
                    let msg = format!("{err:#}");
                    app.status_notice = Some(truncate_notice(&msg));
                    app.tab_mut().transcript.push(TranscriptEntry::Error {
                        text: format!("Failed to open session: {msg}"),
                        expanded: false,
                    });
                }
            }
            false
        }
        LoopMsg::AcpResolved(result) => {
            match result {
                Ok(opened) => {
                    let _ = mount_opened_session(app, opened, SessionOpenMode::NewTab, loop_tx);
                }
                Err(err) => {
                    let text = format!("ACP session unavailable: {err:#}");
                    app.status_notice = Some(truncate_notice(&text));
                    app.tab_mut()
                        .transcript
                        .push(TranscriptEntry::System { text });
                }
            }
            false
        }
        LoopMsg::WebServerStarted(result) => {
            match result {
                Ok((handle, start)) => {
                    app.modal = None;
                    app.web_server = Some(RunningWebServer {
                        port: start.port,
                        url: start.url.clone(),
                        handle,
                    });
                    let msg = format!(
                        "Web UI listening at {}\nUsername: meerai\nPassword: {}\nStop with /serve stop.",
                        start.url, start.secret
                    );
                    app.status_notice = Some(format!("Web UI listening at {}", start.url));
                    let tab = app.tab_mut();
                    tab.auto_follow = true;
                    tab.transcript.push(TranscriptEntry::System { text: msg });
                    tab.transcript_version = tab.transcript_version.wrapping_add(1);
                }
                Err(err) => {
                    let msg = format!("{err:#}");
                    app.status_notice = Some(truncate_notice(&msg));
                    if let Some(ActiveModal::ServeDialog {
                        port_input,
                        editing_secret,
                        ..
                    }) = app.modal.take()
                    {
                        app.modal = Some(ActiveModal::ServeDialog {
                            port_input,
                            secret_input: String::new(),
                            editing_secret,
                            error: Some(msg),
                            busy: false,
                        });
                    } else {
                        app.tab_mut().transcript.push(TranscriptEntry::Error {
                            text: format!("Failed to start web UI: {msg}"),
                            expanded: false,
                        });
                    }
                }
            }
            false
        }
        LoopMsg::MemoriesLoaded(result) => {
            if let Some(ActiveModal::Memory {
                query, selected, ..
            }) = app.modal.take()
            {
                match result {
                    Ok(entries) => {
                        let selected = selected.min(entries.len().saturating_sub(1));
                        app.modal = Some(ActiveModal::Memory {
                            entries,
                            query,
                            selected,
                            loading: false,
                            error: None,
                        });
                    }
                    Err(error) => {
                        app.modal = Some(ActiveModal::Memory {
                            entries: Vec::new(),
                            query,
                            selected: 0,
                            loading: false,
                            error: Some(format!("{error:#}")),
                        });
                    }
                }
            }
            true
        }
        LoopMsg::RoutesLoaded(result) => {
            if let Some(ActiveModal::Routing { selected, .. }) = app.modal.take() {
                match result {
                    Ok(settings) => {
                        let selected = selected.min(settings.routes.len().saturating_sub(1));
                        app.modal = Some(ActiveModal::Routing {
                            routes: settings.routes,
                            selected,
                            loading: false,
                            error: None,
                        });
                    }
                    Err(error) => {
                        app.modal = Some(ActiveModal::Routing {
                            routes: Vec::new(),
                            selected: 0,
                            loading: false,
                            error: Some(format!("{error:#}")),
                        });
                    }
                }
            }
            true
        }
        LoopMsg::PacksLoaded(result) => {
            if let Some(ActiveModal::Packs {
                definitions,
                tab,
                selected,
                ..
            }) = app.modal.take()
            {
                match result {
                    Ok(packs) => {
                        let selected = selected.min(packs.len().saturating_sub(1));
                        app.modal = Some(ActiveModal::Packs {
                            packs,
                            definitions,
                            tab,
                            selected,
                            loading: false,
                            error: None,
                        });
                    }
                    Err(error) => {
                        app.modal = Some(ActiveModal::Packs {
                            packs: Vec::new(),
                            definitions,
                            tab,
                            selected: 0,
                            loading: false,
                            error: Some(format!("{error:#}")),
                        });
                    }
                }
            }
            true
        }
        LoopMsg::DefinitionsLoaded(result) => {
            if let Some(ActiveModal::Packs {
                packs,
                tab,
                selected,
                ..
            }) = app.modal.take()
            {
                match result {
                    Ok(definitions) => {
                        app.modal = Some(ActiveModal::Packs {
                            packs,
                            definitions,
                            tab,
                            selected,
                            loading: false,
                            error: None,
                        });
                    }
                    Err(error) => {
                        app.modal = Some(ActiveModal::Packs {
                            packs,
                            definitions: meerai_host::HostDefinitionsDiagnostics::default(),
                            tab,
                            selected,
                            loading: false,
                            error: Some(format!("{error:#}")),
                        });
                    }
                }
            }
            true
        }
        LoopMsg::ModalError(message) => {
            if let Some(modal) = app.modal.as_mut() {
                match modal {
                    ActiveModal::Memory { error, loading, .. } => {
                        *error = Some(message);
                        *loading = false;
                    }
                    ActiveModal::Routing { error, loading, .. } => {
                        *error = Some(message);
                        *loading = false;
                    }
                    ActiveModal::Packs { error, loading, .. } => {
                        *error = Some(message);
                        *loading = false;
                    }
                    _ => {}
                }
            }
            true
        }
        LoopMsg::HostActionDone { tab_id, result } => {
            if let Some(tab) = app.tabs.iter_mut().find(|tab| tab.tab_id == tab_id) {
                tab.auto_follow = true;
                match result {
                    Ok(text) => tab.transcript.push(TranscriptEntry::System { text }),
                    Err(error) => tab.transcript.push(TranscriptEntry::Error {
                        text: format!("{error:#}"),
                        expanded: false,
                    }),
                }
            }
            true
        }
        LoopMsg::BlobLoaded { key, result } => {
            if let Some(ActiveModal::Context(panel)) = app.modal.as_mut() {
                match result {
                    Ok(content) => {
                        panel.blob_key = Some(key);
                        panel.blob_content = Some(content);
                        panel.blob_scroll = 0;
                        panel.error = None;
                    }
                    Err(error) => panel.error = Some(format!("{error:#}")),
                }
            }
            true
        }
        LoopMsg::WebServerStopped => {
            app.web_server_hit = None;
            app.status_notice = Some("Stopped local web server".to_string());
            let tab = app.tab_mut();
            tab.auto_follow = true;
            tab.transcript.push(TranscriptEntry::System {
                text: "Stopped local web server.".to_string(),
            });
            tab.transcript_version = tab.transcript_version.wrapping_add(1);
            false
        }
    }
}

fn mount_opened_session(
    app: &mut AppState,
    opened: OpenedSession,
    mode: SessionOpenMode,
    loop_tx: &mpsc::UnboundedSender<LoopMsg>,
) -> Option<u64> {
    if let Some(reg) = app.registry.clone() {
        let (_keep_tx, dummy_rx) = mpsc::unbounded_channel();
        let ui_opened = OpenedSession {
            session_id: opened.session_id.clone(),
            title: opened.title.clone(),
            backend: opened.backend.clone(),
            provider: opened.provider.clone(),
            model: opened.model.clone(),
            approval_mode: opened.approval_mode,
            events: opened.events.clone(),
            cmd_tx: opened.cmd_tx.clone(),
            event_rx: dummy_rx,
        };
        reg.register_opened(opened);
        app.apply_opened_session(ui_opened, mode)
            .map(|(tab_id, _)| tab_id)
    } else if let Some((tab_id, event_rx)) = app.apply_opened_session(opened, mode) {
        spawn_tab_forwarder(tab_id, event_rx, loop_tx.clone());
        Some(tab_id)
    } else {
        None
    }
}

fn mount_registry_detail(app: &mut AppState, detail: LiveSessionDetail, mode: SessionOpenMode) {
    let Some(reg) = app.registry.as_ref() else {
        return;
    };
    let Some(cmd_tx) = reg.cmd_tx(&detail.summary.session_id) else {
        return;
    };
    let (_keep_tx, dummy_rx) = mpsc::unbounded_channel();
    let ui_opened = OpenedSession {
        session_id: detail.summary.session_id.clone(),
        title: detail.summary.title.clone(),
        backend: detail.summary.backend.clone(),
        provider: detail.summary.provider.clone(),
        model: detail.summary.model.clone(),
        approval_mode: detail.summary.approval_mode,
        events: Vec::new(),
        cmd_tx,
        event_rx: dummy_rx,
    };
    if let Some((tab_id, _)) = app.apply_opened_session(ui_opened, mode)
        && let Some(idx) = app.tab_index_by_id(tab_id)
    {
        let tab = &mut app.tabs[idx];
        tab.transcript = detail.entries.into_iter().map(live_to_transcript).collect();
        tab.is_running = detail.summary.status != meerai_host::SessionStatus::Idle;
        tab.active_tool = detail.summary.active_tool;
        tab.pending_approvals = detail.pending_approvals;
        tab.pending_questions = detail.pending_questions;
        tab.transcript_version = tab.transcript_version.wrapping_add(1);
    }
}

fn live_to_transcript(entry: LiveEntry) -> TranscriptEntry {
    match entry {
        LiveEntry::User { text, .. } => TranscriptEntry::User { text },
        LiveEntry::Assistant {
            text, reasoning, ..
        } => TranscriptEntry::Assistant {
            text,
            reasoning,
            streaming: false,
            reasoning_expanded: false,
        },
        LiveEntry::System { text, .. } => TranscriptEntry::System { text },
        LiveEntry::Error { message, .. } => TranscriptEntry::Error {
            text: message,
            expanded: false,
        },
        LiveEntry::ToolCall {
            id,
            name,
            args,
            status,
            output,
            ..
        } => TranscriptEntry::ToolCall(ToolCallEntry {
            call_id: id,
            name,
            args,
            status: map_status_str(&status),
            output,
            expanded: false,
        }),
        LiveEntry::Subagent {
            id,
            agent,
            status,
            report,
            child_session_id,
            ..
        } => TranscriptEntry::Subagent(crate::app::SubagentEntry {
            id,
            agent,
            status: map_status_str(&status),
            report,
            child_session_id,
            model: None,
            approval_mode: "ask".to_string(),
            steps_used: 0,
            max_steps: 0,
            usage: Default::default(),
            est_cost_usd: None,
            active_tool: None,
            expanded: false,
            nested: None,
            nested_refreshed_at: None,
            nested_loading: false,
        }),
        LiveEntry::Question {
            id,
            questions,
            answers,
            resolved,
        } => TranscriptEntry::Question {
            request_id: id,
            questions,
            answers,
            resolved,
        },
    }
}

fn map_status_str(status: &str) -> ToolCallStatus {
    match status {
        "proposed" => ToolCallStatus::Proposed,
        "running" => ToolCallStatus::Running,
        "succeeded" => ToolCallStatus::Succeeded,
        "stopped" => ToolCallStatus::Stopped,
        "cancelled" => ToolCallStatus::Cancelled,
        _ => ToolCallStatus::Failed,
    }
}

fn handle_registry_event(app: &mut AppState, ev: RegistryEvent) -> bool {
    match ev.kind {
        RegistryEventKind::UserPromptSubmitted {
            session_id, text, ..
        } => {
            if let Some(pos) = app
                .tui_prompt_echoes
                .iter()
                .position(|(sid, t)| sid == &session_id && t == &text)
            {
                app.tui_prompt_echoes.remove(pos);
                return false;
            }
            if let Some(idx) = app.tab_index_for_session(&session_id) {
                let tab = &mut app.tabs[idx];
                if tab.title.is_none() {
                    tab.title = Some(text.lines().next().unwrap_or("").trim().to_string());
                }
                tab.transcript.push(TranscriptEntry::User { text });
                tab.is_running = true;
                tab.auto_follow = true;
                tab.transcript_version = tab.transcript_version.wrapping_add(1);
            }
            false
        }
        RegistryEventKind::SessionUpdated { session } => {
            if let Some(idx) = app.tab_index_for_session(&session.session_id) {
                let tab = &mut app.tabs[idx];
                tab.title = session.title;
                tab.active_provider = session.provider;
                tab.model = session.model;
                tab.approval_mode = session.approval_mode;
                tab.active_tool = session.active_tool;
            }
            false
        }
        RegistryEventKind::FileChangesUpdated { changes } => {
            if app.file_changes != changes {
                app.file_changes = changes;
            }
            false
        }
        RegistryEventKind::SessionClosed { session_id, .. } => {
            if app.tabs.len() > 1
                && let Some(idx) = app.tab_index_for_session(&session_id)
            {
                app.close_tab(idx);
            }
            false
        }
        RegistryEventKind::SessionReset { session_id } => {
            if let Some(idx) = app.tab_index_for_session(&session_id) {
                let tab = &mut app.tabs[idx];
                if !tab.transcript.is_empty() {
                    tab.transcript.clear();
                    tab.pending_assistant_idx = None;
                    tab.assistant_items.clear();
                    tab.tool_items.clear();
                    tab.subagent_items.clear();
                    tab.turn_activity.clear();
                    tab.turn_id = None;
                    tab.is_running = false;
                    tab.transcript_version = tab.transcript_version.wrapping_add(1);
                }
            }
            false
        }
        RegistryEventKind::SessionRewound {
            session_id,
            entries: Some(entries),
            error: None,
            ..
        } => {
            if let Some(idx) = app.tab_index_for_session(&session_id) {
                let tab = &mut app.tabs[idx];
                tab.transcript = entries.into_iter().map(live_to_transcript).collect();
                tab.is_running = false;
                tab.transcript_version = tab.transcript_version.wrapping_add(1);
            }
            true
        }
        RegistryEventKind::Reloaded { host_info, .. } => {
            app.context_temperature = host_info
                .settings
                .context
                .temperature
                .and_then(meerai_common::settings::Temperature::effective);
            app.context_effort = host_info.settings.context.effort;
            app.explicit_context_window = host_info.settings.context.context_window_tokens;
            if let Some(dir) = app.config_dir.as_deref()
                && let Ok(user) = meerai_common::settings::load_settings_from(dir)
            {
                app.settings = user;
            }
            true
        }
        _ => false,
    }
}

fn truncate_notice(msg: &str) -> String {
    const MAX: usize = 60;
    if msg.chars().count() <= MAX {
        msg.to_string()
    } else {
        let mut out: String = msg.chars().take(MAX.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn event_touches_workspace(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::TurnCompleted { .. }
            | AgentEvent::Error { .. }
            | AgentEvent::ItemCompleted {
                item: SessionItem::ToolCall { .. }
                    | SessionItem::CommandRun { .. }
                    | SessionItem::FileChange { .. }
                    | SessionItem::SubagentRun { .. }
            }
    )
}

fn spawn_git_status_refresh(
    workspace_root: PathBuf,
    tx: mpsc::UnboundedSender<Vec<FileChangeEntry>>,
    in_flight: &mut bool,
) {
    if *in_flight {
        return;
    }
    *in_flight = true;
    tokio::spawn(async move {
        let changes = tokio::task::spawn_blocking(move || list_git_changes(&workspace_root))
            .await
            .unwrap_or_default();
        let _ = tx.send(changes);
    });
}
