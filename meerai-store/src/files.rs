use std::fs::{self, DirBuilder, Metadata, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, anyhow, ensure};
use meerai_core::fs_perms::{DIR_MODE, FILE_MODE, set_mode_sync};

pub(crate) fn prepare_default_dir(dir: &Path) -> Result<()> {
    create_parents(dir)?;
    ensure!(
        fs::symlink_metadata(dir)?.is_dir(),
        "default store directory is not a real directory: {}",
        dir.display()
    );
    set_mode_sync(dir, DIR_MODE)
}

pub(crate) fn prepare(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .context("missing SQLite database file name")?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    create_parents(parent)?;
    let path = parent.canonicalize()?.join(name);

    prepare_db_file(&path)?;
    harden_sidecars(&path)?;
    Ok(path)
}

fn prepare_db_file(path: &Path) -> Result<()> {
    // A same-process opener must not enter SQLite while the creation descriptor is still open.
    static CREATE: Mutex<()> = Mutex::new(());
    let _guard = CREATE
        .lock()
        .map_err(|_| anyhow!("SQLite store file creation lock poisoned"))?;
    if !harden_file(path)? {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(FILE_MODE);
        }
        match options.open(path) {
            Ok(file) => drop(file),
            Err(err) if err.kind() == ErrorKind::AlreadyExists => (),
            Err(err) => return Err(err).context("create private SQLite database file"),
        }
        ensure!(harden_file(path)?, "SQLite database file disappeared");
    }
    Ok(())
}

fn create_parents(dir: &Path) -> Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(DIR_MODE);
    }
    builder
        .create(dir)
        .with_context(|| format!("create SQLite directory {}", dir.display()))
}

pub(crate) fn harden_sidecars(path: &Path) -> Result<()> {
    for path in sidecars(path) {
        harden_file(&path)?;
    }
    Ok(())
}

pub(crate) fn validate_existing(path: &Path) -> Result<bool> {
    if !validate_file(path)? {
        return Ok(false);
    }
    for path in sidecars(path) {
        validate_file(&path)?;
    }
    Ok(true)
}

fn sidecars(path: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    ["-wal", "-shm", "-journal"].into_iter().map(|suffix| {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        PathBuf::from(sidecar)
    })
}

fn validate_file(path: &Path) -> Result<bool> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err).with_context(|| format!("inspect {}", path.display())),
    };
    ensure!(
        metadata.is_file() && has_single_link(&metadata),
        "SQLite path is not a regular, singly linked file: {}",
        path.display()
    );
    Ok(true)
}

fn harden_file(path: &Path) -> Result<bool> {
    if !validate_file(path)? {
        return Ok(false);
    }
    // Don't open and close an extra descriptor: that can release SQLite's POSIX record locks.
    match set_mode_sync(path, FILE_MODE) {
        Ok(()) => Ok(true),
        Err(err)
            if err
                .downcast_ref::<std::io::Error>()
                .is_some_and(|err| err.kind() == ErrorKind::NotFound) =>
        {
            Ok(false)
        }
        Err(err) => Err(err),
    }
}

#[cfg(unix)]
fn has_single_link(metadata: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.nlink() == 1
}

#[cfg(not(unix))]
fn has_single_link(_metadata: &Metadata) -> bool {
    true
}
