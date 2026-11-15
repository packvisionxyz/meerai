use std::path::Path;

use globset::Glob;
use ignore::WalkBuilder;
use meerai_core::ToolDefinition;

use crate::code::paths::{display_path, relative_path};
use crate::code::{CodeTools, ToolRun, failure_run, success_run};

const MAX_GLOB_RESULTS: usize = 500;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct GlobArgs {
    #[schemars(
        description = "Glob pattern matched against workspace-relative paths, for example \"**/*.rs\"."
    )]
    pattern: String,

    #[schemars(description = "Directory to search. Defaults to the workspace root.")]
    path: Option<String>,
}

pub(crate) fn definition(name: &str) -> ToolDefinition {
    crate::code::definition_for::<GlobArgs>(
        name,
        "Find files whose path matches a glob pattern. Respects .gitignore and sorts results.",
    )
}

impl CodeTools {
    pub(crate) async fn glob(&self, args: GlobArgs) -> ToolRun {
        let raw_root = args.path.clone().unwrap_or_else(|| ".".to_string());
        let root = match self.resolve_for_read(&raw_root).await {
            Ok(root) => root,
            Err(issue) => return issue.into_run(),
        };

        let workspace = self.canonical_workspace_root().await;
        let pattern = args.pattern.clone();
        let task_pattern = pattern.clone();
        let task_workspace = workspace.clone();
        let task_root = root.clone();

        let result = tokio::task::spawn_blocking(move || {
            collect(&task_workspace, &task_root, &task_pattern)
        })
        .await;

        match result {
            Ok(Ok((matches, truncated))) => {
                if matches.is_empty() {
                    return success_run(format!(
                        "No files matching \"{pattern}\" in {}",
                        display_path(&workspace, &root)
                    ));
                }

                let mut output = matches.join("\n");
                if truncated {
                    output.push_str(&format!(
                        "\n(truncated: showing first {MAX_GLOB_RESULTS} files)"
                    ));
                }
                success_run(output)
            }
            Ok(Err(message)) => failure_run(message),
            Err(error) => failure_run(format!("glob task failed: {error}")),
        }
    }
}

fn collect(workspace: &Path, root: &Path, pattern: &str) -> Result<(Vec<String>, bool), String> {
    let matcher = Glob::new(pattern)
        .map_err(|error| format!("invalid glob pattern: {error}"))?
        .compile_matcher();

    let mut matches: Vec<String> = Vec::new();
    let mut truncated = false;

    if root.is_file() {
        let display = relative_path(workspace, root, root);
        if matcher.is_match(&display) {
            matches.push(display);
        }
        return Ok((matches, false));
    }

    for entry in WalkBuilder::new(root)
        .require_git(false)
        .git_global(false)
        .build()
        .flatten()
    {
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }

        let display = relative_path(workspace, root, entry.path());
        if !matcher.is_match(&display) {
            continue;
        }

        if matches.len() >= MAX_GLOB_RESULTS {
            truncated = true;
            break;
        }

        matches.push(display);
    }

    matches.sort();
    matches.dedup();

    Ok((matches, truncated))
}
