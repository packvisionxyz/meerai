use std::path::Path;

use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::sinks::UTF8;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder};
use ignore::WalkBuilder;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentHit {
    pub path: String,
    pub line: usize,
    pub column: Option<usize>,
    pub text: String,
}

pub fn build_smart_matcher(pattern: &str, literal_fallback: bool) -> Result<RegexMatcher, String> {
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

pub fn build_searcher() -> Searcher {
    SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .line_number(true)
        .build()
}

pub fn search_file_with_matcher(
    searcher: &mut Searcher,
    matcher: &RegexMatcher,
    file: &Path,
    display_path: &str,
    max_per_file: usize,
    max_total_remaining: usize,
    capture_column: bool,
) -> (Vec<ContentHit>, bool) {
    let mut hits = Vec::new();
    let mut hit_file_limit = false;

    let _ = searcher.search_path(
        matcher,
        file,
        UTF8(|line_num, line| {
            let column = if capture_column {
                matcher
                    .find(line.as_bytes())
                    .ok()
                    .flatten()
                    .map(|m| line[..m.start().min(line.len())].chars().count() + 1)
            } else {
                None
            };
            hits.push(ContentHit {
                path: display_path.to_string(),
                line: line_num as usize,
                column,
                text: line.trim_end_matches(['\n', '\r']).to_string(),
            });
            if hits.len() >= max_per_file {
                hit_file_limit = true;
            }
            Ok(hits.len() < max_per_file && hits.len() < max_total_remaining)
        }),
    );

    (hits, hit_file_limit)
}

pub fn list_workspace_files(
    root: &Path,
    skipped_dirs: &[&str],
    max_depth: usize,
    max_files: usize,
) -> Vec<String> {
    let mut files = Vec::new();
    let skipped: Vec<String> = skipped_dirs.iter().map(|s| (*s).to_string()).collect();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .parents(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .require_git(false)
        .max_depth(Some(max_depth))
        .filter_entry(move |entry| {
            if entry.depth() == 0 {
                return true;
            }
            let name = entry.file_name().to_string_lossy();
            !skipped.iter().any(|skip| skip == name.as_ref())
        })
        .build();

    for entry in walker.flatten() {
        if files.len() >= max_files {
            break;
        }
        if entry.file_type().is_some_and(|ft| ft.is_file())
            && let Ok(rel) = entry.path().strip_prefix(root)
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
