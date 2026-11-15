use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

pub const DIR_MODE: u32 = 0o700;
pub const FILE_MODE: u32 = 0o600;

pub fn create_private_dir_sync(dir: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(DIR_MODE);
    }
    builder
        .create(dir)
        .with_context(|| format!("failed to create {}", dir.display()))?;
    set_mode_sync(dir, DIR_MODE)
}

pub fn write_private_sync(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(FILE_MODE);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("failed to write {}", path.display()))?;
    file.flush()
        .with_context(|| format!("failed to flush {}", path.display()))?;
    drop(file);
    set_mode_sync(path, FILE_MODE)
}

#[cfg(unix)]
pub fn set_mode_sync(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("failed to set permissions on {}", path.display()))
}

#[cfg(not(unix))]
pub fn set_mode_sync(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}
