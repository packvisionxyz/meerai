use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::EventStream;
use futures::StreamExt;
use meerai_agents::{AgentEvent, SessionItem};
use tokio::sync::mpsc;
use tokio::time::{self, MissedTickBehavior};

use crate::app::{AppState, FileChangeEntry};
use crate::diff::list_git_changes;
use crate::terminal::TerminalGuard;
use crate::ui;

const TICK_INTERVAL: Duration = Duration::from_millis(80);
const GIT_POLL_TICKS: usize = 40;

pub async fn run_event_loop(
    terminal: &mut TerminalGuard,
    app: &mut AppState,
    mut event_rx: mpsc::UnboundedReceiver<AgentEvent>,
) -> Result<()> {
    let mut term_events = EventStream::new();
    let mut ticker = time::interval(TICK_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

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
                    }
                    Some(Err(err)) => return Err(err.into()),
                    None => app.quit(),
                }
            }
            maybe_agent = event_rx.recv() => {
                match maybe_agent {
                    Some(ev) => {
                        let mut should_refresh_git = event_touches_workspace(&ev);
                        app.handle_agent_event(ev);
                        while let Ok(next_ev) = event_rx.try_recv() {
                            if event_touches_workspace(&next_ev) {
                                should_refresh_git = true;
                            }
                            app.handle_agent_event(next_ev);
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
                let was_running = app.is_running;
                let had_notice = app.status_notice.is_some();
                app.on_tick();
                if was_running || (had_notice && app.status_notice.is_none()) {
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

fn event_touches_workspace(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::TurnCompleted { .. }
            | AgentEvent::Error { .. }
            | AgentEvent::ItemCompleted {
                item: SessionItem::ToolCall { .. }
                    | SessionItem::CommandRun { .. }
                    | SessionItem::FileChange { .. }
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
