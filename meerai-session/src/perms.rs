use std::path::Path;

use anyhow::{Context, Result};
use tokio::fs;
use tokio::io::AsyncWriteExt;

pub(crate) const DIR_MODE: u32 = 0o700;
pub(crate) const FILE_MODE: u32 = 0o600;

pub(crate) fn append_options() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    options.mode(FILE_MODE);
    options
}

pub(crate) fn create_options() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(FILE_MODE);
    options
}

pub(crate) async fn create_private_dir(dir: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(DIR_MODE);
    builder
        .create(dir)
        .await
        .with_context(|| format!("failed to create {}", dir.display()))?;
    tighten_dir(dir).await
}

pub(crate) async fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = create_options()
        .open(path)
        .await
        .with_context(|| format!("failed to create {}", path.display()))?;
    file.write_all(bytes)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    file.flush()
        .await
        .with_context(|| format!("failed to flush {}", path.display()))?;
    drop(file);
    tighten_file(path).await
}

pub(crate) async fn tighten_dir(dir: &Path) -> Result<()> {
    set_mode(dir, DIR_MODE).await
}

pub(crate) async fn tighten_file(path: &Path) -> Result<()> {
    set_mode(path, FILE_MODE).await
}

pub(crate) fn create_private_dir_blocking(dir: &Path) -> Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(DIR_MODE);
    }
    builder
        .create(dir)
        .with_context(|| format!("failed to create {}", dir.display()))?;
    set_mode_blocking(dir, DIR_MODE)
}

pub(crate) fn write_private_blocking(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;

    let mut options = std::fs::OpenOptions::new();
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
    set_mode_blocking(path, FILE_MODE)
}

#[cfg(unix)]
async fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .await
        .with_context(|| format!("failed to set permissions on {}", path.display()))
}

#[cfg(not(unix))]
async fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_mode_blocking(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .with_context(|| format!("failed to set permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_mode_blocking(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}
