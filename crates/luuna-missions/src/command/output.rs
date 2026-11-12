//! Success and error envelopes and the `--format` mode
//! (`internal/cli/app.go`, `internal/cli/issue_read.go:129-143`).
//!
//! - Every JSON payload goes through Go's `Context.JSON`: a `json.Encoder`
//!   with HTML escaping off and two-space indentation, ending in a newline.
//!   An [`Envelope`] is built compact, field by field in Go's struct order,
//!   and printed with that encoder's rules. Record objects embedded with
//!   [`Envelope::raw`] keep the escapes `json.Marshal` gave them (SD4).
//! - A command error prints `{"success":false,"command":<name>,"error":<message>}`
//!   on stdout only when the literal `--json` token is among the arguments;
//!   otherwise, including under `--format json` alone, it prints
//!   `Error: <message>` on stderr (SD5). The dispatcher in `mod.rs` applies
//!   this.

use crate::json;

/// A JSON object written field by field, in the order Go's struct declares
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Envelope {
    compact: Vec<u8>,
}

impl Envelope {
    /// `{"success":<success>,"command":<command>`, the start of every
    /// envelope.
    pub fn new(success: bool, command: &str) -> Self {
        Self {
            compact: b"{".to_vec(),
        }
        .boolean("success", success)
        .string("command", command)
    }

    /// An object with no fields yet, for payloads Go builds as a map and so
    /// writes with its keys sorted.
    pub fn object() -> Self {
        Self {
            compact: b"{".to_vec(),
        }
    }

    fn key(&mut self, key: &str) {
        if self.compact.len() > 1 {
            self.compact.push(b',');
        }
        json::quote(&mut self.compact, key, false);
        self.compact.push(b':');
    }

    /// Adds a string field, written as the encoder writes it (HTML
    /// characters literal).
    pub fn string(mut self, key: &str, value: &str) -> Self {
        self.key(key);
        json::quote(&mut self.compact, value, false);
        self
    }

    /// Adds a boolean field.
    pub fn boolean(mut self, key: &str, value: bool) -> Self {
        self.key(key);
        self.compact
            .extend_from_slice(if value { b"true" } else { b"false" });
        self
    }

    /// Adds an integer field.
    pub fn integer(mut self, key: &str, value: i64) -> Self {
        self.key(key);
        self.compact.extend_from_slice(value.to_string().as_bytes());
        self
    }

    /// Adds a field whose value is already compact JSON, such as a record
    /// from [`Ticket::marshal_json`](crate::Ticket::marshal_json). Its
    /// strings are kept as they are.
    pub fn raw(mut self, key: &str, compact: &[u8]) -> Self {
        self.key(key);
        self.compact.extend_from_slice(compact);
        self
    }

    /// What `Context.JSON` prints for the envelope: indented by two spaces,
    /// with a final newline.
    pub fn render(mut self) -> Vec<u8> {
        self.compact.push(b'}');
        // Every part was written as valid JSON, or handed in as compact JSON
        // by the caller; anything else is a bug in this crate.
        json::to_output(&self.compact).expect("an envelope is valid JSON")
    }
}

/// The output formats of the read commands (`issueFormats`).
pub(crate) const FORMATS: [&str; 5] = ["markdown", "compact", "plain", "ids", "json"];

/// Go's `formatMode`: the last `--format` value, else `json` with the
/// `--json` switch, else `markdown`; any other value is Go's error message.
/// `--format json` selects JSON output only; error output still follows the
/// literal `--json` token (SD5).
pub(crate) fn format_mode(options: &super::options::Options) -> Result<&'static str, String> {
    let mut mode = options.value("format");
    if mode.is_empty() && options.has("json") {
        mode = "json";
    }
    if mode.is_empty() {
        mode = "markdown";
    }
    FORMATS
        .into_iter()
        .find(|valid| *valid == mode)
        .ok_or_else(|| {
            format!("Invalid --format value: {mode}. Valid: markdown|compact|plain|ids|json")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_envelope_prints_as_the_go_encoder_does() {
        let rendered = Envelope::new(true, "init")
            .string("dir", "/tmp/a <b> & c\u{2028}")
            .integer("count", 2)
            .raw("issue", br#"{"title":"\u003ci\u003e","n":[1,{}]}"#)
            .render();
        assert_eq!(
            String::from_utf8(rendered).unwrap(),
            "{\n  \"success\": true,\n  \"command\": \"init\",\n  \"dir\": \"/tmp/a <b> & c\\u2028\",\n  \"count\": 2,\n  \"issue\": {\n    \"title\": \"\\u003ci\\u003e\",\n    \"n\": [\n      1,\n      {}\n    ]\n  }\n}\n"
        );
    }

    #[test]
    fn the_format_mode_follows_go() {
        use super::super::options::parse;
        let mode = |args: &[&str]| {
            let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
            format_mode(&parse(&args, &["format"], &["json"]).unwrap())
        };
        assert_eq!(mode(&[]), Ok("markdown"));
        assert_eq!(mode(&["--json"]), Ok("json"));
        assert_eq!(mode(&["--format", "ids"]), Ok("ids"));
        assert_eq!(mode(&["--json", "--format", "plain"]), Ok("plain"));
        assert_eq!(mode(&["--format=json"]), Ok("json"));
        assert_eq!(
            mode(&["--format", "xml"]),
            Err("Invalid --format value: xml. Valid: markdown|compact|plain|ids|json".to_owned())
        );
    }
}
