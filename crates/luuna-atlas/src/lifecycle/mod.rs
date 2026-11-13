//! Native Atlas lifecycle and config operations, with explicit invocation state.
use crate::{options::Context, CommandOutput};
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub flags: Vec<(String, String)>,
    pub input: Vec<u8>,
}
pub fn run(_ctx: &Context, command: &str, _args: &[String], _options: &Options) -> CommandOutput {
    CommandOutput::failure(command, &format!("not implemented: {command}"))
}
