use std::fs;
use std::path::Path;

use crate::completion::{MatchRank, score_candidate};
use crate::workspace::{LocalWorkspace, WorkspaceQuery};

const MAX_PREVIEW_LINES: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchMode {
    Files,
    Content,
}

impl SearchMode {
    pub fn toggle(self) -> Self {
        match self {
            Self::Files => Self::Content,
            Self::Content => Self::Files,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Files => "Files",
            Self::Content => "Content (rg)",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    pub path: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub snippet: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchPreview {
    pub title: String,
    pub lines: Vec<(usize, String, bool)>,
    pub scroll: usize,
}

const MAX_SEARCH_RESULTS: usize = 100;

pub fn discover_workspace_files(workspace: &dyn WorkspaceQuery) -> Vec<String> {
    workspace.list_files()
}

pub fn local_workspace_files(workspace_root: &Path) -> Vec<String> {
    LocalWorkspace::new(workspace_root).list_files()
}

pub fn search_files(files: &[String], query: &str) -> Vec<SearchMatch> {
    let trimmed = query.trim();
    let mut scored: Vec<(MatchRank, &str)> = files
        .iter()
        .filter_map(|path| score_candidate(path, trimmed).map(|rank| (rank, path.as_str())))
        .collect();

    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
    scored
        .into_iter()
        .take(MAX_SEARCH_RESULTS)
        .map(|(_, path)| SearchMatch {
            path: path.to_string(),
            line: None,
            column: None,
            snippet: String::new(),
        })
        .collect()
}

pub fn search_content(
    workspace: &dyn WorkspaceQuery,
    files: &[String],
    query: &str,
) -> Vec<SearchMatch> {
    workspace
        .search_content(files, query)
        .into_iter()
        .map(|hit| SearchMatch {
            path: hit.path,
            line: hit.line,
            column: hit.column,
            snippet: hit.snippet,
        })
        .collect()
}

pub fn build_search_preview(
    workspace_root: &Path,
    item: &SearchMatch,
    viewport_height: usize,
) -> SearchPreview {
    let title = match item.line {
        Some(ln) => format!("{}:{ln}", item.path),
        None => item.path.clone(),
    };
    let target_path = workspace_root.join(&item.path);
    let Ok(content) = fs::read_to_string(&target_path) else {
        return SearchPreview {
            title,
            lines: vec![(1, "(binary or unreadable file)".to_string(), false)],
            scroll: 0,
        };
    };

    let all_lines: Vec<&str> = content.lines().collect();
    if all_lines.is_empty() {
        return SearchPreview {
            title,
            lines: vec![(1, "(empty file)".to_string(), false)],
            scroll: 0,
        };
    }

    let focus_line = item.line.unwrap_or(1).clamp(1, all_lines.len());
    let half_window = MAX_PREVIEW_LINES / 2;
    let start_idx = focus_line.saturating_sub(1).saturating_sub(half_window);
    let end_idx = (start_idx + MAX_PREVIEW_LINES).min(all_lines.len());
    let start_idx = end_idx.saturating_sub(MAX_PREVIEW_LINES);

    let highlight_line = item.line.unwrap_or(0);
    let mut lines = Vec::with_capacity(end_idx - start_idx);
    let mut target_rel_idx = 0usize;

    for (offset, text) in all_lines[start_idx..end_idx].iter().enumerate() {
        let line_no = start_idx + offset + 1;
        let is_highlighted = line_no == highlight_line;
        if is_highlighted {
            target_rel_idx = offset;
        }
        lines.push((line_no, (*text).to_string(), is_highlighted));
    }

    let view_h = viewport_height.max(6);
    let max_scroll = lines.len().saturating_sub(view_h);
    let scroll = if item.line.is_some() {
        target_rel_idx.saturating_sub(view_h / 2).min(max_scroll)
    } else {
        0
    };

    SearchPreview {
        title,
        lines,
        scroll,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::LocalWorkspace;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempSearchDir {
        root: PathBuf,
    }

    impl TempSearchDir {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "meerai_search_test_{name}_{}_{}",
                std::process::id(),
                nanos
            ));
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write(&self, rel: &str, content: &str) {
            let path = self.root.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, content).unwrap();
        }
    }

    impl Drop for TempSearchDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn search_files_respects_gitignore_and_ranks_matches() {
        let ws = TempSearchDir::new("gitignore");
        ws.write(".gitignore", "ignored_dir/\n*.log\n");
        ws.write("ignored_dir/secret.rs", "fn secret() {}");
        ws.write("debug.log", "ignored log");
        ws.write("meerai-tui/src/app.rs", "pub struct AppState;");
        ws.write("meerai-tui/src/search.rs", "pub enum SearchMode {}");
        ws.write("README.md", "# meerai");

        let files = discover_workspace_files(&LocalWorkspace::new(&ws.root));
        assert!(files.contains(&"meerai-tui/src/app.rs".to_string()));
        assert!(files.contains(&"meerai-tui/src/search.rs".to_string()));
        assert!(!files.contains(&"ignored_dir/secret.rs".to_string()));
        assert!(!files.contains(&"debug.log".to_string()));

        let all = search_files(&files, "");
        assert!(all.len() >= 3);

        let hits = search_files(&files, "sear");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "meerai-tui/src/search.rs");
        assert!(hits[0].line.is_none());
    }

    #[test]
    fn search_content_uses_smart_case_regex_and_literal_fallback() {
        let ws = TempSearchDir::new("content");
        ws.write(
            "src/alpha.rs",
            "fn alpha() {\n    println!(\"needle[0\");\n}\n",
        );
        ws.write("src/beta.rs", "fn beta() {\n    let Needle = 42;\n}\n");

        let files = discover_workspace_files(&LocalWorkspace::new(&ws.root));
        assert_eq!(files, vec!["src/alpha.rs", "src/beta.rs"]);

        let smart_lower = search_content(&LocalWorkspace::new(&ws.root), &files, "needle");
        assert_eq!(smart_lower.len(), 2);
        assert_eq!(smart_lower[0].path, "src/alpha.rs");
        assert_eq!(smart_lower[0].line, Some(2));
        assert_eq!(smart_lower[0].column, Some(15));

        let smart_upper = search_content(&LocalWorkspace::new(&ws.root), &files, "Needle");
        assert_eq!(smart_upper.len(), 1);
        assert_eq!(smart_upper[0].path, "src/beta.rs");
        assert_eq!(smart_upper[0].line, Some(2));

        let regex_hits = search_content(&LocalWorkspace::new(&ws.root), &files, "let\\s+Needle");
        assert_eq!(regex_hits.len(), 1);
        assert_eq!(regex_hits[0].path, "src/beta.rs");

        let unclosed_regex_hits =
            search_content(&LocalWorkspace::new(&ws.root), &files, "needle[0");
        assert_eq!(unclosed_regex_hits.len(), 1);
        assert_eq!(unclosed_regex_hits[0].path, "src/alpha.rs");

        let preview = build_search_preview(&ws.root, &smart_upper[0], 10);
        assert_eq!(preview.title, "src/beta.rs:2");
        assert!(preview.lines.iter().any(|(ln, _, hi)| *ln == 2 && *hi));
    }
}
