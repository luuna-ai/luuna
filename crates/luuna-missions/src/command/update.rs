//! `update` (`internal/cli/issue_mutate.go:137-312`). Plan transitions are
//! refused until s4-missions-lib-4.
//!
//! Ported in task t4.

use super::{Context, Failure};
use crate::error::{Error, THIS_PLAN};

pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let _ = (context, args);
    Err(Error::unsupported("update", THIS_PLAN).into())
}
