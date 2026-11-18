use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use meerai_core::fs_perms::create_private_dir_sync;

use crate::oauth::pkce::uuid_v4;
use crate::secrets::{write_atomic_private, write_new_private};

const HOST_ID_FILE: &str = "agent-host-id";
const HOST_ID_PREFIX: &str = "urn:uuid:";

pub fn load_or_create_host_id(data_dir: &Path) -> Result<String> {
    let path = data_dir.join(HOST_ID_FILE);
    if let Some(existing) = read_host_id(&path)? {
        return Ok(existing);
    }
    create_private_dir_sync(data_dir)
        .with_context(|| format!("failed to create {}", data_dir.display()))?;
    let host_id = format!("{HOST_ID_PREFIX}{}", uuid_v4()?);
    match write_new_private(&path, format!("{host_id}\n").as_bytes()) {
        Ok(()) => Ok(host_id),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            if let Some(existing) = read_host_id(&path)? {
                return Ok(existing);
            }
            write_atomic_private(&path, format!("{host_id}\n").as_bytes())
                .with_context(|| format!("failed to replace {}", path.display()))?;
            Ok(host_id)
        }
        Err(err) => Err(err).with_context(|| format!("failed to write {}", path.display())),
    }
}

fn read_host_id(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(raw) => {
            let trimmed = raw.trim();
            Ok(trimmed
                .starts_with(HOST_ID_PREFIX)
                .then(|| trimmed.to_string()))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("meerai-host-id-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn host_id_is_created_once_and_reused() {
        let dir = temp_dir("stable");
        let first = load_or_create_host_id(&dir).unwrap();
        assert!(first.starts_with("urn:uuid:"));
        assert_eq!(first.len(), "urn:uuid:".len() + 36);
        assert_eq!(load_or_create_host_id(&dir).unwrap(), first);
        assert_eq!(
            fs::read_to_string(dir.join("agent-host-id"))
                .unwrap()
                .trim(),
            first
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join("agent-host-id"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_host_id_file_is_replaced() {
        let dir = temp_dir("corrupt");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("agent-host-id"), "not-a-host-id\n").unwrap();

        let host_id = load_or_create_host_id(&dir).unwrap();
        assert!(host_id.starts_with("urn:uuid:"));
        assert_eq!(load_or_create_host_id(&dir).unwrap(), host_id);
        let _ = fs::remove_dir_all(&dir);
    }
}
