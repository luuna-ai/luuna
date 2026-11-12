//! Native Rust implementation of Beacon, measured against the Go tool.
//!
//! Beacon originates in Canopy, by Jaymin West, released under the MIT
//! licence (see `LICENSE-CANOPY` in this crate's folder). This crate is an
//! independent Rust implementation; it contains no Go source. See
//! `README.md` for provenance, scope and how to run the reference tests.

mod beacon;
mod config;
mod error;
mod gofmt;
// Helpers used by the schema and render code of the next task.
#[allow(dead_code)]
mod gojson;
mod gotime;
mod model;
#[allow(dead_code)]
mod paths;
mod prompts;
mod store;

pub use beacon::{Beacon, Init};
pub use error::BeaconError;
pub use gotime::{Clock, SystemClock};
pub use model::{Atlas, AtlasPrime, Prompt, Rule, Schema, Section};
pub use prompts::{CreatePrompt, ListFilter, UpdatePrompt};
