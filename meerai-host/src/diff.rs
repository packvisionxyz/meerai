use std::fs;
use std::path::Path;

use git2::{Repository, Status, StatusOptions};
use serde::{Deserialize, Serialize};
use similar::{DiffOp, TextDiff};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChangeEntry {
    pub status: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "data")]
pub enum DiffRowKind {
    HunkHeader(String),
    Context,
    Modified,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffCell {
    pub line_no: Option<usize>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffRow {
    pub kind: DiffRowKind,
    pub left: Option<DiffCell>,
    pub right: Option<DiffCell>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiffView {
    pub path: String,
    pub status: String,
    pub additions: usize,
    pub deletions: usize,
    pub rows: Vec<DiffRow>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnifiedLineKind {
    FileHeader,
    HunkHeader,
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnifiedLine {
    pub kind: UnifiedLineKind,
    pub text: String,
}

pub fn parse_unified_diff(text: &str) -> Vec<UnifiedLine> {
    let mut lines = Vec::new();
    for line in text.lines() {
        let parsed = if line.starts_with("--- ") || line.starts_with("+++ ") {
            UnifiedLine {
                kind: UnifiedLineKind::FileHeader,
                text: line.to_string(),
            }
        } else if line.starts_with("@@") {
            UnifiedLine {
                kind: UnifiedLineKind::HunkHeader,
                text: line.to_string(),
            }
        } else if let Some(rest) = line.strip_prefix('+') {
            UnifiedLine {
                kind: UnifiedLineKind::Added,
                text: rest.to_string(),
            }
        } else if let Some(rest) = line.strip_prefix('-') {
            UnifiedLine {
                kind: UnifiedLineKind::Removed,
                text: rest.to_string(),
            }
        } else if let Some(rest) = line.strip_prefix(' ') {
            UnifiedLine {
                kind: UnifiedLineKind::Context,
                text: rest.to_string(),
            }
        } else {
            continue;
        };
        lines.push(parsed);
    }
    lines
}

pub fn diff_counts(lines: &[UnifiedLine]) -> (usize, usize) {
    let added = lines
        .iter()
        .filter(|line| line.kind == UnifiedLineKind::Added)
        .count();
    let removed = lines
        .iter()
        .filter(|line| line.kind == UnifiedLineKind::Removed)
        .count();
    (added, removed)
}

pub fn build_edit_diff(path: &str, old: &str, new: &str) -> Vec<UnifiedLine> {
    if old == new {
        return Vec::new();
    }
    let unified = TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string();
    parse_unified_diff(&unified)
}

pub fn tool_edit_diff(name: &str, args: &str, output: Option<&str>) -> Option<Vec<UnifiedLine>> {
    if !matches!(name, "code-write_file" | "code-edit_file") {
        return None;
    }
    if let Some(output) = output {
        let parsed = parse_unified_diff(output);
        if !parsed.is_empty() {
            return Some(parsed);
        }
    }
    let value = serde_json::from_str::<serde_json::Value>(args).ok()?;
    let path = value.get("path").and_then(|v| v.as_str())?;
    let old = value
        .get("old_string")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let new = value
        .get("new_string")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("content").and_then(|v| v.as_str()))?;
    let lines = build_edit_diff(path, old, new);
    (!lines.is_empty()).then_some(lines)
}

pub fn list_git_changes(workspace_root: &Path) -> Vec<FileChangeEntry> {
    let Ok(repo) = Repository::discover(workspace_root) else {
        return Vec::new();
    };
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(true)
        .include_ignored(false);

    let Ok(statuses) = repo.statuses(Some(&mut opts)) else {
        return Vec::new();
    };

    let mut entries = Vec::new();
    for entry in statuses.iter() {
        let status = entry.status();
        if status.is_ignored() {
            continue;
        }
        let Some(code) = status_label(status) else {
            continue;
        };
        let path = if status.is_index_renamed()
            && let Some(delta) = entry.head_to_index()
            && let (Some(old), Some(new)) = (delta.old_file().path(), delta.new_file().path())
        {
            format!("{} -> {}", old.to_string_lossy(), new.to_string_lossy())
        } else if let Ok(p) = entry.path() {
            p.to_string()
        } else {
            continue;
        };
        entries.push(FileChangeEntry {
            status: code.to_string(),
            path,
        });
    }
    entries
}

fn status_label(status: Status) -> Option<&'static str> {
    if status.is_conflicted() {
        return Some("U");
    }
    let index_flags = Status::INDEX_NEW
        | Status::INDEX_MODIFIED
        | Status::INDEX_DELETED
        | Status::INDEX_RENAMED
        | Status::INDEX_TYPECHANGE;
    if status.is_wt_new() && !status.intersects(index_flags) {
        return Some("??");
    }
    if status.is_index_new() {
        return Some("A");
    }
    if status.is_index_renamed() || status.is_wt_renamed() {
        return Some("R");
    }
    if status.is_wt_deleted() || (status.is_index_deleted() && !status.is_wt_new()) {
        return Some("D");
    }
    if status.intersects(
        Status::INDEX_MODIFIED
            | Status::INDEX_TYPECHANGE
            | Status::WT_MODIFIED
            | Status::WT_TYPECHANGE,
    ) {
        return Some("M");
    }
    None
}

pub fn build_file_diff(workspace_root: &Path, path: &str, status: &str) -> FileDiffView {
    let (old_rel, new_rel) = path.split_once(" -> ").unwrap_or((path, path));
    let repo = Repository::discover(workspace_root).ok();
    let repo_root = repo
        .as_ref()
        .and_then(|r| r.workdir())
        .map(Path::to_path_buf);

    let resolve_wt_path = |rel: &str| {
        if let Some(root) = &repo_root {
            let candidate = root.join(rel);
            if candidate.exists() || !workspace_root.join(rel).exists() {
                return candidate;
            }
        }
        workspace_root.join(rel)
    };

    if status == "??" {
        return build_untracked_or_fallback_diff(&resolve_wt_path(new_rel), path, status);
    }

    let Some(repo) = repo else {
        return build_untracked_or_fallback_diff(&workspace_root.join(new_rel), path, status);
    };

    let head_bytes = read_head_blob(&repo, old_rel);
    let index_bytes = read_index_blob(&repo, new_rel).or_else(|| read_index_blob(&repo, old_rel));
    let wt_bytes = fs::read(resolve_wt_path(new_rel)).ok();

    let pair = match (head_bytes, wt_bytes) {
        (Some(head), Some(wt)) if head != wt => Some((head, wt)),
        (Some(head), Some(_)) if index_bytes.as_deref() != Some(head.as_slice()) => {
            Some((head, index_bytes.unwrap_or_default()))
        }
        (Some(head), None) => Some((head, Vec::new())),
        (None, Some(wt)) => Some((Vec::new(), wt)),
        (None, None) => index_bytes.map(|idx| (Vec::new(), idx)),
        _ => None,
    };

    if let Some((old_bytes, new_bytes)) = pair {
        let (Ok(old_text), Ok(new_text)) =
            (decode_text_bytes(&old_bytes), decode_text_bytes(&new_bytes))
        else {
            return FileDiffView {
                path: path.to_string(),
                status: status.to_string(),
                additions: 0,
                deletions: 0,
                rows: vec![notice_row("(binary)", "(binary file changed)")],
            };
        };

        let (rows, additions, deletions) = diff_texts(old_text, new_text);
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

    build_untracked_or_fallback_diff(&resolve_wt_path(new_rel), path, status)
}

fn read_head_blob(repo: &Repository, rel: &str) -> Option<Vec<u8>> {
    let head = repo.head().ok()?;
    let tree = head.peel_to_tree().ok()?;
    let entry = tree.get_path(Path::new(rel)).ok()?;
    let blob = repo.find_blob(entry.id()).ok()?;
    Some(blob.content().to_vec())
}

fn read_index_blob(repo: &Repository, rel: &str) -> Option<Vec<u8>> {
    let index = repo.index().ok()?;
    let entry = index.get_path(Path::new(rel), 0)?;
    let blob = repo.find_blob(entry.id).ok()?;
    Some(blob.content().to_vec())
}

fn decode_text_bytes(bytes: &[u8]) -> Result<&str, ()> {
    if bytes.contains(&0) {
        return Err(());
    }
    std::str::from_utf8(bytes).map_err(|_| ())
}

fn notice_row(left: &str, right: &str) -> DiffRow {
    DiffRow {
        kind: DiffRowKind::Context,
        left: Some(DiffCell {
            line_no: None,
            text: left.to_string(),
        }),
        right: Some(DiffCell {
            line_no: None,
            text: right.to_string(),
        }),
    }
}

fn build_untracked_or_fallback_diff(
    full_path: &Path,
    display_path: &str,
    status: &str,
) -> FileDiffView {
    match fs::read_to_string(full_path) {
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
                rows.push(notice_row("(empty file)", "(empty file)"));
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
            rows: vec![notice_row(
                "(unavailable)",
                "(file not readable or deleted)",
            )],
        },
    }
}

pub fn diff_texts(old: &str, new: &str) -> (Vec<DiffRow>, usize, usize) {
    let diff = TextDiff::from_lines(old, new);
    let mut rows = Vec::new();
    let mut additions = 0usize;
    let mut deletions = 0usize;

    for group in diff.grouped_ops(3) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let old_start = first.old_range().start;
        let old_end = last.old_range().end;
        let new_start = first.new_range().start;
        let new_end = last.new_range().end;
        let old_len = old_end - old_start;
        let new_len = new_end - new_start;
        let header = format!(
            "@@ -{},{} +{},{} @@",
            if old_len == 0 {
                old_start
            } else {
                old_start + 1
            },
            old_len,
            if new_len == 0 {
                new_start
            } else {
                new_start + 1
            },
            new_len,
        );
        rows.push(DiffRow {
            kind: DiffRowKind::HunkHeader(header),
            left: None,
            right: None,
        });

        for op in group {
            match op {
                DiffOp::Equal {
                    old_index,
                    new_index,
                    len,
                } => {
                    for offset in 0..len {
                        let old_ln = old_index + offset;
                        let new_ln = new_index + offset;
                        rows.push(DiffRow {
                            kind: DiffRowKind::Context,
                            left: Some(DiffCell {
                                line_no: Some(old_ln + 1),
                                text: trim_line_end(diff.old_slice(old_ln).unwrap_or_default())
                                    .to_string(),
                            }),
                            right: Some(DiffCell {
                                line_no: Some(new_ln + 1),
                                text: trim_line_end(diff.new_slice(new_ln).unwrap_or_default())
                                    .to_string(),
                            }),
                        });
                    }
                }
                DiffOp::Delete {
                    old_index, old_len, ..
                } => {
                    for offset in 0..old_len {
                        let old_ln = old_index + offset;
                        deletions += 1;
                        rows.push(DiffRow {
                            kind: DiffRowKind::Removed,
                            left: Some(DiffCell {
                                line_no: Some(old_ln + 1),
                                text: trim_line_end(diff.old_slice(old_ln).unwrap_or_default())
                                    .to_string(),
                            }),
                            right: None,
                        });
                    }
                }
                DiffOp::Insert {
                    new_index, new_len, ..
                } => {
                    for offset in 0..new_len {
                        let new_ln = new_index + offset;
                        additions += 1;
                        rows.push(DiffRow {
                            kind: DiffRowKind::Added,
                            left: None,
                            right: Some(DiffCell {
                                line_no: Some(new_ln + 1),
                                text: trim_line_end(diff.new_slice(new_ln).unwrap_or_default())
                                    .to_string(),
                            }),
                        });
                    }
                }
                DiffOp::Replace {
                    old_index,
                    old_len,
                    new_index,
                    new_len,
                } => {
                    let pair_count = old_len.max(new_len);
                    for offset in 0..pair_count {
                        let left = (offset < old_len).then(|| DiffCell {
                            line_no: Some(old_index + offset + 1),
                            text: trim_line_end(
                                diff.old_slice(old_index + offset).unwrap_or_default(),
                            )
                            .to_string(),
                        });
                        let right = (offset < new_len).then(|| DiffCell {
                            line_no: Some(new_index + offset + 1),
                            text: trim_line_end(
                                diff.new_slice(new_index + offset).unwrap_or_default(),
                            )
                            .to_string(),
                        });
                        let kind = match (&left, &right) {
                            (Some(_), Some(_)) => {
                                deletions += 1;
                                additions += 1;
                                DiffRowKind::Modified
                            }
                            (Some(_), None) => {
                                deletions += 1;
                                DiffRowKind::Removed
                            }
                            (None, Some(_)) => {
                                additions += 1;
                                DiffRowKind::Added
                            }
                            (None, None) => continue,
                        };
                        rows.push(DiffRow { kind, left, right });
                    }
                }
            }
        }
    }

    (rows, additions, deletions)
}

fn trim_line_end(line: &str) -> &str {
    line.trim_end_matches(&['\r', '\n'][..])
}

pub fn current_branch(root: &Path) -> Option<String> {
    let repo = git2::Repository::discover(root).ok()?;
    let head = repo.head().ok()?;
    head.shorthand().ok().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::Signature;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "meerai_diff_{label}_{}_{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn init_repo_with_commit(root: &Path, files: &[(&str, &str)]) -> Repository {
        let repo = Repository::init(root).unwrap();
        let sig = Signature::now("Test", "test@example.com").unwrap();
        let mut index = repo.index().unwrap();
        for (rel, body) in files {
            let full = root.join(rel);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(full, body).unwrap();
            index.add_path(Path::new(rel)).unwrap();
        }
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        drop(tree);
        repo
    }

    #[test]
    fn diff_texts_pairs_modified_and_added_lines() {
        let old = "let a = 1;\nlet b = 2;\nreturn a + b;\n";
        let new = "let a = 1;\nlet b = 20;\nlet c = 30;\nreturn a + b;\n";
        let (rows, additions, deletions) = diff_texts(old, new);
        assert_eq!(additions, 2);
        assert_eq!(deletions, 1);
        assert_eq!(rows.len(), 5);
        assert!(matches!(rows[0].kind, DiffRowKind::HunkHeader(_)));
        assert_eq!(rows[1].kind, DiffRowKind::Context);
        assert_eq!(rows[1].left.as_ref().unwrap().line_no, Some(1));
        assert_eq!(rows[1].right.as_ref().unwrap().line_no, Some(1));

        assert_eq!(rows[2].kind, DiffRowKind::Modified);
        assert_eq!(rows[2].left.as_ref().unwrap().text, "let b = 2;");
        assert_eq!(rows[2].right.as_ref().unwrap().text, "let b = 20;");

        assert_eq!(rows[3].kind, DiffRowKind::Added);
        assert!(rows[3].left.is_none());
        assert_eq!(rows[3].right.as_ref().unwrap().line_no, Some(3));
        assert_eq!(rows[3].right.as_ref().unwrap().text, "let c = 30;");

        assert_eq!(rows[4].kind, DiffRowKind::Context);
        assert_eq!(rows[4].left.as_ref().unwrap().line_no, Some(3));
        assert_eq!(rows[4].right.as_ref().unwrap().line_no, Some(4));
    }

    #[test]
    fn build_file_diff_reads_untracked_file() {
        let root = temp_dir("untracked");
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

    #[test]
    fn list_changes_and_build_file_diff_use_git2_and_similar() {
        let root = temp_dir("git2");
        let repo = init_repo_with_commit(
            &root,
            &[("src/lib.rs", "one\ntwo\nthree\n"), ("old.txt", "bye\n")],
        );

        fs::write(root.join("src/lib.rs"), "one\nTWO\nthree\nfour\n").unwrap();
        fs::write(root.join("new.txt"), "added line\n").unwrap();
        fs::write(root.join("notes.md"), "untracked\n").unwrap();
        fs::remove_file(root.join("old.txt")).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("new.txt")).unwrap();
        index.write().unwrap();

        let changes = list_git_changes(&root);
        assert!(changes.contains(&FileChangeEntry {
            status: "M".to_string(),
            path: "src/lib.rs".to_string(),
        }));
        assert!(changes.contains(&FileChangeEntry {
            status: "A".to_string(),
            path: "new.txt".to_string(),
        }));
        assert!(changes.contains(&FileChangeEntry {
            status: "D".to_string(),
            path: "old.txt".to_string(),
        }));
        assert!(changes.contains(&FileChangeEntry {
            status: "??".to_string(),
            path: "notes.md".to_string(),
        }));

        let modified = build_file_diff(&root, "src/lib.rs", "M");
        assert_eq!(modified.additions, 2);
        assert_eq!(modified.deletions, 1);
        assert!(
            modified
                .rows
                .iter()
                .any(|row| row.kind == DiffRowKind::Modified
                    && row.left.as_ref().is_some_and(|c| c.text == "two")
                    && row.right.as_ref().is_some_and(|c| c.text == "TWO"))
        );

        let added = build_file_diff(&root, "new.txt", "A");
        assert_eq!(added.additions, 1);
        assert_eq!(added.deletions, 0);

        let deleted = build_file_diff(&root, "old.txt", "D");
        assert_eq!(deleted.additions, 0);
        assert_eq!(deleted.deletions, 1);

        let _ = fs::remove_dir_all(&root);
    }
}
