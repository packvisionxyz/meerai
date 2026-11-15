use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use crossterm::event::EventStream;
use futures::StreamExt;
use meerai_agents::{AgentEvent, SessionItem};
use meerai_session::{SessionId, SessionSummary};
use tokio::sync::mpsc;
use tokio::time::{self, MissedTickBehavior};

use crate::app::{AppState, FileChangeEntry, PendingSessionAction, TranscriptEntry};
use crate::diff::list_git_changes;
use crate::terminal::TerminalGuard;
use crate::{OpenedSession, SessionOpener, ui};

const TICK_INTERVAL: Duration = Duration::from_millis(80);
const GIT_POLL_TICKS: usize = 40;

enum SessionResolveOutcome {
    AlreadyOpen(SessionId),
    Opened(OpenedSession),
}

enum LoopMsg {
    Agent {
        tab_id: u64,
        event: AgentEvent,
    },
    StreamClosed {
        tab_id: u64,
    },
    SessionsListed(Result<Vec<SessionSummary>>),
    SessionResolved {
        replace_empty_tab: bool,
        result: Result<SessionResolveOutcome>,
    },
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
    spawn_tab_forwarder(app.tab().tab_id, event_rx, loop_tx.clone());

    let (git_tx, mut git_rx) = mpsc::unbounded_channel::<Vec<FileChangeEntry>>();
    let mut git_in_flight = false;
    let mut ticks_since_git = 0usize;

    spawn_git_status_refresh(
        app.workspace_root.clone(),
        git_tx.clone(),
        &mut git_in_flight,
    );

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
            }
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
    let Some(opener) = app.session_opener.clone() else {
        return false;
    };

    let mut changed = false;
    for action in actions {
        match action {
            PendingSessionAction::RefreshSessions => {
                let opener = opener.clone();
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
                let replace_empty_tab = app.should_replace_active_tab();
                let opener = opener.clone();
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let result = opener
                        .open_session(&id)
                        .await
                        .map(SessionResolveOutcome::Opened);
                    let _ = tx.send(LoopMsg::SessionResolved {
                        replace_empty_tab,
                        result,
                    });
                });
            }
            PendingSessionAction::OpenSessionQuery(query) => {
                let replace_empty_tab = app.should_replace_active_tab();
                let open_ids: HashSet<SessionId> =
                    app.tabs.iter().map(|tab| tab.session_id.clone()).collect();
                let opener = opener.clone();
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let result = resolve_and_open_query(&opener, &open_ids, &query).await;
                    let _ = tx.send(LoopMsg::SessionResolved {
                        replace_empty_tab,
                        result,
                    });
                });
            }
            PendingSessionAction::CreateNewTab => {
                let opener = opener.clone();
                let tx = loop_tx.clone();
                tokio::spawn(async move {
                    let result = opener
                        .create_session()
                        .await
                        .map(SessionResolveOutcome::Opened);
                    let _ = tx.send(LoopMsg::SessionResolved {
                        replace_empty_tab: false,
                        result,
                    });
                });
            }
        }
    }
    changed
}

async fn resolve_and_open_query(
    opener: &Arc<dyn SessionOpener>,
    open_ids: &HashSet<SessionId>,
    query: &str,
) -> Result<SessionResolveOutcome> {
    let sessions = opener.list_sessions().await?;
    let trimmed = query.trim();
    if let Some(exact) = sessions.iter().find(|s| s.id.as_str() == trimmed) {
        if open_ids.contains(&exact.id) {
            return Ok(SessionResolveOutcome::AlreadyOpen(exact.id.clone()));
        }
        return opener
            .open_session(&exact.id)
            .await
            .map(SessionResolveOutcome::Opened);
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
            opener
                .open_session(&single.id)
                .await
                .map(SessionResolveOutcome::Opened)
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
            let touches = event_touches_workspace(&event);
            app.handle_tab_agent_event(tab_id, event);
            touches
        }
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
        LoopMsg::SessionsListed(result) => {
            app.apply_listed_sessions(result);
            false
        }
        LoopMsg::SessionResolved {
            replace_empty_tab,
            result,
        } => {
            match result {
                Ok(SessionResolveOutcome::AlreadyOpen(id)) => {
                    if let Some(idx) = app.tab_index_for_session(&id) {
                        app.switch_to_tab(idx);
                        app.status_notice = Some(format!("Switched to tab {}", idx + 1));
                    }
                }
                Ok(SessionResolveOutcome::Opened(opened)) => {
                    if let Some((new_tab_id, event_rx)) =
                        app.apply_opened_session(opened, replace_empty_tab)
                    {
                        spawn_tab_forwarder(new_tab_id, event_rx, loop_tx.clone());
                    }
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
