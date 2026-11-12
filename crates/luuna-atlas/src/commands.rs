//! The commands `init`, `add` and `config show`.
//!
//! Each command has a typed form, returning a result, and a `run_*` form, returning the
//! [`CommandOutput`] the Go tool produces for the same command with `--json --quiet`.
//! Every command takes the project root as a parameter; none runs git or reads the
//! environment to find it. Task t1 declares the commands; task t5 fills them in.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::{AtlasError, Result};
use crate::output::CommandOutput;

/// The result of `init`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitResult {
    /// Whether `.luuna/atlas/` did not exist before.
    pub created: bool,
    /// The store directory, `<project_root>/.luuna/atlas`.
    pub path: PathBuf,
}

/// The result of `add`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddResult {
    pub domain: String,
}

/// Creates the store, or fills in whatever is missing from it.
pub fn init(_project_root: &Path) -> Result<InitResult> {
    Err(AtlasError::not_implemented("commands::init (task t5)"))
}

/// Adds a domain to an existing store.
pub fn add(_project_root: &Path, _domain: &str) -> Result<AddResult> {
    Err(AtlasError::not_implemented("commands::add (task t5)"))
}

/// The store's config with defaults merged in.
pub fn config_show(_project_root: &Path) -> Result<Config> {
    Err(AtlasError::not_implemented(
        "commands::config_show (task t5)",
    ))
}

/// `luuna-atlas init --json --quiet`.
pub fn run_init(_project_root: &Path) -> CommandOutput {
    CommandOutput::not_implemented("init")
}

/// `luuna-atlas add <domain> --json --quiet`.
pub fn run_add(_project_root: &Path, _domain: &str) -> CommandOutput {
    CommandOutput::not_implemented("add")
}

/// `luuna-atlas config show --json --quiet`.
pub fn run_config_show(_project_root: &Path) -> CommandOutput {
    CommandOutput::not_implemented("config show")
}
