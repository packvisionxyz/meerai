use super::render::*;
use super::*;

pub(super) fn script_from_args(name: &str, args: &str) -> Option<(String, String)> {
    if !name.ends_with("run_script") {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(args).ok()?;
    let language = value
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("script")
        .to_string();
    if let Some(script) = value.get("script").and_then(|v| v.as_str()) {
        return Some((language, script.to_string()));
    }
    let path = value.get("path").and_then(|v| v.as_str())?;
    Some((language, format!("(file) {path}")))
}

pub(super) fn tool_status_badge(status: &ToolCallStatus) -> (&'static str, Color) {
    match status {
        ToolCallStatus::Proposed => ("proposed", theme::p().warning),
        ToolCallStatus::Running => ("running", theme::p().warning),
        ToolCallStatus::Succeeded => ("ok", theme::p().success),
        ToolCallStatus::Failed => ("fail", theme::p().error),
        ToolCallStatus::Stopped => ("stop", theme::p().muted),
        ToolCallStatus::Cancelled => ("cancelled", theme::p().muted),
    }
}

pub(super) fn tool_target(name: &str, args: &str, script: &Option<(String, String)>) -> String {
    if let Some((language, code)) = script {
        let first = code.lines().next().unwrap_or("").trim();
        return truncate_single_line(&format!("{language} · {first}"), 60);
    }
    let value = serde_json::from_str::<serde_json::Value>(args).ok();
    let field = |keys: &[&str]| -> Option<String> {
        let value = value.as_ref()?;
        keys.iter().find_map(|key| {
            value
                .get(*key)
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
    };
    let target = match name {
        "code-read_file" | "code-write_file" | "code-edit_file" => {
            field(&["path", "file", "file_path"])
        }
        "code-list_dir" => field(&["path", "dir"]),
        "code-grep" => field(&["query", "pattern", "regex"]),
        "code-glob" => field(&["query", "pattern", "glob"]),
        "code-shell" => field(&["command", "cmd"]),
        _ => None,
    };
    match target {
        Some(target) if !target.trim().is_empty() => truncate_single_line(target.trim(), 60),
        _ => truncate_single_line(args.trim(), 60),
    }
}

pub(super) fn tool_path(tc: &ToolCallEntry) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(&tc.args)
        .ok()
        .and_then(|value| {
            ["path", "file", "file_path"].iter().find_map(|key| {
                value
                    .get(*key)
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            })
        })
        .or_else(|| {
            tc.output.as_deref().and_then(|output| {
                output.lines().find_map(|line| {
                    line.strip_prefix("+++ b/")
                        .or_else(|| line.strip_prefix("+++ "))
                        .map(str::to_string)
                })
            })
        })
}

pub(super) fn edit_diff(tc: &ToolCallEntry) -> Option<Vec<crate::diff::UnifiedLine>> {
    meerai_host::diff::tool_edit_diff(&tc.name, &tc.args, tc.output.as_deref())
}

pub(super) fn render_edit_diff(
    lines: &mut Vec<TaggedTranscriptLine>,
    diff: &[crate::diff::UnifiedLine],
    language: Option<crate::syntax::Language>,
    idx: usize,
    memo: &mut TranscriptBuildMemo,
) {
    use crate::diff::UnifiedLineKind;
    const MAX_DIFF_LINES: usize = 200;
    let body: Vec<&crate::diff::UnifiedLine> = diff
        .iter()
        .filter(|line| line.kind != UnifiedLineKind::FileHeader)
        .collect();
    let visible = body.len().min(MAX_DIFF_LINES);
    let source = body
        .iter()
        .take(visible)
        .map(|line| {
            if line.kind == UnifiedLineKind::HunkHeader {
                ""
            } else {
                line.text.as_str()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let tokens = memo.tokens(idx, language, &source, || {
        crate::syntax::token_lines(language, &source)
    });

    for (position, line) in body.iter().take(visible).enumerate() {
        let line_tokens = tokens.get(position).map(Vec::as_slice).unwrap_or(&[]);
        let tagged = match line.kind {
            UnifiedLineKind::HunkHeader => with_hover(
                banded_line(
                    Line::from(vec![
                        Span::styled("  ", Style::default()),
                        Span::styled(line.text.clone(), Style::default().fg(theme::p().faint)),
                    ]),
                    Some(idx),
                    4,
                ),
                idx,
            ),
            UnifiedLineKind::Context => {
                code_diff_line(idx, " ", theme::p().diff_context, line_tokens, &line.text)
            }
            UnifiedLineKind::Added => {
                code_diff_line(idx, "+", theme::p().diff_added, line_tokens, &line.text)
            }
            UnifiedLineKind::Removed => {
                code_diff_line(idx, "-", theme::p().diff_removed, line_tokens, &line.text)
            }
            UnifiedLineKind::FileHeader => continue,
        };
        lines.push(tagged);
    }

    if body.len() > visible {
        lines.push(nested_line(idx, "…".to_string(), true));
    }
}

pub(super) fn code_diff_line(
    idx: usize,
    sign: &str,
    color: Color,
    tokens: &[crate::syntax::Token],
    text: &str,
) -> TaggedTranscriptLine {
    let base = Style::default().fg(color);
    let mut spans = vec![
        Span::styled("  ", Style::default()),
        Span::styled(format!("{sign} "), Style::default().fg(color)),
        Span::styled("  ", Style::default()),
    ];
    if tokens.is_empty() {
        spans.push(Span::styled(text.to_string(), base));
    } else {
        spans.extend(crate::syntax::line_spans(tokens, base));
    }
    with_hover(banded_line(Line::from(spans), Some(idx), 4), idx)
}
