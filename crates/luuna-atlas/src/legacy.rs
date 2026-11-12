//! The one-way migration of a legacy `.mulch/` store to `.luuna/atlas/`.
//!
//! Task t1 declares the function; task t5 fills it in.

use std::path::Path;

use crate::error::{AtlasError, Result};

/// The legacy store directory.
pub const LEGACY_STORE_DIR: &str = ".mulch";

/// The legacy config file name inside the legacy store.
pub const LEGACY_CONFIG_FILE: &str = "mulch.config.yaml";

/// Moves `<project_root>/.mulch/` to `<project_root>/.luuna/atlas/` exactly as the Go
/// tool does. Returns the notice the Go tool prints on stderr when it migrated, and
/// `None` when there was nothing to migrate.
pub fn migrate_legacy_store(_project_root: &Path) -> Result<Option<String>> {
    Err(AtlasError::not_implemented(
        "legacy::migrate_legacy_store (task t5)",
    ))
}
