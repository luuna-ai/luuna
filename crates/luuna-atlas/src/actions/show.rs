//! Native record lookup.
use crate::{options::Context, CommandOutput};
pub fn run_show(_ctx: &Context, _ids: &[String]) -> CommandOutput {
    CommandOutput::failure("show", "not implemented: show")
}
