//! The small custom argument parser used by Roots commands.

use crate::errors::{RootsError, RootsResult};
use crate::suggestions::closest;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlagKind {
    Boolean,
    String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlagSpec {
    pub name: &'static str,
    pub kind: FlagKind,
    pub short: Option<char>,
    pub multiple: bool,
    pub description: &'static str,
    pub placeholder: Option<&'static str>,
}

impl FlagSpec {
    pub const fn boolean(
        name: &'static str,
        short: Option<char>,
        description: &'static str,
    ) -> Self {
        Self {
            name,
            kind: FlagKind::Boolean,
            short,
            multiple: false,
            description,
            placeholder: None,
        }
    }

    pub const fn string(
        name: &'static str,
        short: Option<char>,
        multiple: bool,
        placeholder: &'static str,
        description: &'static str,
    ) -> Self {
        Self {
            name,
            kind: FlagKind::String,
            short,
            multiple,
            description,
            placeholder: Some(placeholder),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FlagValue {
    Bool(bool),
    String(String),
    List(Vec<String>),
}

pub type Flags = BTreeMap<String, FlagValue>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParsedArgs {
    pub positionals: Vec<String>,
    pub flags: Flags,
}

fn unknown_flag(raw: &str, specs: &[FlagSpec]) -> RootsError {
    let input = raw.trim_start_matches('-');
    let candidates: Vec<&str> = specs.iter().map(|spec| spec.name).collect();
    let suffix = closest(input, &candidates, 2)
        .map(|hint| format!(" Did you mean --{hint}?"))
        .unwrap_or_default();
    RootsError::usage(format!("unknown flag: {raw}.{suffix}"))
}

fn find_spec<'a>(specs: &'a [FlagSpec], name: &str) -> Option<&'a FlagSpec> {
    specs.iter().find(|spec| spec.name == name)
}

fn find_short(specs: &[FlagSpec], short: char) -> Option<&FlagSpec> {
    specs.iter().find(|spec| spec.short == Some(short))
}

fn set_flag(flags: &mut Flags, spec: &FlagSpec, value: Option<String>) -> RootsResult<()> {
    match spec.kind {
        FlagKind::Boolean => {
            flags.insert(spec.name.to_owned(), FlagValue::Bool(true));
        }
        FlagKind::String => {
            let value = value.ok_or_else(|| {
                RootsError::usage(format!("flag --{} requires a value", spec.name))
            })?;
            if spec.multiple {
                match flags.get_mut(spec.name) {
                    Some(FlagValue::List(values)) => values.push(value),
                    Some(FlagValue::String(previous)) => {
                        let previous = std::mem::take(previous);
                        flags.insert(spec.name.to_owned(), FlagValue::List(vec![previous, value]));
                    }
                    _ => {
                        flags.insert(spec.name.to_owned(), FlagValue::List(vec![value]));
                    }
                }
            } else {
                flags.insert(spec.name.to_owned(), FlagValue::String(value));
            }
        }
    }
    Ok(())
}

fn is_known_flag(token: &str, specs: &[FlagSpec]) -> bool {
    if let Some(long) = token.strip_prefix("--") {
        let key = long.split('=').next().unwrap_or_default();
        return !key.is_empty() && find_spec(specs, key).is_some();
    }
    let mut chars = token.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some('-'), Some(short), None) if short.is_ascii_alphabetic() && find_short(specs, short).is_some()
    )
}

/// Parse command arguments using the historical Roots grammar.
pub fn parse_args(argv: &[String], specs: &[FlagSpec]) -> RootsResult<ParsedArgs> {
    let mut parsed = ParsedArgs::default();
    let mut index = 0;

    while index < argv.len() {
        let argument = &argv[index];
        if argument == "--" {
            parsed.positionals.extend_from_slice(&argv[index + 1..]);
            break;
        }

        let (raw, spec, embedded_value) = if let Some(rest) = argument.strip_prefix("--") {
            let (name, embedded_value) = match rest.split_once('=') {
                Some((name, value)) => (name, Some(value.to_owned())),
                None => (rest, None),
            };
            let spec = find_spec(specs, name).ok_or_else(|| unknown_flag(argument, specs))?;
            (argument.as_str(), spec, embedded_value)
        } else if argument.len() > 1
            && argument.starts_with('-')
            && !argument
                .as_bytes()
                .get(1)
                .is_some_and(|byte| byte.is_ascii_digit())
        {
            let mut chars = argument.chars();
            let _dash = chars.next();
            let short = chars.next().unwrap_or_default();
            if chars.next().is_some() {
                return Err(unknown_flag(argument, specs));
            }
            let spec = find_short(specs, short).ok_or_else(|| unknown_flag(argument, specs))?;
            (argument.as_str(), spec, None)
        } else {
            parsed.positionals.push(argument.clone());
            index += 1;
            continue;
        };

        match spec.kind {
            FlagKind::Boolean if embedded_value.is_some() => {
                return Err(RootsError::usage(format!(
                    "flag --{} does not take a value",
                    spec.name
                )));
            }
            FlagKind::Boolean => set_flag(&mut parsed.flags, spec, None)?,
            FlagKind::String => {
                let value = match embedded_value {
                    Some(value) => value,
                    None => {
                        let next = argv.get(index + 1);
                        if next.is_none() || next.is_some_and(|next| is_known_flag(next, specs)) {
                            let hint = next
                                .map(|next| {
                                    format!(
                                        " (got {next}; write --{}={next} to pass it literally)",
                                        spec.name
                                    )
                                })
                                .unwrap_or_default();
                            return Err(RootsError::usage(format!(
                                "flag --{} requires a value{hint}",
                                spec.name
                            )));
                        }
                        index += 1;
                        argv[index].clone()
                    }
                };
                set_flag(&mut parsed.flags, spec, Some(value))?;
            }
        }
        let _ = raw;
        index += 1;
    }
    Ok(parsed)
}

