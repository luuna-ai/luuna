//! The Go tool's commands as library calls.
//!
//! [`run`] takes an [`Invocation`] (the arguments Go's `main` would get,
//! the working directory, stdin, the environment and an optional explicit
//! store directory) and returns an [`Outcome`] with exactly the stdout,
//! stderr and exit code the Go tool produces. A failure that Go reports to
//! its user is an `Outcome` with exit code 1, not an `Err`. `Err` means the
//! invocation needs behaviour that is not ported yet
//! ([`Error::Unsupported`](crate::Error::Unsupported)) or that the library
//! itself failed.
//!
//! This plan ports init, create, show, list, search, update and block.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::error::{Error, Result, THIS_PLAN};

mod block;
mod create;
mod init;
mod options;
mod output;
mod read;
mod update;

/// One run of a command.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Invocation {
    /// The arguments after the program name, as Go's `os.Args[1:]`.
    pub args: Vec<String>,
    /// The working directory, used for store discovery and in output.
    pub cwd: PathBuf,
    /// The bytes on stdin.
    pub stdin: Vec<u8>,
    /// The complete environment the command sees. Nothing is inherited from
    /// the calling process.
    pub env: BTreeMap<String, String>,
    /// An explicit `.luuna/missions` directory, used instead of discovery.
    pub store_dir: Option<PathBuf>,
}

impl Invocation {
    /// An invocation of `args` in `cwd`, with empty stdin, an empty
    /// environment and store discovery.
    pub fn new<I, S>(cwd: impl Into<PathBuf>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            args: args.into_iter().map(Into::into).collect(),
            cwd: cwd.into(),
            stdin: Vec::new(),
            env: BTreeMap::new(),
            store_dir: None,
        }
    }

    /// Sets the bytes on stdin.
    pub fn with_stdin(mut self, stdin: impl Into<Vec<u8>>) -> Self {
        self.stdin = stdin.into();
        self
    }

    /// Replaces the environment.
    pub fn with_env(mut self, env: BTreeMap<String, String>) -> Self {
        self.env = env;
        self
    }

    /// Uses `dir` as the store directory instead of discovering it.
    pub fn with_store_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.store_dir = Some(dir.into());
        self
    }
}

/// What a command printed and how it exited.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Bytes written to stdout.
    pub stdout: Vec<u8>,
    /// Bytes written to stderr.
    pub stderr: Vec<u8>,
    /// The process exit code Go would use.
    pub exit_code: i32,
}

/// Runs one command as the Go tool would.
pub fn run(invocation: &Invocation) -> Result<Outcome> {
    let name = invocation
        .args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .map(String::as_str);
    match name {
        Some("init") => init::run(invocation),
        Some("create") => create::run(invocation),
        Some("update") => update::run(invocation),
        Some("block") => block::run(invocation),
        Some("show") => read::show(invocation),
        Some("list") => read::list(invocation),
        Some("search") => read::search(invocation),
        _ => Err(Error::unsupported("command dispatch", THIS_PLAN)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_builder_sets_every_field() {
        let env = BTreeMap::from([("USER".to_owned(), "parity".to_owned())]);
        let invocation = Invocation::new("/work", ["list", "--json"])
            .with_stdin("input")
            .with_env(env.clone())
            .with_store_dir("/work/.luuna/missions");
        assert_eq!(invocation.args, ["list", "--json"]);
        assert_eq!(invocation.cwd, PathBuf::from("/work"));
        assert_eq!(invocation.stdin, b"input");
        assert_eq!(invocation.env, env);
        assert_eq!(
            invocation.store_dir,
            Some(PathBuf::from("/work/.luuna/missions"))
        );
    }
}
