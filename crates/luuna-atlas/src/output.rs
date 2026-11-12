//! What a command prints: the bytes the Go tool writes when run with `--json --quiet`.
//!
//! Task t1 declares the type; task t5 renders the envelopes.

/// A command's exit code and output streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    /// The output of a command that is declared but not yet ported: exit code 1 and an
    /// explicit message on stderr.
    pub(crate) fn not_implemented(command: &str) -> Self {
        CommandOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: format!(
                "luuna-atlas (Rust port): command `{command}` is not implemented yet (task t5)\n"
            ),
        }
    }
}
