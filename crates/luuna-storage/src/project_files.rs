//! The per-project store file `<data_dir>/projects/<id>/store.json`.
//!
//! An absent file (or folder) means runtime mode. A file that exists but
//! cannot be trusted is refused, never defaulted. Nothing here follows a link,
//! and writes are private and atomic.
use crate::{Result, StorageError};
use luuna_domain::{valid_project_id, ProjectStoreFile, StoreMode};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const STORE_FILE: &str = "store.json";
const TEMP_FILE: &str = "store.json.tmp";
/// Largest store file accepted; the real file is a few hundred bytes.
const MAX_STORE_FILE_BYTES: u64 = 4096;

fn invalid<T>(message: &str) -> Result<T> {
    Err(StorageError::Invalid(message.to_string()))
}

fn require_project_id(project_id: &str) -> Result<()> {
    if valid_project_id(project_id) {
        Ok(())
    } else {
        invalid("project id is not valid")
    }
}

fn is_link(metadata: &fs::Metadata) -> bool {
    let link = metadata.file_type().is_symlink();
    #[cfg(windows)]
    let link = {
        use std::os::windows::fs::MetadataExt;
        link || metadata.file_attributes() & 0x400 != 0
    };
    link
}

/// `Ok(None)` when `path` is absent; a link or the wrong type is refused.
fn existing(path: &Path, directory: bool) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if is_link(&metadata) {
                return invalid("project store path contains a link");
            }
            if directory && !metadata.is_dir() {
                return invalid("project store path component is not a directory");
            }
            if !directory && !metadata.is_file() {
                return invalid("project store file is not a regular file");
            }
            Ok(Some(metadata))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// `<data_dir>/projects/<project_id>/store.json`, for a valid project id only.
pub fn project_store_file_path(data_dir: &Path, project_id: &str) -> Result<PathBuf> {
    require_project_id(project_id)?;
    Ok(data_dir.join("projects").join(project_id).join(STORE_FILE))
}

/// The recorded mode, or runtime when no file exists. A file that exists but
/// cannot be used is an error.
pub fn read_project_store_mode(data_dir: &Path, project_id: &str) -> Result<StoreMode> {
    let path = project_store_file_path(data_dir, project_id)?;
    let folder = data_dir.join("projects").join(project_id);
    if existing(&data_dir.join("projects"), true)?.is_none()
        || existing(&folder, true)?.is_none()
        || existing(&path, false)?.is_none()
    {
        return Ok(StoreMode::Runtime);
    }
    let mut bytes = Vec::new();
    let file = match fs::File::open(&path) {
        Ok(file) => file,
        // Removed between the check and the open: still absent.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StoreMode::Runtime)
        }
        Err(error) => return Err(error.into()),
    };
    file.take(MAX_STORE_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_STORE_FILE_BYTES {
        return invalid("project store file is too large");
    }
    let parsed = ProjectStoreFile::parse(&bytes)?;
    if parsed.project_id != project_id {
        return invalid("project store file belongs to another project");
    }
    Ok(parsed.mode)
}

/// Record `mode` for the project: private, written to a temporary file,
/// synced and renamed over the previous file. Nothing is touched when the id
/// or the mode is invalid or a link is in the way.
pub fn write_project_store_mode(data_dir: &Path, project_id: &str, mode: &StoreMode) -> Result<()> {
    let path = project_store_file_path(data_dir, project_id)?;
    let file = ProjectStoreFile {
        project_id: project_id.to_string(),
        mode: mode.clone(),
    };
    let json = file.to_json()?;
    ProjectStoreFile::parse(json.as_bytes())?;

    let projects = data_dir.join("projects");
    let folder = projects.join(project_id);
    let temporary = folder.join(TEMP_FILE);
    // Every check happens before anything is created or removed.
    existing(&projects, true)?;
    existing(&folder, true)?;
    existing(&path, false)?;
    let stale = existing(&temporary, false)?.is_some();

    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&folder)?;
    if stale {
        fs::remove_file(&temporary)?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> std::io::Result<()> {
        let mut handle = options.open(&temporary)?;
        handle.write_all(json.as_bytes())?;
        handle.sync_all()?;
        drop(handle);
        fs::rename(&temporary, &path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    #[cfg(unix)]
    fs::File::open(&folder)?.sync_all()?;
    Ok(())
}
