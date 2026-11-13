//! The `plan` command and plan transitions, owned by s4-missions-lib-4.
//!
//! The command table in `command/mod.rs` routes `plan` to [`PLAN`]. At this
//! commit it is `Entry::Deferred` naming s4-missions-lib-4. The
//! status-change seams [`check_status_change`] and [`apply_status_change`]
//! are what a command that changes ticket statuses calls around its write
//! (Go's `planTransitionHook`, `internal/cli/issue_mutate.go:14-20`,
//! `:62-67`); at this commit the first refuses whenever a plan could be
//! involved and the second does nothing. s4-missions-lib-4 replaces them,
//! adds its files in this folder and replaces the `checks` it owns.

#[allow(
    dead_code,
    reason = "plan handlers are implemented in the following tasks"
)]
mod record;

#[allow(dead_code, reason = "native plan prompt wiring is the next step")]
mod atlas;
#[allow(
    dead_code,
    reason = "plan edit/submit/link handlers consume backrefs in following tasks"
)]
mod backref;
#[allow(dead_code, reason = "native plan reader wiring is the next step")]
mod common;
#[allow(
    dead_code,
    reason = "mutation handlers consume these guards in following tasks"
)]
mod guard;
mod mutate;
mod prompt;
#[allow(
    dead_code,
    reason = "mutation handlers consume these guards in following tasks"
)]
mod raw;
mod reader;
#[allow(dead_code, reason = "native plan show wiring is the next step")]
mod tree;
#[allow(dead_code, reason = "native plan reader wiring is the next step")]
mod validation;
use std::io;

use crate::error::{Error, Result, PLAN_4};
use crate::model::Ticket;
use crate::store::{DataFile, Store};

// The foundation's unchanged check functions retain their Deferred arms.
// Keep that representation constructed without adding a deferred live route.
const _: super::Entry = super::Entry::Deferred(PLAN_4);

/// `plan`.
pub(super) const PLAN: super::Entry = super::Entry::Ported(run);

fn run(c: &mut super::Context<'_>, args: &[String]) -> std::result::Result<(), super::Failure> {
    let Some(name) = args.first() else {
        let help = super::app::command_help(&["plan".into(), "--help".into()])?;
        c.stdout.extend(help.stdout);
        c.exit_code = 1;
        return Ok(());
    };
    match name.as_str() {
        "templates" => reader::templates(c, &args[1..]),
        "list" => reader::list(c, &args[1..]),
        "show" => reader::show(c, &args[1..]),
        "validate" => reader::validate(c, &args[1..]),
        "prompt" => prompt::run(c, &args[1..]),
        "create" => mutate::create(c, &args[1..]),
        "outcome" => mutate::outcome(c, &args[1..]),
        "review" => mutate::review(c, &args[1..]),
        "edit" | "submit" | "adopt" | "reorder" | "release" => {
            Err(Error::unsupported(format!("the plan {name} command"), PLAN_4).into())
        }
        _ => Err(super::Failure::message(format!(
            "error: unknown command '{name}'"
        ))),
    }
}
/// Checks, before anything is written, that changing the status of
/// `tickets` needs no plan transition. At this commit: refused, naming
/// s4-missions-lib-4, when a ticket has a `plan_id` or `plans.jsonl` holds
/// anything but ASCII white space.
#[allow(
    dead_code,
    reason = "called by s4-missions-lib-2's close and s4-missions-lib-4's update"
)]
pub(in crate::command) fn check_status_change(store: &Store, tickets: &[&Ticket]) -> Result<()> {
    if let Some(ticket) = tickets.iter().find(|ticket| !ticket.plan_id.is_empty()) {
        return Err(Error::unsupported(
            format!(
                "a status change on {}, a ticket with plan context (plan_id {}) (plan transitions)",
                ticket.id(),
                ticket.plan_id
            ),
            PLAN_4,
        ));
    }
    let path = store.path(DataFile::Plans);
    let content = match std::fs::read(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(Error::io("read", path, error)),
    };
    // Go's ASCII white space (`asciiSpace` in package bytes).
    let blank = content
        .iter()
        .all(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'\x0b' | b'\x0c'));
    if !blank {
        let feature = match tickets.first() {
            Some(ticket) => format!(
                "a status change on {} while plans.jsonl holds plans (plan transitions)",
                ticket.id()
            ),
            None => "a status change while plans.jsonl holds plans (plan transitions)".to_owned(),
        };
        return Err(Error::unsupported(feature, PLAN_4));
    }
    Ok(())
}

/// Applies the plan transitions that changing the status of the tickets
/// `ids` (now `tickets`) at the time `now` causes. At this commit there are
/// none: [`check_status_change`] has refused every case with one.
#[allow(
    dead_code,
    reason = "called by s4-missions-lib-2's close and s4-missions-lib-4's update"
)]
pub(in crate::command) fn apply_status_change(
    _store: &Store,
    _ids: &[String],
    _tickets: &[Ticket],
    _now: &str,
) -> Result<()> {
    Ok(())
}

