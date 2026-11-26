//! Baseline evidence and the transactional gate into feature implementation.
//!
//! A baseline run records one repository's required check results at an exact
//! commit on an exact host. `begin_implementation` is the only way into
//! `Implementing`: it re-reads the durable approval, the bound plan, the
//! project's current manifest and the latest baseline per repository inside
//! one write transaction, and rejects anything stale.
use crate::{append_project_event, authorize_run, now, transition, Result, StorageError, Store};
use luuna_domain::{
    BaselineEvidence, BaselineStatus, CheckResult, DomainError, PlanRevision, RunState,
    TicketPurpose,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use std::collections::BTreeMap;

fn status_text(status: BaselineStatus) -> Result<String> {
    Ok(serde_json::to_value(status)?
        .as_str()
        .unwrap_or("interrupted")
        .to_owned())
}

impl Store {
    /// Open a baseline record for a run that has already been created.
    pub fn start_baseline(&mut self, evidence: &BaselineEvidence) -> Result<()> {
        evidence.identity.validate()?;
        if evidence.id.trim().is_empty()
            || evidence.status != BaselineStatus::Running
            || !evidence.results.is_empty()
            || evidence.required_checks.is_empty()
        {
            return Err(StorageError::Invalid(
                "a new baseline starts running with required checks and no results".into(),
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let run_project: String = tx
            .query_row(
                "SELECT project_id FROM runs WHERE id=?1",
                [&evidence.run_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound(evidence.run_id.clone()))?;
        if run_project != evidence.project_id {
            return Err(StorageError::Invalid(
                "baseline run belongs to a different project".into(),
            ));
        }
        // One running baseline per repository: two would run the same recipes
        // in the same developer checkout at once. The runtime refuses the
        // second request; this keeps the rule in the transaction too.
        let running: Option<String> = tx
            .query_row(
                "SELECT b.run_id FROM baselines b JOIN runs r ON r.id=b.run_id \
                 WHERE b.project_id=?1 AND b.repository_id=?2 AND b.status='running' \
                 AND b.run_id<>?3 AND r.state IN (?4,?5) LIMIT 1",
                params![
                    evidence.project_id,
                    evidence.identity.repository_id,
                    evidence.run_id,
                    crate::state_json(RunState::Queued)?,
                    crate::state_json(RunState::Running)?
                ],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(run) = running {
            return Err(StorageError::AlreadyExists(format!(
                "baseline run {run} is already running for repository {}",
                evidence.identity.repository_id
            )));
        }
        tx.execute(
            "INSERT INTO baselines(id,project_id,repository_id,run_id,status,payload,started_at,finished_at) VALUES (?1,?2,?3,?4,?5,?6,?7,NULL)",
            params![
                evidence.id,
                evidence.project_id,
                evidence.identity.repository_id,
                evidence.run_id,
                status_text(evidence.status)?,
                serde_json::to_string(evidence)?,
                evidence.started_at
            ],
        )?;
        append_project_event(
            &tx,
            &evidence.project_id,
            "baseline.started",
            json!({
                "baseline_id": evidence.id,
                "run_id": evidence.run_id,
                "repository_id": evidence.identity.repository_id,
                "manifest_hash": evidence.identity.manifest_hash,
                "manifest_revision": evidence.manifest_revision,
                "commit": evidence.identity.commit,
                "tree": evidence.identity.tree,
                "required_checks": evidence.required_checks,
            }),
        )?;
        tx.commit()?;
        self.mirror_logs(&[&evidence.project_id]);
        Ok(())
    }

    /// Append one deterministic check result to a running baseline.
    pub fn record_check_result(&mut self, baseline_id: &str, result: &CheckResult) -> Result<()> {
        result.validate()?;
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let (status, run_id): (String, String) = tx
            .query_row(
                "SELECT status,run_id FROM baselines WHERE id=?1",
                [baseline_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound(baseline_id.into()))?;
        if status != "running" {
            return Err(StorageError::Invalid(
                "check results can only be added to a running baseline".into(),
            ));
        }
        if run_id != result.run_id {
            return Err(StorageError::Invalid(
                "check result run does not match the baseline run".into(),
            ));
        }
        tx.execute(
            "INSERT INTO check_results(id,baseline_id,recipe_id,payload) VALUES (?1,?2,?3,?4)",
            params![
                result.id,
                baseline_id,
                result.recipe_id,
                serde_json::to_string(result)?
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Close a baseline with its aggregate status; results already recorded stay.
    pub fn finish_baseline(
        &mut self,
        baseline_id: &str,
        status: BaselineStatus,
        reason: Option<&str>,
    ) -> Result<BaselineEvidence> {
        if status == BaselineStatus::Running {
            return Err(StorageError::Invalid(
                "a finished baseline needs a terminal status".into(),
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut evidence = load_baseline(&tx, baseline_id)?
            .ok_or_else(|| StorageError::NotFound(baseline_id.into()))?;
        if evidence.status != BaselineStatus::Running {
            return Err(StorageError::Invalid("baseline is already finished".into()));
        }
        let finished_at = now();
        evidence.status = status;
        evidence.finished_at = Some(finished_at.clone());
        evidence.reason = reason.map(ToOwned::to_owned);
        let mut stored = evidence.clone();
        stored.results.clear();
        tx.execute(
            "UPDATE baselines SET status=?1,payload=?2,finished_at=?3 WHERE id=?4",
            params![
                status_text(status)?,
                serde_json::to_string(&stored)?,
                finished_at,
                baseline_id
            ],
        )?;
        append_project_event(
            &tx,
            &evidence.project_id,
            "baseline.finished",
            json!({
                "baseline_id": evidence.id,
                "run_id": evidence.run_id,
                "repository_id": evidence.identity.repository_id,
                "manifest_hash": evidence.identity.manifest_hash,
                "commit": evidence.identity.commit,
                "environment_fingerprint": evidence.environment_fingerprint,
                "status": status,
                "reason": evidence.reason,
                "results": evidence.results.iter().map(|r| json!({
                    "recipe_id": r.recipe_id, "status": r.status, "exit_code": r.exit_code,
                    "candidate_hash": r.candidate_hash, "artifact_refs": r.artifact_refs,
                })).collect::<Vec<_>>(),
            }),
        )?;
        tx.commit()?;
        self.mirror_logs(&[&evidence.project_id]);
        Ok(evidence)
    }

    pub fn get_baseline(&self, id: &str) -> Result<Option<BaselineEvidence>> {
        load_baseline(&self.connection, id)
    }

    pub fn baselines(&self, project_id: &str) -> Result<Vec<BaselineEvidence>> {
        let mut statement = self
            .connection
            .prepare("SELECT id FROM baselines WHERE project_id=?1 ORDER BY rowid DESC")?;
        let ids = statement
            .query_map([project_id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ids.iter()
            .map(|id| {
                load_baseline(&self.connection, id)?
                    .ok_or_else(|| StorageError::NotFound(id.clone()))
            })
            .collect()
    }

    pub fn latest_baseline(
        &self,
        project_id: &str,
        repository_id: &str,
    ) -> Result<Option<BaselineEvidence>> {
        latest_baseline(&self.connection, project_id, repository_id)
    }

    /// Enter `Implementing` only with a durable approval for the bound plan, the
    /// project's current manifest, and a passed baseline per repository that
    /// binds to the plan's manifest hash, base commit and this environment.
    ///
    /// A `testing_setup` plan is the prerequisite that creates that baseline
    /// (ADR 0022): it is admitted without one, under every other gate, and
    /// the transition records `prerequisite: testing_setup` instead of
    /// baseline identities. The purpose is part of the approved plan hash, so
    /// no feature plan can claim it.
    pub fn begin_implementation(
        &mut self,
        run_id: &str,
        expected_revision: u64,
        environment_fingerprint: &str,
    ) -> Result<crate::RunRecord> {
        if environment_fingerprint.is_empty() {
            return Err(StorageError::Invalid(
                "environment fingerprint is required".into(),
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let run =
            crate::get_run(&tx, run_id)?.ok_or_else(|| StorageError::NotFound(run_id.into()))?;
        if run.state != RunState::Approved {
            return Err(StorageError::Invalid("initial implementation requires an approved workflow; repairs use begin_candidate_repair".into()));
        }
        authorize_run(&tx, run_id)?;
        let (plan_id, plan_revision): (String, u64) = tx
            .query_row(
                "SELECT plan_id,plan_revision FROM runs WHERE id=?1 AND plan_id IS NOT NULL",
                [run_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| StorageError::Invalid("run is not bound to a plan".into()))?;
        let plan: PlanRevision = tx
            .query_row(
                "SELECT payload FROM plan_revisions WHERE id=?1 AND revision=?2",
                params![plan_id, plan_revision],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(|payload| serde_json::from_str(&payload))
            .transpose()?
            .ok_or_else(|| StorageError::NotFound(plan_id.clone()))?;
        // The ticket, manifest and recorded base commits must still be what the
        // plan was made against; an external edit or re-confirmation blocks scheduling.
        crate::writer::check_ticket_binding(&tx, &plan)?;
        crate::writer::check_manifest_binding(&tx, &plan)?;
        let mut baselines = BTreeMap::new();
        let without_baseline = plan.purpose == TicketPurpose::TestingSetup;
        for repository_id in plan.base_commits.keys() {
            if without_baseline {
                break;
            }
            let evidence =
                latest_baseline(&tx, &plan.project_id, repository_id)?.ok_or_else(|| {
                    StorageError::Domain(DomainError::BaselineRequired(format!(
                        "no baseline has been recorded for repository {repository_id}"
                    )))
                })?;
            evidence.authorizes(&plan, repository_id, environment_fingerprint)?;
            // The verdict and its run settle in separate writes; evidence whose run
            // never reached `completed` (a crash in between, later interrupted) is
            // not a settled baseline.
            let baseline_run = crate::get_run(&tx, &evidence.run_id)?
                .ok_or_else(|| StorageError::NotFound(evidence.run_id.clone()))?;
            if baseline_run.state != RunState::Completed {
                return Err(StorageError::Domain(DomainError::BaselineRequired(
                    format!(
                        "baseline {} belongs to run {} which settled as {:?}, not completed",
                        evidence.id, evidence.run_id, baseline_run.state
                    ),
                )));
            }
            baselines.insert(repository_id.clone(), evidence.id.clone());
        }
        crate::writer::require_repositories_free(&tx, run_id, &plan)?;
        transition(
            &tx,
            run_id,
            expected_revision,
            RunState::Implementing,
            "run.implementing",
            json!({
                "plan_id": plan.id,
                "plan_revision": plan.revision,
                "plan_hash": plan.hash,
                "manifest_hash": plan.manifest_hash,
                "baselines": baselines,
                "prerequisite": without_baseline.then_some(luuna_domain::TESTING_SETUP_PREREQUISITE),
                "environment_fingerprint": environment_fingerprint,
            }),
        )?;
        tx.commit()?;
        self.get_run(run_id)?
            .ok_or_else(|| StorageError::NotFound(run_id.into()))
    }
}

/// Called by startup recovery after active runs were interrupted in the same
/// transaction: a baseline still running when the runtime stopped, or one whose
/// verdict was written but whose run never settled, can never be reported passed.
pub(crate) fn interrupt_running_baselines(
    tx: &rusqlite::Transaction<'_>,
    skipped: &mut Vec<crate::SkippedRecord>,
) -> Result<usize> {
    let mut statement = tx.prepare(
        "SELECT b.id, b.status, b.project_id FROM baselines b JOIN runs r ON r.id=b.run_id \
         WHERE b.status='running' OR (b.status='passed' AND r.state=?1)",
    )?;
    let ids = statement
        .query_map([crate::state_json(RunState::Interrupted)?], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(statement);
    let finished_at = now();
    for (id, previous, project_id) in &ids {
        let mut evidence = match load_baseline(tx, id) {
            Ok(Some(evidence)) => evidence,
            Ok(None) => return Err(StorageError::NotFound(id.clone())),
            Err(StorageError::Json(error)) => {
                // The status column alone is enough: the implementation gate
                // decodes the record and fails closed on it anyway.
                let reason = format!(
                    "runtime restarted before the baseline settled; {}; not evidence",
                    crate::undecodable(&error)
                );
                tx.execute(
                    "UPDATE baselines SET status='interrupted',finished_at=?1 WHERE id=?2",
                    params![finished_at, id],
                )?;
                append_project_event(
                    tx,
                    project_id,
                    "baseline.finished",
                    json!({"baseline_id": id, "status": BaselineStatus::Interrupted, "reason": reason}),
                )?;
                skipped.push(crate::SkippedRecord {
                    table: "baselines",
                    id: id.clone(),
                    reason,
                });
                continue;
            }
            Err(error) => return Err(error),
        };
        evidence.status = BaselineStatus::Interrupted;
        evidence.finished_at = Some(finished_at.clone());
        evidence.reason = Some(if previous == "passed" {
            "runtime restarted before the baseline run settled; the verdict is not evidence".into()
        } else {
            "runtime restarted before the baseline finished".into()
        });
        let mut stored = evidence.clone();
        stored.results.clear();
        tx.execute(
            "UPDATE baselines SET status='interrupted',payload=?1,finished_at=?2 WHERE id=?3",
            params![serde_json::to_string(&stored)?, finished_at, id],
        )?;
        append_project_event(
            tx,
            &evidence.project_id,
            "baseline.finished",
            json!({
                "baseline_id": evidence.id,
                "run_id": evidence.run_id,
                "repository_id": evidence.identity.repository_id,
                "manifest_hash": evidence.identity.manifest_hash,
                "commit": evidence.identity.commit,
                "status": BaselineStatus::Interrupted,
                "reason": evidence.reason,
            }),
        )?;
    }
    Ok(ids.len())
}

fn load_baseline(connection: &Connection, id: &str) -> Result<Option<BaselineEvidence>> {
    let payload: Option<String> = connection
        .query_row("SELECT payload FROM baselines WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .optional()?;
    let Some(payload) = payload else {
        return Ok(None);
    };
    let mut evidence: BaselineEvidence = serde_json::from_str(&payload)?;
    let mut statement = connection
        .prepare("SELECT payload FROM check_results WHERE baseline_id=?1 ORDER BY rowid")?;
    let results = statement
        .query_map([id], |r| r.get::<_, String>(0))?
        .map(|row| Ok(serde_json::from_str::<CheckResult>(&row?)?))
        .collect::<Result<Vec<_>>>()?;
    evidence.results = results;
    Ok(Some(evidence))
}

/// The most recently recorded baseline. Insertion order (rowid; baselines are
/// never deleted and a vacuumed copy keeps their relative order) decides, never
/// the wall-clock `started_at`, so a clock stepped backwards cannot let an
/// older passed baseline mask a newer failed one.
pub(crate) fn latest_baseline(
    connection: &Connection,
    project_id: &str,
    repository_id: &str,
) -> Result<Option<BaselineEvidence>> {
    let id: Option<String> = connection
        .query_row(
            "SELECT id FROM baselines WHERE project_id=?1 AND repository_id=?2 ORDER BY rowid DESC LIMIT 1",
            params![project_id, repository_id],
            |r| r.get(0),
        )
        .optional()?;
    id.map(|id| load_baseline(connection, &id)?.ok_or_else(|| StorageError::NotFound(id)))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use luuna_domain::{BaselineIdentity, DirtyState, RepositoryIdentity, RepositoryObservation};

    fn store_with_project() -> Store {
        let mut store = Store::open(":memory:").unwrap();
        let observation = RepositoryObservation {
            local_path: "/srv/checkouts/app".into(),
            identity: RepositoryIdentity {
                remote: Some("https://example.invalid/app.git".into()),
                push_remote: None,
                root_commits: vec!["1".repeat(40)],
                shallow: false,
            },
            head_commit: "2".repeat(40),
            branch: Some("main".into()),
            remote_default_branch: None,
            local_branches: vec!["main".into()],
            dirty: DirtyState::default(),
            git_version: "git version 2.43.0".into(),
            observed_at: "2026-10-17T00:00:00.000Z".into(),
        };
        store
            .register_project("project", "Project", &[("app".into(), observation)])
            .unwrap();
        store
    }

    fn running_baseline(store: &mut Store, run_id: &str) -> BaselineEvidence {
        let run = store
            .create_run(run_id, "project", json!({"kind": "baseline"}))
            .unwrap();
        store
            .transition_run(
                &run.id,
                run.revision,
                RunState::Running,
                "run.running",
                json!({}),
            )
            .unwrap();
        BaselineEvidence {
            id: format!("baseline-{run_id}"),
            run_id: run_id.into(),
            project_id: "project".into(),
            manifest_revision: 1,
            identity: BaselineIdentity {
                manifest_hash: format!("sha256:{}", "a".repeat(64)),
                repository_id: "app".into(),
                commit: "2".repeat(40),
                tree: "3".repeat(40),
            },
            environment_fingerprint: format!("sha256:{}", "e".repeat(64)),
            status: BaselineStatus::Running,
            required_checks: vec!["unit".into()],
            results: vec![],
            started_at: "2026-10-17T00:00:00.000Z".into(),
            finished_at: None,
            reason: None,
        }
    }

    #[test]
    fn one_running_baseline_per_repository_is_enforced_in_the_transaction() {
        let mut store = store_with_project();
        let first = running_baseline(&mut store, "run-1");
        store.start_baseline(&first).unwrap();
        let second = running_baseline(&mut store, "run-2");
        match store.start_baseline(&second) {
            Err(StorageError::AlreadyExists(reason)) => {
                assert!(reason.contains("run-1"), "{reason}")
            }
            other => panic!("a second running baseline must be refused: {other:?}"),
        }
        store
            .finish_baseline(&first.id, BaselineStatus::Failed, Some("settled"))
            .unwrap();
        store.start_baseline(&second).unwrap();
    }
}
