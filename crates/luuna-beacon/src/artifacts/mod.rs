//! Owner: plan s3-beacon-lib-3.
//!
//! This folder will hold the artifact commands: skill, agent and bundle
//! artifacts, doctor, prime, onboard and sync.
//!
//! The plan declares its submodules here, re-exports its public items here
//! and adds its operations as `impl Beacon` blocks in this folder. Public
//! names are prefixed with `Artifact`, so the glob re-exports of the two
//! later plans in `lib.rs` cannot clash.

/// Errors of the artifact commands. Empty until the plan adds variants.
#[derive(Debug)]
pub enum ArtifactsError {}

impl std::fmt::Display for ArtifactsError {
    fn fmt(&self, _formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {}
    }
}

impl std::error::Error for ArtifactsError {}
