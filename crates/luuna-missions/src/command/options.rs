//! The hand-written, Commander-compatible option parser
//! (`internal/cli/options.go`), with its quirks copied:
//!
//! - `--quiet`, `--verbose`, `--timing` and `--help` are switches of every
//!   command, and `-q` and `-h` are the only single-dash options; any other
//!   argument starting with `-` is an unknown option.
//! - An unknown option is never printed as JSON, even with `--json` (SD6):
//!   it is [`OptionError::Unknown`], which the dispatcher prints as
//!   `error: unknown option '<arg>'` on stderr.
//! - A value flag takes the next argument unless that begins with `--`, in
//!   which case the value is reported as missing (SD7); `--k=v` passes any
//!   value. A missing value is an ordinary command error.
//! - A switch given as `--switch=value` is an unknown option.
//! - `--` ends the options; everything after it is positional.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// The switches every command accepts.
const GLOBAL_SWITCHES: [&str; 4] = ["quiet", "verbose", "timing", "help"];

/// The parsed options (Go's `Options`). Repeated value flags keep every
/// value in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Options {
    /// Values of value flags, by name without `--`.
    pub values: BTreeMap<String, Vec<String>>,
    /// Switches that were given, by name without `--`.
    pub switches: BTreeSet<String>,
    /// Positional arguments, in order.
    pub positionals: Vec<String>,
}

impl Options {
    /// The last value of `name`, or `""` (Go's `Value`).
    pub fn value(&self, name: &str) -> &str {
        self.values
            .get(name)
            .and_then(|values| values.last())
            .map_or("", String::as_str)
    }

    /// Every value of `name`, in order (Go's `Repeated`).
    pub fn repeated(&self, name: &str) -> &[String] {
        self.values.get(name).map_or(&[], Vec::as_slice)
    }

    /// Whether `name` was given as a switch or with a value (Go's `Has`).
    pub fn has(&self, name: &str) -> bool {
        self.switches.contains(name) || !self.repeated(name).is_empty()
    }
}

/// Why the arguments could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OptionError {
    /// Go's `unknownOptionError`: printed on stderr, never as JSON (SD6).
    Unknown(String),
    /// A value flag without a value (SD7 for a next argument beginning with
    /// `--`): an ordinary command error.
    Missing(String),
}

impl fmt::Display for OptionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(option) => write!(formatter, "unknown option '{option}'"),
            Self::Missing(key) => write!(formatter, "option '--{key}' argument missing"),
        }
    }
}

/// Go's `ParseOptions(args, valueFlags, switchFlags)`.
pub(crate) fn parse(
    args: &[String],
    value_flags: &[&str],
    switch_flags: &[&str],
) -> Result<Options, OptionError> {
    let mut result = Options::default();
    let is_value = |key: &str| value_flags.contains(&key);
    let is_switch = |key: &str| switch_flags.contains(&key) || GLOBAL_SWITCHES.contains(&key);
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        index += 1;
        if arg == "--" {
            result.positionals.extend(args[index..].iter().cloned());
            break;
        }
        if arg == "-q" {
            result.switches.insert("quiet".to_owned());
            continue;
        }
        if arg == "-h" {
            result.switches.insert("help".to_owned());
            continue;
        }
        let Some(key) = arg.strip_prefix("--") else {
            if arg.starts_with('-') {
                return Err(OptionError::Unknown(arg.clone()));
            }
            result.positionals.push(arg.clone());
            continue;
        };
        let (key, inline) = match key.split_once('=') {
            Some((key, value)) => (key, Some(value)),
            None => (key, None),
        };
        if is_value(key) {
            let value = match inline {
                Some(value) => value.to_owned(),
                None => match args.get(index) {
                    Some(next) if !next.starts_with("--") => {
                        index += 1;
                        next.clone()
                    }
                    _ => return Err(OptionError::Missing(key.to_owned())),
                },
            };
            result.values.entry(key.to_owned()).or_default().push(value);
            continue;
        }
        if is_switch(key) && inline.is_none() {
            result.switches.insert(key.to_owned());
            continue;
        }
        return Err(OptionError::Unknown(arg.clone()));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn parsed(values: &[&str]) -> Result<Options, OptionError> {
        parse(&args(values), &["title", "label"], &["json", "all"])
    }

    #[test]
    fn value_flags_take_the_next_argument() {
        let options = parsed(&["--title", "Fix it", "rest"]).unwrap();
        assert_eq!(options.value("title"), "Fix it");
        assert_eq!(options.positionals, ["rest"]);
        assert!(options.has("title"));
        assert!(!options.has("label"));
        assert_eq!(options.value("label"), "");
        // A value that begins with a single dash is still a value.
        assert_eq!(parsed(&["--title", "-q"]).unwrap().value("title"), "-q");
    }

    #[test]
    fn switches_are_recorded() {
        let options = parsed(&["--json", "--all", "-q", "-h", "--verbose", "--timing"]).unwrap();
        for name in ["json", "all", "quiet", "help", "verbose", "timing"] {
            assert!(options.has(name), "{name}");
        }
        assert!(parsed(&["--quiet"]).unwrap().has("quiet"));
    }

    #[test]
    fn the_inline_form_passes_any_value() {
        let options = parsed(&["--title=--not-a-flag", "--label="]).unwrap();
        assert_eq!(options.value("title"), "--not-a-flag");
        assert_eq!(options.repeated("label"), [""]);
        assert!(options.has("label"));
        assert_eq!(parsed(&["--title=a=b"]).unwrap().value("title"), "a=b");
    }

    #[test]
    fn repeated_flags_keep_every_value_and_the_last_wins() {
        let options =
            parsed(&["--label", "a", "--label=b", "--title", "x", "--title", "y"]).unwrap();
        assert_eq!(options.repeated("label"), ["a", "b"]);
        assert_eq!(options.value("title"), "y");
        assert_eq!(options.repeated("title"), ["x", "y"]);
    }

    #[test]
    fn a_value_beginning_with_two_dashes_is_missing() {
        // SD7.
        let error = parsed(&["--title", "--json"]).unwrap_err();
        assert_eq!(error, OptionError::Missing("title".to_owned()));
        assert_eq!(error.to_string(), "option '--title' argument missing");
        assert_eq!(
            parsed(&["--title"]).unwrap_err(),
            OptionError::Missing("title".to_owned())
        );
        assert_eq!(
            parsed(&["--title", "--"]).unwrap_err(),
            OptionError::Missing("title".to_owned())
        );
    }

    #[test]
    fn single_dash_options_other_than_q_and_h_are_unknown() {
        for option in ["-v", "-x", "-", "-qq", "-json"] {
            let error = parsed(&[option]).unwrap_err();
            assert_eq!(error, OptionError::Unknown(option.to_owned()));
            assert_eq!(error.to_string(), format!("unknown option '{option}'"));
        }
    }

    #[test]
    fn unknown_long_options_and_inline_switches_are_unknown() {
        for option in ["--nope", "--json=true", "--quiet=1", "--=x", "--format"] {
            assert_eq!(
                parsed(&[option]).unwrap_err(),
                OptionError::Unknown(option.to_owned()),
                "{option}"
            );
        }
    }

    #[test]
    fn two_dashes_end_the_options() {
        let options = parsed(&["a", "--", "--json", "-x", "--title"]).unwrap();
        assert_eq!(options.positionals, ["a", "--json", "-x", "--title"]);
        assert!(!options.has("json"));
        assert_eq!(parsed(&["", "b"]).unwrap().positionals, ["", "b"]);
    }
}
