//! Native Rust implementation of Beacon, measured against the Go tool.
//!
//! Beacon originates in Canopy, by Jaymin West, released under the MIT
//! licence (see `LICENSE-CANOPY` in this crate's folder). This crate is an
//! independent Rust implementation; it contains no Go source. See
//! `README.md` for provenance, scope and how to run the reference tests.

mod artifacts;
mod authoring;
mod beacon;
mod config;
mod error;
mod gofmt;
// `write_any` is kept for the frontmatter emitter of a later plan.
#[allow(dead_code)]
mod gojson;
mod gotime;
mod model;
mod owned_json;
// `artifacts_file` is kept for the artifact commands of a later plan.
#[allow(dead_code)]
mod paths;
mod payload;
mod prompts;
mod re2;
mod resolve;
mod schemas;
mod store;

pub use artifacts::*;
pub use authoring::*;
pub use beacon::{Beacon, Init};
pub use error::BeaconError;
pub use gotime::{Clock, SystemClock};
pub use model::{Atlas, AtlasPrime, Prompt, Rule, Schema, Section};
pub use owned_json::JsonValue;
pub use payload::{Rendered, Validation};
pub use prompts::{CreatePrompt, ListFilter, UpdatePrompt};
pub use schemas::CreateSchema;
