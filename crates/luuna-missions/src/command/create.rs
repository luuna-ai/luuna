//! `create` (`internal/cli/issue_create.go:102-206`).
//!
//! Ported in task t4.

use super::{Invocation, Outcome};
use crate::error::{Error, Result, THIS_PLAN};

pub(super) fn run(invocation: &Invocation) -> Result<Outcome> {
    let _ = invocation;
    Err(Error::unsupported("create", THIS_PLAN))
}
