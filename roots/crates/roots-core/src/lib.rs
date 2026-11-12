//! Domain and persistence implementation for the Luuna Roots CLI.
//!
//! The public CLI contract deliberately remains compatible with the former
//! TypeScript/Bun implementation. This crate owns reusable behavior while the
//! `roots-cli` crate owns process and terminal interaction.

pub const NAME: &str = "@luuna/roots-cli";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod actor;
pub mod args;
pub mod attention;
pub mod blame;
pub mod boundary;
pub mod claims;
pub mod claude_settings;
pub mod color;
pub mod config;
pub mod decisions;
pub mod drift;
pub mod errors;
pub mod events;
pub mod graph;
pub mod graph_rules;
pub mod guard;
pub mod headings;
pub mod hook_guard;
pub mod ids;
pub mod integrations;
pub mod mentions;
pub mod model;
pub mod notes;
pub mod output;
pub mod paths;
pub mod presentation;
pub mod proposals;
pub mod prose;
pub mod questions;
pub mod resolve;
pub mod rules;
pub mod slug;
pub mod sprouts;
pub mod store;
pub mod suggestions;
pub mod tend;
pub mod tier;
pub mod time;
pub mod verify;
pub mod workspace;
pub mod yaml;

/// The time crate is exposed under an alias because this crate also owns the
/// Roots-specific `time` helper module.
pub use ::time as time_crate;
/// Re-export the serialization trait dependency for sibling CLI crates without
/// making every compatibility command repeat it in its manifest.
pub use serde;
/// Re-export the JSON value types used by the stable CLI envelope helpers.
///
/// Keeping these types here means downstream command crates do not need to
/// duplicate the JSON dependency merely to return a Roots-compatible result.
pub use serde_json;
