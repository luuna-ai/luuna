//! The project store resolver: the one place that decides which component
//! store a project's requests reach.
//!
//! The mode is read from `<data_dir>/projects/<id>/store.json` on every
//! resolution (no file means runtime). Runtime mode resolves to the managed
//! workspace under the data directory, exactly as before. A repository or both
//! file must name a repository registered for the project, otherwise it is
//! refused. Repository mode resolves to that repository's registered checkout,
//! whose `.luuna/<component>` stores are used as they are (ADR 0061); the
//! checkout is re-checked on every resolution. Both mode is still refused, and
//! a store file that cannot be used is never treated as runtime: all of these
//! refuse before any component data is read or written. User memory belongs to
//! no project and always lives in the single runtime user store. The only
//! file I/O here is reading the checkout's metadata; luuna_storage reads the
//! mode file.
use crate::{internal, not_found, ApiError, AppState};
use axum::http::StatusCode;
use luuna_domain::{valid_project_id, StoreMode};
use luuna_storage::StorageError;
use std::path::{Component, Path, PathBuf};

pub use luuna_components::StoreRoot;

/// Why a project's component store cannot be resolved.
#[derive(Debug, Clone, PartialEq)]
pub enum StoreRefusal {
    /// The id is not a valid project id, so no path is formed from it.
    InvalidProject,
    /// The recorded mode is not available in this version.
    NotAvailable(StoreMode),
    /// The registered checkout cannot hold a repository store: it is not an
    /// absolute real directory, a path element is a link, it is not a main
    /// checkout with its own `.git` directory, its `.luuna` is not a real
    /// directory, or it overlaps the data directory.
    UnusableCheckout,
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
            StoreRefusal::UnusableCheckout => {
                f.write_str("the registered checkout cannot hold a repository store")
            }
        }
    }
}

impl std::error::Error for StoreRefusal {}

/// The component store for `project_id` in `mode`, under `data_dir`. Runtime
/// mode forms a path and touches nothing. Repository mode needs the
/// registered checkout of the repository the mode names, and checks it with
/// [`check_checkout`]; without one it is refused.
pub fn resolve_project_store(
    data_dir: &Path,
    project_id: &str,
    mode: &StoreMode,
    checkout: Option<&Path>,
) -> Result<StoreRoot, StoreRefusal> {
    if !valid_project_id(project_id) {
        return Err(StoreRefusal::InvalidProject);
    }
    match mode {
        StoreMode::Runtime => Ok(StoreRoot::Managed(
            data_dir
                .join("components")
                .join("projects")
                .join(project_id),
        )),
        StoreMode::Repository { .. } => {
            let checkout = checkout.ok_or(StoreRefusal::UnusableCheckout)?;
            check_checkout(checkout, data_dir)?;
            Ok(StoreRoot::Checkout(checkout.to_owned()))
        }
        StoreMode::Both { .. } => Err(StoreRefusal::NotAvailable(mode.clone())),
    }
}

/// Synchronous checks of a registered checkout, repeated on every
/// resolution: an absolute path with no `..`; every element a real directory
/// (no link or reparse point); `<root>/.git` a directory, so a linked worktree
/// or gitfile checkout is refused; `<root>/.luuna`, when present, a real
/// directory; and neither the checkout nor the data directory inside the
/// other. No git process runs and nothing is written.
pub fn check_checkout(checkout: &Path, data_dir: &Path) -> Result<(), StoreRefusal> {
    if !checkout.is_absolute() {
        return Err(StoreRefusal::UnusableCheckout);
    }
    let mut current = PathBuf::new();
    for part in checkout.components() {
        if matches!(part, Component::ParentDir) {
            return Err(StoreRefusal::UnusableCheckout);
        }
        current.push(part);
        if !real_directory(&current) {
            return Err(StoreRefusal::UnusableCheckout);
        }
    }
    if !real_directory(&checkout.join(".git")) {
        return Err(StoreRefusal::UnusableCheckout);
    }
    match std::fs::symlink_metadata(checkout.join(".luuna")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ if real_directory(&checkout.join(".luuna")) => {}
        _ => return Err(StoreRefusal::UnusableCheckout),
    }
    let canonical = |path: &Path| {
        std::fs::canonicalize(path)
            .map(|path| luuna_git::strip_verbatim_prefix(&path))
            .map_err(|_| StoreRefusal::UnusableCheckout)
    };
    let (root, data) = (canonical(checkout)?, canonical(data_dir)?);
    if root.starts_with(&data) || data.starts_with(&root) {
        return Err(StoreRefusal::UnusableCheckout);
    }
    Ok(())
}

/// Store folders of the components' upstream tools (Seeds, Mulch, Canopy).
/// A component's own `init` moves such a folder to `.luuna/<component>` and
/// rewrites ignore files, so the runtime never initialises a checkout store
/// while one is present, and a new project does not default to that checkout
/// (ADR 0067).
pub(crate) const LEGACY_STORES: [(&str, &str); 3] = [
    ("missions", ".seeds"),
    ("atlas", ".mulch"),
    ("beacon", ".canopy"),
];

/// The first upstream store folder present in `checkout`, as (component,
/// folder). Anything at that path counts, even a link.
pub(crate) fn legacy_store(checkout: &Path) -> Option<(&'static str, &'static str)> {
    LEGACY_STORES
        .into_iter()
        .find(|(_, folder)| std::fs::symlink_metadata(checkout.join(folder)).is_ok())
}

/// Most entries and folder depth looked through in a checkout component store.
const TREE_MAX_ENTRIES: usize = 100_000;
const TREE_MAX_DEPTH: usize = 32;

