use std::path::{Path, PathBuf};

use meerai_common::settings::default_data_dir;
use meerai_session::SessionId;
use serde::{Deserialize, Serialize};

const TABS_FILE: &str = "open-tabs.json";

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

fn tabs_path() -> Option<PathBuf> {
    default_data_dir().map(|dir| dir.join(TABS_FILE))
}

fn load_from(path: &Path) -> SavedTabs {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return SavedTabs::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn restored_tabs_for(workspace: &Path) -> RestoredTabs {
    tabs_path()
        .map(|path| restored_from(&path, workspace))
        .unwrap_or_default()
}

fn restored_from(path: &Path, workspace: &Path) -> RestoredTabs {
    let saved = load_from(path);
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
    tabs: &[SessionId],
    active: usize,
    workspace: &Path,
    empty_session: Option<&SessionId>,
) {
    let Some(path) = tabs_path() else {
        return;
    };
    save_to(&path, tabs, active, workspace, empty_session);
}

fn save_to(
    path: &Path,
    tabs: &[SessionId],
    active: usize,
    workspace: &Path,
    empty_session: Option<&SessionId>,
) {
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }
    let saved = SavedTabs {
        sessions: tabs.iter().map(|id| id.to_string()).collect(),
        active,
        workspace: Some(workspace.to_string_lossy().to_string()),
        empty_session: empty_session.map(|id| id.to_string()),
    };
    if let Ok(raw) = serde_json::to_string(&saved) {
        let _ = std::fs::write(path, raw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_tabs_file() -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("meerai-tabs-{}", SessionId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        (dir.join(TABS_FILE), dir)
    }

    #[test]
    fn saves_and_restores_the_workspace_tabs_and_empty_session() {
        let (path, dir) = temp_tabs_file();
        let workspace = PathBuf::from("/tmp/workspace");
        let kept = SessionId::new();
        let empty = SessionId::new();

        save_to(
            &path,
            &[kept.clone(), empty.clone()],
            1,
            &workspace,
            Some(&empty),
        );

        let restored = restored_from(&path, &workspace);
        assert_eq!(restored.sessions, vec![kept, empty.clone()]);
        assert_eq!(restored.empty_session, Some(empty));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn ignores_tabs_saved_for_another_workspace() {
        let (path, dir) = temp_tabs_file();
        let session = SessionId::new();
        save_to(
            &path,
            std::slice::from_ref(&session),
            0,
            Path::new("/tmp/workspace-a"),
            Some(&session),
        );

        assert_eq!(
            restored_from(&path, Path::new("/tmp/workspace-b")),
            RestoredTabs::default()
        );
        std::fs::remove_dir_all(dir).ok();
    }
}
