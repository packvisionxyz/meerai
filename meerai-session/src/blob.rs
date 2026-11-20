use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use anyhow::{Context, Result};
use meerai_core::env::now_millis;
use meerai_core::text::{ceil_char_boundary, floor_char_boundary};
use meerai_store::Store;
use rusqlite::{OptionalExtension, params};

use crate::ids::SessionId;

#[derive(Clone)]
pub struct BlobStore {
    store: Store,
    session_id: SessionId,
}

impl BlobStore {
    pub fn new(store: Store, session_id: SessionId) -> Self {
        Self { store, session_id }
    }

    pub fn put(&self, content: &str) -> Result<String> {
        let key = blob_key(content);
        let compressed =
            zstd::stream::encode_all(content.as_bytes(), 3).context("compress spilled output")?;
        let byte_len = i64::try_from(content.len())?;
        self.store.immediate(|tx| {
            tx.execute(
                "INSERT INTO session_blobs (session_id, blob_key, byte_len, content_zstd, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(session_id, blob_key) DO UPDATE SET
                    byte_len = excluded.byte_len,
                    content_zstd = excluded.content_zstd,
                    created_at_ms = excluded.created_at_ms",
                params![
                    self.session_id.as_str(),
                    key,
                    byte_len,
                    compressed,
                    now_millis() as i64
                ],
            )?;
            Ok(())
        })?;
        Ok(key)
    }

    pub fn get(&self, key: &str) -> Result<Option<String>> {
        let stored: Option<Vec<u8>> = self.store.read(|conn| {
            Ok(conn
                .query_row(
                    "SELECT content_zstd FROM session_blobs WHERE session_id = ?1 AND blob_key = ?2",
                    params![self.session_id.as_str(), key],
                    |row| row.get(0),
                )
                .optional()?)
        })?;
        stored
            .map(|bytes| {
                let decoded = zstd::stream::decode_all(bytes.as_slice())
                    .context("decompress spilled output")?;
                String::from_utf8(decoded).context("spilled output is not valid UTF-8")
            })
            .transpose()
    }

    pub fn list(&self) -> Result<Vec<(String, u64)>> {
        self.store.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT blob_key, byte_len FROM session_blobs
                 WHERE session_id = ?1 ORDER BY created_at_ms DESC, blob_key",
            )?;
            let rows = stmt.query_map([self.session_id.as_str()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}

pub struct SpillOutcome {
    pub retained: String,
    pub bytes_omitted: u64,
    pub blob_key: Option<String>,
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
            blob_key: None,
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
            blob_key: None,
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
        Ok(Ok(key)) => {
            let notice = format!(
                "(Omitted {bytes_omitted} bytes. Full result stored as blob {key}; open the context \
                 blob viewer to inspect it.)"
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
                blob_key: Some(key),
                warning: None,
            }
        }
        Ok(Err(err)) => SpillOutcome {
            retained: text.to_string(),
            bytes_omitted: 0,
            blob_key: None,
            warning: Some(format!("failed to store spilled output: {err}")),
        },
        Err(err) => SpillOutcome {
            retained: text.to_string(),
            bytes_omitted: 0,
            blob_key: None,
            warning: Some(format!("failed to store spilled output: {err}")),
        },
    }
}

fn blob_key(content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}-{}.txt", hasher.finish(), content.len())
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

    fn sample(lines: usize) -> String {
        (0..lines)
            .map(|index| format!("line {index} with some padded content for spilling\n"))
            .collect()
    }

    fn blob_store() -> BlobStore {
        let store = Store::open(":memory:").unwrap();
        let session = SessionId::new();
        store
            .immediate(|tx| {
                tx.execute(
                    "INSERT INTO sessions (id, project_root, cwd, created_at_ms, updated_at_ms)
                     VALUES (?1, '/w', '/w', 0, 0)",
                    [session.as_str()],
                )?;
                Ok(())
            })
            .unwrap();
        BlobStore::new(store, session)
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
        let blobs = blob_store();
        let text = "short output\n";

        let outcome = apply_spill(text, 64, &blobs).await;

        assert_eq!(outcome.retained, text);
        assert_eq!(outcome.bytes_omitted, 0);
        assert!(outcome.blob_key.is_none());
        assert!(outcome.warning.is_none());
    }

    #[tokio::test]
    async fn large_results_keep_head_and_tail_and_store_blob() {
        let blobs = blob_store();
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
        let key = outcome.blob_key.as_ref().unwrap();
        assert!(outcome.retained.contains(&format!(
            "(Omitted {} bytes. Full result stored as blob {}",
            outcome.bytes_omitted, key
        )));
        assert_eq!(blobs.get(key).unwrap().as_deref(), Some(text.as_str()));
        assert_eq!(blobs.list().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn storage_failure_fails_open() {
        let store = Store::open(":memory:").unwrap();
        store
            .immediate(|tx| {
                tx.execute("DROP TABLE session_blobs", [])?;
                Ok(())
            })
            .unwrap();
        let blobs = BlobStore::new(store, SessionId::new());
        let text = sample(400);

        let outcome = apply_spill(&text, 40, &blobs).await;

        assert_eq!(outcome.retained, text);
        assert_eq!(outcome.bytes_omitted, 0);
        assert!(outcome.blob_key.is_none());
        assert!(outcome.warning.is_some());
    }

    #[test]
    fn head_and_tail_split_on_line_boundaries() {
        let text = "aa\nbb\ncc\ndd\n";
        assert_eq!(head_boundary(text, 7), 6);
        assert_eq!(tail_boundary(text, 7), 6);
    }
}
