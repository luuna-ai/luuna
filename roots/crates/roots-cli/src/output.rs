//! Human and JSON output behavior shared by command implementations.

use crate::io::Io;
use roots_core::color::{Colors, color_enabled};
use roots_core::output::success_envelope;
use roots_core::serde_json::{Map, Value};

#[derive(Clone, Copy, Debug)]
pub struct OutputOptions<'a> {
    pub command: &'a str,
    pub json: bool,
    pub quiet: bool,
}

/// One output facade per command invocation.
///
/// It deliberately does not hold the `Io` borrow. That lets a command both
/// write structured output and use stdin/terminal capabilities through its
/// context without borrow gymnastics.
#[derive(Clone, Debug)]
pub struct Output {
    pub command: String,
    pub json: bool,
    pub quiet: bool,
    pub colors: Colors,
}

impl Output {
    pub fn new(io: &dyn Io, options: OutputOptions<'_>) -> Self {
        Self {
            command: options.command.to_owned(),
            json: options.json,
            quiet: options.quiet,
            colors: Colors::new(
                !options.json
                    && color_enabled(
                        io.env("NO_COLOR"),
                        io.env("FORCE_COLOR"),
                        io.stdout_is_tty(),
                    ),
            ),
        }
    }

    /// Emit the command success envelope in JSON mode. No-op for human output.
    pub fn result(&self, io: &mut dyn Io, data: Map<String, Value>) {
        if self.json {
            io.stdout(&success_envelope(&self.command, data));
        }
    }

    /// Emit a primary human line. JSON mode intentionally emits no text.
    pub fn line(&self, io: &mut dyn Io, text: &str) {
        if !self.json {
            io.stdout(&format!("{text}\n"));
        }
    }

    pub fn lines(&self, io: &mut dyn Io, texts: &[String]) {
        if !self.json && !texts.is_empty() {
            io.stdout(&format!("{}\n", texts.join("\n")));
        }
    }

    pub fn success(&self, io: &mut dyn Io, message: &str) {
        if !self.json && !self.quiet {
            io.stdout(&format!("{} {message}\n", self.colors.green("✓")));
        }
    }

    pub fn info(&self, io: &mut dyn Io, message: &str) {
        if !self.json && !self.quiet {
            io.stdout(&format!("{message}\n"));
        }
    }

    /// Warnings remain on stderr even in JSON mode, matching the legacy CLI.
    pub fn warn(&self, io: &mut dyn Io, message: &str) {
        if !self.quiet {
            io.stderr(&format!("{} {message}\n", self.colors.yellow("!")));
        }
    }
}
