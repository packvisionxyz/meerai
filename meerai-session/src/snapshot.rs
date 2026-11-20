use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use meerai_core::env::now_millis;
use meerai_store::Store;
use rusqlite::{OptionalExtension, params};

use crate::ids::{SessionId, TurnId};

const MAX_SNAPSHOT_BYTES: u64 = 2 * 1024 * 1024;
const KEEP_TURNS: usize = 100;

static STAGING_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct SnapshotStore {
    store: Store,
    session_id: SessionId,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RestoreReport {
    pub restored: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
    pub skipped: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
enum AppliedOp {
    Restored { target: PathBuf, content: Vec<u8> },
    Deleted,
}

#[derive(Clone, Debug)]
struct SnapshotRow {
    rel_path: String,
    existed: bool,
    content_zstd: Option<Vec<u8>>,
}

impl SnapshotStore {
    pub fn new(store: Store, session_id: SessionId) -> Self {
        Self { store, session_id }
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
        if self.entry_exists(turn_id, &relative)? {
            return Ok(None);
        }

        let metadata = match fs::symlink_metadata(&full) {
            Ok(metadata) => metadata,
            Err(_) => {
                self.insert_entry(turn_id, &relative, false, None)?;
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
        let compressed =
            zstd::stream::encode_all(bytes.as_slice(), 3).context("compress snapshot content")?;
        self.insert_entry(turn_id, &relative, true, Some(compressed))?;
        self.prune_turns()?;
        Ok(Some(full))
    }

    fn restore_blocking(&self, turn_id: &TurnId, workspace: &Path) -> Result<RestoreReport> {
        let rows = self.rows(turn_id)?;
        let mut report = RestoreReport::default();
        let mut applied = Vec::new();
        for row in rows {
            if let Err(error) = apply_entry(workspace, &row, &mut applied, &mut report) {
                return Err(roll_back(error, &applied));
            }
        }
        Ok(report)
    }

    fn entry_exists(&self, turn_id: &TurnId, relative: &str) -> Result<bool> {
        let exists: Option<bool> = self.store.read(|conn| {
            Ok(conn
                .query_row(
                    "SELECT 1 FROM session_snapshots
                     WHERE session_id = ?1 AND turn_id = ?2 AND rel_path = ?3",
                    params![self.session_id.as_str(), turn_id.as_str(), relative],
                    |_| Ok(true),
                )
                .optional()?)
        })?;
        Ok(exists.unwrap_or(false))
    }

    fn insert_entry(
        &self,
        turn_id: &TurnId,
        relative: &str,
        existed: bool,
        content: Option<Vec<u8>>,
    ) -> Result<()> {
        self.store.immediate(|tx| {
            tx.execute(
                "INSERT INTO session_snapshots
                    (session_id, turn_id, rel_path, existed, content_zstd, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    self.session_id.as_str(),
                    turn_id.as_str(),
                    relative,
                    i64::from(existed),
                    content,
                    now_millis() as i64
                ],
            )?;
            Ok(())
        })
    }

    fn rows(&self, turn_id: &TurnId) -> Result<Vec<SnapshotRow>> {
        self.store.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT rel_path, existed, content_zstd FROM session_snapshots
                 WHERE session_id = ?1 AND turn_id = ?2 ORDER BY rel_path",
            )?;
            let rows =
                stmt.query_map(params![self.session_id.as_str(), turn_id.as_str()], |row| {
                    Ok(SnapshotRow {
                        rel_path: row.get(0)?,
                        existed: row.get::<_, i64>(1)? != 0,
                        content_zstd: row.get(2)?,
                    })
                })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    fn prune_turns(&self) -> Result<usize> {
        self.store.immediate(|tx| {
            let mut stmt = tx.prepare(
                "SELECT turn_id FROM session_snapshots
                 WHERE session_id = ?1 GROUP BY turn_id
                 ORDER BY MAX(created_at_ms) DESC LIMIT -1 OFFSET ?2",
            )?;
            let stale: Vec<String> = stmt
                .query_map(
                    params![self.session_id.as_str(), KEEP_TURNS as i64],
                    |row| row.get(0),
                )?
                .collect::<rusqlite::Result<_>>()?;
            drop(stmt);
            let mut removed = 0;
            for turn_id in stale {
                removed += tx.execute(
                    "DELETE FROM session_snapshots WHERE session_id = ?1 AND turn_id = ?2",
                    params![self.session_id.as_str(), turn_id],
                )?;
            }
            Ok(removed)
        })
    }
}

fn apply_entry(
    workspace: &Path,
    row: &SnapshotRow,
    applied: &mut Vec<AppliedOp>,
    report: &mut RestoreReport,
) -> Result<()> {
    let target = safe_join(workspace, &row.rel_path)?;
    if !row.existed {
        match fs::remove_file(&target) {
            Ok(()) => {
                report.deleted.push(target.clone());
                applied.push(AppliedOp::Deleted);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to remove {}", target.display()));
            }
        }
        return Ok(());
    }

    let Some(compressed) = row.content_zstd.as_deref() else {
        report.skipped.push(target);
        return Ok(());
    };
    let bytes = match zstd::stream::decode_all(compressed) {
        Ok(bytes) => bytes,
        Err(_) => {
            report.skipped.push(target);
            return Ok(());
        }
    };
    stage_into_place(&target, &bytes)?;
    applied.push(AppliedOp::Restored {
        target: target.clone(),
        content: bytes,
    });
    report.restored.push(target);
    Ok(())
}

fn stage_into_place(target: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let temp = staging_path(target);
    let staged = fs::write(&temp, bytes)
        .with_context(|| format!("failed to stage {}", temp.display()))
        .and_then(|()| keep_permissions(target, &temp))
        .and_then(|()| {
            fs::rename(&temp, target)
                .with_context(|| format!("failed to restore {}", target.display()))
        });
    if staged.is_err() {
        let _ = fs::remove_file(&temp);
    }
    staged
}

fn staging_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
    target.with_file_name(format!(
        ".{name}.meerai-restore-{}-{seq}.tmp",
        std::process::id()
    ))
}

fn keep_permissions(from: &Path, to: &Path) -> Result<()> {
    match fs::metadata(from) {
        Ok(metadata) => fs::set_permissions(to, metadata.permissions())
            .with_context(|| format!("failed to set permissions on {}", to.display())),
        Err(_) => Ok(()),
    }
}

fn roll_back(error: anyhow::Error, applied: &[AppliedOp]) -> anyhow::Error {
    let failures = rollback(applied);
    if failures.is_empty() {
        return error;
    }
    anyhow::anyhow!("{error:#}; rollback failed: {}", failures.join("; "))
}

fn rollback(applied: &[AppliedOp]) -> Vec<String> {
    let mut failures = Vec::new();
    for op in applied.iter().rev() {
        if let AppliedOp::Restored { target, content } = op
            && let Err(error) = fs::write(target, content)
        {
            failures.push(format!("{}: {error}", target.display()));
        }
    }
    failures
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

    fn snapshot_store() -> SnapshotStore {
        let store = Store::open(":memory:").unwrap();
        let session = SessionId::new();
        store
            .immediate(|tx| {
                tx.execute(
                    "INSERT INTO sessions (id, project_root, cwd, created_at_ms, updated_at_ms)
                     VALUES (?1, '/w', '/w', 0, 0)",
                    [session.as_str()],
                )?;
                Ok(())
            })
            .unwrap();
        SnapshotStore::new(store, session)
    }

    #[tokio::test]
    async fn captures_and_restores_modified_and_new_files() {
        let workspace = temp_dir("restore");
        let store = snapshot_store();
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
    async fn rolls_back_earlier_restores_when_a_later_file_fails() {
        let workspace = temp_dir("rollback");
        let store = snapshot_store();
        let turn = TurnId::new();
        fs::write(workspace.join("a.txt"), "original-a").unwrap();
        fs::write(workspace.join("b.txt"), "original-b").unwrap();
        store
            .capture(&turn, &workspace, Path::new("a.txt"))
            .await
            .unwrap();
        store
            .capture(&turn, &workspace, Path::new("b.txt"))
            .await
            .unwrap();

        fs::write(workspace.join("a.txt"), "changed-a").unwrap();
        fs::remove_file(workspace.join("b.txt")).unwrap();
        fs::create_dir(workspace.join("b.txt")).unwrap();

        let error = store.restore(&turn, &workspace).await.unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("b.txt"), "{message}");
        assert_eq!(
            fs::read_to_string(workspace.join("a.txt")).unwrap(),
            "original-a"
        );
        assert!(workspace.join("b.txt").is_dir());
        let leftovers: Vec<String> = fs::read_dir(&workspace)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("meerai-restore"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");

        fs::remove_dir_all(&workspace).ok();
    }

    #[tokio::test]
    async fn restores_a_tracked_deletion() {
        let workspace = temp_dir("delete");
        let store = snapshot_store();
        let turn = TurnId::new();
        store
            .capture(&turn, &workspace, Path::new("removed.txt"))
            .await
            .unwrap();
        fs::write(workspace.join("removed.txt"), "created").unwrap();

        let report = store.restore(&turn, &workspace).await.unwrap();
        assert!(!workspace.join("removed.txt").exists());
        assert_eq!(report.deleted, vec![workspace.join("removed.txt")]);
        assert!(report.restored.is_empty());

        fs::remove_dir_all(&workspace).ok();
    }

    #[tokio::test]
    async fn rejects_paths_outside_the_workspace() {
        let workspace = temp_dir("outside");
        let store = snapshot_store();
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
