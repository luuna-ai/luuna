//! Native edit command.
use crate::{options::Context, CommandOutput};
#[derive(Clone, Debug, Default)]
pub struct EditOptions {
    pub flags: Vec<(String, String)>,
}
pub fn run_edit(_ctx: &Context, _domain: &str, _id: &str, _options: &EditOptions) -> CommandOutput {
    CommandOutput::failure("edit", "not implemented: edit")
}
