use std::path::Path;

use anyhow::{Context, Result};
pub(crate) use meerai_core::fs_perms::{
    DIR_MODE, FILE_MODE, create_private_dir_sync as create_private_dir_blocking,
    write_private_sync as write_private_blocking,
};
use tokio::fs;
use tokio::io::AsyncWriteExt;

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
