use std::io::ErrorKind;

use meerai_core::{FileChangeKind, FileDiff, ToolDefinition};

use crate::code::paths::display_path;
use crate::code::{CodeTools, ToolRun, diff, failure_run, success_run};

const MAX_REPORTED_LINES: usize = 20;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct EditFileArgs {
    #[schemars(description = "File path, relative to the workspace root or absolute.")]
    #[serde(alias = "file", alias = "file_path", alias = "filename")]
    path: String,

    #[schemars(
        description = "Text to replace. An exact match wins; otherwise a unique whitespace/line-prefix-normalized match is used."
    )]
    #[serde(alias = "old_text", alias = "old")]
    old_string: String,

    #[schemars(description = "Replacement text for old_string.")]
    #[serde(alias = "new_text", alias = "new")]
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

        let exact_lines = occurrence_lines(&content, &args.old_string);
        let (needle, repaired) = if exact_lines.is_empty() {
            match repair_anchor(&content, &args.old_string) {
                Repair::Unique { text } => (text, true),
                Repair::Ambiguous(lines) => {
                    return failure_run(ambiguous_message(&display, &lines));
                }
                Repair::Missing(candidates) => {
                    return failure_run(missing_message(&display, &candidates));
                }
            }
        } else {
            (args.old_string.clone(), false)
        };

        let matches = occurrence_lines(&content, &needle);
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

        let replacement = if repaired {
            let line_ending = if content.contains("\r\n") {
                "\r\n"
            } else {
                "\n"
            };
            normalize_newlines(&args.new_string, line_ending)
        } else {
            args.new_string.clone()
        };
        let replaced = if replace_all { matches.len() } else { 1 };
        let updated = if replace_all {
            content.replace(&needle, &replacement)
        } else {
            content.replacen(&needle, &replacement, 1)
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
        if repaired {
            summary.push_str(" Anchor repaired by normalizing whitespace or line prefixes.");
        }
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

enum Repair {
    Unique { text: String },
    Ambiguous(Vec<usize>),
    Missing(Vec<(usize, String)>),
}

fn repair_anchor(content: &str, old: &str) -> Repair {
    let old_lines: Vec<&str> = old.lines().map(strip_line_prefix).map(str::trim).collect();
    if old_lines.is_empty() || old_lines.iter().all(|line| line.is_empty()) {
        return Repair::Missing(Vec::new());
    }
    let file_lines: Vec<&str> = content.split_inclusive('\n').collect();
    if old_lines.len() > file_lines.len() {
        return Repair::Missing(closest_lines(&file_lines, &old_lines));
    }

    let mut windows = Vec::new();
    for start in 0..=file_lines.len() - old_lines.len() {
        if old_lines
            .iter()
            .enumerate()
            .all(|(offset, old_line)| file_lines[start + offset].trim() == *old_line)
        {
            windows.push(start);
        }
    }

    match windows.as_slice() {
        [start] => {
            let mut text = file_lines[*start..*start + old_lines.len()].concat();
            if !old.ends_with('\n') {
                text = text.trim_end_matches(['\n', '\r']).to_string();
            }
            Repair::Unique { text }
        }
        [] => Repair::Missing(closest_lines(&file_lines, &old_lines)),
        _ => Repair::Ambiguous(windows.iter().map(|start| start + 1).collect()),
    }
}

fn strip_line_prefix(line: &str) -> &str {
    let trimmed = line.trim_start();
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return line;
    }
    let rest = &trimmed[digits..];
    if let Some(rest) = rest.strip_prefix(':').or_else(|| rest.strip_prefix('|')) {
        return rest.trim_start();
    }
    line
}

fn closest_lines(file_lines: &[&str], old_lines: &[&str]) -> Vec<(usize, String)> {
    let Some(first) = old_lines.iter().find(|line| !line.is_empty()) else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    for (index, line) in file_lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed == *first || trimmed.contains(*first) || first.contains(trimmed) {
            candidates.push((index + 1, trimmed.to_string()));
            if candidates.len() == 3 {
                break;
            }
        }
    }
    candidates
}

fn ambiguous_message(display: &str, lines: &[usize]) -> String {
    let shown = lines
        .iter()
        .take(MAX_REPORTED_LINES)
        .map(usize::to_string)
        .collect::<Vec<String>>()
        .join(", ");
    let suffix = if lines.len() > MAX_REPORTED_LINES {
        ", ..."
    } else {
        ""
    };
    format!(
        "old_string not found in {display}; it matches {} places after normalizing whitespace or line prefixes (lines {shown}{suffix}). Include more context to make it unique.",
        lines.len()
    )
}

fn missing_message(display: &str, candidates: &[(usize, String)]) -> String {
    let mut message = format!("old_string not found in {display}.");
    if !candidates.is_empty() {
        message.push_str(" Closest lines:");
        for (line, text) in candidates {
            message.push_str(&format!("\n  {line}: {text}"));
        }
        message
            .push_str("\nCopy the exact text from the file, or use read_file with offset/limit.");
    }
    message
}

fn normalize_newlines(text: &str, line_ending: &str) -> String {
    if line_ending == "\n" {
        text.replace("\r\n", "\n")
    } else {
        text.replace("\r\n", "\n").replace('\n', line_ending)
    }
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
