//! `block <id> --by <blocker>` (`internal/cli/issue_links.go:161-190`).
//!
//! Ported in task t4.

use super::{Invocation, Outcome};
use crate::error::{Error, Result, THIS_PLAN};

pub(super) fn run(invocation: &Invocation) -> Result<Outcome> {
    let _ = invocation;
    Err(Error::unsupported("block", THIS_PLAN))
}
