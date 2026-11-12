//! What a command prints: the bytes the Go tool writes when run with `--json --quiet`.
//!
//! A success envelope is pretty JSON on stdout (`success`, `command`, then the command's
//! own keys). A failure envelope `{success: false, command, error}` goes to stderr with
//! exit code 1.

use crate::json::{self, Object, Value};

/// A command's exit code and output streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    /// A command that succeeded and printed `value` as pretty JSON and a newline.
    pub fn json(value: &Value) -> Self {
        CommandOutput {
            exit_code: 0,
            stdout: pretty_line(value),
            stderr: String::new(),
        }
    }

    /// A command that succeeded: `fields` after `success` and `command`, in the order
    /// `order` lists (the Go tool's `orderCommandOutput`); other keys follow, sorted.
    pub fn success(command: &str, fields: Object, order: &[&str]) -> Self {
        let mut envelope = Object::new();
        for (key, value) in fields.iter() {
            envelope.insert(key, value.clone());
        }
        envelope.insert("success", true);
        envelope.insert("command", command);
        let keys = ["success", "command", "error"]
            .into_iter()
            .chain(order.iter().copied());
        envelope.remember_order(keys);
        CommandOutput::json(&Value::Object(envelope))
    }

    /// A command that failed in `--json` mode: the failure envelope on stderr, exit 1.
    pub fn failure(command: &str, message: &str) -> Self {
        let mut envelope = Object::new();
        envelope.insert("success", false);
        envelope.insert("command", command);
        envelope.insert("error", message);
        envelope.remember_order(["success", "command", "error"]);
        CommandOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: pretty_line(&Value::Object(envelope)),
        }
    }

    /// A failure that the Go tool prints as plain text on stderr (a line and a newline),
    /// exit 1, with no envelope.
    pub fn plain_failure(message: &str) -> Self {
        CommandOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: format!("{message}\n"),
        }
    }

    /// Puts `text` (a line without its newline) before what the command wrote on stderr,
    /// as the Go tool's migration notice comes before everything else.
    pub fn with_stderr_prefix(mut self, text: &str) -> Self {
        self.stderr = format!("{text}\n{}", self.stderr);
        self
    }
}

/// `JSONPretty(value)` and the newline `fmt.Fprintln` adds.
fn pretty_line(value: &Value) -> String {
    let mut text = json::to_indented_string(value).unwrap_or_default();
    text.push('\n');
    text
}
