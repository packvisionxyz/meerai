use std::path::Path;

use ignore::WalkBuilder;
use meerai_core::ToolDefinition;

use crate::code::paths::is_sensitive_path;
use crate::code::{CodeTools, ToolRun, failure_run, success_run};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct ListDirArgs {
    #[schemars(description = "Directory path, relative to the workspace root or absolute.")]
    #[serde(alias = "dir", alias = "directory", alias = "file_path")]
    path: String,
}

pub(crate) fn definition(name: &str) -> ToolDefinition {
    crate::code::definition_for::<ListDirArgs>(
        name,
        "List a directory with sorted entries, marking directories and file sizes. Respects .gitignore.",
    )
}

impl CodeTools {
    pub(crate) async fn list_dir(&self, args: ListDirArgs) -> ToolRun {
        let target = match self.resolve_for_read(&args.path).await {
            Ok(target) => target,
            Err(issue) => return issue.into_run(),
        };

        match tokio::fs::metadata(&target).await {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return failure_run(format!("path is not a directory: {}", args.path)),
            Err(error) => {
                return failure_run(format!("cannot inspect {}: {error}", args.path));
            }
        }

        let allow_sensitive = self.allow_sensitive_files();
        let task_root = target.clone();
        let result =
            tokio::task::spawn_blocking(move || collect_entries(&task_root, allow_sensitive)).await;

        match result {
            Ok(Ok(entries)) => success_run(render(entries)),
            Ok(Err(message)) => failure_run(message),
            Err(error) => failure_run(format!("list task failed: {error}")),
        }
    }
}

fn collect_entries(root: &Path, allow_sensitive: bool) -> Result<Vec<(String, bool, u64)>, String> {
    let mut entries = Vec::new();

    for entry in WalkBuilder::new(root)
        .require_git(false)
        .git_global(false)
        .max_depth(Some(1))
        .build()
    {
        let entry = entry.map_err(|error| format!("cannot walk {}: {error}", root.display()))?;
        if entry.depth() == 0 {
            continue;
        }

        if !allow_sensitive && is_sensitive_path(entry.path()) {
            continue;
        }

        let name = entry.file_name().to_string_lossy().into_owned();
        let metadata = entry.metadata().ok();
        let is_dir = metadata.as_ref().is_some_and(|metadata| metadata.is_dir());
        let size = metadata
            .as_ref()
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        entries.push((name, is_dir, size));
    }

    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(entries)
}

fn render(entries: Vec<(String, bool, u64)>) -> String {
    if entries.is_empty() {
        return "(empty directory)".to_string();
    }

    let mut output = String::new();
    for (name, is_dir, size) in entries {
        if is_dir {
            output.push_str(&format!("d {name}/\n"));
        } else {
            output.push_str(&format!("f {name} ({size} bytes)\n"));
        }
    }

    output
}
