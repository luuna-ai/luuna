//! Read-only projections of authoritative state for Home and the Kanban board (formerly
//! packages/api-client/src/projections.ts, which the desktop still renders): nothing here decides
//! anything, and the runtime's run state is the only source of a column.
use crate::failures::Failure;
use crate::js::J;

fn type_error() -> Failure {
    Failure::type_error()
}

/// `value.key` where `value` must not be null or undefined.
fn field<'a>(value: &'a J, key: &str) -> Result<&'a J, Failure> {
    if value.nullish() {
        Err(type_error())
    } else {
        Ok(value.get(key))
    }
}

fn items(value: &J) -> Result<&[J], Failure> {
    value.as_arr().map(|v| v.as_slice()).ok_or_else(type_error)
}

pub const BOARD_COLUMNS: [(&str, &str, &str); 8] = [
    ("backlog", "Backlog", "Tickets without a plan revision."),
    ("testing_prerequisite", "Testing prerequisite", "Repositories whose latest baseline shows no usable testing setup (the runtime offers a testing-setup ticket), and workflows blocked by a check that could not run (testing_blocked)."),
    ("plan", "Discovery / plan", "Workflows in discovery or exploration (discovered, exploring, queued, running), and settled workflows whose next step is the next plan revision (completed, failed, cancelled, interrupted)."),
    ("approval", "Approval", "Plan revisions waiting for the developer's exact-hash approval (plan_ready)."),
    ("implementation", "Implementation", "Approved plans and running writers (approved, implementing)."),
    ("checks", "Checks", "Candidates whose required checks run or wait to run (validating)."),
    ("review", "Review", "Independent review in progress, or a review that did not pass and waits for a repair or a new revision (reviewing, paused)."),
    ("prs", "PRs", "Reviewed candidates; publishing and merging stay with the developer (reviewed)."),
];

pub const BOARD_NOTE: &str = "This board is a projection of the runtime's recorded run states. Cards are read-only and open the ticket's own panel; dragging a card is not implemented, because moving a card can never bypass a validation gate or an approval.";

fn run_column(state: &J) -> Option<&'static str> {
    Some(match state.as_str()? {
        "queued" | "running" | "discovered" | "exploring" | "completed" | "failed"
        | "cancelled" | "interrupted" => "plan",
        "testing_blocked" => "testing_prerequisite",
        "plan_ready" => "approval",
        "approved" | "implementing" => "implementation",
        "validating" => "checks",
        "reviewing" | "paused" => "review",
        "reviewed" => "prs",
        _ => return None,
    })
}

/// JavaScript `a > b` for the two values compared here (timestamps as strings).
fn greater(a: &J, b: &J) -> bool {
    match (a, b) {
        (J::Str(x), J::Str(y)) => {
            x.encode_utf16().cmp(y.encode_utf16()) == std::cmp::Ordering::Greater
        }
        _ => {
            let (x, y) = (a.to_number(), b.to_number());
            x > y
        }
    }
}

fn plan_revision(run: &J) -> Result<f64, Failure> {
    let value = field(run, "metadata").and_then(|m| field(m, "plan_revision"))?;
    Ok(if value.nullish() {
        0.0
    } else {
        value.to_number()
    })
}

/// The workflow run of a ticket's latest plan revision, if any.
pub fn latest_workflow<'a>(ticket: &J, runs: &'a [J]) -> Result<Option<&'a J>, Failure> {
    let mut latest: Option<&J> = None;
    let ticket_id = field(ticket, "id")?;
    for run in runs {
        let metadata = field(run, "metadata")?;
        if field(metadata, "kind")?.as_str() != Some("workflow")
            || field(metadata, "ticket_id")? != ticket_id
        {
            continue;
        }
        let revision = plan_revision(run)?;
        let seen = match latest {
            Some(l) => plan_revision(l)?,
            None => -1.0,
        };
        let newer = match latest {
            None => true,
            Some(l) => {
                revision > seen
                    || (revision == seen
                        && greater(field(run, "updated_at")?, field(l, "updated_at")?))
            }
        };
        if newer {
            latest = Some(run);
        }
    }
    Ok(latest)
}

