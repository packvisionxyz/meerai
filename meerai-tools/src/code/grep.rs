use std::path::{Path, PathBuf};

use globset::{Glob, GlobMatcher};
use ignore::WalkBuilder;
use meerai_core::ToolDefinition;

use crate::code::paths::{display_path, relative_path};
use crate::code::{CodeTools, ToolRun, failure_run, success_run};
use crate::workspace_search::{build_searcher, build_smart_matcher, search_file_with_matcher};

const MAX_MATCHES_PER_FILE: usize = 50;
const MAX_TOTAL_MATCHES: usize = 200;
const MAX_REPORTED_FILES: usize = 10;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct GrepArgs {
    #[schemars(
        description = "Regular expression to search for. Matching is case-insensitive unless the pattern contains an uppercase letter."
    )]
    pattern: String,

    #[schemars(description = "File or directory to search. Defaults to the workspace root.")]
    path: Option<String>,

    #[schemars(description = "Glob filter for file paths, for example \"*.rs\".")]
    include: Option<String>,
}

pub(crate) fn definition(name: &str) -> ToolDefinition {
    crate::code::definition_for::<GrepArgs>(
        name,
        "Search file contents with a regular expression. Respects .gitignore and returns path:line:text matches.",
    )
}

impl CodeTools {
    pub(crate) async fn grep(&self, args: GrepArgs) -> ToolRun {
        if args.pattern.is_empty() {
            return failure_run("pattern must not be empty".to_string());
        }

        let raw_root = args.path.clone().unwrap_or_else(|| ".".to_string());
        let root = match self.resolve_for_read(&raw_root).await {
            Ok(root) => root,
            Err(issue) => return issue.into_run(),
        };

        let workspace = self.canonical_workspace_root().await;
        let pattern = args.pattern.clone();
        let include = args.include.clone();
        let task_workspace = workspace.clone();
        let task_root = root.clone();

        let result = tokio::task::spawn_blocking(move || {
            search(&task_workspace, &task_root, &pattern, include.as_deref())
        })
        .await;

        match result {
            Ok(Ok(output)) => success_run(output),
            Ok(Err(message)) => failure_run(message),
            Err(error) => failure_run(format!("search task failed: {error}")),
        }
    }
}

fn search(
    workspace: &Path,
    root: &Path,
    pattern: &str,
    include: Option<&str>,
) -> Result<String, String> {
    let matcher = build_smart_matcher(pattern, false)?;

    let include: Option<GlobMatcher> = include
        .map(|glob| {
            Glob::new(glob)
                .map(|glob| glob.compile_matcher())
                .map_err(|error| format!("invalid include glob: {error}"))
        })
        .transpose()?;

    let files = collect_files(root);
    let mut searcher = build_searcher();

    let mut matches: Vec<String> = Vec::new();
    let mut per_file_capped: Vec<String> = Vec::new();
    let mut total_capped = false;

    for file in files {
        if matches.len() >= MAX_TOTAL_MATCHES {
            break;
        }

        let path = relative_path(workspace, root, &file);
        if include
            .as_ref()
            .is_some_and(|include| !include.is_match(&path))
        {
            continue;
        }

        let remaining = MAX_TOTAL_MATCHES.saturating_sub(matches.len());
        let (hits, file_capped) = search_file_with_matcher(
            &mut searcher,
            &matcher,
            &file,
            &path,
            MAX_MATCHES_PER_FILE,
            remaining,
            false,
        );

        for hit in hits {
            matches.push(format!("{}:{}:{}", hit.path, hit.line, hit.text));
        }

        if file_capped {
            per_file_capped.push(path);
        }
    }

    if matches.len() >= MAX_TOTAL_MATCHES {
        total_capped = true;
    }

    if matches.is_empty() {
        return Ok(format!(
            "No matches for \"{pattern}\" in {}",
            display_path(workspace, root)
        ));
    }

    let mut output = matches.join("\n");
    if total_capped {
        output.push_str(&format!(
            "\n(truncated: capped at {MAX_TOTAL_MATCHES} matches)"
        ));
    }
    for path in per_file_capped.iter().take(MAX_REPORTED_FILES) {
        output.push_str(&format!(
            "\n(truncated: stopped after {MAX_MATCHES_PER_FILE} matches in {path})"
        ));
    }

    Ok(output)
}

fn collect_files(root: &Path) -> Vec<PathBuf> {
    if root.is_file() {
        return vec![root.to_path_buf()];
    }

    let mut files: Vec<PathBuf> = WalkBuilder::new(root)
        .require_git(false)
        .git_global(false)
        .build()
        .flatten()
        .filter(|entry| {
            entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file())
        })
        .map(|entry| entry.into_path())
        .collect();

    files.sort();
    files
}