pub fn flag_string<'a>(flags: &'a Flags, name: &str) -> Option<&'a str> {
    match flags.get(name) {
        Some(FlagValue::String(value)) => Some(value),
        _ => None,
    }
}

pub fn flag_bool(flags: &Flags, name: &str) -> bool {
    matches!(flags.get(name), Some(FlagValue::Bool(true)))
}

pub fn flag_list<'a>(flags: &'a Flags, name: &str) -> Vec<&'a str> {
    match flags.get(name) {
        Some(FlagValue::List(values)) => values.iter().map(String::as_str).collect(),
        Some(FlagValue::String(value)) => vec![value],
        _ => Vec::new(),
    }
}

pub fn flag_int(flags: &Flags, name: &str) -> RootsResult<Option<u64>> {
    let Some(value) = flag_string(flags, name) else {
        return Ok(None);
    };
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(RootsError::usage(format!(
            "--{name} must be a non-negative integer (got \"{value}\")"
        )));
    }
    value.parse::<u64>().map(Some).map_err(|_| {
        RootsError::usage(format!(
            "--{name} must be a non-negative integer (got \"{value}\")"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPECS: &[FlagSpec] = &[
        FlagSpec::boolean("json", None, ""),
        FlagSpec::boolean("help", Some('h'), ""),
        FlagSpec::string("status", None, false, "<status>", ""),
        FlagSpec::string("limit", Some('n'), false, "<limit>", ""),
        FlagSpec::string("cite", None, true, "<cite>", ""),
    ];

    fn argv(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn supports_positionals_and_flags_in_any_order() {
        let parsed = parse_args(
            &argv(&["show", "--json", "x", "--status", "planted", "-n", "3"]),
            SPECS,
        )
        .unwrap();
        assert_eq!(parsed.positionals, ["show", "x"]);
        assert!(flag_bool(&parsed.flags, "json"));
        assert_eq!(flag_string(&parsed.flags, "status"), Some("planted"));
        assert_eq!(flag_string(&parsed.flags, "limit"), Some("3"));
    }

    #[test]
    fn supports_equals_repeatable_values_and_terminator() {
        let parsed = parse_args(
            &argv(&[
                "--status=a=b",
                "--cite",
                "r-1:x",
                "--cite=r-2:y",
                "--",
                "--json",
                "-h",
            ]),
            SPECS,
        )
        .unwrap();
        assert_eq!(flag_string(&parsed.flags, "status"), Some("a=b"));
        assert_eq!(flag_list(&parsed.flags, "cite"), ["r-1:x", "r-2:y"]);
        assert_eq!(parsed.positionals, ["--json", "-h"]);
    }

    #[test]
    fn treats_lone_dash_and_negative_numbers_as_positionals() {
        let parsed = parse_args(&argv(&["-", "-5"]), SPECS).unwrap();
        assert_eq!(parsed.positionals, ["-", "-5"]);
    }

    #[test]
    fn returns_compatibility_errors() {
        assert!(
            parse_args(&argv(&["--stauts", "x"]), SPECS)
                .unwrap_err()
                .message
                .contains("Did you mean --status")
        );
        assert_eq!(
            parse_args(&argv(&["-z"]), SPECS).unwrap_err().code,
            crate::errors::ErrorCode::Usage
        );
        assert!(
            parse_args(&argv(&["--status"]), SPECS)
                .unwrap_err()
                .message
                .contains("requires a value")
        );
        assert!(
            parse_args(&argv(&["--status", "--status"]), SPECS)
                .unwrap_err()
                .message
                .contains("--status=--status")
        );
        assert!(
            parse_args(&argv(&["--json=1"]), SPECS)
                .unwrap_err()
                .message
                .contains("does not take a value")
        );
    }

    #[test]
    fn exposes_typed_accessors() {
        let parsed = parse_args(&argv(&["--json", "--limit", "5", "--cite", "a"]), SPECS).unwrap();
        assert!(flag_bool(&parsed.flags, "json"));
        assert!(!flag_bool(&parsed.flags, "help"));
        assert_eq!(flag_string(&parsed.flags, "limit"), Some("5"));
        assert_eq!(flag_string(&parsed.flags, "json"), None);
        assert_eq!(flag_int(&parsed.flags, "limit").unwrap(), Some(5));
        assert_eq!(flag_int(&parsed.flags, "status").unwrap(), None);
        assert_eq!(flag_list(&parsed.flags, "cite"), ["a"]);
        let mut one = Flags::new();
        one.insert("x".to_owned(), FlagValue::String("one".to_owned()));
        assert_eq!(flag_list(&one, "x"), ["one"]);
        assert!(flag_int(&one, "x").is_err());
    }
}
