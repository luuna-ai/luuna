//! Native, bounded integration of the compiled-in Atlas, Beacon and Missions
//! libraries (ADR 0053, ADR 0066).
//!
//! Calls run in this process, not a sandbox. Callers choose private,
//! project-scoped workspaces; no developer store is discovered or imported.
/// Runtime connection and provider credentials never enter native component children.
fn retain_component_env(env: &mut BTreeMap<String, String>) {
    env.retain(|key, _| {
        let key = key.to_ascii_uppercase();
        key != "LUUNA_URL"
            && key != "LUUNA_PI_ARGS_JSON"
            && !key.ends_with("_API_KEY")
            && !key.ends_with("_TOKEN")
            && !key.starts_with("LUUNA_PI_")
    });
}
mod atlas;
mod beacon;
pub use atlas::{call_atlas_in, call_atlas_native, run_atlas_native};
mod missions;
pub use beacon::{call_beacon_in, call_beacon_native, run_beacon_native};
pub use missions::{call_missions_in, call_missions_native, run_missions_native};

/// Where a component call finds its store (ADR 0061).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StoreRoot {
    /// The private managed workspace under the data directory. It is created
    /// with mode 0700 and a store from before ADR 0028 is moved into place.
    Managed(PathBuf),
    /// A registered checkout that the project's store mode file selects. It
    /// is used as it is: nothing is created, moved or re-permissioned, a
    /// store is never initialised there, and only `.luuna/<component>` is
    /// read or written.
    Checkout(PathBuf),
}

impl StoreRoot {
    pub fn path(&self) -> &Path {
        match self {
            StoreRoot::Managed(path) | StoreRoot::Checkout(path) => path,
        }
    }
    pub fn is_checkout(&self) -> bool {
        matches!(self, StoreRoot::Checkout(_))
    }
}

/// The canonical root a native call works in. A managed workspace is
/// prepared as before; a checkout is only checked (absolute, no `..`, no
/// linked element, a directory) and can never be initialised.
fn open_store(store: &StoreRoot, initializing: bool) -> Result<PathBuf> {
    match store {
        StoreRoot::Managed(path) => {
            let root = prepare_workspace(path)?;
            adopt_legacy_layout(&root)?;
            Ok(root)
        }
        StoreRoot::Checkout(path) => {
            if initializing || !path.is_absolute() {
                return Err(ComponentError::Workspace);
            }
            let mut current = PathBuf::new();
            for part in path.components() {
                if matches!(part, Component::ParentDir) {
                    return Err(ComponentError::Workspace);
                }
                current.push(part);
                match std::fs::symlink_metadata(&current) {
                    Ok(metadata) if !linked(&metadata) && metadata.is_dir() => {}
                    _ => return Err(ComponentError::Workspace),
                }
            }
            path.canonicalize().map_err(|_| ComponentError::Workspace)
        }
    }
}

use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};
use thiserror::Error;

pub type Result<T> = std::result::Result<T, ComponentError>;
const MAX_STDOUT: usize = 4 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum ComponentError {
    #[error("{0}")]
    Atlas(luuna_atlas::AtlasError),
    #[error("{0}")]
    Missions(luuna_missions::Error),
    #[error("component integration is unavailable")]
    Unavailable,
    #[error("unknown component")]
    Identity,
    #[error("component workspace is invalid or contains links outside its managed data")]
    Workspace,
    #[error("component operation exceeded its deadline and the owned process was stopped")]
    Timeout,
    #[error("component output exceeded its bounded contract")]
    OutputLimit,
    #[error("component returned invalid structured output")]
    Protocol,
    #[error("component refused the requested operation")]
    Refused,
    #[error("component configuration contains executable hooks; managed integration does not authorize them")]
    Hooks,
}

