//! Owner: plan s3-beacon-lib-3.
//!
//! This folder will hold the artifact commands: skill, agent and bundle
//! artifacts, doctor, prime, onboard and sync.
//!
//! The plan declares its submodules here, re-exports its public items here
//! and adds its operations as `impl Beacon` blocks in this folder. Public
//! names start with `Artifact`, `Doctor`, `Prime`, `Onboard` (and `ONBOARD_`
//! for the constant), `Git` or `Sync`, so the glob re-exports of the two
//! other library plans in `lib.rs` cannot clash.

// The codec and the snapshot are used in part by the commands the later
// tasks of the plan add; until then only their unit tests call the rest.
#[allow(dead_code)]
mod base64;
mod commands;
mod doctor;
mod export;
#[allow(dead_code)]
mod model;
mod onboard;
mod prime;
#[allow(dead_code)]
mod snapshot;

pub use commands::{ArtifactHistory, ArtifactRead};
pub use doctor::{DoctorCheck, DoctorReport, DoctorStatus, DoctorSummary};
pub use export::{ArtifactExport, ArtifactExported};
pub use model::{Artifact, ArtifactFile};
pub use onboard::{OnboardAction, OnboardMode, OnboardStatus, Onboarded, ONBOARD_SNIPPET};
pub use prime::{Prime, PrimeOptions};

use crate::gofmt::quote;
use std::path::PathBuf;

/// The three kinds of artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArtifactKind {
    Skill,
    Agent,
    Bundle,
}

impl ArtifactKind {
    /// The kind as stored in a record and written in messages.
    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactKind::Skill => "skill",
            ArtifactKind::Agent => "agent",
            ArtifactKind::Bundle => "bundle",
        }
    }
}

/// Errors of the artifact commands. The text of each variant is Go's message
/// (`cli.go`, `artifact.go`), or, for the last three, the refusal text the
/// plan fixes for the data-loss cases it adds (RD55-RD57).
#[derive(Debug, thiserror::Error)]
pub enum ArtifactsError {
    #[error("artifact subcommand required")]
    SubcommandRequired,
    #[error("unknown {kind} subcommand: {subcommand}")]
    UnknownSubcommand { kind: String, subcommand: String },
    #[error("invalid artifact record {kind} {name}@{version}: {cause}")]
    InvalidRecord {
        kind: String,
        name: String,
        version: i64,
        cause: String,
    },
    #[error("conflicting artifact version: {kind} {name}@{version}")]
    ConflictingVersion {
        kind: String,
        name: String,
        version: i64,
    },
    #[error("{kind} {} not found", quote(.reference))]
    NotFound { kind: String, reference: String },
    #[error("source path required")]
    SourcePathRequired,
    #[error("--name is required")]
    NameRequired,
    #[error("name and source path required")]
    NameAndSourceRequired,
    #[error("artifact name required")]
    ArtifactNameRequired,
    #[error("file path required")]
    FilePathRequired,
    #[error("file {} not found", quote(.0))]
    FileNotFound(String),
    #[error("--out is required for this export")]
    OutRequired,
    #[error("refusing to export over a project or Beacon data directory")]
    RefusingDataDirectory,
    #[error("destination already exists: {}", .0.display())]
    DestinationExists(PathBuf),
    #[error("agent export requires exactly one stored file")]
    AgentExportNeedsOneFile,
    #[error("refusing to replace an export not managed by {kind} {}", quote(.name))]
    NotManaged { kind: String, name: String },
    #[error("artifact name must be 1-80 lowercase letters, digits, dots, dashes, or underscores, beginning with a letter or digit")]
    InvalidName,
    #[error("source symlinks are not allowed")]
    SourceSymlink,
    #[error("source symlinks are not allowed: {0}")]
    SourceSymlinkAt(String),
    #[error("artifact file limit exceeded")]
    FileLimit,
    #[error("artifact file exceeds 4 MiB: {0}")]
    TooLarge(String),
    #[error("credential-like file rejected: {0}")]
    CredentialLike(String),
    #[error("skill must contain SKILL.md")]
    SkillNeedsSkillMd,
    #[error("bundle must contain .claude-plugin/plugin.json")]
    BundleNeedsPlugin,
    #[error("invalid artifact kind")]
    InvalidKind,
    #[error("invalid artifact name: {0}")]
    InvalidArtifactName(String),
    #[error("invalid artifact record")]
    InvalidArtifactRecord,
    #[error("artifact hash mismatch: {0}")]
    HashMismatch(String),
    #[error("artifact manifest hash mismatch")]
    ManifestMismatch,
    #[error("illegal base64 data at input byte {offset}")]
    Base64 { offset: usize },
    #[error("doctor found failures")]
    DoctorFailed,
    #[error("git {step}: {output}")]
    Git { step: String, output: String },
    /// RD55: `cause` is the operating system's error text.
    #[error("refusing to overwrite {}: it cannot be read: {cause}", .file.display())]
    OnboardUnreadable { file: PathBuf, cause: String },
    /// RD56: the path is shown with U+FFFD for each invalid byte.
    #[error("refusing to store a path that is not valid UTF-8: {0}")]
    NonUtf8Path(String),
    /// RD57.
    #[error("refusing to overwrite {} outside the export directory", .0.display())]
    ExportWouldOverwrite(PathBuf),
}

impl From<ArtifactsError> for crate::BeaconError {
    fn from(error: ArtifactsError) -> Self {
        crate::BeaconError::Artifacts(error)
    }
}
