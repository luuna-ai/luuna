//! Native reader and writer for `.luuna/missions/`, ported from the Go
//! `luuna-missions` tool at commit 2fbbf8e5b6a238ea47543e1dad112fe1716faec4.
//!
//! The crate copies the Go tool exactly: the stored records, the lock files
//! and the output of every command it ports are byte for byte what Go
//! produces. Behaviour that is not ported yet returns
//! [`Error::Unsupported`] naming the plan that ports it; it never silently
//! differs from Go.
//!
//! Records are called tickets in this API ([`Ticket`]); on disk they keep
//! their Go names (`issues.jsonl`, the JSON key `issue`).
//!
//! There is no binary. Commands run as library calls through
//! [`command::run`], which takes an [`command::Invocation`] (arguments,
//! working directory, stdin, environment and an optional explicit store
//! directory) and returns an [`command::Outcome`] (stdout, stderr and exit
//! code). See `README.md` and `UPSTREAM.md` in the crate.

pub mod command;
mod config;
mod error;
mod gopath;
mod json;
mod json_error;
pub mod model;
pub mod store;

pub use error::{Error, Result};
pub use model::Ticket;
pub use store::{DataFile, Store, StoreLock};
