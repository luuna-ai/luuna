//! The record type registry: the six built-in types and the custom types and field
//! aliases declared under `custom_types` in config.yaml.
//!
//! Task t1 declares the type; task t3 fills it in.

use crate::config::Config;
use crate::error::{AtlasError, Result};

/// The record types known to one store.
#[derive(Debug, Clone)]
pub struct Registry {
    _types: (),
}

impl Registry {
    /// Builds the registry for a store. `None` gives the built-in types only, as the Go
    /// tool does when no config could be read.
    pub fn from_config(_config: Option<&Config>) -> Result<Registry> {
        Err(AtlasError::not_implemented(
            "Registry::from_config (task t3)",
        ))
    }
}
