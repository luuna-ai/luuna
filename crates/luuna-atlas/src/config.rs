//! config.yaml: reading with the Go tool's defaults, writing, and adding domains.
//!
//! Every function takes the store's `.luuna/atlas` directory as a parameter. Task t1
//! declares the functions; task t4 fills them in.

use std::path::Path;

use crate::error::{AtlasError, Result};
use crate::json::Object;

/// A parsed config.yaml, keys in file order.
pub type Config = Object;

/// The line `init` adds to the project's `.gitattributes`.
pub const GIT_ATTRIBUTES_LINE: &str = ".luuna/atlas/expertise/*.jsonl merge=union";

/// Refuses a domain name that does not match `^[a-zA-Z0-9][a-zA-Z0-9_-]*$`, with the Go
/// tool's message.
pub fn validate_domain_name(_domain: &str) -> Result<()> {
    Err(AtlasError::not_implemented(
        "config::validate_domain_name (task t4)",
    ))
}

/// Reads `<atlas_dir>/config.yaml` and merges the Go tool's defaults into it.
pub fn read_config(_atlas_dir: &Path) -> Result<Config> {
    Err(AtlasError::not_implemented("config::read_config (task t4)"))
}

/// Rewrites `<atlas_dir>/config.yaml` from `config`.
pub fn write_config(_atlas_dir: &Path, _config: &Config) -> Result<()> {
    Err(AtlasError::not_implemented(
        "config::write_config (task t4)",
    ))
}

/// Adds `domain` to config.yaml if absent and creates its empty expertise file if absent.
pub fn add_domain(_atlas_dir: &Path, _domain: &str) -> Result<()> {
    Err(AtlasError::not_implemented("config::add_domain (task t4)"))
}
