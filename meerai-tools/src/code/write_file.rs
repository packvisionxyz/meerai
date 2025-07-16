use std::io::ErrorKind;

use meerai_core::{FileChangeKind, FileDiff, ToolDefinition};

use crate::code::paths::display_path;
use crate::code::{CodeTools, ToolRun, diff, failure_run, success_run};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct WriteFileArgs {
    #[schemars(description = "File path, relative to the workspace root or absolute.")]
    path: String,

    #[schemars(description = "Full file content to write. Parent directories are created.")]
    content: String,
}

pub(crate) fn definition(name: &str) -> ToolDefinition {
    crate::code::definition_for::<WriteFileArgs>(
        name,
        "Create or overwrite a text file and report a unified diff of the change.",
    )
}

impl CodeTools {
    pub(crate) async fn write_file(&self, args: WriteFileArgs) -> ToolRun {
        let target = match self.resolve_for_write(&args.path).await {
            Ok(target) => target,
            Err(issue) => return issue.into_run(),
        };
        let display = display_path(&self.canonical_workspace_root().await, &target);

        let existing = match tokio::fs::symlink_metadata(&target).await {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => return failure_run(format!("cannot inspect {display}: {error}")),
        };

        if existing.as_ref().is_some_and(|metadata| metadata.is_dir()) {
            return failure_run(format!("path is a directory: {display}"));
        }

        let previous = if existing.is_some() {
            match tokio::fs::read(&target).await {
                Ok(bytes) => String::from_utf8(bytes).ok(),
                Err(error) => return failure_run(format!("cannot read {display}: {error}")),
            }
        } else {
            None
        };

        if let Some(parent) = target.parent()
            && let Err(error) = tokio::fs::create_dir_all(parent).await
        {
            return failure_run(format!(
                "cannot create directory {}: {error}",
                parent.display()
            ));
        }

        if let Err(error) = tokio::fs::write(&target, args.content.as_bytes()).await {
            return failure_run(format!("cannot write {display}: {error}"));
        }

        let kind = if existing.is_some() {
            FileChangeKind::Modified
        } else {
            FileChangeKind::Added
        };
        let unified = diff::unified_diff(
            &display,
            previous.as_deref().unwrap_or_default(),
            &args.content,
        );

        let mut summary = format!(
            "Wrote {} bytes to {display} ({} lines).",
            args.content.len(),
            args.content.lines().count()
        );
        let stdout = if unified.is_empty() {
            summary.push_str(" No content change.");
            summary
        } else {
            format!("{summary}\n\n{unified}")
        };

        let mut run = success_run(stdout);
        run.file_changes = vec![FileDiff {
            path: display,
            kind,
            unified_diff: unified,
        }];
        run
    }
}
