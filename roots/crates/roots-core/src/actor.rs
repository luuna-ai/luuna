//! Resolution and validation of Roots actors.
//!
//! The CLI deliberately accepts the historic `ROOTS_*` variables alongside
//! the Luuna-prefixed aliases.  Keep this module independent from process
//! globals so callers and tests can inject both the environment and cwd.

use crate::errors::{RootsError, RootsResult};
use crate::model::Actor;
use crate::slug::kebab;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

/// A deterministic process environment supplied by the caller.
///
/// `BTreeMap` is used instead of consulting `std::env` directly so that a
/// command runner can pass its inherited environment exactly and tests do not
/// leak machine-specific identities.
pub type Environment = BTreeMap<String, String>;

/// The small portion of process state actor resolution needs.
#[derive(Clone, Copy, Debug)]
pub struct ActorContext<'a> {
    pub cwd: &'a Path,
    pub env: &'a Environment,
}

impl<'a> ActorContext<'a> {
    #[must_use]
    pub const fn new(cwd: &'a Path, env: &'a Environment) -> Self {
        Self { cwd, env }
    }
}

/// Return an injectable snapshot of the current process environment.
#[must_use]
pub fn process_environment() -> Environment {
    std::env::vars().collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActorKind {
    Human,
    Agent,
    Roots,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedActor {
    pub kind: ActorKind,
    pub name: String,
}

fn valid_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Parse one of `human:<name>`, `agent:<name>`, `roots`, or `roots:<rule>`.
#[must_use]
pub fn parse_actor(actor: &str) -> Option<ParsedActor> {
    if actor == "roots" {
        return Some(ParsedActor {
            kind: ActorKind::Roots,
            name: String::new(),
        });
    }
    let (kind, name) = actor.split_once(':')?;
    let kind = match kind {
        "human" => ActorKind::Human,
        "agent" => ActorKind::Agent,
        "roots" => ActorKind::Roots,
        _ => return None,
    };
    valid_name(name).then(|| ParsedActor {
        kind,
        name: name.to_owned(),
    })
}

#[must_use]
pub fn is_human_actor(actor: &str) -> bool {
    parse_actor(actor).is_some_and(|parsed| parsed.kind == ActorKind::Human)
}

#[must_use]
pub fn is_agent_actor(actor: &str) -> bool {
    parse_actor(actor).is_some_and(|parsed| parsed.kind == ActorKind::Agent)
}

#[must_use]
pub fn roots_actor(rule: Option<&str>) -> Actor {
    match rule {
        Some(rule) if !rule.is_empty() => format!("roots:{rule}"),
        _ => "roots".to_owned(),
    }
}

/// Read `git config user.name` using the provided cwd and environment.
///
/// Like the former Bun implementation, failure to start git, a non-zero exit,
/// or an empty result simply means no Git identity is available.
#[must_use]
pub fn git_user_name(context: ActorContext<'_>) -> Option<String> {
    let output = Command::new("git")
        .args(["config", "user.name"])
        .current_dir(context.cwd)
        .env_clear()
        .envs(context.env)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!name.is_empty()).then_some(name)
}

fn human_from_raw(raw: &str, source: &str) -> RootsResult<Actor> {
    let name_source = match raw.split_once(':') {
        Some((prefix, _)) if prefix != "human" => {
            return Err(RootsError::guard(format!(
                "{source} must be a human name, got \"{raw}\""
            )));
        }
        Some((_, name)) => name,
        None => raw,
    };
    let name = kebab(name_source);
    if name.is_empty() {
        return Err(RootsError::guard(format!(
            "{source} does not contain a usable name (\"{raw}\")"
        )));
    }
    Ok(format!("human:{name}"))
}

/// Resolve the human actor required for a human command.
pub fn resolve_human_actor(context: ActorContext<'_>) -> RootsResult<Actor> {
    // `??` in the TypeScript implementation intentionally means a defined but
    // blank LUUNA_ROOTS_USER masks ROOTS_USER and falls through to Git.
    let configured = context
        .env
        .get("LUUNA_ROOTS_USER")
        .or_else(|| context.env.get("ROOTS_USER"));
    if let Some(raw) = configured
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        let source = if context
            .env
            .get("LUUNA_ROOTS_USER")
            .is_some_and(|value| !value.is_empty())
        {
            "LUUNA_ROOTS_USER"
        } else {
            "ROOTS_USER"
        };
        return human_from_raw(raw, source);
    }
    if let Some(git) = git_user_name(context) {
        return human_from_raw(&git, "git user.name");
    }
    Err(RootsError::guard(
        "cannot tell who you are: set LUUNA_ROOTS_USER or `git config user.name` (human commands need a name)",
    ))
}

