//! `prime` (`cli.go:1320-1338`): the workflow context text, or the project's
//! own `PRIME.md` in its place.

use crate::Beacon;
use std::fs;

const DEFAULT: &str = "# Luuna Beacon Workflow Context\n\n- Storage: `.luuna/beacon/prompts.jsonl`\n- Use `lbc update`, then `lbc emit`; do not edit emitted files.\n- Run `lbc render <name>` to resolve composition.\n";
const COMPACT: &str =
    "# Luuna Beacon Quick Reference\n\nlbc list\nlbc render <name>\nlbc emit --all\nlbc update <name>\n";

/// The flags of `prime`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PrimeOptions {
    pub compact: bool,
    pub export: bool,
}

/// The text `prime` prints, as the bytes Go holds in its string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prime {
    pub content: Vec<u8>,
}

impl Prime {
    /// The content as Go's `encoding/json` carries a string: each byte that
    /// is not valid UTF-8 becomes U+FFFD (DL11, AS58). The result serialises
    /// as a JSON string.
    pub fn go_json_text(&self) -> String {
        let mut out = String::with_capacity(self.content.len());
        let mut rest = self.content.as_slice();
        while !rest.is_empty() {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    out.push_str(text);
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    out.push_str(
                        std::str::from_utf8(&rest[..valid]).expect("the prefix is valid UTF-8"),
                    );
                    out.push('\u{FFFD}');
                    rest = &rest[valid + 1..];
                }
            }
        }
        out
    }
}

impl Beacon {
    /// Go's `prime`. It never fails: the legacy move's error and any error
    /// reading `PRIME.md` are ignored (GD70).
    pub fn prime(&self, options: &PrimeOptions) -> Prime {
        let mut content = if options.compact { COMPACT } else { DEFAULT }
            .as_bytes()
            .to_vec();
        if !options.export && self.migrate_legacy().is_ok() {
            if let Ok(custom) = fs::read(self.store_dir().join("PRIME.md")) {
                content = custom;
            }
        }
        Prime { content }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_bytes_become_one_replacement_each() {
        let prime = Prime {
            content: b"a\xe2\x82b\xffc\xc3\xa9".to_vec(),
        };
        assert_eq!(prime.go_json_text(), "a\u{FFFD}\u{FFFD}b\u{FFFD}c\u{e9}");
    }

    #[test]
    fn default_compact_custom_and_export() {
        let dir = tempfile::tempdir().unwrap();
        let beacon = Beacon::open(dir.path()).unwrap();
        let plain = beacon.prime(&PrimeOptions::default());
        assert_eq!(plain.content, DEFAULT.as_bytes());
        let compact = beacon.prime(&PrimeOptions {
            compact: true,
            export: false,
        });
        assert_eq!(compact.content, COMPACT.as_bytes());
        fs::create_dir_all(beacon.store_dir()).unwrap();
        fs::write(beacon.store_dir().join("PRIME.md"), b"mine\n").unwrap();
        assert_eq!(beacon.prime(&PrimeOptions::default()).content, b"mine\n");
        let exported = beacon.prime(&PrimeOptions {
            compact: false,
            export: true,
        });
        assert_eq!(exported.content, DEFAULT.as_bytes());
    }
}
