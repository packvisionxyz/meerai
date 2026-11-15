use std::io::ErrorKind;

use meerai_core::{FileChangeKind, FileDiff, ToolDefinition};

use crate::code::paths::display_path;
use crate::code::{CodeTools, ToolRun, diff, failure_run, success_run};

const MAX_REPORTED_LINES: usize = 20;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct EditFileArgs {
    #[schemars(description = "File path, relative to the workspace root or absolute.")]
    path: String,

    #[schemars(description = "Exact text to replace, including indentation and newlines.")]
    old_string: String,

    #[schemars(description = "Replacement text for old_string.")]
    new_string: String,

    #[schemars(description = "Replace every occurrence instead of requiring a unique match.")]
    #[serde(default, deserialize_with = "meerai_core::lenient::option_bool_value")]
    replace_all: Option<bool>,
}

pub(crate) fn definition(name: &str) -> ToolDefinition {
    crate::code::definition_for::<EditFileArgs>(
        name,
        "Replace exact text in a file and report a unified diff. Requires a unique match unless replace_all is set.",
    )
}

impl CodeTools {
    pub(crate) async fn edit_file(&self, args: EditFileArgs) -> ToolRun {
        if args.old_string.is_empty() {
            return failure_run("old_string must not be empty".to_string());
        }

        let target = match self.resolve_for_write(&args.path).await {
            Ok(target) => target,
            Err(issue) => return issue.into_run(),
        };
        let display = display_path(&self.canonical_workspace_root().await, &target);
        let replace_all = args.replace_all.unwrap_or(false);

        let content = match tokio::fs::read(&target).await {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(content) => content,
                Err(_) => return failure_run(format!("file is not valid UTF-8: {display}")),
            },
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return failure_run(format!("file does not exist: {display}"));
            }
            Err(error) => return failure_run(format!("cannot read {display}: {error}")),
        };

        let matches = occurrence_lines(&content, &args.old_string);
        if matches.is_empty() {
            return failure_run(format!("old_string not found in {display}"));
        }

        if matches.len() > 1 && !replace_all {
            let shown = matches
                .iter()
                .take(MAX_REPORTED_LINES)
                .map(usize::to_string)
                .collect::<Vec<String>>()
                .join(", ");
            let suffix = if matches.len() > MAX_REPORTED_LINES {
                ", ..."
            } else {
                ""
            };
            return failure_run(format!(
                "old_string occurs {} times in {display} (lines {shown}{suffix}); make the match unique or set replace_all to true",
                matches.len()
            ));
        }

        let replaced = if replace_all { matches.len() } else { 1 };
        let updated = if replace_all {
            content.replace(&args.old_string, &args.new_string)
        } else {
            content.replacen(&args.old_string, &args.new_string, 1)
        };

        if let Err(error) = tokio::fs::write(&target, updated.as_bytes()).await {
            return failure_run(format!("cannot write {display}: {error}"));
        }

        let unified = diff::unified_diff(&display, &content, &updated);
        let label = if replaced == 1 {
            "occurrence"
        } else {
            "occurrences"
        };
        let mut summary = format!("Replaced {replaced} {label} in {display}.");
        let stdout = if unified.is_empty() {
            summary.push_str(" No content change.");
            summary
        } else {
            format!("{summary}\n\n{unified}")
        };

        let mut run = success_run(stdout);
        run.file_changes = vec![FileDiff {
            path: display,
            kind: FileChangeKind::Modified,
            unified_diff: unified,
        }];
        run
    }
}

fn occurrence_lines(content: &str, needle: &str) -> Vec<usize> {
    let mut lines = Vec::new();
    let mut offset = 0;

    while let Some(index) = content[offset..].find(needle) {
        let position = offset + index;
        let line = content[..position]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1;
        lines.push(line);
        offset = position + needle.len();
    }

    lines
}

#[cfg(test)]
mod lenient_tests {
    use super::*;

    #[test]
    fn accepts_string_booleans_for_replace_all() {
        let args: EditFileArgs = serde_json::from_str(
            r#"{"path":"a.rs","old_string":"a","new_string":"b","replace_all":"true"}"#,
        )
        .unwrap();
        assert_eq!(args.replace_all, Some(true));
    }
}
