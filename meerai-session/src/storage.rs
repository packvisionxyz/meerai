use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::event::LoggedEvent;

const ACTIVE_PREFIX: &str = "session.v";
const ACTIVE_SUFFIX: &str = ".jsonl";
const SEALED_SUFFIX: &str = ".jsonl.zstd";
const COMPRESSION_LEVEL: i32 = 3;

pub(crate) fn active_name(generation: u64) -> String {
    format!("{ACTIVE_PREFIX}{generation}{ACTIVE_SUFFIX}")
}

pub(crate) fn sealed_name(generation: u64) -> String {
    format!("{ACTIVE_PREFIX}{generation}{SEALED_SUFFIX}")
}

pub(crate) fn is_log_file_name(name: &str) -> bool {
    parse_generation(name, ACTIVE_SUFFIX).is_some()
        || parse_generation(name, SEALED_SUFFIX).is_some()
}

pub(crate) struct LoadedLog {
    pub(crate) events: Vec<LoggedEvent>,
    pub(crate) active_generation: u64,
    pub(crate) active_size: u64,
}

struct Discovered {
    sealed: Vec<(u64, PathBuf)>,
    plain: Vec<(u64, PathBuf)>,
    active_generation: u64,
}

pub(crate) async fn load(dir: &Path) -> Result<LoadedLog> {
    let discovered = discover(dir).await?;

    let mut text = String::new();
    if !discovered.sealed.is_empty() {
        let mut compressed = Vec::new();
        for (_, path) in &discovered.sealed {
            let chunk = fs::read(path)
                .await
                .with_context(|| format!("failed to read {}", path.display()))?;
            compressed.extend_from_slice(&chunk);
        }
        let decoded = zstd::stream::decode_all(compressed.as_slice()).with_context(|| {
            format!(
                "failed to decompress session generations in {}",
                dir.display()
            )
        })?;
        text.push_str(&String::from_utf8(decoded).with_context(|| {
            format!(
                "session generations in {} are not valid UTF-8",
                dir.display()
            )
        })?);
    }

    for (_, path) in &discovered.plain {
        let chunk = fs::read_to_string(path)
            .await
            .with_context(|| format!("failed to read {}", path.display()))?;
        text.push_str(&chunk);
    }

    let active_path = dir.join(active_name(discovered.active_generation));
    let active_size = match fs::metadata(&active_path).await {
        Ok(metadata) => metadata.len(),
        Err(err) if err.kind() == ErrorKind::NotFound => 0,
        Err(err) => {
            return Err(anyhow::Error::from(err)
                .context(format!("failed to inspect {}", active_path.display())));
        }
    };

    Ok(LoadedLog {
        events: parse_events(&text),
        active_generation: discovered.active_generation,
        active_size,
    })
}

pub(crate) async fn seal(dir: &Path, generation: u64) -> Result<()> {
    let plain = dir.join(active_name(generation));
    let bytes = match fs::read(&plain).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(
                anyhow::Error::from(err).context(format!("failed to read {}", plain.display()))
            );
        }
    };
    if bytes.is_empty() {
        return Ok(());
    }

    let compressed = zstd::stream::encode_all(bytes.as_slice(), COMPRESSION_LEVEL)
        .with_context(|| format!("failed to compress {}", plain.display()))?;
    let sealed = dir.join(sealed_name(generation));
    let tmp = dir.join(format!("{}.tmp", sealed_name(generation)));
    let mut file = crate::perms::create_options()
        .open(&tmp)
        .await
        .with_context(|| format!("failed to create {}", tmp.display()))?;
    file.write_all(&compressed)
        .await
        .with_context(|| format!("failed to write {}", tmp.display()))?;
    file.sync_all()
        .await
        .with_context(|| format!("failed to sync {}", tmp.display()))?;
    drop(file);
    crate::perms::tighten_file(&tmp).await?;
    fs::rename(&tmp, &sealed)
        .await
        .with_context(|| format!("failed to rename {} to {}", tmp.display(), sealed.display()))?;

    match fs::remove_file(&plain).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => {
            Err(anyhow::Error::from(err).context(format!("failed to remove {}", plain.display())))
        }
    }
}

async fn discover(dir: &Path) -> Result<Discovered> {
    let mut sealed: BTreeMap<u64, PathBuf> = BTreeMap::new();
    let mut plain: BTreeMap<u64, PathBuf> = BTreeMap::new();

    let mut entries = match fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Ok(Discovered {
                sealed: Vec::new(),
                plain: Vec::new(),
                active_generation: 0,
            });
        }
        Err(err) => {
            return Err(
                anyhow::Error::from(err).context(format!("failed to list {}", dir.display()))
            );
        }
    };

    while let Some(entry) = entries
        .next_entry()
        .await
        .with_context(|| format!("failed to list {}", dir.display()))?
    {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if let Some(generation) = parse_generation(name, SEALED_SUFFIX) {
            sealed.insert(generation, entry.path());
        } else if let Some(generation) = parse_generation(name, ACTIVE_SUFFIX) {
            plain.insert(generation, entry.path());
        }
    }

    let duplicates: Vec<u64> = plain
        .keys()
        .filter(|generation| sealed.contains_key(generation))
        .copied()
        .collect();
    for generation in duplicates {
        if let Some(path) = plain.remove(&generation) {
            let _ = fs::remove_file(path).await;
        }
    }

    let active_generation = match (sealed.keys().next_back(), plain.keys().next_back()) {
        (None, None) => 0,
        (None, Some(max)) => *max,
        (Some(max), Some(plain_max)) if plain_max > max => *plain_max,
        (Some(max), _) => max + 1,
    };

    Ok(Discovered {
        sealed: sealed.into_iter().collect(),
        plain: plain.into_iter().collect(),
        active_generation,
    })
}

fn parse_generation(name: &str, suffix: &str) -> Option<u64> {
    let raw = name.strip_prefix(ACTIVE_PREFIX)?.strip_suffix(suffix)?;
    raw.parse().ok()
}

fn parse_events(raw: &str) -> Vec<LoggedEvent> {
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<LoggedEvent>(line).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_generation_names() {
        assert_eq!(parse_generation("session.v0.jsonl", ACTIVE_SUFFIX), Some(0));
        assert_eq!(
            parse_generation("session.v12.jsonl.zstd", SEALED_SUFFIX),
            Some(12)
        );
        assert_eq!(parse_generation("meta.json", ACTIVE_SUFFIX), None);
        assert_eq!(parse_generation("session.vx.jsonl", ACTIVE_SUFFIX), None);
    }
}