/// Whether a checkout's `.luuna/<component>` store is usable: a real folder
/// whose `config.yaml` is a regular file and which holds no link, junction or
/// special file anywhere below it, so nothing the component reads or writes
/// can lead outside the checkout. A store too large to look through counts
/// as unusable.
pub fn checkout_component_usable(checkout: &Path, component: &str) -> bool {
    fn walk(dir: &Path, depth: usize, seen: &mut usize) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        for entry in entries {
            *seen += 1;
            let Ok(entry) = entry else { return false };
            let path = entry.path();
            if *seen > TREE_MAX_ENTRIES {
                return false;
            }
            if real_directory(&path) {
                if depth >= TREE_MAX_DEPTH || !walk(&path, depth + 1, seen) {
                    return false;
                }
            } else if !real_file(&path) {
                return false;
            }
        }
        true
    }
    let store = checkout.join(".luuna").join(component);
    real_directory(&checkout.join(".luuna"))
        && real_directory(&store)
        && real_file(&store.join("config.yaml"))
        && walk(&store, 0, &mut 0)
}

fn unlinked(metadata: &std::fs::Metadata) -> bool {
    let link = metadata.file_type().is_symlink();
    #[cfg(windows)]
    let link = {
        use std::os::windows::fs::MetadataExt;
        link || metadata.file_attributes() & 0x400 != 0
    };
    !link
}

fn real_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| unlinked(&metadata) && metadata.is_file())
}

fn real_directory(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| {
        let link = metadata.file_type().is_symlink();
        #[cfg(windows)]
        let link = {
            use std::os::windows::fs::MetadataExt;
            link || metadata.file_attributes() & 0x400 != 0
        };
        !link && metadata.is_dir()
    })
}

/// The single user-scope memory store, whatever any project's mode is.
pub fn user_store_path(data_dir: &Path) -> PathBuf {
    data_dir.join("components").join("user")
}

/// The component store of a registered project, checked in this order: an
/// invalid or unregistered id is not found; a store file that cannot be used
/// is refused; a repository named by the file that is not registered for the
/// project is refused; both mode is refused; in repository mode, a registered
/// checkout that fails [`check_checkout`] is refused. A refusal never falls
/// back to the runtime store. The checkout path always comes from the
/// registration, so a rebind moves the store with it.
pub(crate) fn project_store(state: &AppState, project_id: &str) -> Result<StoreRoot, ApiError> {
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
    let mut checkout = None;
    if let Some(name) = mode.repository() {
        // The guard is a temporary of this statement and is dropped before
        // anything else is locked.
        let registrations = state
            .store
            .lock()
            .map_err(internal)?
            .repositories(project_id)
            .map_err(internal)?;
        let Some(registration) = registrations.into_iter().find(|r| r.repository_id == name) else {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "This project's store mode file names a repository that is not registered for this project; fix or remove the file. No component data was read or written".into(),
            ));
        };
        checkout = Some(PathBuf::from(registration.local_path));
    }
    match resolve_project_store(
        &state.config.data_dir,
        project_id,
        &mode,
        checkout.as_deref(),
    ) {
        Ok(store) => Ok(store),
        Err(StoreRefusal::InvalidProject) => Err(not_found()),
        Err(StoreRefusal::UnusableCheckout) => Err(ApiError(
            StatusCode::CONFLICT,
            "This project's store mode is 'repository', but that repository's registered checkout cannot hold its stores: it must be a main checkout with a .git directory, without links, outside the data directory. No component data was read or written".into(),
        )),
        Err(StoreRefusal::NotAvailable(mode)) => Err(ApiError(
            StatusCode::CONFLICT,
            format!(
                "This project's component store mode is '{}'; only the runtime and repository stores are available in this version. No component data was read or written",
                mode.name()
            ),
        )),
    }
}

/// The user-scope memory store of this runtime.
pub(crate) fn user_store(state: &AppState) -> StoreRoot {
    StoreRoot::Managed(user_store_path(&state.config.data_dir))
}

/// Component commands that change a store; only these are followed by a
/// commit (ADR 0068). A change another command made is picked up by the next
/// commit.
const WRITES: [&str; 8] = [
    "create", "update", "block", "add", "record", "edit", "delete", "schema",
];

/// One commit at a time across the runtime, so two store changes never race
/// on the same branch.
static COMMITS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// After a component write to a checkout store, commit `.luuna/` to the
/// checkout's current branch (ADR 0068). A failure is logged and never fails
/// the component call: the change is already on disk and the next commit
/// carries it. `command` of `None` is an explicit commit (component enable).
pub(crate) async fn commit_checkout_store(
    state: &AppState,
    store: &StoreRoot,
    component: &str,
    command: Option<&str>,
) {
    if !state.store_auto_commit || !store.is_checkout() {
        return;
    }
    if command.is_some_and(|command| !WRITES.contains(&command)) {
        return;
    }
    let summary = match command {
        Some(command) => format!("{component} {command}"),
        None => component.to_owned(),
    };
    let message = format!(
        "luuna: {summary}\n\nRecorded by Luuna in .luuna/ (ADR 0068). Set LUUNA_STORE_AUTO_COMMIT=0 to stop these commits."
    );
    let _guard = COMMITS.lock().await;
    match state.git.commit_store(store.path(), &message).await {
        Ok(luuna_git::StoreCommit::Busy) => eprintln!(
            "luuna-runtime: .luuna changes left uncommitted: a merge, rebase or similar is in progress in the checkout"
        ),
        Ok(_) => {}
        Err(error) => eprintln!("luuna-runtime: .luuna changes left uncommitted: {error}"),
    }
}
