use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use meerai_core::ToolRun;

use crate::code::{CodeTools, failure_run, refused_run};

pub(crate) enum PathIssue {
    Missing(String),
    Escape(String),
    Failure(String),
}

impl PathIssue {
    pub(crate) fn into_run(self) -> ToolRun {
        match self {
            PathIssue::Escape(reason) => refused_run(reason),
            PathIssue::Missing(reason) | PathIssue::Failure(reason) => failure_run(reason),
        }
    }
}

impl CodeTools {
    pub(crate) async fn canonical_workspace_root(&self) -> PathBuf {
        canonicalize_or_keep(&self.workspace_root).await
    }

    pub(crate) async fn resolve_for_read(&self, raw: &str) -> Result<PathBuf, PathIssue> {
        let workspace = self.canonical_workspace_root().await;
        let candidate = join(&workspace, raw);
        let (resolved, exists) = locate(&candidate).await?;

        let mut readable = vec![workspace];
        for root in self.readable_roots() {
            readable.push(canonicalize_or_keep(root).await);
        }

        if !readable.iter().any(|root| resolved.starts_with(root)) {
            return Err(PathIssue::Escape(format!("path escapes workspace: {raw}")));
        }

        if exists {
            Ok(resolved)
        } else {
            Err(PathIssue::Missing(format!("path does not exist: {raw}")))
        }
    }

    pub(crate) async fn resolve_for_write(&self, raw: &str) -> Result<PathBuf, PathIssue> {
        let workspace = self.canonical_workspace_root().await;
        let candidate = join(&workspace, raw);
        let (resolved, _) = locate(&candidate).await?;

        let mut writable = vec![workspace];
        for root in self.writable_roots() {
            writable.push(canonicalize_or_keep(root).await);
        }

        if writable.iter().any(|root| resolved.starts_with(root)) {
            Ok(resolved)
        } else {
            Err(PathIssue::Escape(format!("path escapes workspace: {raw}")))
        }
    }
}

async fn locate(candidate: &Path) -> Result<(PathBuf, bool), PathIssue> {
    match tokio::fs::canonicalize(candidate).await {
        Ok(canonical) => Ok((canonical, true)),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let (existing, missing) = nearest_existing(candidate).await?;
            let canonical = tokio::fs::canonicalize(&existing).await.map_err(|error| {
                PathIssue::Failure(format!(
                    "cannot resolve directory {}: {error}",
                    existing.display()
                ))
            })?;
            let mut resolved = canonical;
            for component in missing.into_iter().rev() {
                resolved.push(component);
            }
            Ok((resolved, false))
        }
        Err(error) => Err(PathIssue::Failure(format!(
            "cannot resolve path {}: {error}",
            candidate.display()
        ))),
    }
}

async fn nearest_existing(candidate: &Path) -> Result<(PathBuf, Vec<OsString>), PathIssue> {
    let mut missing: Vec<OsString> = Vec::new();
    let mut probe = candidate;

    loop {
        match tokio::fs::symlink_metadata(probe).await {
            Ok(_) => return Ok((probe.to_path_buf(), missing)),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let Some(name) = probe.file_name() else {
                    return Err(PathIssue::Failure(format!(
                        "cannot resolve parent directory: {}",
                        candidate.display()
                    )));
                };
                missing.push(name.to_os_string());
                let Some(parent) = probe.parent() else {
                    return Err(PathIssue::Failure(format!(
                        "cannot resolve parent directory: {}",
                        candidate.display()
                    )));
                };
                probe = parent;
            }
            Err(error) => {
                return Err(PathIssue::Failure(format!(
                    "cannot inspect path {}: {error}",
                    probe.display()
                )));
            }
        }
    }
}

fn join(root: &Path, raw: &str) -> PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

async fn canonicalize_or_keep(path: &Path) -> PathBuf {
    tokio::fs::canonicalize(path)
        .await
        .unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn display_path(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(relative) if !relative.as_os_str().is_empty() => flatten(relative),
        Ok(_) => ".".to_string(),
        Err(_) => flatten(path),
    }
}

pub(crate) fn relative_path(workspace: &Path, root: &Path, path: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(workspace)
        && !relative.as_os_str().is_empty()
    {
        return flatten(relative);
    }

    if let Ok(relative) = path.strip_prefix(root)
        && !relative.as_os_str().is_empty()
    {
        return flatten(relative);
    }

    flatten(path)
}

pub(crate) fn flatten(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
