//! Owner: plan s3-beacon-lib-2.
//!
//! The plan's `PlanStep` and `PlanDone` implementations and the
//! normalisation for its own commands live here. The hook must leave the
//! documents of every other command unchanged.

/// Sorts the arrays of a document of one of this plan's commands that Go
/// builds from map iteration. A no-op until the plan fills it.
pub fn sort_map_ordered(_command: &str, _doc: &mut serde_json::Value, _project: &str) {}
