//! Native prime, Git commands and hook operations with explicit invocation state.
pub mod active;
pub mod command;
pub mod format;
pub mod git_commands;
mod go_sort;
pub mod guard;
pub mod inject;
use crate::{options::Context, CommandOutput};
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub flags: Vec<(String, String)>,
    pub input: Vec<u8>,
}
pub fn run(ctx: &Context, command: &str, args: &[String], options: &Options) -> CommandOutput {
    crate::actions::record::run(ctx, command, |a| match command {
        "prime" => self::command::run(a, args, &options.flags),
        "learn" => git_commands::learn(a, &options.flags),
        "diff" => git_commands::diff(a, &options.flags),
        "sync" => git_commands::sync(a, &options.flags),
        "hook" => {
            a.output.stdout.push_str(&inject::hook_output(
                &options.input,
                a.ctx,
                Some(&a.registry),
                a.ctx.globals.allow_unknown_types,
            ));
            Ok(())
        }
        _ => Err(crate::actions::record::message(format!(
            "not implemented: {command}"
        ))),
    })
}
