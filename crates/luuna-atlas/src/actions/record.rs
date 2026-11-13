//! Native record command.
use crate::{options::Context, CommandOutput};
#[derive(Clone, Debug, Default)]
pub struct RecordOptions {
    pub content: Option<String>,
    pub flags: Vec<(String, String)>,
}
pub fn run_record(_ctx: &Context, _domain: &str, _options: &RecordOptions) -> CommandOutput {
    CommandOutput::failure("record", "not implemented: record")
}