/// Compiled-in native components. Discovery never inspects a checkout, executable,
/// environment override or component package. Store admission happens per request.
#[derive(Clone, Debug, Default)]
pub struct Components {
    _native: (),
}
impl Components {
    pub fn discover() -> Result<Self> {
        Ok(Self::default())
    }
    pub fn describe(&self) -> String {
        "native Atlas, Beacon and Missions libraries".into()
    }
    pub async fn call(&self, component: &str, workspace: &Path, args: &[String]) -> Result<Value> {
        self.call_in(component, &StoreRoot::Managed(workspace.to_owned()), args)
            .await
    }
    pub async fn call_in(
        &self,
        component: &str,
        store: &StoreRoot,
        args: &[String],
    ) -> Result<Value> {
        match component.strip_prefix("luuna-").unwrap_or(component) {
            "atlas" => call_atlas_in(store, args).await,
            "beacon" => call_beacon_in(store, args).await,
            "missions" => call_missions_in(store, args).await,
            _ => Err(ComponentError::Identity),
        }
    }
}

/// Whether `path` is a link that belongs to the operating system's own layout
/// rather than to anything a user or project controls: a root-owned link
/// directly below a root-owned `/` that only root may change. macOS ships
/// `/var`, `/tmp` and `/etc` as such aliases for `/private/...`, and every
/// temporary and per-user folder sits below one of them. Every other link stays
/// refused.
pub fn is_system_alias(path: &Path, metadata: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if !metadata.file_type().is_symlink() || metadata.uid() != 0 {
            return false;
        }
        let root = Path::new("/");
        path.parent() == Some(root)
            && std::fs::symlink_metadata(root)
                .is_ok_and(|root| root.is_dir() && root.uid() == 0 && root.mode() & 0o022 == 0)
    }
    #[cfg(not(unix))]
    {
        let _ = (path, metadata);
        false
    }
}
fn linked(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
fn private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|_| ComponentError::Workspace)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| ComponentError::Workspace)?;
    }
    Ok(())
}
fn prepare_workspace(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|_| ComponentError::Workspace)?
            .join(path)
    };
    let mut current = PathBuf::new();
    for part in path.components() {
        if matches!(part, Component::ParentDir) {
            return Err(ComponentError::Workspace);
        }
        current.push(part);
        if std::fs::symlink_metadata(&current)
            .is_ok_and(|metadata| linked(&metadata) && !is_system_alias(&current, &metadata))
        {
            return Err(ComponentError::Workspace);
        }
    }
    private_dir(&path)?;
    let path = path.canonicalize().map_err(|_| ComponentError::Workspace)?;
    let home = path.join(".luuna-component-home");
    if std::fs::symlink_metadata(&home).is_ok_and(|metadata| linked(&metadata)) {
        return Err(ComponentError::Workspace);
    }
    private_dir(&home)?;
    check_data_links(&home, 0, &mut 0)?;
    Ok(path)
}
/// Project data root shared by the three component stores (ADR 0028).
const DATA_ROOT: &str = ".luuna";

/// Relative store directory for a component name (`missions`, `atlas`; any
/// other name is `beacon`), below the workspace.
#[cfg(test)]
fn data_dir(name: &str) -> &'static str {
    match name {
        "missions" => ".luuna/missions",
        "atlas" => ".luuna/atlas",
        _ => ".luuna/beacon",
    }
}

type LegacyStore = (
    &'static str,
    &'static str,
    Option<(&'static str, &'static str)>,
);
/// Layout written by runtime versions before ADR 0028: (legacy directory,
/// new directory, optional (legacy config file, new config file) rename).
/// Kept in one place so it can be deleted once no managed store predates it.
const LEGACY_LAYOUT: [LegacyStore; 3] = [
    (".seeds", ".luuna/missions", None),
    (
        ".mulch",
        ".luuna/atlas",
        Some(("mulch.config.yaml", "config.yaml")),
    ),
    (".canopy", ".luuna/beacon", None),
];

