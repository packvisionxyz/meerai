use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::ids::TurnId;

const MANIFEST_FILE: &str = "manifest.json";
const MAX_SNAPSHOT_BYTES: u64 = 2 * 1024 * 1024;
const KEEP_TURNS: usize = 100;

#[derive(Clone, Debug)]
pub struct SnapshotStore {
    root: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RestoreReport {
    pub restored: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
    pub skipped: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct SnapshotManifest {
    #[serde(default)]
    entries: BTreeMap<String, SnapshotEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SnapshotEntry {
    blob: String,
    existed: bool,
}

impl SnapshotStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub async fn capture(
        &self,
        turn_id: &TurnId,
        workspace: &Path,
        path: &Path,
    ) -> Result<Option<PathBuf>> {
        let store = self.clone();
        let turn_id = turn_id.clone();
        let workspace = workspace.to_path_buf();
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || store.capture_blocking(&turn_id, &workspace, &path))
            .await
            .context("snapshot task failed")?
    }

    pub async fn restore(&self, turn_id: &TurnId, workspace: &Path) -> Result<RestoreReport> {
        let store = self.clone();
        let turn_id = turn_id.clone();
        let workspace = workspace.to_path_buf();
        tokio::task::spawn_blocking(move || store.restore_blocking(&turn_id, &workspace))
            .await
            .context("restore task failed")?
    }

    fn capture_blocking(
        &self,
        turn_id: &TurnId,
        workspace: &Path,
        path: &Path,
    ) -> Result<Option<PathBuf>> {
        let full = if path.is_absolute() {
            path.to_path_buf()
        } else {
            workspace.join(path)
        };
        let relative = relative_path(workspace, &full)?;
        let turn_dir = self.root.join(turn_id.as_str());
        let manifest_path = turn_dir.join(MANIFEST_FILE);
        let mut manifest = read_manifest(&manifest_path)?;
        if manifest.entries.contains_key(&relative) {
            return Ok(None);
        }

        let metadata = match fs::symlink_metadata(&full) {
            Ok(metadata) => metadata,
            Err(_) => {
                manifest.entries.insert(
                    relative,
                    SnapshotEntry {
                        blob: String::new(),
                        existed: false,
                    },
                );
                write_manifest(&turn_dir, &manifest_path, &manifest)?;
                return Ok(None);
            }
        };
        if metadata.file_type().is_symlink() {
            return Ok(None);
        }
        if !metadata.is_file() || metadata.len() > MAX_SNAPSHOT_BYTES {
            return Ok(None);
        }

        let bytes = fs::read(&full)
            .with_context(|| format!("failed to read snapshot source {}", full.display()))?;
        let blob = format!("{:016x}-{}.bin", hash_path(&relative), bytes.len());
        fs::create_dir_all(&turn_dir)
            .with_context(|| format!("failed to create {}", turn_dir.display()))?;
        fs::write(turn_dir.join(&blob), &bytes)
            .with_context(|| format!("failed to write snapshot blob {blob}"))?;
        manifest.entries.insert(
            relative,
            SnapshotEntry {
                blob,
                existed: true,
            },
        );
        write_manifest(&turn_dir, &manifest_path, &manifest)?;
        let _ = prune_turns(&self.root, KEEP_TURNS);
        Ok(Some(full))
    }

    fn restore_blocking(&self, turn_id: &TurnId, workspace: &Path) -> Result<RestoreReport> {
        let turn_dir = self.root.join(turn_id.as_str());
        let manifest = read_manifest(&turn_dir.join(MANIFEST_FILE))?;
        let mut report = RestoreReport::default();
        for (relative, entry) in manifest.entries {
            let target = safe_join(workspace, &relative)?;
            if !entry.existed {
                match fs::remove_file(&target) {
                    Ok(()) => report.deleted.push(target),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("failed to remove {}", target.display()));
                    }
                }
                continue;
            }
            let blob = turn_dir.join(&entry.blob);
            let bytes = match fs::read(&blob) {
                Ok(bytes) => bytes,
                Err(_) => {
                    report.skipped.push(target);
                    continue;
                }
            };
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            fs::write(&target, bytes)
                .with_context(|| format!("failed to restore {}", target.display()))?;
            report.restored.push(target);
        }
        Ok(report)
    }
}

fn read_manifest(path: &Path) -> Result<SnapshotManifest> {
    match fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse snapshot manifest {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(SnapshotManifest::default())
        }
        Err(error) => Err(error)
            .with_context(|| format!("failed to read snapshot manifest {}", path.display())),
    }
}

fn write_manifest(dir: &Path, path: &Path, manifest: &SnapshotManifest) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let raw =
        serde_json::to_string_pretty(manifest).context("failed to encode snapshot manifest")?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, raw).with_context(|| format!("failed to write {}", temp.display()))?;
    fs::rename(&temp, path).with_context(|| format!("failed to seal {}", path.display()))
}

fn relative_path(workspace: &Path, path: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(workspace)
        .with_context(|| format!("snapshot path {} is outside the workspace", path.display()))?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) => parts.push(name.to_string_lossy().to_string()),
            _ => bail!("invalid snapshot path {}", relative.display()),
        }
    }
    if parts.is_empty() {
        bail!("snapshot path is empty");
    }
    Ok(parts.join("/"))
}

fn safe_join(root: &Path, relative: &str) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            bail!("invalid snapshot entry {relative}");
        }
        path.push(part);
    }
    Ok(path)
}

fn hash_path(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn prune_turns(root: &Path, keep: usize) -> Result<usize> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return Ok(0),
    };
    let mut turns: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    if turns.len() <= keep {
        return Ok(0);
    }
    turns.sort_by_key(|(modified, _)| *modified);
    let remove_count = turns.len().saturating_sub(keep);
    let mut removed = 0;
    for (_, path) in turns.into_iter().take(remove_count) {
        if fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "meerai-snapshot-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn captures_and_restores_modified_and_new_files() {
        let workspace = temp_dir("restore");
        let store = SnapshotStore::new(workspace.join("snapshots"));
        let turn = TurnId::new();
        fs::write(workspace.join("a.txt"), "old").unwrap();

        store
            .capture(&turn, &workspace, Path::new("a.txt"))
            .await
            .unwrap();
        fs::write(workspace.join("a.txt"), "new").unwrap();
        store
            .capture(&turn, &workspace, Path::new("b.txt"))
            .await
            .unwrap();
        fs::write(workspace.join("b.txt"), "created").unwrap();

        let report = store.restore(&turn, &workspace).await.unwrap();
        assert_eq!(fs::read_to_string(workspace.join("a.txt")).unwrap(), "old");
        assert!(!workspace.join("b.txt").exists());
        assert_eq!(report.restored.len(), 1);
        assert_eq!(report.deleted.len(), 1);

        fs::remove_dir_all(&workspace).ok();
    }

    #[tokio::test]
    async fn rejects_paths_outside_the_workspace() {
        let workspace = temp_dir("outside");
        let store = SnapshotStore::new(workspace.join("snapshots"));
        let error = store
            .capture(&TurnId::new(), &workspace, Path::new("../escape"))
            .await
            .unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("outside the workspace") || message.contains("invalid snapshot path"),
            "{message}"
        );
        fs::remove_dir_all(&workspace).ok();
    }
}
