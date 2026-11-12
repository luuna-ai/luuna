//! The plan index the read and workflow commands consult (Go's
//! `loadPlanIndex`, `internal/cli/issue_read.go:87-114`), owned by
//! s4-missions-lib-2.
//!
//! At this commit [`load`] gives an empty index when `plans.jsonl` is
//! missing or holds only ASCII white space, which is exactly what Go's index
//! holds then, and refuses with
//! [`Error::Unsupported`](crate::Error::Unsupported) naming
//! s4-missions-lib-2 otherwise. s4-missions-lib-2 replaces the body of
//! [`load`] and the `checks` it owns.

use std::collections::HashMap;
use std::io;

use crate::error::{Error, Result, PLAN_2};
use crate::model::Ticket;
use crate::store::{DataFile, Store};

/// The plans of `plans.jsonl`, by plan id and by seed ticket.
#[allow(
    dead_code,
    reason = "read by the commands of s4-missions-lib-2 and by s4-missions-lib-3's prime"
)]
pub(crate) struct PlanIndex {
    /// The status of each plan, by plan id (Go's `byID`).
    by_id: HashMap<String, String>,
    /// The status of the latest plan seeded by each ticket (Go's `bySeed`).
    by_seed: HashMap<String, String>,
}

/// Go's `loadPlanIndex` for the store.
#[allow(
    dead_code,
    reason = "called by the commands of s4-missions-lib-2 and by s4-missions-lib-3's prime"
)]
pub(crate) fn load(store: &Store) -> Result<PlanIndex> {
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
        return Err(Error::unsupported(
            "the plan index (plans.jsonl has records)",
            PLAN_2,
        ));
    }
    Ok(PlanIndex {
        by_id: HashMap::new(),
        by_seed: HashMap::new(),
    })
}

impl PlanIndex {
    /// The status of the plan `ticket` names in its `plan_id`, if that plan
    /// exists (Go's `forIssue`).
    #[allow(
        dead_code,
        reason = "called by the commands of s4-missions-lib-2 and by s4-missions-lib-3's prime"
    )]
    pub(crate) fn status_for_ticket(&self, ticket: &Ticket) -> Option<&str> {
        if ticket.plan_id.is_empty() {
            return None;
        }
        self.by_id.get(&ticket.plan_id).map(String::as_str)
    }

    /// The status of the latest plan seeded by the ticket `ticket_id`.
    #[allow(
        dead_code,
        reason = "called by the commands of s4-missions-lib-2 and by s4-missions-lib-3's prime"
    )]
    pub(crate) fn status_for_seed(&self, ticket_id: &str) -> Option<&str> {
        self.by_seed.get(ticket_id).map(String::as_str)
    }
}

/// The expectations of `the_plan_index_answers_as_its_owner_states` in
/// `command/mod.rs`.
#[cfg(test)]
pub(super) mod checks {
    use std::fs;

    use super::load;
    use crate::error::{Error, PLAN_2};
    use crate::model::Ticket;
    use crate::store::Store;

    /// Two plan records seeded by the same ticket, the later one with a
    /// child.
    const TWO_PLANS: &str = concat!(
        r#"{"id":"plan-a","seed":"p-1","template":"t","status":"draft","revision":1,"sections":{},"children":[],"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        "\n",
        r#"{"id":"plan-b","seed":"p-1","template":"t","status":"approved","revision":2,"sections":{},"children":["p-2"],"createdAt":"2026-01-02T00:00:00.000Z","updatedAt":"2026-01-02T00:00:00.000Z"}"#,
        "\n",
    );

    /// A missing `plans.jsonl` and one holding only white space give an
    /// index that knows no plan; [`TWO_PLANS`] gives, while deferred,
    /// Unsupported naming s4-missions-lib-2 and, once ported, the statuses
    /// Go's `loadPlanIndex` gives, as literals.
    pub(in crate::command) fn plans_file() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".luuna/missions");
        fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir).unwrap();
        let mut planned = Ticket::default();
        planned.id = "p-2".to_owned();
        planned.plan_id = "plan-b".to_owned();
        let mut seed = Ticket::default();
        seed.id = "p-1".to_owned();

        // Missing, then white space only: always empty.
        for content in [None, Some(" \n\t\r\n\n")] {
            if let Some(content) = content {
                fs::write(dir.join("plans.jsonl"), content).unwrap();
            }
            let index = load(&store).unwrap();
            for ticket in [&planned, &seed] {
                assert_eq!(index.status_for_ticket(ticket), None, "{content:?}");
                assert_eq!(index.status_for_seed(ticket.id()), None, "{content:?}");
            }
        }

        fs::write(dir.join("plans.jsonl"), TWO_PLANS).unwrap();
        match load(&store) {
            Err(Error::Unsupported { feature, plan }) => {
                assert_eq!(plan, PLAN_2);
                assert_eq!(feature, "the plan index (plans.jsonl has records)");
            }
            Err(other) => panic!("expected Unsupported, got {other:?}"),
            Ok(_) => panic!("expected Unsupported, got an index"),
        }
    }
}
