//! Native Go-compatible plan lifecycle, raw records and ticket context.
//! Status callers hold plans then issues locks. Preflight sees prospective
//! statuses and refuses a lossy plan rewrite before either store is written.

mod record;

mod atlas;
mod backref;
mod common;
pub(in crate::command) mod context;
mod edit;
#[cfg(test)]
pub(in crate::command) mod foundation_checks;
mod guard;
mod links;
mod mutate;
mod prompt;
mod raw;
mod reader;
mod submit;
mod transitions;
mod tree;
mod validation;

use crate::error::{Result, PLAN_4};
use crate::model::Ticket;
use crate::store::Store;

// The foundation's unchanged check functions retain their Deferred arms.
// Keep that representation constructed without adding a deferred live route.
const _: super::Entry = super::Entry::Deferred(PLAN_4);

/// The shared read index must apply full Go Plan decode/baseline acceptance.
pub(in crate::command) fn accepts_record(line: &[u8]) -> bool {
    <record::Plan as crate::store::Record>::from_line(line).is_some()
}

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
        "edit" => edit::run(c, &args[1..]),
        "submit" => submit::run(c, &args[1..]),
        "adopt" => links::run(c, &args[1..], false),
        "release" => links::run(c, &args[1..], true),
        "reorder" => links::reorder(c, &args[1..]),
        _ => Err(super::Failure::message(format!(
            "error: unknown command '{name}'"
        ))),
    }
}
/// Checks, before anything is written, that changing the status of
/// `tickets` needs no plan transition. At this commit: refused, naming
/// s4-missions-lib-4, when a ticket has a `plan_id` or `plans.jsonl` holds
/// anything but ASCII white space.
pub(in crate::command) fn check_status_change(store: &Store, tickets: &[&Ticket]) -> Result<()> {
    transitions::check(store, tickets)
}

/// Applies the plan transitions that changing the status of the tickets
/// `ids` (now `tickets`) at the time `now` causes. At this commit there are
/// none: [`check_status_change`] has refused every case with one.
pub(in crate::command) fn apply_status_change(
    store: &Store,
    ids: &[String],
    tickets: &[Ticket],
    now: &str,
) -> Result<()> {
    transitions::apply(store, ids, tickets, now)
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
        check_status_change(&store, &[plain, planned]).unwrap();
        fs::write(dir.join("plans.jsonl"), ONE_PLAN).unwrap();
        check_status_change(&store, &[plain]).unwrap();

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
