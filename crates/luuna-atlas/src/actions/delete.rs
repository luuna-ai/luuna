//! Native deletion.
use crate::{options::Context, CommandOutput};
#[derive(Clone, Debug, Default)]
pub struct DeleteOptions {
    pub records: Option<String>,
    pub all_except: Option<String>,
    pub dry_run: bool,
}
pub fn run_delete(
    _ctx: &Context,
    _domain: &str,
    _id: Option<&str>,
    _options: &DeleteOptions,
) -> CommandOutput {
    CommandOutput::failure("delete", "not implemented: delete")
}
