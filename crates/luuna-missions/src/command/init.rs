//! `init`: creates `.luuna/missions/` and the merge=union lines in the
//! project's `.gitattributes` (`internal/cli/issue_create.go:19-50`,
//! `internal/config/config.go:115-172`).
//!
//! Ported in task t3.

use super::{Invocation, Outcome};
use crate::error::{Error, Result, THIS_PLAN};

pub(super) fn run(invocation: &Invocation) -> Result<Outcome> {
    let _ = invocation;
    Err(Error::unsupported("init", THIS_PLAN))
}
