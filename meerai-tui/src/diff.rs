use std::fs;
use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffRowKind {
    HunkHeader(String),
    Context,
    Modified,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffCell {
    pub line_no: Option<usize>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffRow {
    pub kind: DiffRowKind,
    pub left: Option<DiffCell>,
    pub right: Option<DiffCell>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiffView {
    pub path: String,
    pub status: String,
    pub additions: usize,
    pub deletions: usize,
    pub rows: Vec<DiffRow>,
}

pub fn build_file_diff(workspace_root: &Path, path: &str, status: &str) -> FileDiffView {
    let target_path = path.rsplit(" -> ").next().unwrap_or(path);
    if status != "??"
        && let Some(raw_diff) = run_git_diff(workspace_root, target_path)
    {
        let (rows, additions, deletions) = parse_unified_diff(&raw_diff);
        if !rows.is_empty() {
            return FileDiffView {
                path: path.to_string(),
                status: status.to_string(),
                additions,
                deletions,
                rows,
            };
        }
    }

    build_untracked_or_fallback_diff(workspace_root, target_path, path, status)
}

fn run_git_diff(workspace_root: &Path, path: &str) -> Option<String> {
    let candidates: [&[&str]; 3] = [
        &["diff", "--no-color", "-U3", "HEAD", "--", path],
        &["diff", "--no-color", "-U3", "--", path],
        &["diff", "--no-color", "-U3", "--cached", "--", path],
    ];

    for args in candidates {
        if let Ok(output) = Command::new("git")
            .args(args)
            .current_dir(workspace_root)
            .output()
            && output.status.success()
        {
            let text = String::from_utf8_lossy(&output.stdout).into_owned();
            if !text.trim().is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn build_untracked_or_fallback_diff(
    workspace_root: &Path,
    target_path: &str,
    display_path: &str,
    status: &str,
) -> FileDiffView {
    let full_path = workspace_root.join(target_path);
    match fs::read_to_string(&full_path) {
        Ok(content) => {
            let mut rows = Vec::new();
            for (idx, line) in content.lines().enumerate() {
                rows.push(DiffRow {
                    kind: DiffRowKind::Added,
                    left: None,
                    right: Some(DiffCell {
                        line_no: Some(idx + 1),
                        text: line.to_string(),
                    }),
                });
            }
            if rows.is_empty() {
                rows.push(DiffRow {
                    kind: DiffRowKind::Context,
                    left: Some(DiffCell {
                        line_no: None,
                        text: "(empty file)".to_string(),
                    }),
                    right: Some(DiffCell {
                        line_no: None,
                        text: "(empty file)".to_string(),
                    }),
                });
                return FileDiffView {
                    path: display_path.to_string(),
                    status: status.to_string(),
                    additions: 0,
                    deletions: 0,
                    rows,
                };
            }
            let additions = rows.len();
            FileDiffView {
                path: display_path.to_string(),
                status: status.to_string(),
                additions,
                deletions: 0,
                rows,
            }
        }
        Err(_) => FileDiffView {
            path: display_path.to_string(),
            status: status.to_string(),
            additions: 0,
            deletions: 0,
            rows: vec![DiffRow {
                kind: DiffRowKind::Context,
                left: Some(DiffCell {
                    line_no: None,
                    text: "(unavailable)".to_string(),
                }),
                right: Some(DiffCell {
                    line_no: None,
                    text: "(file not readable or deleted)".to_string(),
                }),
            }],
        },
    }
}

pub fn parse_unified_diff(raw_diff: &str) -> (Vec<DiffRow>, usize, usize) {
    let mut rows = Vec::new();
    let mut additions = 0usize;
    let mut deletions = 0usize;

    let mut in_hunk = false;
    let mut old_line = 1usize;
    let mut new_line = 1usize;
    let mut pending_removed: Vec<(usize, String)> = Vec::new();
    let mut pending_added: Vec<(usize, String)> = Vec::new();

    for line in raw_diff.lines() {
        if let Some((old_start, new_start)) = parse_hunk_header(line) {
            flush_pending(
                &mut rows,
                &mut pending_removed,
                &mut pending_added,
                &mut additions,
                &mut deletions,
            );
            in_hunk = true;
            old_line = old_start;
            new_line = new_start;
            rows.push(DiffRow {
                kind: DiffRowKind::HunkHeader(line.to_string()),
                left: None,
                right: None,
            });
            continue;
        }

        if !in_hunk {
            continue;
        }

        if let Some(rest) = line.strip_prefix('-') {
            pending_removed.push((old_line, rest.to_string()));
            old_line += 1;
        } else if let Some(rest) = line.strip_prefix('+') {
            pending_added.push((new_line, rest.to_string()));
            new_line += 1;
        } else if let Some(rest) = line.strip_prefix(' ') {
            flush_pending(
                &mut rows,
                &mut pending_removed,
                &mut pending_added,
                &mut additions,
                &mut deletions,
            );
            rows.push(DiffRow {
                kind: DiffRowKind::Context,
                left: Some(DiffCell {
                    line_no: Some(old_line),
                    text: rest.to_string(),
                }),
                right: Some(DiffCell {
                    line_no: Some(new_line),
                    text: rest.to_string(),
                }),
            });
            old_line += 1;
            new_line += 1;
        } else if line.starts_with("\\ No newline at end of file") {
            continue;
        } else if line.starts_with("diff ") || line.starts_with("index ") {
            flush_pending(
                &mut rows,
                &mut pending_removed,
                &mut pending_added,
                &mut additions,
                &mut deletions,
            );
            in_hunk = false;
        }
    }

    flush_pending(
        &mut rows,
        &mut pending_removed,
        &mut pending_added,
        &mut additions,
        &mut deletions,
    );

    (rows, additions, deletions)
}

fn flush_pending(
    rows: &mut Vec<DiffRow>,
    removed: &mut Vec<(usize, String)>,
    added: &mut Vec<(usize, String)>,
    additions: &mut usize,
    deletions: &mut usize,
) {
    let pair_count = removed.len().max(added.len());
    for i in 0..pair_count {
        let left = removed.get(i).map(|(ln, text)| DiffCell {
            line_no: Some(*ln),
            text: text.clone(),
        });
        let right = added.get(i).map(|(ln, text)| DiffCell {
            line_no: Some(*ln),
            text: text.clone(),
        });
        let kind = match (&left, &right) {
            (Some(_), Some(_)) => {
                *deletions += 1;
                *additions += 1;
                DiffRowKind::Modified
            }
            (Some(_), None) => {
                *deletions += 1;
                DiffRowKind::Removed
            }
            (None, Some(_)) => {
                *additions += 1;
                DiffRowKind::Added
            }
            (None, None) => continue,
        };
        rows.push(DiffRow { kind, left, right });
    }
    removed.clear();
    added.clear();
}

fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    let rest = line.strip_prefix("@@ -")?;
    let end_marker = rest.find(" @@")?;
    let range_part = &rest[..end_marker];
    let (old_part, new_part) = range_part.split_once(" +")?;
    let old_start = old_part
        .split_once(',')
        .map(|(s, _)| s)
        .unwrap_or(old_part)
        .parse::<usize>()
        .ok()?;
    let new_start = new_part
        .split_once(',')
        .map(|(s, _)| s)
        .unwrap_or(new_part)
        .parse::<usize>()
        .ok()?;
    Some((old_start, new_start))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parse_unified_diff_pairs_modified_and_added_lines() {
        let raw = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -10,4 +10,5 @@ pub fn demo() {
 let a = 1;
-let b = 2;
+let b = 20;
+let c = 30;
 return a + b;
";
        let (rows, additions, deletions) = parse_unified_diff(raw);
        assert_eq!(additions, 2);
        assert_eq!(deletions, 1);
        assert_eq!(rows.len(), 5);
        assert!(matches!(rows[0].kind, DiffRowKind::HunkHeader(_)));
        assert_eq!(rows[1].kind, DiffRowKind::Context);
        assert_eq!(rows[1].left.as_ref().unwrap().line_no, Some(10));
        assert_eq!(rows[1].right.as_ref().unwrap().line_no, Some(10));

        assert_eq!(rows[2].kind, DiffRowKind::Modified);
        assert_eq!(rows[2].left.as_ref().unwrap().text, "let b = 2;");
        assert_eq!(rows[2].right.as_ref().unwrap().text, "let b = 20;");

        assert_eq!(rows[3].kind, DiffRowKind::Added);
        assert!(rows[3].left.is_none());
        assert_eq!(rows[3].right.as_ref().unwrap().line_no, Some(12));
        assert_eq!(rows[3].right.as_ref().unwrap().text, "let c = 30;");

        assert_eq!(rows[4].kind, DiffRowKind::Context);
        assert_eq!(rows[4].left.as_ref().unwrap().line_no, Some(12));
        assert_eq!(rows[4].right.as_ref().unwrap().line_no, Some(13));
    }

    #[test]
    fn build_file_diff_reads_untracked_file() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("meerai_diff_test_{}_{}", std::process::id(), nanos));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("notes.txt"), "line one\nline two\n").unwrap();

        let diff = build_file_diff(&root, "notes.txt", "??");
        assert_eq!(diff.additions, 2);
        assert_eq!(diff.deletions, 0);
        assert_eq!(diff.rows.len(), 2);
        assert_eq!(diff.rows[0].kind, DiffRowKind::Added);
        assert_eq!(diff.rows[0].right.as_ref().unwrap().text, "line one");
        assert_eq!(diff.rows[1].right.as_ref().unwrap().line_no, Some(2));

        let _ = fs::remove_dir_all(&root);
    }
}
