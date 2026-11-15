//! Embedded Atlas subprocess policy; raw calls retain reference behavior.
use crate::options::Env;
pub(crate) use luuna_process::capture as managed;
use std::{
    io,
    process::{Command, Output},
    time::Duration,
};
pub(crate) fn output(command: &mut Command, env: &Env) -> io::Result<Output> {
    if !env.managed_children() {
        return command.output();
    }
    managed(command, None, Duration::from_secs(30), 4 * 1024 * 1024)
}
