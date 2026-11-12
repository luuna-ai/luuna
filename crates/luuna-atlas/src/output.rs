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

    /// Like [`CommandOutput::json`] for an object, but first applies the Go tool's
    /// `orderCommandOutput` (the `Emit` step): if the object has a non-empty `command`
    /// key, it is reordered as an envelope of that command would be. `config show` can
    /// reach this, since a config may have a `command` key of its own.
    pub fn emit(object: &Object) -> Self {
        let mut object = object.clone();
        order_command_output(&mut object);
        CommandOutput::json(&Value::Object(object))
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

/// The keys that follow `success`, `command` and `error` in a command's envelope
/// (the table in the Go tool's `orderCommandOutput`).
fn command_keys(command: &str) -> &'static [&'static str] {
    match command {
        "init" => &["created", "path"],
        "add" => &["domain"],
        "record" => &[
            "action", "wouldDo", "domain", "type", "index", "record", "created", "updated",
            "skipped", "errors", "warnings",
        ],
        "edit" => &["domain", "id", "type", "record"],
        "query" => &["domains"],
        "show" => &["records"],
        "search" => &["query", "total", "domains"],
        "rank" => &["count", "records"],
        "ready" => &["count", "entries"],
        "status" => &["domains", "governance", "shelf_life"],
        "stale" => &["records"],
        "validate" => &[
            "valid",
            "totalRecords",
            "totalErrors",
            "totalWarnings",
            "errors",
            "warnings",
        ],
        "doctor" => &["checks", "summary", "fixed"],
        "audit" => &["report", "suggestions"],
        "archive" => &["domain", "dryRun", "archived", "kept"],
        "restore" => &["domain", "id", "type", "summary", "archive_reason"],
        "delete" => &[
            "domain", "dryRun", "deleted", "kept", "id", "type", "summary",
        ],
        "delete-domain" => &["domain", "dryRun", "deletedFile", "recordCount"],
        "move" => &[
            "dryRun",
            "sourceDomain",
            "targetDomain",
            "record",
            "incomingReferences",
            "warnings",
        ],
        "outcome" => &[
            "action",
            "domain",
            "id",
            "outcomes",
            "outcome",
            "total_outcomes",
        ],
        "prune" => &[
            "dryRun",
            "hard",
            "aggressive",
            "checkAnchors",
            "totalPruned",
            "totalDemoted",
            "totalAnchorDemoted",
            "totalSupersessionDemoted",
            "results",
            "warnings",
        ],
        "compact" => &[
            "action",
            "candidates",
            "compacted",
            "results",
            "wouldCompact",
            "groups",
            "domain",
            "removed",
            "replacement",
            "warnings",
        ],
        "onboard" => &["file", "action"],
        "setup" => &[
            "target",
            "provider",
            "source",
            "path",
            "recipe",
            "action",
            "providers",
            "message",
        ],
        "sync" => &["validated", "committed", "errors", "message"],
        "learn" => &[
            "changedFiles",
            "suggestedDomains",
            "unmatchedFiles",
            "message",
        ],
        "diff" => &["since", "domains", "message"],
        "upgrade" => &["error", "current", "latest", "up_to_date", "updated"],
        _ => &[],
    }
}

/// The Go tool's `orderCommandOutput`: when `object` has a `command` key whose text
/// (the Go `Text` conversion) is not empty, put `success`, `command`, `error` and that
/// command's own keys first, in that order, and the other keys after them, sorted.
pub(crate) fn order_command_output(object: &mut Object) {
    let command = crate::registry::go_text(object.get("command"));
    if command.is_empty() {
        return;
    }
    let keys = ["success", "command", "error"]
        .into_iter()
        .chain(command_keys(&command).iter().copied());
    object.remember_order(keys);
}

/// `JSONPretty(value)` and the newline `fmt.Fprintln` adds.
fn pretty_line(value: &Value) -> String {
    let mut text = json::to_indented_string(value).unwrap_or_default();
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(text: &str) -> Vec<String> {
        let value = json::parse(text).expect("json");
        value
            .as_object()
            .expect("object")
            .keys()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn emit_reorders_an_object_that_has_a_command_key() {
        let mut object = Object::new();
        object.insert("zeta", 1.0);
        object.insert("path", "p");
        object.insert("command", "init");
        object.insert("alpha", 2.0);
        object.insert("created", true);
        object.insert("success", true);
        let out = CommandOutput::emit(&object);
        assert_eq!(out.exit_code, 0);
        assert_eq!(
            keys(&out.stdout),
            ["success", "command", "created", "path", "alpha", "zeta"]
        );
    }

    #[test]
    fn emit_with_an_unknown_command_sorts_after_the_envelope_keys() {
        let mut object = Object::new();
        object.insert("b", 1.0);
        object.insert("command", "nope");
        object.insert("a", 2.0);
        assert_eq!(
            keys(&CommandOutput::emit(&object).stdout),
            ["command", "a", "b"]
        );
    }

    #[test]
    fn emit_leaves_an_object_without_a_usable_command_alone() {
        let mut object = Object::new();
        object.insert("b", 1.0);
        object.insert("a", 2.0);
        object.insert("command", "");
        let plain = CommandOutput::json(&Value::Object(object.clone()));
        assert_eq!(CommandOutput::emit(&object), plain);
        object.remove("command");
        assert_eq!(
            CommandOutput::emit(&object),
            CommandOutput::json(&Value::Object(object.clone()))
        );
    }
}
