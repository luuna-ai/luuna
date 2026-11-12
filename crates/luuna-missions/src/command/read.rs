//! `show`, `list` and `search` with every output format
//! (`internal/cli/issue_read.go`, `internal/cli/issue_query.go`). Plan
//! context is refused until s4-missions-lib-4.
//!
//! Ported in task t5.

use super::{Invocation, Outcome};
use crate::error::{Error, Result, THIS_PLAN};

pub(super) fn show(invocation: &Invocation) -> Result<Outcome> {
    let _ = invocation;
    Err(Error::unsupported("show", THIS_PLAN))
}

pub(super) fn list(invocation: &Invocation) -> Result<Outcome> {
    let _ = invocation;
    Err(Error::unsupported("list", THIS_PLAN))
}

pub(super) fn search(invocation: &Invocation) -> Result<Outcome> {
    let _ = invocation;
    Err(Error::unsupported("search", THIS_PLAN))
}
