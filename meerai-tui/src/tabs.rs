use std::path::{Path, PathBuf};

use meerai_common::settings::default_data_dir;
use meerai_session::SessionId;
use serde::{Deserialize, Serialize};

const TABS_FILE: &str = "open-tabs.json";

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SavedTabs {
    #[serde(default)]
    pub sessions: Vec<String>,
    #[serde(default)]
    pub active: usize,
    #[serde(default)]
    pub workspace: Option<String>,
}

fn tabs_path() -> Option<PathBuf> {
    default_data_dir().map(|dir| dir.join(TABS_FILE))
}

pub fn load() -> SavedTabs {
    let Some(path) = tabs_path() else {
        return SavedTabs::default();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return SavedTabs::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn saved_sessions_for(workspace: &Path) -> Vec<SessionId> {
    let saved = load();
    if saved.workspace.as_deref() != Some(workspace.to_string_lossy().as_ref()) {
        return Vec::new();
    }
    saved
        .sessions
        .iter()
        .filter_map(|raw| SessionId::parse(raw).ok())
        .collect()
}

pub fn save(tabs: &[SessionId], active: usize, workspace: &Path) {
    let Some(path) = tabs_path() else {
        return;
    };
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }
    let saved = SavedTabs {
        sessions: tabs.iter().map(|id| id.to_string()).collect(),
        active,
        workspace: Some(workspace.to_string_lossy().to_string()),
    };
    if let Ok(raw) = serde_json::to_string(&saved) {
        let _ = std::fs::write(path, raw);
    }
}
