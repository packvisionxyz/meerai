use std::path::Path;

use anyhow::{Result, bail};
use meerai_common::settings::AppSettings;
use meerai_core::env::now_millis;
use meerai_session::{
    Session, SessionConfig, SessionId, SessionSummary, TurnId, default_sessions_root,
    delete_session, latest_session, list_sessions, session_has_user_input,
};

use crate::cli::{SessionTarget, SessionsCommand};

pub(crate) const EMPTY_SESSION_MAX_AGE_MS: u64 = 6 * 60 * 60 * 1_000;

pub(crate) fn session_label(summary: &SessionSummary) -> &str {
    summary
        .title
        .as_deref()
        .filter(|title| !title.is_empty())
        .or_else(|| summary.model.as_deref().filter(|model| !model.is_empty()))
        .unwrap_or("")
}

pub(crate) fn format_session_row(summary: &SessionSummary) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        summary.id,
        summary.updated_at_ms,
        summary.bytes,
        session_label(summary)
    )
}

pub(crate) fn select_prunable(
    sessions: &[SessionSummary],
    now_ms: u64,
    days: u64,
    keep: usize,
    exclude: &[SessionId],
) -> Vec<SessionId> {
    let cutoff = now_ms.saturating_sub(days.saturating_mul(86_400_000));
    let mut ordered: Vec<&SessionSummary> = sessions.iter().collect();
    ordered.sort_by_key(|summary| std::cmp::Reverse(summary.updated_at_ms));
    ordered
        .into_iter()
        .skip(keep)
        .filter(|summary| !exclude.contains(&summary.id))
        .filter(|summary| summary.updated_at_ms < cutoff)
        .map(|summary| summary.id.clone())
        .collect()
}

pub(crate) async fn run_blocking<T>(task: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|err| anyhow::anyhow!("session task failed: {err}"))?
}

pub(crate) fn auto_prune_config(settings: &AppSettings) -> Option<(u64, usize)> {
    let sessions = settings.sessions.as_ref()?;
    let days = sessions.auto_prune_days?;
    Some((days, sessions.keep.unwrap_or(0)))
}

pub(crate) async fn prune_old_sessions(
    root: std::path::PathBuf,
    days: u64,
    keep: usize,
    exclude: &[SessionId],
) -> Result<usize> {
    let list_root = root.clone();
    let sessions = run_blocking(move || list_sessions(&list_root)).await?;
    let ids = select_prunable(&sessions, now_millis(), days, keep, exclude);
    let count = ids.len();
    if count == 0 {
        return Ok(0);
    }
    run_blocking(move || {
        for id in &ids {
            delete_session(&root, id)?;
        }
        Ok(())
    })
    .await?;
    Ok(count)
}

pub(crate) fn format_prune_message(count: usize, days: u64) -> String {
    format!("Pruned {count} sessions (older than {days} days)")
}

pub(crate) async fn auto_prune_sessions(
    settings: &AppSettings,
    active: &SessionId,
    open: &[SessionId],
) {
    let Some((days, keep)) = auto_prune_config(settings) else {
        return;
    };
    let mut exclude = Vec::with_capacity(open.len() + 1);
    exclude.push(active.clone());
    exclude.extend_from_slice(open);
    match prune_old_sessions(default_sessions_root(), days, keep, &exclude).await {
        Ok(0) => {}
        Ok(count) => eprintln!("{}", format_prune_message(count, days)),
        Err(err) => eprintln!("Warning: automatic session prune failed: {err:#}"),
    }
}

pub(crate) async fn prune_empty_sessions(active: &SessionId, open: &[SessionId]) {
    let mut exclude = Vec::with_capacity(open.len() + 1);
    exclude.push(active.clone());
    exclude.extend_from_slice(open);
    match remove_empty_sessions(&default_sessions_root(), EMPTY_SESSION_MAX_AGE_MS, &exclude).await
    {
        Ok(0) => {}
        Ok(count) => eprintln!("Removed {count} empty sessions"),
        Err(err) => eprintln!("Warning: empty session cleanup failed: {err:#}"),
    }
}

