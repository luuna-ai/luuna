//! The project commands, owned by s4-missions-lib-3: sync, doctor, tpl,
//! migrate-from-beads, prime, onboard, upgrade, config, setup and guard.
//!
//! The command table in `command/mod.rs` routes each of them to a constant
//! here. At this commit every constant is `Entry::Deferred` naming
//! s4-missions-lib-3, so the command returns
//! [`Error::Unsupported`](crate::Error::Unsupported) exactly as before.
//! s4-missions-lib-3 points each constant at its handler, adds its files in
//! this folder and replaces the `checks` it owns, and does not edit
//! `command/mod.rs`.

use crate::error::PLAN_3;
mod config;
mod doctor;
mod doctor_fix;
mod lossy;
mod stale_lock;
mod template_record;
mod tpl;

/// `sync`.
pub(super) const SYNC: super::Entry = super::Entry::Deferred(PLAN_3);
/// `doctor`.
pub(super) const DOCTOR: super::Entry = super::Entry::Ported(doctor::run);
/// `tpl`.
pub(super) const TPL: super::Entry = super::Entry::Ported(tpl::run);
/// `migrate-from-beads`.
pub(super) const MIGRATE_FROM_BEADS: super::Entry = super::Entry::Deferred(PLAN_3);
/// `prime`.
pub(super) const PRIME: super::Entry = super::Entry::Deferred(PLAN_3);
/// `onboard`.
pub(super) const ONBOARD: super::Entry = super::Entry::Deferred(PLAN_3);
/// `upgrade`.
pub(super) const UPGRADE: super::Entry = super::Entry::Deferred(PLAN_3);
/// `config`.
pub(super) const CONFIG: super::Entry = super::Entry::Ported(config::run);
/// `setup`.
pub(super) const SETUP: super::Entry = super::Entry::Deferred(PLAN_3);
/// `guard`.
pub(super) const GUARD: super::Entry = super::Entry::Deferred(PLAN_3);

/// The expectations of `every_other_go_command_answers_as_its_plan_states`
/// in `command/mod.rs` for this module's commands, whose names stay there.
#[cfg(test)]
pub(super) mod checks {
    use super::super::{run, Entry, Invocation};
    use crate::error::{Error, PLAN_3};

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
    /// deferred: Unsupported naming s4-missions-lib-3. Once ported, Go's exact
    /// outcome in a folder without a store, as literals.
    pub(in crate::command) fn command(name: &str) {
        let entry = match name {
            "sync" => super::SYNC,
            "doctor" => super::DOCTOR,
            "tpl" => super::TPL,
            "migrate-from-beads" => super::MIGRATE_FROM_BEADS,
            "prime" => super::PRIME,
            "onboard" => super::ONBOARD,
            "upgrade" => super::UPGRADE,
            "config" => super::CONFIG,
            "setup" => super::SETUP,
            "guard" => super::GUARD,
            other => panic!("{other} is not a command of this module"),
        };
        match entry {
            Entry::Deferred(plan) => {
                assert_eq!(plan, PLAN_3, "{name}");
                let error = run(&invocation(&[name, "--json"])).unwrap_err();
                assert_eq!(
                    error.to_string(),
                    format!(
                        "the {name} command is not supported yet; it is ported in s4-missions-lib-3"
                    ),
                    "{name}"
                );
                assert_eq!(plan_of(error), plan, "{name}");
            }
            Entry::Ported(_) => ported(name),
        }
    }
    fn ported(name: &str) {
        match name {
            "config" | "tpl" => {
                let temp = tempfile::tempdir().unwrap();
                let out = run(&Invocation::new(temp.path(), [name, "--json"])).unwrap();
                assert_eq!(out.stdout, b"");
                assert_eq!(out.stderr, b"error: unknown option '--json'\n");
                assert_eq!(out.exit_code, 1);
            }
            "doctor" => {
                let temp = tempfile::tempdir().unwrap();
                let out = run(&Invocation::new(temp.path(), ["doctor", "--json"])).unwrap();
                assert_eq!(out.stdout,b"{\n  \"success\": false,\n  \"command\": \"doctor\",\n  \"error\": \"Not in a missions project. Run `luuna-missions init` first.\"\n}\n");
                assert_eq!(out.stderr, b"");
                assert_eq!(out.exit_code, 1);
            }
            _ => panic!("no Go literal for {name}"),
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
            ("sync", SYNC),
            ("doctor", DOCTOR),
            ("tpl", TPL),
            ("migrate-from-beads", MIGRATE_FROM_BEADS),
            ("prime", PRIME),
            ("onboard", ONBOARD),
            ("upgrade", UPGRADE),
            ("config", CONFIG),
            ("setup", SETUP),
            ("guard", GUARD),
        ] {
            match (find(name), constant) {
                (Some(Entry::Deferred(routed)), Entry::Deferred(own)) => {
                    assert_eq!(routed, own, "{name}");
                    assert_eq!(own, PLAN_3, "{name}");
                }
                (Some(Entry::Ported(_)), Entry::Ported(_)) => {}
                (None, _) => panic!("{name} is not in the command table"),
                _ => panic!("{name}: the command table and this module differ in kind"),
            }
        }
    }
}
