//! Owner: plan s3-beacon-lib-2.
//!
//! This folder will hold the authoring commands: markdown render with
//! yaml.v3-style frontmatter, emit, import, pin, unpin, diff, tree, stats and
//! config show, set, unset and schema.
//!
//! The plan declares its submodules here, re-exports its public items here
//! and adds its operations as `impl Beacon` blocks in this folder. Public
//! names are prefixed with `Authoring`, so the glob re-exports of the two
//! later plans in `lib.rs` cannot clash.

/// Errors of the authoring commands. Empty until the plan adds variants.
#[derive(Debug)]
pub enum AuthoringError {}

impl std::fmt::Display for AuthoringError {
    fn fmt(&self, _formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {}
    }
}

impl std::error::Error for AuthoringError {}
