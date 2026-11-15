use std::fmt::Write as _;

use meerai_core::ToolDefinition;

use crate::code::{CodeTools, ToolRun, failure_run, success_run};

const DEFAULT_LIMIT: usize = 2000;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct ReadFileArgs {
    #[schemars(description = "File path, relative to the workspace root or absolute.")]
    path: String,

    #[schemars(description = "1-based line number to start reading from. Defaults to 1.")]
    #[serde(default, deserialize_with = "meerai_core::lenient::option_usize_value")]
    offset: Option<usize>,

    #[schemars(description = "Maximum number of lines to return. Defaults to 2000.")]
    #[serde(default, deserialize_with = "meerai_core::lenient::option_usize_value")]
    limit: Option<usize>,
}

pub(crate) fn definition(name: &str) -> ToolDefinition {
    crate::code::definition_for::<ReadFileArgs>(
        name,
        "Read a UTF-8 text file with line numbers, optionally limited to a line window.",
    )
}

impl CodeTools {
    pub(crate) async fn read_file(&self, args: ReadFileArgs) -> ToolRun {
        let path = match self.resolve_for_read(&args.path).await {
            Ok(path) => path,
            Err(issue) => return issue.into_run(),
        };

        match tokio::fs::metadata(&path).await {
            Ok(metadata) if metadata.is_dir() => {
                return failure_run(format!("path is a directory: {}", args.path));
            }
            Ok(_) => {}
            Err(error) => {
                return failure_run(format!("cannot inspect {}: {error}", args.path));
            }
        }

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(content) => content,
            Err(error) => return failure_run(format!("cannot read {}: {error}", args.path)),
        };

        success_run(render_lines(&content, args.offset, args.limit))
    }
}

fn render_lines(content: &str, offset: Option<usize>, limit: Option<usize>) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    if total == 0 {
        return "(file is empty)".to_string();
    }

    let first = offset.unwrap_or(1).max(1);
    let limit = limit.unwrap_or(DEFAULT_LIMIT).max(1);
    let start = (first - 1).min(total);
    let end = start.saturating_add(limit).min(total);

    if start >= total {
        return format!("(no lines: {total} line(s) in file, offset {first} is past the end)");
    }

    let mut output = String::new();
    for (index, line) in lines[start..end].iter().enumerate() {
        let line_number = start + index + 1;
        let _ = writeln!(output, "{line_number}:{line}");
    }

    if end < total {
        let _ = writeln!(
            output,
            "(truncated: showing lines {}-{end} of {total})",
            start + 1
        );
    }

    output
}