/// The expectations of `every_other_go_command_answers_as_its_plan_states`
/// and `the_status_change_seams_answer_as_their_owner_states` in
/// `command/mod.rs`.
#[cfg(test)]
pub(super) mod checks {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;

    use super::super::{run, Entry, Invocation};
    use super::{apply_status_change, check_status_change};
    use crate::error::{Error, PLAN_4};
    use crate::model::Ticket;
    use crate::store::Store;

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
    /// deferred: Unsupported naming s4-missions-lib-4. Once ported, Go's exact
    /// outcome in a folder without a store, as literals.
    pub(in crate::command) fn command(name: &str) {
        let entry = match name {
            "plan" => super::PLAN,
            other => panic!("{other} is not a command of this module"),
        };
        match entry {
            Entry::Deferred(plan) => {
                assert_eq!(plan, PLAN_4, "{name}");
                let error = run(&invocation(&[name, "--json"])).unwrap_err();
                assert_eq!(
                    error.to_string(),
                    format!(
                        "the {name} command is not supported yet; it is ported in s4-missions-lib-4"
                    ),
                    "{name}"
                );
                assert_eq!(plan_of(error), plan, "{name}");
            }
            Entry::Ported(_) => {
                let temp = tempfile::tempdir().unwrap();
                let out = run(&Invocation::new(temp.path(), ["plan", "--json"])).unwrap();
                assert_eq!(out.stdout,b"{\n  \"success\": false,\n  \"command\": \"plan\",\n  \"error\": \"error: unknown command '--json'\"\n}\n");
                assert_eq!(out.stderr, b"");
                assert_eq!(out.exit_code, 1);
            }
        }
    }

    const ISSUES: &str = concat!(
        r#"{"id":"p-1","title":"A","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        "\n",
        r#"{"id":"p-2","title":"B","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","plan_id":"plan-a"}"#,
        "\n",
    );

    const ONE_PLAN: &str = concat!(
        r#"{"id":"plan-a","seed":"p-1","template":"t","status":"approved","revision":1,"sections":{},"children":["p-2"],"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        "\n",
    );

    /// Every file in `dir` with its bytes.
    fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let name = entry.file_name().into_string().unwrap();
                (name, fs::read(entry.path()).unwrap())
            })
            .collect()
    }

    fn unsupported(result: crate::Result<()>, text: &str) {
        match result {
            Err(error @ Error::Unsupported { .. }) => {
                assert_eq!(error.to_string(), text);
                assert_eq!(plan_of(error), PLAN_4);
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    /// The status-change seams on literal stores: a ticket without
    /// `plan_id` and an empty `plans.jsonl`, a ticket with `plan_id`, and a
    /// `plans.jsonl` with a plan. While deferred: Ok, then Unsupported
    /// naming s4-missions-lib-4 twice, and `apply_status_change` changes no
    /// file. Once ported, Go's `planTransitionHook` result for the same
    /// inputs and the literal `plans.jsonl` after.
    pub(in crate::command) fn status_change_seams() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".luuna/missions");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.yaml"), "project: \"p\"\nversion: \"1\"\n").unwrap();
        fs::write(dir.join("issues.jsonl"), ISSUES).unwrap();
        fs::write(dir.join("plans.jsonl"), "").unwrap();
        let store = Store::open(&dir).unwrap();
        let tickets = store.read_tickets().unwrap();
        let [plain, planned]: [&Ticket; 2] = [&tickets[0], &tickets[1]];
        assert_eq!((plain.id(), planned.plan_id.as_str()), ("p-1", "plan-a"));

        check_status_change(&store, &[plain]).unwrap();
        unsupported(
            check_status_change(&store, &[plain, planned]),
            "a status change on p-2, a ticket with plan context (plan_id plan-a) (plan transitions) is not supported yet; it is ported in s4-missions-lib-4",
        );
        fs::write(dir.join("plans.jsonl"), ONE_PLAN).unwrap();
        unsupported(
            check_status_change(&store, &[plain]),
            "a status change on p-1 while plans.jsonl holds plans (plan transitions) is not supported yet; it is ported in s4-missions-lib-4",
        );

        let before = files(&dir);
        let mut closed = plain.clone();
        closed.status = "closed".to_owned();
        apply_status_change(
            &store,
            &["p-1".to_owned()],
            &[closed],
            "2026-01-03T00:00:00.000Z",
        )
        .unwrap();
        assert_eq!(files(&dir), before);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{find, Entry};
    use super::*;

    #[test]
    fn its_route_points_at_this_module() {
        match (find("plan"), PLAN) {
            (Some(Entry::Deferred(routed)), Entry::Deferred(own)) => {
                assert_eq!(routed, own, "plan");
                assert_eq!(own, PLAN_4, "plan");
            }
            (Some(Entry::Ported(_)), Entry::Ported(_)) => {}
            (None, _) => panic!("plan is not in the command table"),
            _ => panic!("plan: the command table and this module differ in kind"),
        }
    }
}