/// Every ticket as a card in the column of its latest workflow's state, plus one card per offer.
pub fn board_projection(projects: &[J], tickets: &[J], runs: &[J]) -> Result<J, Failure> {
    let mut cards: Vec<J> = Vec::new();
    for view in projects {
        let prerequisites = field(view, "prerequisites")?;
        let list = if prerequisites.nullish() {
            &[][..]
        } else {
            items(prerequisites)?
        };
        for prerequisite in list {
            let repository = field(prerequisite, "repository_id")?;
            cards.push(crate::obj![
                ("column", "testing_prerequisite"),
                ("kind", "prerequisite"),
                ("project_id", field(field(view, "project")?, "id")?.clone()),
                (
                    "title",
                    format!("Testing setup needed for {}", repository.js())
                ),
                ("repository_id", repository.clone()),
                ("reason", field(prerequisite, "reason")?.clone()),
            ]);
        }
    }
    for ticket in tickets {
        let run = latest_workflow(ticket, runs)?;
        let column = match run {
            Some(run) => run_column(field(run, "state")?)
                .map(J::from)
                .unwrap_or(J::Undefined),
            None => J::str("backlog"),
        };
        let mut card = crate::obj![
            ("column", column),
            ("kind", "ticket"),
            ("project_id", field(ticket, "project_id")?.clone()),
            ("title", field(ticket, "title")?.clone()),
            ("ticket_id", field(ticket, "id")?.clone()),
            ("ticket_revision", field(ticket, "revision")?.clone()),
        ];
        let purpose = field(ticket, "purpose")?;
        if purpose.truthy() && purpose.as_str() != Some("feature") {
            card.set("purpose", purpose.clone());
        }
        if let Some(run) = run {
            card.set("run_id", field(run, "id")?.clone())
                .set("state", field(run, "state")?.clone())
                .set("plan_revision", J::Num(plan_revision(run)?));
        }
        cards.push(card);
    }
    let columns = BOARD_COLUMNS
        .iter()
        .map(|(id, title, description)| {
            crate::obj![
                (
                    "column",
                    crate::obj![
                        ("id", *id),
                        ("title", *title),
                        ("description", *description)
                    ]
                ),
                (
                    "cards",
                    J::Arr(
                        cards
                            .iter()
                            .filter(|card| card.get("column").as_str() == Some(id))
                            .cloned()
                            .collect()
                    )
                ),
            ]
        })
        .collect::<Vec<_>>();
    Ok(crate::obj![
        ("columns", J::Arr(columns)),
        ("note", BOARD_NOTE)
    ])
}

const BLOCKING: [&str; 4] = ["failed", "paused", "interrupted", "testing_blocked"];
const SETTLED: [&str; 5] = [
    "completed",
    "failed",
    "cancelled",
    "interrupted",
    "reviewed",
];

