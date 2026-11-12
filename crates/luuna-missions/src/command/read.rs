//! `show`, `list` and `search` with every output format
//! (`internal/cli/issue_read.go`, `internal/cli/issue_query.go`). Plan
//! context is refused until s4-missions-lib-4.
//!
//! Ported in task t5.

use super::{Context, Failure};
use crate::error::{Error, THIS_PLAN};

pub(super) fn show(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let _ = (context, args);
    Err(Error::unsupported("show", THIS_PLAN).into())
}

pub(super) fn list(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let _ = (context, args);
    Err(Error::unsupported("list", THIS_PLAN).into())
}

pub(super) fn search(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let _ = (context, args);
    Err(Error::unsupported("search", THIS_PLAN).into())
}
