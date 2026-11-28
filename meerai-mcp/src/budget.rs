use crate::tool::McpTool;

pub const DEFAULT_FULL_BUDGET_TOKENS: usize = 2_000;
pub const INDEX_ENTRY_CHARS: usize = 120;
pub const DEFAULT_MAX_LOADED_TOOLS: usize = 12;
pub const DEFAULT_MAX_LOADED_TOKENS: usize = 4_000;

const CHARS_PER_TOKEN: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionMode {
    Full,
    Index,
}

impl AdmissionMode {
    pub fn is_indexed(self) -> bool {
        matches!(self, Self::Index)
    }
}

pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(CHARS_PER_TOKEN)
}

pub fn value_tokens(value: &serde_json::Value) -> usize {
    estimate_tokens(&value.to_string())
}

pub fn tool_tokens(tool: &McpTool) -> usize {
    estimate_tokens(&tool.runtime_name())
        + estimate_tokens(&tool.description)
        + value_tokens(&tool.schema)
}

pub fn total_tokens(tools: &[McpTool]) -> usize {
    tools.iter().map(tool_tokens).sum()
}

pub fn select_mode(tokens: usize, budget: usize) -> AdmissionMode {
    if tokens <= budget {
        AdmissionMode::Full
    } else {
        AdmissionMode::Index
    }
}

pub fn index_entry(tool: &McpTool) -> String {
    let summary = summary_of(&tool.description);
    let name = tool.runtime_name();
    if summary.is_empty() {
        return truncate(&name, INDEX_ENTRY_CHARS);
    }
    truncate(&format!("{name}: {summary}"), INDEX_ENTRY_CHARS)
}

fn summary_of(description: &str) -> String {
    let first_line = description
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let sentence_end = first_line.find(". ").map(|index| index + 1);
    match sentence_end {
        Some(end) => first_line[..end].to_string(),
        None => first_line.to_string(),
    }
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub const TRUNCATION_MARKER: &str = "\n…more tools available";

pub fn render_index(tools: &[McpTool], limit_chars: usize) -> String {
    let entries: Vec<String> = tools.iter().map(index_entry).collect();
    let full_len: usize = entries
        .iter()
        .map(|entry| entry.chars().count())
        .sum::<usize>()
        + entries.len().saturating_sub(1);
    let fits = full_len <= limit_chars;

    let budget = if fits {
        limit_chars
    } else {
        limit_chars.saturating_sub(TRUNCATION_MARKER.chars().count())
    };

    let mut out = String::new();
    for entry in &entries {
        let entry_len = entry.chars().count();
        let addition = if out.is_empty() {
            entry_len
        } else {
            entry_len + 1
        };
        if out.chars().count() + addition > budget {
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(entry);
    }

    if !fits {
        out.push_str(TRUNCATION_MARKER);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, description: &str, schema_bytes: usize) -> McpTool {
        McpTool {
            server: "srv".to_string(),
            name: name.to_string(),
            description: description.to_string(),
            schema: serde_json::json!({
                "type": "object",
                "description": "x".repeat(schema_bytes)
            }),
            read_only: false,
        }
    }

    #[test]
    fn a_small_surface_stays_full() {
        let tools = vec![tool("a", "short", 10), tool("b", "short", 10)];
        let tokens = total_tokens(&tools);
        assert_eq!(
            select_mode(tokens, DEFAULT_FULL_BUDGET_TOKENS),
            AdmissionMode::Full
        );
    }

    #[test]
    fn a_large_surface_collapses_to_an_index() {
        let tools = vec![
            tool("a", "short", 4_000),
            tool("b", "short", 4_000),
            tool("c", "short", 4_000),
        ];
        let tokens = total_tokens(&tools);
        assert!(tokens > DEFAULT_FULL_BUDGET_TOKENS, "{tokens}");
        assert_eq!(
            select_mode(tokens, DEFAULT_FULL_BUDGET_TOKENS),
            AdmissionMode::Index
        );
    }

    #[test]
    fn the_boundary_is_inclusive() {
        assert_eq!(select_mode(100, 100), AdmissionMode::Full);
        assert_eq!(select_mode(101, 100), AdmissionMode::Index);
    }

    #[test]
    fn index_entries_are_one_line_and_capped() {
        let long = tool("search_issues", &"detail ".repeat(100), 0);
        let entry = index_entry(&long);
        assert!(
            entry.starts_with("mcp__srv__search_issues: detail"),
            "{entry}"
        );
        assert!(entry.chars().count() <= INDEX_ENTRY_CHARS, "{entry}");
        assert!(!entry.contains('\n'));
    }

    #[test]
    fn index_entries_drop_empty_descriptions() {
        let bare = tool("ping", "", 0);
        assert_eq!(index_entry(&bare), "mcp__srv__ping");
    }

    #[test]
    fn the_rendered_index_is_bounded_and_marks_truncation() {
        let tools: Vec<McpTool> = (0..40)
            .map(|index| tool(&format!("tool_{index:02}"), "does a thing", 0))
            .collect();
        let rendered = render_index(&tools, 400);
        assert!(
            rendered.chars().count() <= 400,
            "{}",
            rendered.chars().count()
        );
        assert!(rendered.contains("more tools available"), "{rendered}");
        assert!(rendered.contains("mcp__srv__tool_00"));
    }

    #[test]
    fn a_small_index_has_no_truncation_marker() {
        let tools = vec![tool("a", "one", 0), tool("b", "two", 0)];
        let rendered = render_index(&tools, 4_000);
        assert!(!rendered.contains("more tools"));
        assert_eq!(rendered.lines().count(), 2);
    }

    #[test]
    fn token_estimate_is_about_quarter_length() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }
}
