use std::path::{Path, PathBuf};
use std::sync::Arc;

use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::sinks::UTF8;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder};
use ignore::WalkBuilder;

const MAX_SCAN_DEPTH: usize = 16;
const MAX_SCAN_FILES: usize = 10_000;
const MAX_MATCHES_PER_FILE: usize = 50;
const MAX_SEARCH_RESULTS: usize = 100;
const SKIPPED_DIRS: &[&str] = &[".git", "target", "node_modules"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceMatch {
    pub path: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub snippet: String,
}

pub trait WorkspaceQuery: std::fmt::Debug + Send + Sync {
    fn list_files(&self) -> Vec<String>;
    fn search_content(&self, files: &[String], query: &str) -> Vec<WorkspaceMatch>;
}

#[derive(Clone, Debug)]
pub struct LocalWorkspace {
    root: PathBuf,
}

impl LocalWorkspace {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

pub fn local_workspace(root: impl Into<PathBuf>) -> Arc<dyn WorkspaceQuery> {
    Arc::new(LocalWorkspace::new(root))
}

fn build_matcher(pattern: &str, literal_fallback: bool) -> Result<RegexMatcher, String> {
    RegexMatcherBuilder::new()
        .case_smart(true)
        .line_terminator(Some(b'\n'))
        .build(pattern)
        .or_else(|error| {
            if !literal_fallback {
                return Err(format!("invalid pattern: {error}"));
            }
            RegexMatcherBuilder::new()
                .case_smart(true)
                .line_terminator(Some(b'\n'))
                .build(&regex::escape(pattern))
                .map_err(|err| format!("invalid pattern: {err}"))
        })
}

fn build_searcher() -> Searcher {
    SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .line_number(true)
        .build()
}

impl WorkspaceQuery for LocalWorkspace {
    fn list_files(&self) -> Vec<String> {
        let mut files = Vec::new();
        let skipped: Vec<String> = SKIPPED_DIRS.iter().map(|s| (*s).to_string()).collect();
        let walker = WalkBuilder::new(&self.root)
            .hidden(false)
            .parents(false)
            .git_ignore(true)
            .git_global(false)
            .git_exclude(true)
            .require_git(false)
            .max_depth(Some(MAX_SCAN_DEPTH))
            .filter_entry(move |entry| {
                if entry.depth() == 0 {
                    return true;
                }
                let name = entry.file_name().to_string_lossy();
                !skipped.iter().any(|skip| skip == name.as_ref())
            })
            .build();

        for entry in walker.flatten() {
            if files.len() >= MAX_SCAN_FILES {
                break;
            }
            if entry.file_type().is_some_and(|ft| ft.is_file())
                && let Ok(rel) = entry.path().strip_prefix(&self.root)
            {
                let normalized = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                if !normalized.is_empty() {
                    files.push(normalized);
                }
            }
        }

        files.sort();
        files.dedup();
        files
    }

    fn search_content(&self, files: &[String], query: &str) -> Vec<WorkspaceMatch> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        let Ok(matcher) = build_matcher(trimmed, true) else {
            return Vec::new();
        };
        let mut searcher = build_searcher();
        let mut matches = Vec::new();
        for rel in files {
            if matches.len() >= MAX_SEARCH_RESULTS {
                break;
            }
            let full_path = self.root.join(rel);
            let remaining = MAX_SEARCH_RESULTS.saturating_sub(matches.len());
            let mut hits = Vec::new();
            let _ = searcher.search_path(
                &matcher,
                &full_path,
                UTF8(|line_num, line| {
                    let column = matcher
                        .find(line.as_bytes())
                        .ok()
                        .flatten()
                        .map(|m| line[..m.start().min(line.len())].chars().count() + 1);
                    hits.push(WorkspaceMatch {
                        path: rel.clone(),
                        line: Some(line_num as usize),
                        column,
                        snippet: line.trim_end_matches(['\n', '\r']).to_string(),
                    });
                    Ok(hits.len() < MAX_MATCHES_PER_FILE && hits.len() < remaining)
                }),
            );
            matches.extend(hits);
        }
        matches
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempWorkspace {
        root: PathBuf,
    }

    impl TempWorkspace {
        fn new(name: &str) -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "meerai_tui_workspace_test_{name}_{}_{}",
                std::process::id(),
                unique
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

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn local_workspace_lists_and_searches_without_tool_crates() {
        let ws = TempWorkspace::new("local");
        ws.write(".gitignore", "target/\n");
        ws.write("src/alpha.rs", "fn alpha() {\n    let needle = 1;\n}\n");
        ws.write("target/skip.rs", "needle in target");

        let workspace = LocalWorkspace::new(&ws.root);
        let files = workspace.list_files();
        assert!(files.contains(&"src/alpha.rs".to_string()));
        assert!(!files.contains(&"target/skip.rs".to_string()));

        let hits = workspace.search_content(&files, "needle");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "src/alpha.rs");
        assert_eq!(hits[0].line, Some(2));
    }

    #[test]
    fn custom_workspace_query_supplies_results() {
        #[derive(Debug)]
        struct RemoteWorkspace;

        impl WorkspaceQuery for RemoteWorkspace {
            fn list_files(&self) -> Vec<String> {
                vec!["remote.rs".to_string()]
            }

            fn search_content(&self, _files: &[String], query: &str) -> Vec<WorkspaceMatch> {
                vec![WorkspaceMatch {
                    path: "remote.rs".to_string(),
                    line: Some(1),
                    column: Some(1),
                    snippet: query.to_string(),
                }]
            }
        }

        let workspace: Arc<dyn WorkspaceQuery> = Arc::new(RemoteWorkspace);
        assert_eq!(workspace.list_files(), vec!["remote.rs"]);
        assert_eq!(workspace.search_content(&[], "x")[0].snippet, "x");
    }
}
