//! `create` (`internal/cli/issue_create.go:102-206`).
//!
//! Ported in task t4.

use super::{Context, Failure};
use crate::error::{Error, THIS_PLAN};

pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let _ = (context, args);
    Err(Error::unsupported("create", THIS_PLAN).into())
}