/// Projects with their validation health and the blockers waiting on the developer.
/// `baselines` and `tickets` are per project, in the order of `projects`.
pub fn home_summary(
    projects: &[J],
    baselines: &[(String, J)],
    tickets: &[(String, J)],
    runs: &[J],
) -> Result<J, Failure> {
    let lookup =
        |list: &[(String, J)], id: &str| list.iter().find(|(k, _)| k == id).map(|(_, v)| v.clone());
    let mut out_projects = Vec::new();
    let mut blockers = Vec::new();
    for view in projects {
        let project = field(view, "project")?;
        let id = field(project, "id")?.clone();
        let id_text = id.js();
        let repositories = items(field(view, "repositories")?)?;
        let repository_ids: Vec<J> = repositories
            .iter()
            .map(|r| field(r, "repository_id").cloned())
            .collect::<Result<_, _>>()?;
        let recorded = lookup(baselines, &id_text).unwrap_or(J::Arr(Vec::new()));
        let recorded = items(&recorded)?.to_vec();
        // The runtime lists baselines newest first (storage rowid DESC).
        let mut latest: Vec<Option<J>> = Vec::new();
        for repository in &repository_ids {
            let mut found = None;
            for baseline in &recorded {
                if field(field(baseline, "identity")?, "repository_id")? == repository {
                    found = Some(baseline.clone());
                    break;
                }
            }
            latest.push(found);
        }
        let prerequisites_value = field(view, "prerequisites")?;
        let prerequisites: Vec<J> = if prerequisites_value.nullish() {
            Vec::new()
        } else {
            items(prerequisites_value)?.to_vec()
        };
        let project_tickets = lookup(tickets, &id_text).unwrap_or(J::Arr(Vec::new()));
        let project_tickets = items(&project_tickets)?.to_vec();
        let mut workflows = Vec::new();
        for run in runs {
            if field(run, "project_id")? == &id
                && field(field(run, "metadata")?, "kind")?.as_str() == Some("workflow")
            {
                workflows.push(run.clone());
            }
        }
        let mut repository_rows = Vec::new();
        for (index, repository) in repositories.iter().enumerate() {
            let baseline = match &latest[index] {
                Some(b) => {
                    let mut summary = crate::obj![
                        ("run_id", field(b, "run_id")?.clone()),
                        ("status", field(b, "status")?.clone()),
                        ("manifest_revision", field(b, "manifest_revision")?.clone()),
                        ("commit", field(field(b, "identity")?, "commit")?.clone()),
                    ];
                    let reason = field(b, "reason")?;
                    if reason.truthy() {
                        summary.set("reason", reason.clone());
                    }
                    summary
                }
                None => J::Null,
            };
            let mut prerequisite = J::Null;
            for offer in &prerequisites {
                if field(offer, "repository_id")? == &repository_ids[index] {
                    prerequisite = offer.clone();
                    break;
                }
            }
            repository_rows.push(crate::obj![
                ("repository_id", repository_ids[index].clone()),
                ("local_path", field(repository, "local_path")?.clone()),
                ("baseline", baseline),
                ("prerequisite", prerequisite),
            ]);
        }
        let mut active = 0usize;
        for run in &workflows {
            if !SETTLED.contains(&field(run, "state")?.js().as_str()) {
                active += 1;
            }
        }
        let manifest = field(view, "manifest")?;
        let manifest_hash = if manifest.nullish() {
            J::Null
        } else {
            manifest.get("hash").or(&J::Null).clone()
        };
        out_projects.push(crate::obj![
            ("id", id.clone()),
            ("name", field(project, "name")?.clone()),
            ("revision", field(project, "revision")?.clone()),
            ("manifest_hash", manifest_hash),
            ("repositories", J::Arr(repository_rows)),
            ("tickets", project_tickets.len()),
            ("active_workflows", active),
        ]);
        for prerequisite in &prerequisites {
            let repository = field(prerequisite, "repository_id")?;
            blockers.push(crate::obj![
                ("kind", "prerequisite"),
                ("project_id", id.clone()),
                (
                    "title",
                    format!("Testing setup needed for {}", repository.js())
                ),
                ("detail", field(prerequisite, "reason")?.clone()),
                ("repository_id", repository.clone()),
            ]);
        }
        for ticket in &project_tickets {
            let Some(run) = latest_workflow(ticket, &workflows)? else {
                continue;
            };
            let state = field(run, "state")?;
            if !BLOCKING.contains(&state.js().as_str()) {
                continue;
            }
            let revision = field(field(run, "metadata")?, "plan_revision")?;
            let revision = if revision.nullish() {
                "?".to_string()
            } else {
                revision.js()
            };
            blockers.push(crate::obj![
                ("kind", "workflow"),
                ("project_id", id.clone()),
                ("title", field(ticket, "title")?.clone()),
                (
                    "detail",
                    format!(
                        "workflow {} (plan r{revision}) is {}",
                        field(run, "id")?.js(),
                        state.js().replace('_', " ")
                    )
                ),
                ("run_id", field(run, "id")?.clone()),
                ("state", state.clone()),
                ("ticket_id", field(ticket, "id")?.clone()),
            ]);
        }
    }
    Ok(crate::obj![
        ("projects", J::Arr(out_projects)),
        ("blockers", J::Arr(blockers))
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_uses_the_newest_baseline_including_a_new_failure_after_a_pass() {
        let project = crate::js::parse(r#"{"project":{"id":"p","name":"P","revision":1},"repositories":[{"repository_id":"app","local_path":"/app"}],"manifest":null}"#).unwrap();
        for (new_status, old_status) in [("passed", "blocked"), ("failed", "passed")] {
            let baseline = |run: &str, status: &str, repository: &str| {
                crate::obj![
                    ("run_id", run),
                    ("status", status),
                    ("manifest_revision", 1usize),
                    (
                        "identity",
                        crate::obj![("repository_id", repository), ("commit", "commit")]
                    )
                ]
            };
            let summary = home_summary(
                std::slice::from_ref(&project),
                &[(
                    "p".into(),
                    J::Arr(vec![
                        baseline("other", "blocked", "other"),
                        baseline("new", new_status, "app"),
                        baseline("old", old_status, "app"),
                    ]),
                )],
                &[],
                &[],
            )
            .unwrap();
            let chosen = summary
                .get("projects")
                .at(0)
                .get("repositories")
                .at(0)
                .get("baseline");
            assert_eq!(chosen.get("run_id").as_str(), Some("new"));
            assert_eq!(chosen.get("status").as_str(), Some(new_status));
        }
    }
}
