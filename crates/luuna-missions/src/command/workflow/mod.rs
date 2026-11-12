//! The workflow commands, owned by s4-missions-lib-2: ready, close, dep,
//! label, blocked, stats, completions and unblock.
//!
//! The command table in `command/mod.rs` routes each of them to a constant
//! here. At this commit every constant is `Entry::Deferred` naming
//! s4-missions-lib-2, so the command returns
//! [`Error::Unsupported`](crate::Error::Unsupported) exactly as before.
//! s4-missions-lib-2 points each constant at its handler, adds its files in
//! this folder and replaces the `checks` it owns, and does not edit
//! `command/mod.rs`.

use crate::error::PLAN_2;

pub(in crate::command) mod lossy;

/// `ready`.
pub(super) const READY: super::Entry = super::Entry::Deferred(PLAN_2);
/// `close`.
pub(super) const CLOSE: super::Entry = super::Entry::Deferred(PLAN_2);
/// `dep`.
pub(super) const DEP: super::Entry = super::Entry::Deferred(PLAN_2);
/// `label`.
pub(super) const LABEL: super::Entry = super::Entry::Deferred(PLAN_2);
/// `blocked`.
pub(super) const BLOCKED: super::Entry = super::Entry::Deferred(PLAN_2);
/// `stats`.
pub(super) const STATS: super::Entry = super::Entry::Deferred(PLAN_2);
/// `completions`.
pub(super) const COMPLETIONS: super::Entry = super::Entry::Deferred(PLAN_2);
/// `unblock`.
pub(super) const UNBLOCK: super::Entry = super::Entry::Deferred(PLAN_2);

/// The expectations of `every_other_go_command_answers_as_its_plan_states`
/// in `command/mod.rs` for this module's commands, whose names stay there.
#[cfg(test)]
pub(super) mod checks {
    use super::super::{run, Entry, Invocation};
    use crate::error::{Error, PLAN_2};

    fn invocation(args: &[&str]) -> Invocation {
        Invocation::new(std::env::temp_dir(), args.iter().copied())
    }

    fn plan_of(error: Error) -> String {
        match error {
            Error::Unsupported { plan, .. } => plan,
            other => panic!("expected Unsupported, got {other}"),
        }
    }

    /// `<name> --json` in the system temp folder. While the command is
    /// deferred: Unsupported naming s4-missions-lib-2. Once ported, Go's exact
    /// outcome in a folder without a store, as literals.
    pub(in crate::command) fn command(name: &str) {
        let entry = match name {
            "ready" => super::READY,
            "close" => super::CLOSE,
            "dep" => super::DEP,
            "label" => super::LABEL,
            "blocked" => super::BLOCKED,
            "stats" => super::STATS,
            "completions" => super::COMPLETIONS,
            "unblock" => super::UNBLOCK,
            other => panic!("{other} is not a command of this module"),
        };
        match entry {
            Entry::Deferred(plan) => {
                assert_eq!(plan, PLAN_2, "{name}");
                let error = run(&invocation(&[name, "--json"])).unwrap_err();
                assert_eq!(
                    error.to_string(),
                    format!(
                        "the {name} command is not supported yet; it is ported in s4-missions-lib-2"
                    ),
                    "{name}"
                );
                assert_eq!(plan_of(error), plan, "{name}");
            }
            Entry::Ported(_) => unreachable!("{name} is not ported at the slots commit"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{find, Entry};
    use super::*;

    #[test]
    fn its_routes_point_at_this_module() {
        for (name, constant) in [
            ("ready", READY),
            ("close", CLOSE),
            ("dep", DEP),
            ("label", LABEL),
            ("blocked", BLOCKED),
            ("stats", STATS),
            ("completions", COMPLETIONS),
            ("unblock", UNBLOCK),
        ] {
            match (find(name), constant) {
                (Some(Entry::Deferred(routed)), Entry::Deferred(own)) => {
                    assert_eq!(routed, own, "{name}");
                    assert_eq!(own, PLAN_2, "{name}");
                }
                (Some(Entry::Ported(_)), Entry::Ported(_)) => {}
                (None, _) => panic!("{name} is not in the command table"),
                _ => panic!("{name}: the command table and this module differ in kind"),
            }
        }
    }
}
