//! Native search.
use crate::{options::Context, CommandOutput};
#[derive(Clone, Debug, Default)]
pub struct SearchOptions {
    pub flags: Vec<(String, String)>,
}
pub fn run_search(_ctx: &Context, _query: Option<&str>, _options: &SearchOptions) -> CommandOutput {
    CommandOutput::failure("search", "not implemented: search")
}
