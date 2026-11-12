//! The fixture groups of s4-missions-lib-4, the plan that owns this file.
//!
//! s4-missions-lib-4 adds a [`Group`] here for each fixture it moves into
//! scope, using only files from its own list in [`super::DEFERRED`], and
//! sets [`IN_SCOPE_COUNT`] to the number of files in its groups. Each group
//! is replayed by a `parity_*` wrapper in
//! `tests/later_plans/s4_missions_lib_4.rs`.

use super::Group;

/// The fixtures s4-missions-lib-4 has moved into scope, by parity test.
pub const IN_SCOPE: &[Group] = &[];

/// How many fixtures s4-missions-lib-4 has moved into scope.
pub const IN_SCOPE_COUNT: usize = 0;
