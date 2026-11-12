//! Native Rust implementation of Beacon, measured against the Go tool.
//!
//! Beacon originates in Canopy, by Jaymin West, released under the MIT
//! licence (see `LICENSE-CANOPY` in this crate's folder). This crate is an
//! independent Rust implementation; it contains no Go source. See
//! `README.md` for provenance, scope and how to run the reference tests.

// The format and store layer. Its crate-private helpers are used by the
// command modules built on top of it; until every one of them is in place,
// some helpers have no caller yet.
#[allow(dead_code)]
mod config;
mod error;
#[allow(dead_code)]
mod gofmt;
#[allow(dead_code)]
mod gojson;
#[allow(dead_code)]
mod gotime;
mod model;
#[allow(dead_code)]
mod paths;
#[allow(dead_code)]
mod store;

pub use error::BeaconError;
pub use gotime::{Clock, SystemClock};
pub use model::{Atlas, AtlasPrime, Prompt, Rule, Schema, Section};
