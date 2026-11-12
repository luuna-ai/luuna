//! `block <id> --by <blocker>` (`internal/cli/issue_links.go:161-190`).
//!
//! Ported in task t4.

use super::{Context, Failure};
use crate::error::{Error, THIS_PLAN};

pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let _ = (context, args);
    Err(Error::unsupported("block", THIS_PLAN).into())
}