pub(crate) async fn empty_session_ids(root: &Path, candidates: &[SessionId]) -> Vec<SessionId> {
    if candidates.is_empty() {
        return Vec::new();
    }
    let list_root = root.to_path_buf();
    let Ok(sessions) = run_blocking(move || list_sessions(&list_root)).await else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for id in candidates {
        let Some(summary) = sessions.iter().find(|summary| &summary.id == id) else {
            continue;
        };
        if summary.parent_session_id.is_some() || has_title(summary) {
            continue;
        }
        if matches!(session_has_user_input(&summary.dir).await, Ok(false)) {
            ids.push(id.clone());
        }
    }
    ids
}

pub(crate) async fn remove_empty_sessions(
    root: &Path,
    max_age_ms: u64,
    exclude: &[SessionId],
) -> Result<usize> {
    let now = now_millis();
    let sessions = list_sessions(root)?;
    let mut removed = 0usize;
    for summary in sessions {
        if exclude.iter().any(|id| id == &summary.id) || summary.parent_session_id.is_some() {
            continue;
        }
        if has_title(&summary) {
            continue;
        }
        if now.saturating_sub(summary.updated_at_ms) < max_age_ms {
            continue;
        }
        match session_has_user_input(&summary.dir).await {
            Ok(true) => continue,
            Ok(false) => {}
            Err(_) => continue,
        }
        if delete_session(root, &summary.id)? {
            removed += 1;
        }
    }
    Ok(removed)
}

fn has_title(summary: &SessionSummary) -> bool {
    summary
        .title
        .as_deref()
        .is_some_and(|title| !title.trim().is_empty())
}

pub(crate) async fn run_sessions(command: &SessionsCommand) -> Result<()> {
    let root = default_sessions_root();
    match command {
        SessionsCommand::List => {
            let list_root = root.clone();
            let sessions = run_blocking(move || list_sessions(&list_root)).await?;
            println!("ID\tUPDATED_MS\tBYTES\tTITLE_OR_MODEL");
            for summary in sessions {
                println!("{}", format_session_row(&summary));
            }
        }
        SessionsCommand::Remove {
            target: SessionTarget::Id(raw),
        } => {
            let id = SessionId::parse(raw)?;
            let delete_root = root.clone();
            let delete_id = id.clone();
            let removed = run_blocking(move || delete_session(&delete_root, &delete_id)).await?;
            if !removed {
                bail!("session not found: {id}");
            }
            println!("removed {id}");
        }
        SessionsCommand::Remove {
            target: SessionTarget::All,
        } => {
            let list_root = root.clone();
            let sessions = run_blocking(move || list_sessions(&list_root)).await?;
            let count = sessions.len();
            let ids: Vec<SessionId> = sessions.into_iter().map(|summary| summary.id).collect();
            let delete_root = root.clone();
            run_blocking(move || {
                for id in &ids {
                    delete_session(&delete_root, id)?;
                }
                Ok(())
            })
            .await?;
            println!("removed {count} sessions");
        }
        SessionsCommand::Prune { days, keep } => {
            let list_root = root.clone();
            let sessions = run_blocking(move || list_sessions(&list_root)).await?;
            let ids = select_prunable(&sessions, now_millis(), *days, *keep, &[]);
            let count = ids.len();
            let delete_root = root.clone();
            run_blocking(move || {
                for id in &ids {
                    delete_session(&delete_root, id)?;
                }
                Ok(())
            })
            .await?;
            println!("removed {count} sessions");
        }
        SessionsCommand::Fork { id, turn } => {
            let workspace = std::env::current_dir()?;
            let forked = fork_session(&root, &workspace, id, turn.as_deref()).await?;
            println!("forked {} from {id}", forked.id());
        }
    }
    Ok(())
}

pub(crate) async fn fork_session(
    root: &Path,
    workspace: &Path,
    id_raw: &str,
    turn_raw: Option<&str>,
) -> Result<Session> {
    let source_id = SessionId::parse(id_raw)?;
    let config = SessionConfig {
        root: root.to_path_buf(),
        workspace: workspace.to_path_buf(),
    };
    let boundary = turn_raw.map(TurnId::from_raw);
    Session::fork_stored(&config, &source_id, boundary.as_ref()).await
}

pub(crate) async fn continue_session_id(root: &Path) -> Result<SessionId> {
    let root = root.to_path_buf();
    run_blocking(move || latest_session(&root))
        .await?
        .map(|summary| summary.id)
        .ok_or_else(|| anyhow::anyhow!("no sessions found to continue"))
}
