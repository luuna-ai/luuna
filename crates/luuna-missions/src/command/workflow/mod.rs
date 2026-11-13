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

mod blocked;
mod close;
mod completions;
mod dep;
mod label;
pub(in crate::command) mod lossy;
mod ready;
mod render;
mod schedule;
mod stats;
mod unblock;

/// `ready`.
pub(super) const READY: super::Entry = super::Entry::Ported(ready::run);
/// `close`.
pub(super) const CLOSE: super::Entry = super::Entry::Ported(close::run);
/// `dep`.
pub(super) const DEP: super::Entry = super::Entry::Ported(dep::run);
/// `label`.
pub(super) const LABEL: super::Entry = super::Entry::Ported(label::run);
/// `blocked`.
pub(super) const BLOCKED: super::Entry = super::Entry::Ported(blocked::run);
/// `stats`.
pub(super) const STATS: super::Entry = super::Entry::Ported(stats::run);
/// `completions`.
pub(super) const COMPLETIONS: super::Entry = super::Entry::Ported(completions::run);
/// `unblock`.
pub(super) const UNBLOCK: super::Entry = super::Entry::Ported(unblock::run);

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
            Entry::Ported(_) => match name {
                // Go: FindDir fails in a folder without a store, so the
                // command prints the error envelope (--json is among the
                // arguments) and exits 1.
                "ready" | "blocked" | "dep" | "label" | "close" | "unblock" | "stats" => {
                    let temp = tempfile::tempdir().unwrap();
                    let outcome = run(&Invocation::new(temp.path(), [name, "--json"])).unwrap();
                    let message = match name {
                        "close" => "Usage: luuna-missions close <id> [ids...] [--reason text]",
                        "dep" => "Usage: luuna-missions dep <add|remove|list> <issue> [depends-on]",
                        "label" => "Usage: luuna-missions label <add|remove|list|list-all>",
                        "unblock" => {
                            "Usage: luuna-missions unblock <id> [--from <blocker-id> | --all]"
                        }
                        _ => "Not in a missions project. Run `luuna-missions init` first.",
                    };
                    assert_eq!(
                        String::from_utf8(outcome.stdout).unwrap(),
                        format!(
                            "{{\n  \"success\": false,\n  \"command\": \"{name}\",\n  \"error\": \"{message}\"\n}}\n"
                        ),
                        "{name}"
                    );
                    assert_eq!(outcome.stderr, b"", "{name}");
                    assert_eq!(outcome.exit_code, 1, "{name}");
                }
                "completions" => {
                    let temp = tempfile::tempdir().unwrap();
                    let result =
                        run(&Invocation::new(temp.path(), ["completions", "--json"])).unwrap();
                    assert_eq!(result.stdout, b"");
                    assert_eq!(
                        result.stderr,
                        "✗ Unknown shell: --json. Supported: bash, zsh, fish\n".as_bytes()
                    );
                    assert_eq!(result.exit_code, 1);
                }
                other => panic!("unexpected workflow input {other}"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{find, Entry};
    use super::*;
    use crate::error::PLAN_2;

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
