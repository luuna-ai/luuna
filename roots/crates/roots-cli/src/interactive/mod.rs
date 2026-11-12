//! Terminal-facing building blocks for Roots' human interactive commands.
//!
//! Keeping these pieces below the command handlers makes the persistence
//! layer deterministic: the terminal, watcher, reducer, and renderer can all
//! be driven with in-memory fakes in tests.

pub mod flow;
pub mod flow_render;
pub mod flow_session;
pub mod heading_run;
pub mod prompt_contract;
pub mod screen;
pub mod session;
pub mod spinner;
pub mod tend_render;
pub mod tend_session;
pub mod terminal;
pub mod think_session;
pub mod watch;
