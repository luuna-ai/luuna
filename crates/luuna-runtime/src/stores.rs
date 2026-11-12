//! The project store resolver: the one place that decides which component
//! store a project's requests reach.
//!
//! The mode is read from `<data_dir>/projects/<id>/store.json` on every
//! resolution (no file means runtime). Runtime mode resolves to the managed
//! workspace under the data directory, exactly as before. A repository or both
//! file must name a repository registered for the project, otherwise it is
//! refused. Repository and both modes are still refused in this version, and a
//! store file that cannot be used is never treated as runtime: all of these
//! refuse before any component data is read or written. User memory belongs to no project and
//! always lives in the single runtime user store. This module does no file
//! I/O itself; luuna_storage reads the mode file.
use crate::{internal, not_found, ApiError, AppState};
use axum::http::StatusCode;
use luuna_domain::{valid_project_id, StoreMode};
use luuna_storage::StorageError;
use std::path::{Path, PathBuf};

/// Why a project's component store cannot be resolved.
#[derive(Debug, Clone, PartialEq)]
pub enum StoreRefusal {
    /// The id is not a valid project id, so no path is formed from it.
    InvalidProject,
    /// The recorded mode is not available in this version.
    NotAvailable(StoreMode),
}

impl std::fmt::Display for StoreRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreRefusal::InvalidProject => f.write_str("project id is not valid"),
            StoreRefusal::NotAvailable(mode) => write!(
                f,
                "the {} store mode is not available in this version; only the runtime store is",
                mode.name()
            ),
        }
    }
}

impl std::error::Error for StoreRefusal {}

/// The component store for `project_id` in `mode`, under `data_dir`. Pure: it
/// forms a path or refuses, and touches nothing.
pub fn resolve_project_store(
    data_dir: &Path,
    project_id: &str,
    mode: &StoreMode,
) -> Result<PathBuf, StoreRefusal> {
    if !valid_project_id(project_id) {
        return Err(StoreRefusal::InvalidProject);
    }
    match mode {
        StoreMode::Runtime => Ok(data_dir
            .join("components")
            .join("projects")
            .join(project_id)),
        StoreMode::Repository { .. } | StoreMode::Both { .. } => {
            Err(StoreRefusal::NotAvailable(mode.clone()))
        }
    }
}

/// The single user-scope memory store, whatever any project's mode is.
pub fn user_store_path(data_dir: &Path) -> PathBuf {
    data_dir.join("components").join("user")
}

/// The component store of a registered project, checked in this order: an
/// invalid or unregistered id is not found; a store file that cannot be used
/// is refused; a repository named by the file that is not registered for the
/// project is refused; a recorded mode other than runtime is refused. A
/// refusal never falls back to the runtime store. The registry check reads
/// only the registrations, never a checkout or a path.
pub(crate) fn project_store(state: &AppState, project_id: &str) -> Result<PathBuf, ApiError> {
    if !valid_project_id(project_id) {
        return Err(not_found());
    }
    state
        .store
        .lock()
        .map_err(internal)?
        .get_project(project_id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    let mode = match luuna_storage::read_project_store_mode(&state.config.data_dir, project_id) {
        Ok(mode) => mode,
        // The file exists but its place or contents cannot be trusted; the
        // contents are refused by the domain parser, everything else by
        // storage. Neither message carries a path or file content.
        Err(error @ (StorageError::Invalid(_) | StorageError::Domain(_))) => {
            eprintln!("luuna-runtime: project {project_id} store mode file refused: {error}");
            return Err(ApiError(
                StatusCode::CONFLICT,
                "This project's store mode file in the data directory cannot be used; fix or remove it. No component data was read or written".into(),
            ));
        }
        Err(error) => return Err(internal(error)),
    };
    if let Some(name) = mode.repository() {
        // The guard is a temporary of this statement and is dropped before
        // anything else is locked.
        let registrations = state
            .store
            .lock()
            .map_err(internal)?
            .repositories(project_id)
            .map_err(internal)?;
        if !registrations.iter().any(|r| r.repository_id == name) {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "This project's store mode file names a repository that is not registered for this project; fix or remove the file. No component data was read or written".into(),
            ));
        }
    }
    match resolve_project_store(&state.config.data_dir, project_id, &mode) {
        Ok(path) => Ok(path),
        Err(StoreRefusal::InvalidProject) => Err(not_found()),
        Err(StoreRefusal::NotAvailable(mode)) => Err(ApiError(
            StatusCode::CONFLICT,
            format!(
                "This project's component store mode is '{}'; only the runtime store is available in this version. No component data was read or written",
                mode.name()
            ),
        )),
    }
}

/// The user-scope memory store of this runtime.
pub(crate) fn user_store(state: &AppState) -> PathBuf {
    user_store_path(&state.config.data_dir)
}