/// Resolve the agent actor from `--as`, `LUUNA_ROOTS_AGENT`, or `ROOTS_AGENT`.
pub fn resolve_agent_actor(as_flag: Option<&str>, env: &Environment) -> RootsResult<Actor> {
    // As above, the first *defined* value wins even when it is blank.
    let raw = as_flag
        .or_else(|| env.get("LUUNA_ROOTS_AGENT").map(String::as_str))
        .or_else(|| env.get("ROOTS_AGENT").map(String::as_str))
        .unwrap_or_default()
        .trim();
    let source = if as_flag.is_some() {
        "--as"
    } else if env
        .get("LUUNA_ROOTS_AGENT")
        .is_some_and(|value| !value.is_empty())
    {
        "LUUNA_ROOTS_AGENT"
    } else {
        "ROOTS_AGENT"
    };
    if raw.is_empty() {
        return Err(RootsError::guard(
            "agent commands need an identity: pass --as agent:<model> or set LUUNA_ROOTS_AGENT",
        ));
    }
    let candidate = if raw.contains(':') {
        raw.to_owned()
    } else {
        format!("agent:{raw}")
    };
    match parse_actor(&candidate) {
        Some(ParsedActor {
            kind: ActorKind::Agent,
            ..
        }) => Ok(candidate),
        Some(ParsedActor {
            kind: ActorKind::Human,
            ..
        }) => Err(RootsError::guard(format!(
            "{source}: agents cannot act as a human (\"{raw}\")"
        ))),
        _ => Err(RootsError::guard(format!(
            "{source}: expected agent:<model>, got \"{raw}\""
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn environment(entries: &[(&str, &str)]) -> Environment {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn parses_the_three_actor_forms() {
        assert_eq!(
            parse_actor("human:jaymin-west"),
            Some(ParsedActor {
                kind: ActorKind::Human,
                name: "jaymin-west".to_owned(),
            })
        );
        assert_eq!(parse_actor("roots").unwrap().kind, ActorKind::Roots);
        assert!(parse_actor("agent:").is_none());
        assert!(parse_actor("bot:x").is_none());
        assert!(is_human_actor("human:a"));
        assert!(!is_agent_actor("human:a"));
        assert_eq!(roots_actor(None), "roots");
        assert_eq!(roots_actor(Some("orphan")), "roots:orphan");
    }

    #[test]
    fn resolves_configured_human_and_agent_identities() {
        let dir = std::env::temp_dir().join(format!("roots-actor-{}", crate::ids::random_hex(8)));
        fs::create_dir_all(&dir).expect("create test directory");
        let human_env = environment(&[("ROOTS_USER", "Jaymin West")]);
        assert_eq!(
            resolve_human_actor(ActorContext::new(&dir, &human_env)).expect("human actor"),
            "human:jaymin-west"
        );
        let agent_env = environment(&[("ROOTS_AGENT", "gpt-5")]);
        assert_eq!(
            resolve_agent_actor(None, &agent_env).expect("agent actor"),
            "agent:gpt-5"
        );
        assert!(resolve_agent_actor(Some("human:jaymin"), &Environment::new()).is_err());
        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn a_blank_new_name_masks_the_legacy_name() {
        let dir =
            std::env::temp_dir().join(format!("roots-actor-empty-{}", crate::ids::random_hex(8)));
        fs::create_dir_all(&dir).expect("create test directory");
        let global_config = dir.join("gitconfig");
        fs::write(&global_config, "").expect("write empty git config");
        let env = environment(&[
            ("LUUNA_ROOTS_USER", ""),
            ("ROOTS_USER", "Legacy Person"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
        ]);
        let mut env = env;
        env.insert(
            "GIT_CONFIG_GLOBAL".to_owned(),
            global_config.to_string_lossy().into_owned(),
        );
        if let Ok(path) = std::env::var("PATH") {
            env.insert("PATH".to_owned(), path);
        }
        assert_eq!(
            resolve_human_actor(ActorContext::new(&dir, &env))
                .expect_err("blank new variable must mask legacy value")
                .message,
            "cannot tell who you are: set LUUNA_ROOTS_USER or `git config user.name` (human commands need a name)"
        );
        fs::remove_dir_all(dir).expect("clean test directory");
    }
}
