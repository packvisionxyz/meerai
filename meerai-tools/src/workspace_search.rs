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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir() -> tempfile::TempDir {
        tempfile::TempDir::new().expect("tempdir")
    }

    fn write(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent");
        }
        fs::write(&path, body).expect("write");
        path
    }

    #[test]
    fn smart_matcher_accepts_a_valid_pattern() {
        let matcher = build_smart_matcher("fn \\w+", false).expect("matcher");
        assert!(matcher.find(b"fn main").expect("find").is_some());
    }

    #[test]
    fn smart_matcher_rejects_an_invalid_pattern_without_fallback() {
        let error = build_smart_matcher("fn (", false).expect_err("must fail");
        assert!(error.starts_with("invalid pattern:"), "{error}");
    }

    #[test]
    fn smart_matcher_escapes_an_invalid_pattern_when_fallback_is_allowed() {
        let matcher = build_smart_matcher("fn (", true).expect("literal matcher");
        assert!(matcher.find(b"called fn ( here").expect("find").is_some());
        assert!(matcher.find(b"fn x").expect("find").is_none());
    }

    #[test]
    fn smart_matcher_is_case_insensitive_for_a_lowercase_pattern() {
        let matcher = build_smart_matcher("alpha", false).expect("matcher");
        assert!(matcher.find(b"alpha").expect("find").is_some());
        assert!(matcher.find(b"ALPHA").expect("find").is_some());
        assert!(matcher.find(b"AlPhA").expect("find").is_some());
    }

    #[test]
    fn smart_matcher_is_case_sensitive_once_the_pattern_has_a_capital() {
        let matcher = build_smart_matcher("Alpha", false).expect("matcher");
        assert!(matcher.find(b"AlphaBeta").expect("find").is_some());
        assert!(matcher.find(b"alphabeta").expect("find").is_none());
    }

    #[test]
    fn search_collects_hits_with_line_numbers() {
        let dir = tempdir();
        let file = write(dir.path(), "a.txt", "one\nalpha here\nthree\n");
        let matcher = build_smart_matcher("alpha", false).expect("matcher");
        let mut searcher = build_searcher();
        let (hits, capped) =
            search_file_with_matcher(&mut searcher, &matcher, &file, "a.txt", 10, 10, false);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line, 2);
        assert_eq!(hits[0].text, "alpha here");
        assert_eq!(hits[0].path, "a.txt");
        assert_eq!(hits[0].column, None);
        assert!(!capped);
    }

    #[test]
    fn search_captures_a_one_based_column() {
        let dir = tempdir();
        let file = write(dir.path(), "a.txt", "xx alphayy\n");
        let matcher = build_smart_matcher("alpha", false).expect("matcher");
        let mut searcher = build_searcher();
        let (hits, _) =
            search_file_with_matcher(&mut searcher, &matcher, &file, "a.txt", 10, 10, true);
        assert_eq!(hits[0].column, Some(4));
    }

    #[test]
    fn search_stops_at_the_per_file_limit_and_reports_it() {
        let dir = tempdir();
        let body = "alpha\nalpha\nalpha\n";
        let file = write(dir.path(), "a.txt", body);
        let matcher = build_smart_matcher("alpha", false).expect("matcher");
        let mut searcher = build_searcher();
        let (hits, capped) =
            search_file_with_matcher(&mut searcher, &matcher, &file, "a.txt", 2, 100, false);
        assert_eq!(hits.len(), 2);
        assert!(capped, "the per-file cap must be reported");
    }

    #[test]
    fn search_trims_carriage_returns() {
        let dir = tempdir();
        let file = write(dir.path(), "a.txt", "alpha\r\n");
        let matcher = build_smart_matcher("alpha", false).expect("matcher");
        let mut searcher = build_searcher();
        let (hits, _) =
            search_file_with_matcher(&mut searcher, &matcher, &file, "a.txt", 10, 10, false);
        assert_eq!(hits[0].text, "alpha");
    }

    #[test]
    fn listing_returns_sorted_relative_paths() {
        let dir = tempdir();
        write(dir.path(), "b.rs", "");
        write(dir.path(), "a.rs", "");
        write(dir.path(), "nested/c.rs", "");
        let files = list_workspace_files(dir.path(), &[], 10, 100);
        assert_eq!(files, vec!["a.rs", "b.rs", "nested/c.rs"]);
    }

    #[test]
    fn listing_skips_named_directories() {
        let dir = tempdir();
        write(dir.path(), "keep.rs", "");
        write(dir.path(), "target/skip.rs", "");
        let files = list_workspace_files(dir.path(), &["target"], 10, 100);
        assert_eq!(files, vec!["keep.rs"]);
    }

    #[test]
    fn listing_honours_the_file_cap() {
        let dir = tempdir();
        for name in ["a.rs", "b.rs", "c.rs"] {
            write(dir.path(), name, "");
        }
        let files = list_workspace_files(dir.path(), &[], 10, 2);
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn listing_honours_the_depth_cap() {
        let dir = tempdir();
        write(dir.path(), "top.rs", "");
        write(dir.path(), "a/b/c/deep.rs", "");
        let files = list_workspace_files(dir.path(), &[], 2, 100);
        assert_eq!(files, vec!["top.rs"]);
    }

    #[test]
    fn listing_never_returns_directories() {
        let dir = tempdir();
        fs::create_dir_all(dir.path().join("empty")).expect("dir");
        write(dir.path(), "file.rs", "");
        let files = list_workspace_files(dir.path(), &[], 10, 100);
        assert_eq!(files, vec!["file.rs"]);
    }
}