/// Move a managed workspace created by an earlier runtime version to the
/// `.luuna/<component>` layout. Idempotent: a store already at the new
/// location is untouched (a store present at both locations is left for the
/// component CLI to refuse), and an interrupted Atlas config rename resumes.
/// Links are refused rather than followed.
pub fn adopt_legacy_layout(workspace: &Path) -> Result<()> {
    let root = workspace.join(DATA_ROOT);
    for (legacy, current, config) in LEGACY_LAYOUT {
        let (legacy, current) = (workspace.join(legacy), workspace.join(current));
        let legacy_is_dir = match std::fs::symlink_metadata(&legacy) {
            Ok(metadata) if linked(&metadata) => return Err(ComponentError::Workspace),
            Ok(metadata) => metadata.is_dir(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => return Err(ComponentError::Workspace),
        };
        if legacy_is_dir && std::fs::symlink_metadata(&current).is_err() {
            match std::fs::symlink_metadata(&root) {
                Ok(metadata) if linked(&metadata) || !metadata.is_dir() => {
                    return Err(ComponentError::Workspace);
                }
                Ok(_) => {}
                Err(_) => private_dir(&root)?,
            }
            std::fs::rename(&legacy, &current).map_err(|_| ComponentError::Workspace)?;
        }
        if let Some((old, new)) = config {
            let (old, new) = (current.join(old), current.join(new));
            if std::fs::symlink_metadata(&old).is_ok_and(|metadata| metadata.is_file())
                && std::fs::symlink_metadata(&new).is_err()
            {
                std::fs::rename(&old, &new).map_err(|_| ComponentError::Workspace)?;
            }
        }
    }
    Ok(())
}

/// `.luuna` itself must be a real directory (when present) and the component
/// store below it must contain no links.
fn check_project_data_links(workspace: &Path, data_dir: &str) -> Result<()> {
    match std::fs::symlink_metadata(workspace.join(DATA_ROOT)) {
        Ok(metadata) if linked(&metadata) || !metadata.is_dir() => {
            return Err(ComponentError::Workspace);
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(ComponentError::Workspace),
    }
    check_data_links(&workspace.join(data_dir), 0, &mut 0)
}
fn check_data_links(path: &Path, depth: usize, count: &mut usize) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(ComponentError::Workspace),
    };
    *count += 1;
    if linked(&metadata) || depth > 16 || *count > 20_000 {
        return Err(ComponentError::Workspace);
    }
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path).map_err(|_| ComponentError::Workspace)? {
            check_data_links(
                &entry.map_err(|_| ComponentError::Workspace)?.path(),
                depth + 1,
                count,
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_checkout_store_is_only_checked_never_prepared_or_initialised() {
        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().canonicalize().unwrap().join("checkout");
        std::fs::create_dir_all(checkout.join(".seeds")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&checkout, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let store = StoreRoot::Checkout(checkout.clone());
        assert_eq!(open_store(&store, false).unwrap(), checkout);
        assert!(open_store(&store, true).is_err(), "init is refused");
        assert!(checkout.join(".seeds").is_dir(), "no legacy rename");
        assert!(!checkout.join(".luuna").exists());
        assert!(!checkout.join(".luuna-component-home").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&checkout).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        for refused in [
            StoreRoot::Checkout("relative/checkout".into()),
            StoreRoot::Checkout(checkout.join("../checkout")),
            StoreRoot::Checkout(checkout.join("missing")),
        ] {
            assert!(open_store(&refused, false).is_err(), "{refused:?}");
        }
        #[cfg(unix)]
        {
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&checkout, &link).unwrap();
            assert!(open_store(&StoreRoot::Checkout(link), false).is_err());
        }
    }
    use super::*;

    #[cfg(unix)]
    #[test]
    fn only_root_owned_links_directly_below_the_filesystem_root_are_system_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("alias");
        std::os::unix::fs::symlink(dir.path(), &link).unwrap();
        let metadata = std::fs::symlink_metadata(&link).unwrap();
        assert!(!is_system_alias(&link, &metadata));
        let plain = std::fs::symlink_metadata(dir.path()).unwrap();
        assert!(!is_system_alias(dir.path(), &plain));
        assert!(prepare_workspace(&link.join("workspace")).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_private_aliases_do_not_refuse_temporary_workspaces() {
        let var = Path::new("/var");
        assert!(is_system_alias(
            var,
            &std::fs::symlink_metadata(var).unwrap()
        ));
        let dir = tempfile::tempdir().unwrap();
        prepare_workspace(&dir.path().join("workspace")).unwrap();
    }

    #[test]
    fn legacy_stores_move_to_the_luuna_layout_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        for (legacy, _, _) in LEGACY_LAYOUT {
            std::fs::create_dir_all(ws.join(legacy)).unwrap();
            std::fs::write(ws.join(legacy).join("data.jsonl"), "{}\n").unwrap();
        }
        std::fs::write(ws.join(".mulch/mulch.config.yaml"), "version: 1\n").unwrap();
        for _ in 0..2 {
            adopt_legacy_layout(ws).unwrap();
            for (legacy, current, _) in LEGACY_LAYOUT {
                assert!(!ws.join(legacy).exists(), "{legacy}");
                assert_eq!(
                    std::fs::read_to_string(ws.join(current).join("data.jsonl")).unwrap(),
                    "{}\n"
                );
            }
            assert!(ws.join(".luuna/atlas/config.yaml").is_file());
            assert!(!ws.join(".luuna/atlas/mulch.config.yaml").exists());
        }
    }

    #[test]
    fn interrupted_config_rename_resumes_and_existing_new_store_wins() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join(".luuna/atlas")).unwrap();
        std::fs::write(ws.join(".luuna/atlas/mulch.config.yaml"), "a\n").unwrap();
        std::fs::create_dir_all(ws.join(".luuna/missions")).unwrap();
        std::fs::create_dir_all(ws.join(".seeds")).unwrap();
        adopt_legacy_layout(ws).unwrap();
        assert!(ws.join(".luuna/atlas/config.yaml").is_file());
        // Both locations present: nothing is merged or overwritten.
        assert!(ws.join(".seeds").is_dir() && ws.join(".luuna/missions").is_dir());
        // Nothing to do on an untouched workspace.
        let empty = tempfile::tempdir().unwrap();
        adopt_legacy_layout(empty.path()).unwrap();
        assert!(!empty.path().join(".luuna").exists());
    }

    #[test]
    fn luuna_root_must_be_a_real_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".luuna"), "file").unwrap();
        assert!(check_project_data_links(dir.path(), data_dir("atlas")).is_err());
        std::fs::create_dir_all(dir.path().join(".seeds")).unwrap();
        assert!(adopt_legacy_layout(dir.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn linked_luuna_root_and_legacy_stores_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (ws, elsewhere) = (dir.path().join("ws"), dir.path().join("elsewhere"));
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(elsewhere.join("missions")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, ws.join(".luuna")).unwrap();
        assert!(check_project_data_links(&ws, data_dir("missions")).is_err());
        std::fs::remove_file(ws.join(".luuna")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, ws.join(".seeds")).unwrap();
        assert!(adopt_legacy_layout(&ws).is_err());
    }

    #[test]
    fn component_futures_fit_small_native_thread_stacks() {
        let runner = Components::default();
        let args = ["list".into()];
        let call = runner.call("atlas", Path::new("unused"), &args);
        let call_bytes = std::mem::size_of_val(&call);
        eprintln!("component call future: {call_bytes} bytes");
        assert!(
            call_bytes <= 4096,
            "component calls must not inflate native request stacks"
        );
    }

    #[cfg(unix)]
    #[test]
    fn links_and_parent_traversal_in_workspaces_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("outside")).unwrap();
        std::os::unix::fs::symlink(root.join("outside"), root.join("link")).unwrap();
        assert!(matches!(
            prepare_workspace(&root.join("link/data")),
            Err(ComponentError::Workspace)
        ));
        assert!(matches!(
            prepare_workspace(Path::new("../synthetic-component-outside")),
            Err(ComponentError::Workspace)
        ));
    }
}
