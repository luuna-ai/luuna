//! `update` (`internal/cli/issue_mutate.go:137-312`). Plan transitions are
//! refused until s4-missions-lib-4.
//!
//! Ported in task t4.

use super::{Invocation, Outcome};
use crate::error::{Error, Result, THIS_PLAN};

pub(super) fn run(invocation: &Invocation) -> Result<Outcome> {
    let _ = invocation;
    Err(Error::unsupported("update", THIS_PLAN))
}
