use std::path::Path;

use meerai_session::SessionId;
use meerai_store::Store;
use serde::{Deserialize, Serialize};

const TABS_KEY: &str = "ui_tabs";

#[derive(Debug, Default, Serialize, Deserialize)]
struct SavedTabs {
    #[serde(default)]
    sessions: Vec<String>,
    #[serde(default)]
    active: usize,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    empty_session: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RestoredTabs {
    pub sessions: Vec<SessionId>,
    pub empty_session: Option<SessionId>,
}

pub fn restored_tabs_for(store: &Store, workspace: &Path) -> RestoredTabs {
    let saved = store
        .config()
        .get::<SavedTabs>(TABS_KEY)
        .ok()
        .flatten()
        .unwrap_or_default();
    restored_from(saved, workspace)
}

fn restored_from(saved: SavedTabs, workspace: &Path) -> RestoredTabs {
    if saved.workspace.as_deref() != Some(workspace.to_string_lossy().as_ref()) {
        return RestoredTabs::default();
    }
    RestoredTabs {
        sessions: saved
            .sessions
            .iter()
            .filter_map(|raw| SessionId::parse(raw).ok())
            .collect(),
        empty_session: saved
            .empty_session
            .as_deref()
            .and_then(|raw| SessionId::parse(raw).ok()),
    }
}

pub fn save(
    store: &Store,
    tabs: &[SessionId],
    active: usize,
    workspace: &Path,
    empty_session: Option<&SessionId>,
) {
    let saved = SavedTabs {
        sessions: tabs.iter().map(|id| id.to_string()).collect(),
        active,
        workspace: Some(workspace.to_string_lossy().to_string()),
        empty_session: empty_session.map(|id| id.to_string()),
    };
    let _ = store.config().set(TABS_KEY, &saved);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open(":memory:").unwrap()
    }

    #[test]
    fn saves_and_restores_the_workspace_tabs_and_empty_session() {
        let store = store();
        let workspace = Path::new("/tmp/workspace");
        let kept = SessionId::new();
        let empty = SessionId::new();

        save(
            &store,
            &[kept.clone(), empty.clone()],
            1,
            workspace,
            Some(&empty),
        );

        let restored = restored_tabs_for(&store, workspace);
        assert_eq!(restored.sessions, vec![kept, empty.clone()]);
        assert_eq!(restored.empty_session, Some(empty));
    }

    #[test]
    fn ignores_tabs_saved_for_another_workspace() {
        let store = store();
        let session = SessionId::new();
        save(
            &store,
            std::slice::from_ref(&session),
            0,
            Path::new("/tmp/workspace-a"),
            Some(&session),
        );

        assert_eq!(
            restored_tabs_for(&store, Path::new("/tmp/workspace-b")),
            RestoredTabs::default()
        );
    }
}
