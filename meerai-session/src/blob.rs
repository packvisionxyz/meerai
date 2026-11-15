use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use anyhow::Result;
use meerai_core::text::{ceil_char_boundary, floor_char_boundary};

pub(crate) const BLOB_DIR_NAME: &str = "blobs";

#[derive(Clone)]
pub struct BlobStore {
    dir: PathBuf,
}

impl BlobStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn for_session(session_dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: session_dir.into().join(BLOB_DIR_NAME),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn put(&self, content: &str) -> Result<PathBuf> {
        crate::perms::create_private_dir_blocking(&self.dir)?;
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        let name = format!("{:016x}-{}.txt", hasher.finish(), content.len());
        let path = self.dir.join(name);
        crate::perms::write_private_blocking(&path, content.as_bytes())?;
        Ok(path)
    }
}

pub struct SpillOutcome {
    pub retained: String,
    pub bytes_omitted: u64,
    pub blob_path: Option<PathBuf>,
    pub warning: Option<String>,
}

pub fn estimate_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

pub async fn apply_spill(text: &str, budget_tokens: u64, blobs: &BlobStore) -> SpillOutcome {
    if estimate_tokens(text) <= budget_tokens {
        return SpillOutcome {
            retained: text.to_string(),
            bytes_omitted: 0,
            blob_path: None,
            warning: None,
        };
    }

    let budget_bytes = budget_tokens.saturating_mul(4) as usize;
    let head_end = head_boundary(text, budget_bytes / 2);
    let tail_start = tail_boundary(text, budget_bytes - budget_bytes / 2).max(head_end);
    let middle = &text[head_end..tail_start];
    if middle.is_empty() {
        return SpillOutcome {
            retained: text.to_string(),
            bytes_omitted: 0,
            blob_path: None,
            warning: None,
        };
    }

    let bytes_omitted = middle.len() as u64;
    let stored = tokio::task::spawn_blocking({
        let blobs = blobs.clone();
        let content = text.to_string();
        move || blobs.put(&content)
    })
    .await;

    match stored {
        Ok(Ok(path)) => {
            let notice = format!(
                "(Omitted {bytes_omitted} bytes. Full result stored at: {} — use read_file with \
                 offset/limit, or grep this path, to inspect it.)",
                path.display()
            );
            let mut retained =
                String::with_capacity(head_end + notice.len() + text.len() - tail_start);
            retained.push_str(&text[..head_end]);
            if !retained.is_empty() && !retained.ends_with('\n') {
                retained.push('\n');
            }
            retained.push_str(&notice);
            retained.push('\n');
            retained.push_str(&text[tail_start..]);
            SpillOutcome {
                retained,
                bytes_omitted,
                blob_path: Some(path),
                warning: None,
            }
        }
        Ok(Err(err)) => SpillOutcome {
            retained: text.to_string(),
            bytes_omitted: 0,
            blob_path: None,
            warning: Some(format!("failed to store spilled output: {err}")),
        },
        Err(err) => SpillOutcome {
            retained: text.to_string(),
            bytes_omitted: 0,
            blob_path: None,
            warning: Some(format!("failed to store spilled output: {err}")),
        },
    }
}

fn head_boundary(text: &str, limit: usize) -> usize {
    if text.len() <= limit {
        return text.len();
    }
    let mut end = 0;
    for line in text.split_inclusive('\n') {
        let next = end + line.len();
        if next > limit {
            break;
        }
        end = next;
    }
    if end == 0 {
        floor_char_boundary(text, limit)
    } else {
        end
    }
}

fn tail_boundary(text: &str, limit: usize) -> usize {
    if text.len() <= limit {
        return 0;
    }
    let target = ceil_char_boundary(text, text.len() - limit);
    match text[target..].find('\n') {
        Some(offset) => target + offset + 1,
        None => target,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::SessionId;

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("meerai-blob-{label}-{}", SessionId::new()))
    }

    fn sample(lines: usize) -> String {
        (0..lines)
            .map(|index| format!("line {index} with some padded content for spilling\n"))
            .collect()
    }

    #[test]
    fn estimates_tokens_from_characters() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_tokens("ééééé"), 2);
    }

    #[tokio::test]
    async fn small_results_pass_through_untouched() {
        let root = temp_root("small");
        let blobs = BlobStore::for_session(&root);
        let text = "short output\n";

        let outcome = apply_spill(text, 64, &blobs).await;

        assert_eq!(outcome.retained, text);
        assert_eq!(outcome.bytes_omitted, 0);
        assert!(outcome.blob_path.is_none());
        assert!(outcome.warning.is_none());
    }

    #[tokio::test]
    async fn large_results_keep_head_and_tail_and_store_blob() {
        let root = temp_root("large");
        let blobs = BlobStore::for_session(&root);
        let text = sample(400);

        let outcome = apply_spill(&text, 40, &blobs).await;

        assert!(outcome.warning.is_none());
        assert!(outcome.bytes_omitted > 0);
        assert!(outcome.retained.starts_with("line 0 with"));
        assert!(
            outcome
                .retained
                .trim_end()
                .ends_with("content for spilling")
        );
        let path = outcome.blob_path.as_ref().unwrap();
        assert!(path.starts_with(blobs.dir()));
        assert!(
            outcome
                .retained
                .contains(&format!("(Omitted {} bytes. Full result stored at: {} — use read_file with offset/limit, or grep this path, to inspect it.)", outcome.bytes_omitted, path.display()))
        );
        let stored = std::fs::read_to_string(path).unwrap();
        assert_eq!(stored, text);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn storage_failure_fails_open() {
        let root = temp_root("failure");
        std::fs::create_dir_all(&root).unwrap();
        let blocked = root.join("blocked");
        std::fs::write(&blocked, "not a directory").unwrap();
        let blobs = BlobStore::new(&blocked);
        let text = sample(400);

        let outcome = apply_spill(&text, 40, &blobs).await;

        assert_eq!(outcome.retained, text);
        assert_eq!(outcome.bytes_omitted, 0);
        assert!(outcome.blob_path.is_none());
        assert!(outcome.warning.is_some());

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn head_and_tail_split_on_line_boundaries() {
        let text = "aa\nbb\ncc\ndd\n";
        assert_eq!(head_boundary(text, 7), 6);
        assert_eq!(tail_boundary(text, 7), 6);
    }
}
