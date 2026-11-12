//! Human-command gates: an interactive TTY and a non-agent session.

use crate::actor::{ActorContext, Environment, resolve_human_actor};
use crate::errors::{RootsError, RootsResult};
use crate::model::Actor;

#[must_use]
pub fn tty_forced_for_tests(env: &Environment) -> bool {
    env.get("ROOTS_FORCE_TTY").is_some_and(|value| value == "1")
        && env.get("NODE_ENV").is_some_and(|value| value == "test")
}

#[must_use]
pub fn has_interactive_tty(env: &Environment, stdin_is_tty: bool) -> bool {
    stdin_is_tty || tty_forced_for_tests(env)
}

/// Refuse a human-content-writing command without an interactive terminal.
pub fn require_tty(env: &Environment, stdin_is_tty: bool, command: &str) -> RootsResult<()> {
    if has_interactive_tty(env, stdin_is_tty) {
        return Ok(());
    }
    Err(RootsError::guard(format!(
        "`roots {command}` writes human content and needs an interactive terminal (TTY); agents cannot run it"
    )))
}

/// Refuse a human structure command when the legacy agent-session variable is
/// set.  This intentionally does *not* inspect `LUUNA_ROOTS_AGENT`: preserving
/// that historic asymmetry is part of the compatibility contract.
pub fn refuse_agent_session(env: &Environment, command: &str) -> RootsResult<()> {
    let Some(agent) = env
        .get("ROOTS_AGENT")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    Err(RootsError::guard(format!(
        "`roots {command}` is a human command, but ROOTS_AGENT is set ({agent}); unset ROOTS_AGENT to run it yourself"
    )))
}

/// Guard a non-interactive human structure command, then resolve its author.
pub fn require_human_command(context: ActorContext<'_>, command: &str) -> RootsResult<Actor> {
    refuse_agent_session(context.env, command)?;
    resolve_human_actor(context)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tty_override_requires_both_test_variables() {
        let mut env = Environment::new();
        env.insert("ROOTS_FORCE_TTY".to_owned(), "1".to_owned());
        assert!(!tty_forced_for_tests(&env));
        env.insert("NODE_ENV".to_owned(), "test".to_owned());
        assert!(tty_forced_for_tests(&env));
        assert!(has_interactive_tty(&env, false));
    }

    #[test]
    fn only_legacy_agent_environment_blocks_human_commands() {
        let mut env = Environment::new();
        env.insert("LUUNA_ROOTS_AGENT".to_owned(), "agent:new".to_owned());
        assert!(refuse_agent_session(&env, "tier").is_ok());
        env.insert("ROOTS_AGENT".to_owned(), "agent:legacy".to_owned());
        assert_eq!(
            refuse_agent_session(&env, "tier")
                .expect_err("must guard")
                .message,
            "`roots tier` is a human command, but ROOTS_AGENT is set (agent:legacy); unset ROOTS_AGENT to run it yourself"
        );
    }
}
