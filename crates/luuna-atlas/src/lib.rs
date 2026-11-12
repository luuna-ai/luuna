//! A native port of the Luuna Atlas store (`.luuna/atlas/`), derived from Luuna Atlas and
//! Mulch under the MIT license (see `LICENSE` in this crate).
//!
//! The crate reads and writes the store byte for byte as the Go tool at reference commit
//! `c4db80c3f2e622afbdd84f09077fd73b2a92d199` does, and implements `init`, `add` and
//! `config show`. It takes the project root as a parameter: it never runs git and never
//! reads the environment to find a store. Nothing here is wired into the runtime.

pub mod actions;
pub mod assets;
pub mod commands;
pub mod config;
pub mod confirm;
pub mod error;
pub mod evidence;
pub mod git;
pub mod hooks;
pub mod json;
pub mod legacy;
pub mod lifecycle;
pub mod options;
pub mod output;
pub mod prime;
pub mod records;
pub mod registry;
pub mod schema;
pub mod store;
pub mod style;
pub mod timefmt;
pub mod tooling;
pub mod yaml;

use std::path::{Path, PathBuf};

pub use error::{AtlasError, Result};
pub use output::CommandOutput;

/// A store located at `<project_root>/.luuna/atlas`.
#[derive(Debug, Clone)]
pub struct AtlasStore {
    project_root: PathBuf,
}

impl AtlasStore {
    /// Opens the store of `project_root`, running the legacy `.mulch/` migration first as
    /// the Go tool does. The store directory need not exist yet. The migration notice the
    /// Go tool prints is dropped here; [`AtlasStore::open_with_notice`] returns it.
    pub fn open(project_root: impl Into<PathBuf>) -> Result<AtlasStore> {
        AtlasStore::open_with_notice(project_root).map(|(store, _)| store)
    }

    /// Like [`AtlasStore::open`], also returning the notice (without its newline) that
    /// the Go tool prints on stderr when it migrated a legacy store.
    pub fn open_with_notice(
        project_root: impl Into<PathBuf>,
    ) -> Result<(AtlasStore, Option<String>)> {
        let project_root = project_root.into();
        let notice = legacy::migrate_legacy_store(&project_root)?;
        Ok((AtlasStore { project_root }, notice))
    }

    /// The project root the store belongs to.
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// `<project_root>/.luuna/atlas`.
    pub fn atlas_dir(&self) -> PathBuf {
        self.project_root.join(".luuna").join("atlas")
    }

    /// `<project_root>/.luuna/atlas/config.yaml`.
    pub fn config_path(&self) -> PathBuf {
        self.atlas_dir().join("config.yaml")
    }

    /// `<project_root>/.luuna/atlas/expertise/<domain>.jsonl`, after checking the name.
    pub fn expertise_path(&self, domain: &str) -> Result<PathBuf> {
        config::validate_domain_name(domain)?;
        Ok(self
            .atlas_dir()
            .join("expertise")
            .join(format!("{domain}.jsonl")))
    }

    /// `<project_root>/.luuna/atlas/archive/<domain>.jsonl`, after checking the name.
    pub fn archive_path(&self, domain: &str) -> Result<PathBuf> {
        config::validate_domain_name(domain)?;
        Ok(self
            .atlas_dir()
            .join("archive")
            .join(format!("{domain}.jsonl")))
    }
}
