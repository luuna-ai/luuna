//! Tickets, plan workflows, worktree leases and attempts (M2c).
//!
//! Legacy tickets are the explicitly temporary local source from ADR 0003;
//! enabled Missions tickets are revision-aware projections with durable delivery
//! intent. A plan workflow binds a
//! run to one immutable plan revision that names the ticket revision, manifest
//! hash and base commits it was planned against. A lease records Luuna's
//! ownership of one detached worktree; at most one lease per repository is
//! active, which is how a second writer is refused. Attempts count against the
//! plan's budget and are interrupted, never completed, by a restart.
use crate::{
    append, append_project_event, now, transition, undecodable, Result, SkippedRecord,
    StorageError, Store,
};
use luuna_domain::{
    AdvisoryResult, AttemptState, DependencyOverride, DomainError, LeaseState, PlanRevision,
    RunState, TaskRole, TaskState, Ticket, TicketType, WorktreeLease,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The fields a ticket revision changes; `None` keeps the current value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketRevision {
    pub title: Option<String>,
    pub intent: Option<String>,
    pub acceptance_criteria: Option<Vec<String>>,
    pub repository_ids: Option<Vec<String>>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub ticket_type: Option<TicketType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
}

impl TicketRevision {
    /// Whether the revision names any field at all.
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.intent.is_none()
            && self.acceptance_criteria.is_none()
            && self.repository_ids.is_none()
            && self.ticket_type.is_none()
            && self.priority.is_none()
            && self.labels.is_none()
    }
    /// The names of the fields this revision changes, for the durable event.
    pub fn named(&self) -> Vec<&'static str> {
        let mut fields = Vec::new();
        if self.title.is_some() {
            fields.push("title");
        }
        if self.intent.is_some() {
            fields.push("intent");
        }
        if self.acceptance_criteria.is_some() {
            fields.push("acceptance_criteria");
        }
        if self.repository_ids.is_some() {
            fields.push("repository_ids");
        }
        if self.ticket_type.is_some() {
            fields.push("type");
        }
        if self.priority.is_some() {
            fields.push("priority");
        }
        if self.labels.is_some() {
            fields.push("labels");
        }
        fields
    }
    /// Apply the named fields to a ticket (revision and timestamps untouched).
    pub fn apply(&self, ticket: &mut Ticket) {
        if let Some(title) = &self.title {
            ticket.title = title.clone();
        }
        if let Some(intent) = &self.intent {
            ticket.intent = intent.clone();
        }
        if let Some(criteria) = &self.acceptance_criteria {
            ticket.acceptance_criteria = criteria.clone();
        }
        if let Some(ids) = &self.repository_ids {
            ticket.repository_ids = ids.clone();
        }
        if let Some(kind) = self.ticket_type {
            ticket.ticket_type = kind;
        }
        if let Some(priority) = self.priority {
            ticket.priority = priority;
        }
        if let Some(labels) = &self.labels {
            ticket.labels = labels.clone();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptRecord {
    pub id: String,
    pub run_id: String,
    pub task_id: String,
    pub number: u32,
    pub state: AttemptState,
    pub payload: Value,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
}

/// A plan revision and the workflow bound to it; `created` is false when an
/// idempotent request replayed what an earlier delivery created.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanOpening {
    pub plan: PlanRevision,
    pub run: crate::RunRecord,
    pub created: bool,
}

/// Durable state of one plan task inside a workflow run (M4, migration 10),
/// with its assignment trace and role (ADR 0023, migration 12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRecord {
    pub run_id: String,
    pub task_id: String,
    pub repository_id: String,
    /// `implementer`, `repository_lead` or `project_orchestrator`.
    #[serde(default = "default_role")]
    pub role: String,
    pub state: TaskState,
    /// The attempt that ran (or runs) this task, once scheduled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_tree: Option<String>,
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Who assigned the task: the approval of the exact plan revision
    /// (`approved_by`, `approval_id`, `plan_id`, `plan_revision`, `plan_hash`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_by: Option<Value>,
    /// The advisory task of an earlier plan revision whose proposal this task
    /// was copied from, as the plan recorded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_from_task: Option<String>,
    /// The plan's dependency overrides that name this task as consumer or
    /// producer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependency_overrides: Vec<DependencyOverride>,
    /// An advisory task's structured result (findings, proposed tasks,
    /// risks) once it completed; never set for an implementer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advisory_result: Option<Value>,
    pub updated_at: String,
}

fn default_role() -> String {
    TaskRole::Implementer.as_str().to_owned()
}

/// The trace fields every `task.*` event repeats, so a task's role and
/// parentage are readable from any of its events.
fn trace_json(task: &TaskRecord) -> Value {
    json!({
        "role": task.role,
        "assigned_by": task.assigned_by,
        "derived_from_task": task.derived_from_task,
        "dependency_overrides": task.dependency_overrides,
    })
}

fn with_trace(mut payload: Value, task: &TaskRecord) -> Value {
    if let (Value::Object(target), Value::Object(trace)) = (&mut payload, trace_json(task)) {
        target.extend(trace);
    }
    payload
}

fn text<T: Serialize>(value: T) -> Result<String> {
    Ok(serde_json::to_value(value)?
        .as_str()
        .unwrap_or_default()
        .to_owned())
}

impl Store {
    /// Reserve a ticket projection (or legacy local ticket). The same project and idempotency key
    /// returns the existing record; the same key with different content is a conflict.
    pub fn create_ticket(
        &mut self,
        ticket: &Ticket,
        idempotency_key: &str,
    ) -> Result<(Ticket, bool)> {
        ticket.validate()?;
        if idempotency_key.trim().is_empty() || idempotency_key.len() > 200 {
            return Err(StorageError::Invalid(
                "an idempotency key of 1..200 bytes is required".into(),
            ));
        }
        if ticket.revision != 1 {
            return Err(StorageError::Invalid(
                "a new ticket starts at revision 1".into(),
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        crate::projects::get_project_in(&tx, &ticket.project_id)?
            .ok_or_else(|| StorageError::NotFound(ticket.project_id.clone()))?;
        let registered: Vec<String> = crate::projects::repositories_in(&tx, &ticket.project_id)?
            .into_iter()
            .map(|r| r.repository_id)
            .collect();
        if let Some(unknown) = ticket
            .repository_ids
            .iter()
            .find(|id| !registered.contains(id))
        {
            return Err(StorageError::Invalid(format!(
                "repository {unknown} is not registered in project {}",
                ticket.project_id
            )));
        }
        if let Some(existing) = ticket_by_key(&tx, &ticket.project_id, idempotency_key)? {
            let original = crate::missions::binding_in(&tx, &existing.id)?
                .map(|binding| binding.original)
                .unwrap_or_else(|| existing.clone());
            if !original.same_content(ticket) {
                return Err(StorageError::AlreadyExists(format!(
                    "idempotency key {idempotency_key} was already used for different ticket content"
                )));
            }
            return Ok((existing, false));
        }
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM tickets WHERE project_id=?1",
            [&ticket.project_id],
            |row| row.get(0),
        )?;
        if count >= 1000 {
            return Err(StorageError::Invalid(
                "ticket limit reached for this project".into(),
            ));
        }
        if load_ticket(&tx, &ticket.id)?.is_some() {
            return Err(StorageError::AlreadyExists(ticket.id.clone()));
        }
        tx.execute(
            "INSERT INTO tickets(id,project_id,revision,idempotency_key,payload,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                ticket.id,
                ticket.project_id,
                ticket.revision,
                idempotency_key,
                serde_json::to_string(ticket)?,
                ticket.created_at,
                ticket.updated_at
            ],
        )?;
        append_project_event(
            &tx,
            &ticket.project_id,
            "ticket.created",
            json!({"ticket_id": ticket.id, "revision": ticket.revision, "source": ticket.source, "title": ticket.title, "repository_ids": ticket.repository_ids}),
        )?;
        tx.commit()?;
        self.mirror_logs(&[&ticket.project_id]);
        self.mirror_tickets(&[&ticket.project_id]);
        Ok((ticket.clone(), true))
    }

    /// Revise the local ticket's intent. Plans bound to the previous revision are
    /// stale from this point and scheduling refuses them until re-planned.
    pub fn update_ticket(
        &mut self,
        ticket_id: &str,
        expected_revision: u64,
        title: &str,
        intent: &str,
        acceptance_criteria: &[String],
    ) -> Result<Ticket> {
        self.revise_ticket(
            ticket_id,
            expected_revision,
            &TicketRevision {
                title: Some(title.to_owned()),
                intent: Some(intent.to_owned()),
                acceptance_criteria: Some(acceptance_criteria.to_vec()),
                ..TicketRevision::default()
            },
        )
    }

    /// Revise the fields of a legacy local ticket that the request names
    /// (`None` keeps the current value) and bump its revision, recording
    /// `ticket.updated`. A Missions-backed ticket is refused here: its intent is
    /// edited through the Missions adapter and reconciled into the projection.
    /// Plans bound to the previous revision are stale from this point.
    pub fn revise_ticket(
        &mut self,
        ticket_id: &str,
        expected_revision: u64,
        changes: &TicketRevision,
    ) -> Result<Ticket> {
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut ticket =
            load_ticket(&tx, ticket_id)?.ok_or_else(|| StorageError::NotFound(ticket_id.into()))?;
        if ticket.source == luuna_domain::TicketSource::Missions {
            return Err(StorageError::Invalid(
                "Missions owns ticket intent; reconcile the source instead of writing SQLite"
                    .into(),
            ));
        }
        if ticket.revision != expected_revision {
            return Err(StorageError::RevisionConflict {
                expected: expected_revision,
                actual: ticket.revision,
            });
        }
        ticket.revision += 1;
        changes.apply(&mut ticket);
        ticket.updated_at = now();
        ticket.validate()?;
        let registered: Vec<String> = crate::projects::repositories_in(&tx, &ticket.project_id)?
            .into_iter()
            .map(|r| r.repository_id)
            .collect();
        if let Some(unknown) = ticket
            .repository_ids
            .iter()
            .find(|id| !registered.contains(id))
        {
            return Err(StorageError::Invalid(format!(
                "repository {unknown} is not registered in project {}",
                ticket.project_id
            )));
        }
        tx.execute(
            "UPDATE tickets SET revision=?1,payload=?2,updated_at=?3 WHERE id=?4 AND revision=?5",
            params![
                ticket.revision,
                serde_json::to_string(&ticket)?,
                ticket.updated_at,
                ticket.id,
                expected_revision
            ],
        )?;
        append_project_event(
            &tx,
            &ticket.project_id,
            "ticket.updated",
            json!({"ticket_id": ticket.id, "revision": ticket.revision, "previous_revision": expected_revision, "source": ticket.source, "fields": changes.named()}),
        )?;
        tx.commit()?;
        self.mirror_logs(&[&ticket.project_id]);
        self.mirror_tickets(&[&ticket.project_id]);
        Ok(ticket)
    }

    pub fn get_ticket(&self, id: &str) -> Result<Option<Ticket>> {
        load_ticket(&self.connection, id)
    }

    pub fn tickets(&self, project_id: &str) -> Result<Vec<Ticket>> {
        let mut statement = self.connection.prepare(
            "SELECT payload FROM tickets WHERE project_id=?1 ORDER BY created_at, rowid",
        )?;
        let rows = statement.query_map([project_id], |r| r.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    /// Persist a new plan revision and open the workflow run that carries it,
    /// already bound and `plan_ready`, in one transaction. The plan must name
    /// the ticket's current revision, the project's current manifest hash and
    /// that manifest's recorded base commits.
    pub fn open_plan_workflow(
        &mut self,
        run_id: &str,
        plan: &PlanRevision,
    ) -> Result<crate::RunRecord> {
        self.open_plan_workflow_once(run_id, plan, None)
            .map(|opened| opened.run)
    }

    /// The plan and workflow an earlier request with this ticket idempotency
    /// key created. The same key with a different request hash is a conflict.
    pub fn plan_request(
        &self,
        ticket_id: &str,
        idempotency_key: &str,
        request_hash: &str,
    ) -> Result<Option<PlanOpening>> {
        plan_request_in(&self.connection, ticket_id, idempotency_key, request_hash)
    }

    /// `open_plan_workflow`, optionally under a client idempotency key and the
    /// hash of the request it names: a key already recorded for the ticket
    /// returns the plan and workflow it created (`created` is false) when the
    /// request hash matches and is refused when it does not. Earlier revisions
    /// of the plan still waiting for approval or implementation can never be
    /// approved or scheduled again (both require the latest revision), so the
    /// same transaction settles them as superseded.
    pub fn open_plan_workflow_once(
        &mut self,
        run_id: &str,
        plan: &PlanRevision,
        request: Option<(&str, &str)>,
    ) -> Result<PlanOpening> {
        plan.validate()?;
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some((key, hash)) = request {
            if let Some(existing) = plan_request_in(&tx, &plan.ticket_id, key, hash)? {
                return Ok(existing);
            }
        }
        let ticket = load_ticket(&tx, &plan.ticket_id)?
            .ok_or_else(|| StorageError::NotFound(plan.ticket_id.clone()))?;
        if ticket.project_id != plan.project_id {
            return Err(StorageError::Invalid(
                "plan project does not match the ticket's project".into(),
            ));
        }
        if ticket.revision != plan.ticket_revision {
            return Err(StorageError::RevisionConflict {
                expected: ticket.revision,
                actual: plan.ticket_revision,
            });
        }
        if let Some(unknown) = plan
            .base_commits
            .keys()
            .find(|id| !ticket.repository_ids.contains(id))
        {
            return Err(StorageError::Invalid(format!(
                "repository {unknown} is outside the ticket's repositories"
            )));
        }
        // The ticket, rather than a client-chosen plan ID, owns live work.
        let active: Option<String> = tx.query_row(
            "SELECT r.id FROM runs r JOIN plan_revisions p ON p.id=r.plan_id AND p.revision=r.plan_revision
             WHERE json_extract(p.payload,'$.ticket_id')=?1 AND
             (r.state IN ('\"implementing\"','\"reviewing\"','\"running\"','\"queued\"','\"exploring\"')
              OR EXISTS(SELECT 1 FROM candidates c WHERE c.run_id=r.id AND c.status IN ('validating','reviewing'))
              OR EXISTS(SELECT 1 FROM attempts a WHERE a.run_id=r.id AND a.state='running')
              OR EXISTS(SELECT 1 FROM worktree_leases l WHERE l.run_id=r.id AND l.state='active')) LIMIT 1",
            [&plan.ticket_id], |row| row.get(0)).optional()?;
        if let Some(active) = active {
            return Err(StorageError::AlreadyExists(format!(
                "ticket {} has live workflow {active}; finish or cancel it before re-planning",
                plan.ticket_id
            )));
        }
        check_manifest_binding(&tx, plan)?;
        let current: u64 = tx.query_row(
            "SELECT COALESCE(MAX(revision),0) FROM plan_revisions WHERE id=?1",
            [&plan.id],
            |row| row.get(0),
        )?;
        if plan.revision != current + 1 {
            return Err(StorageError::RevisionConflict {
                expected: current + 1,
                actual: plan.revision,
            });
        }
        tx.execute(
            "INSERT INTO plan_revisions(id,revision,hash,payload) VALUES (?1,?2,?3,?4)",
            params![
                plan.id,
                plan.revision,
                plan.hash,
                serde_json::to_string(plan)?
            ],
        )?;
        let superseded = {
            let mut statement = tx.prepare(
                "SELECT id,revision,plan_revision FROM runs WHERE plan_id=?1 AND state IN (?2,?3) ORDER BY rowid",
            )?;
            let rows = statement.query_map(
                params![
                    plan.id,
                    crate::state_json(RunState::PlanReady)?,
                    crate::state_json(RunState::Approved)?
                ],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, u64>(1)?,
                        r.get::<_, u64>(2)?,
                    ))
                },
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        for (id, revision, plan_revision) in superseded {
            transition(
                &tx,
                &id,
                revision,
                RunState::Cancelled,
                "plan.superseded",
                json!({"plan_revision": plan_revision, "superseded_by": plan.revision, "reason": format!("plan revision {plan_revision} was superseded by revision {} before implementation; approve the latest revision", plan.revision)}),
            )?;
        }
        let created_at = now();
        let metadata = json!({"kind": "workflow", "ticket_id": plan.ticket_id, "ticket_revision": plan.ticket_revision, "plan_id": plan.id, "plan_revision": plan.revision});
        tx.execute(
            "INSERT INTO runs(id,project_id,state,revision,metadata,created_at,updated_at,plan_id,plan_revision) VALUES (?1,?2,?3,1,?4,?5,?5,?6,?7)",
            params![
                run_id,
                plan.project_id,
                crate::state_json(RunState::Discovered)?,
                serde_json::to_string(&metadata)?,
                created_at,
                plan.id,
                plan.revision
            ],
        )?;
        append(
            &tx,
            run_id,
            "run.created",
            json!({"state": RunState::Discovered}),
        )?;
        transition(
            &tx,
            run_id,
            1,
            RunState::Exploring,
            "run.exploring",
            json!({"ticket_id": plan.ticket_id, "ticket_revision": plan.ticket_revision}),
        )?;
        transition(
            &tx,
            run_id,
            2,
            RunState::PlanReady,
            "plan.ready",
            json!({"plan_id": plan.id, "plan_revision": plan.revision, "plan_hash": plan.hash}),
        )?;
        append_project_event(
            &tx,
            &plan.project_id,
            "plan.recorded",
            json!({"run_id": run_id, "ticket_id": plan.ticket_id, "ticket_revision": plan.ticket_revision, "plan_id": plan.id, "plan_revision": plan.revision, "plan_hash": plan.hash}),
        )?;
        if let Some((key, hash)) = request {
            tx.execute(
                "INSERT INTO plan_requests(ticket_id,idempotency_key,request_hash,plan_id,plan_revision,run_id,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![plan.ticket_id, key, hash, plan.id, plan.revision, run_id, created_at],
            )?;
        }
        tx.commit()?;
        self.mirror_logs(&[&plan.project_id]);
        Ok(PlanOpening {
            plan: plan.clone(),
            run: self
                .get_run(run_id)?
                .ok_or_else(|| StorageError::NotFound(run_id.into()))?,
            created: true,
        })
    }

    pub fn latest_plan(&self, plan_id: &str) -> Result<Option<PlanRevision>> {
        let payload: Option<String> = self
            .connection
            .query_row(
                "SELECT payload FROM plan_revisions WHERE id=?1 ORDER BY revision DESC LIMIT 1",
                [plan_id],
                |r| r.get(0),
            )
            .optional()?;
        payload
            .map(|value| serde_json::from_str(&value).map_err(Into::into))
            .transpose()
    }

    pub fn plans_for_ticket(&self, ticket_id: &str) -> Result<Vec<PlanRevision>> {
        let mut statement = self.connection.prepare(
            "SELECT payload FROM plan_revisions WHERE json_extract(payload,'$.ticket_id')=?1 ORDER BY id, revision",
        )?;
        let rows = statement.query_map([ticket_id], |r| r.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    /// The workflow run bound to one plan revision (the latest recorded).
    pub fn run_for_plan(&self, plan_id: &str, revision: u64) -> Result<Option<crate::RunRecord>> {
        let id: Option<String> = self
            .connection
            .query_row(
                "SELECT id FROM runs WHERE plan_id=?1 AND plan_revision=?2 ORDER BY rowid DESC LIMIT 1",
                params![plan_id, revision],
                |r| r.get(0),
            )
            .optional()?;
        id.map(|id| self.get_run(&id))
            .transpose()
            .map(Option::flatten)
    }

    /// Record Luuna's ownership of a worktree. A second active lease on the same
    /// repository is refused by the database, whatever the caller believed.
    pub fn acquire_worktree_lease(&mut self, lease: &WorktreeLease) -> Result<()> {
        lease.validate()?;
        if lease.state != LeaseState::Active {
            return Err(StorageError::Invalid("a new lease must be active".into()));
        }
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let run = crate::get_run(&tx, &lease.run_id)?
            .ok_or_else(|| StorageError::NotFound(lease.run_id.clone()))?;
        if run.state != RunState::Implementing {
            return Err(StorageError::Invalid(format!(
                "run {} is {:?}, not implementing",
                run.id, run.state
            )));
        }
        let inserted = tx.execute(
            "INSERT INTO worktree_leases(id,run_id,project_id,repository_id,path,base_commit,state,payload,created_at) VALUES (?1,?2,?3,?4,?5,?6,'active',?7,?8)",
            params![
                lease.id,
                lease.run_id,
                lease.project_id,
                lease.repository_id,
                lease.path,
                lease.base_commit,
                serde_json::to_string(lease)?,
                lease.created_at
            ],
        );
        match inserted {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                return Err(StorageError::AlreadyExists(format!(
                    "repository {} already has an active worktree lease or this path is leased",
                    lease.repository_id
                )));
            }
            Err(error) => return Err(error.into()),
        }
        append(
            &tx,
            &lease.run_id,
            "worktree.leased",
            json!({"lease_id": lease.id, "repository_id": lease.repository_id, "path": lease.path, "base_commit": lease.base_commit}),
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn release_worktree_lease(
        &mut self,
        lease_id: &str,
        state: LeaseState,
        candidate_commit: Option<&str>,
        reason: Option<&str>,
    ) -> Result<WorktreeLease> {
        if state == LeaseState::Active {
            return Err(StorageError::Invalid(
                "release needs a settled state".into(),
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut lease =
            load_lease(&tx, lease_id)?.ok_or_else(|| StorageError::NotFound(lease_id.into()))?;
        if lease.state != LeaseState::Active {
            return Err(StorageError::Invalid("lease is already settled".into()));
        }
        lease.state = state;
        lease.candidate_commit = candidate_commit.map(ToOwned::to_owned);
        lease.reason = reason.map(ToOwned::to_owned);
        lease.released_at = Some(now());
        lease.validate()?;
        tx.execute(
            "UPDATE worktree_leases SET state=?1,payload=?2,released_at=?3 WHERE id=?4",
            params![
                text(state)?,
                serde_json::to_string(&lease)?,
                lease.released_at,
                lease_id
            ],
        )?;
        append(
            &tx,
            &lease.run_id,
            "worktree.released",
            json!({"lease_id": lease.id, "repository_id": lease.repository_id, "state": state, "candidate_commit": candidate_commit, "reason": reason}),
        )?;
        tx.commit()?;
        Ok(lease)
    }

    pub fn worktree_leases(&self, run_id: &str) -> Result<Vec<WorktreeLease>> {
        let mut statement = self.connection.prepare(
            "SELECT payload FROM worktree_leases WHERE run_id=?1 ORDER BY created_at, rowid",
        )?;
        let rows = statement.query_map([run_id], |r| r.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    pub fn active_worktree_leases(&self) -> Result<Vec<WorktreeLease>> {
        let mut statement = self.connection.prepare(
            "SELECT payload FROM worktree_leases WHERE state='active' ORDER BY created_at",
        )?;
        let rows = statement.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    /// Open the next attempt for a task; the plan's `max_attempts` is enforced here.
    pub fn start_attempt(
        &mut self,
        id: &str,
        run_id: &str,
        task_id: &str,
        max_attempts: u32,
        payload: Value,
    ) -> Result<AttemptRecord> {
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let previous: u32 = tx.query_row(
            "SELECT COUNT(*) FROM attempts WHERE run_id=?1",
            [run_id],
            |r| r.get(0),
        )?;
        let number = previous + 1;
        if number > max_attempts {
            return Err(StorageError::Domain(DomainError::BudgetExceeded(format!(
                "attempt {number} exceeds max_attempts {max_attempts}"
            ))));
        }
        // One running attempt per task: a plan's task graph (M4) runs several
        // tasks at once, each in its own repository worktree, but never the
        // same task twice.
        let running: u32 = tx.query_row(
            "SELECT COUNT(*) FROM attempts WHERE run_id=?1 AND state='running' AND task_id=?2",
            params![run_id, task_id],
            |r| r.get(0),
        )?;
        if running > 0 {
            return Err(StorageError::AlreadyExists(format!(
                "an attempt is already running for task {task_id} of this run"
            )));
        }
        let record = AttemptRecord {
            id: id.into(),
            run_id: run_id.into(),
            task_id: task_id.into(),
            number,
            state: AttemptState::Running,
            payload,
            started_at: now(),
            finished_at: None,
        };
        tx.execute(
            "INSERT INTO attempts(id,run_id,task_id,number,state,payload,started_at) VALUES (?1,?2,?3,?4,'running',?5,?6)",
            params![
                record.id,
                record.run_id,
                record.task_id,
                record.number,
                serde_json::to_string(&record.payload)?,
                record.started_at
            ],
        )?;
        append(
            &tx,
            run_id,
            "attempt.started",
            json!({"attempt_id": id, "task_id": task_id, "number": number, "max_attempts": max_attempts}),
        )?;
        tx.commit()?;
        Ok(record)
    }

    pub fn finish_attempt(
        &mut self,
        id: &str,
        state: AttemptState,
        payload: Value,
    ) -> Result<AttemptRecord> {
        if !state.is_terminal() {
            return Err(StorageError::Invalid(
                "a finished attempt needs a terminal state".into(),
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut record = load_attempt(&tx, id)?.ok_or_else(|| StorageError::NotFound(id.into()))?;
        if record.state != AttemptState::Running {
            return Err(StorageError::Invalid("attempt is already settled".into()));
        }
        record.state = state;
        record.payload = with_launch(&record.payload, payload);
        record.finished_at = Some(now());
        tx.execute(
            "UPDATE attempts SET state=?1,payload=?2,finished_at=?3 WHERE id=?4",
            params![
                text(state)?,
                serde_json::to_string(&record.payload)?,
                record.finished_at,
                id
            ],
        )?;
        append(
            &tx,
            &record.run_id,
            "attempt.finished",
            json!({"attempt_id": id, "task_id": record.task_id, "number": record.number, "state": state, "payload": record.payload}),
        )?;
        tx.commit()?;
        Ok(record)
    }

    pub fn attempts(&self, run_id: &str) -> Result<Vec<AttemptRecord>> {
        let mut statement = self
            .connection
            .prepare("SELECT id FROM attempts WHERE run_id=?1 ORDER BY number")?;
        let ids = statement
            .query_map([run_id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| load_attempt(&self.connection, &id)?.ok_or(StorageError::NotFound(id)))
            .collect()
    }
}

/// The plan must bind to the project's current confirmed manifest and that
/// manifest's recorded base commits for every repository in scope.
pub(crate) fn check_manifest_binding(tx: &Transaction<'_>, plan: &PlanRevision) -> Result<()> {
    let current: Option<String> = tx
        .query_row(
            "SELECT manifest_hash FROM projects WHERE id=?1",
            [&plan.project_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    if current.as_deref() != Some(plan.manifest_hash.as_str()) {
        return Err(StorageError::Domain(DomainError::BaselineRequired(
            "plan is bound to a manifest that is not the project's current confirmed manifest"
                .into(),
        )));
    }
    let recorded: Option<String> = tx
        .query_row(
            "SELECT base_commits FROM manifest_revisions WHERE project_id=?1 AND hash=?2 ORDER BY revision DESC LIMIT 1",
            params![plan.project_id, plan.manifest_hash],
            |r| r.get(0),
        )
        .optional()?;
    let recorded: BTreeMap<String, String> = recorded
        .map(|value| serde_json::from_str(&value))
        .transpose()?
        .unwrap_or_default();
    for (repository_id, commit) in &plan.base_commits {
        if recorded.get(repository_id) != Some(commit) {
            return Err(StorageError::Domain(DomainError::BaselineRequired(
                format!(
                "plan base commit for {repository_id} is not the base the current manifest recorded"
            ),
            )));
        }
    }
    Ok(())
}

fn plan_request_in(
    connection: &Connection,
    ticket_id: &str,
    idempotency_key: &str,
    request_hash: &str,
) -> Result<Option<PlanOpening>> {
    let row: Option<(String, String, u64, String)> = connection
        .query_row(
            "SELECT request_hash,plan_id,plan_revision,run_id FROM plan_requests WHERE ticket_id=?1 AND idempotency_key=?2",
            params![ticket_id, idempotency_key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((recorded, plan_id, plan_revision, run_id)) = row else {
        return Ok(None);
    };
    if recorded != request_hash {
        return Err(StorageError::AlreadyExists(format!(
            "idempotency key {idempotency_key} was already used for a different plan request"
        )));
    }
    let plan: String = connection.query_row(
        "SELECT payload FROM plan_revisions WHERE id=?1 AND revision=?2",
        params![plan_id, plan_revision],
        |r| r.get(0),
    )?;
    Ok(Some(PlanOpening {
        plan: serde_json::from_str(&plan)?,
        run: crate::get_run(connection, &run_id)?.ok_or(StorageError::NotFound(run_id))?,
        created: false,
    }))
}

/// One writer per repository, decided in the transaction that admits a writer
/// (implementation or repair) before any state changes: a repository with an
/// active lease, or with another workflow already admitted to implementation
/// whose worker has not leased its worktree yet, is refused.
pub(crate) fn require_repositories_free(
    tx: &Transaction<'_>,
    run_id: &str,
    plan: &PlanRevision,
) -> Result<()> {
    for repository_id in plan.base_commits.keys() {
        let other: Option<String> = tx
            .query_row(
                "SELECT run_id FROM worktree_leases WHERE project_id=?1 AND repository_id=?2 AND state='active' AND run_id!=?3
                 UNION ALL
                 SELECT r.id FROM runs r JOIN plan_revisions p ON p.id=r.plan_id AND p.revision=r.plan_revision
                 WHERE r.project_id=?1 AND r.id!=?3 AND r.state=?4
                 AND EXISTS(SELECT 1 FROM json_each(p.payload,'$.base_commits') WHERE key=?2)
                 LIMIT 1",
                params![
                    plan.project_id,
                    repository_id,
                    run_id,
                    crate::state_json(RunState::Implementing)?
                ],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(other) = other {
            return Err(StorageError::AlreadyExists(format!(
                "repository {repository_id} already has an active writer (workflow {other}); one writer per repository"
            )));
        }
    }
    Ok(())
}

/// The ticket a plan names must still be at the revision it was planned against.
pub(crate) fn check_ticket_binding(tx: &Transaction<'_>, plan: &PlanRevision) -> Result<()> {
    let ticket = load_ticket(tx, &plan.ticket_id)?
        .ok_or_else(|| StorageError::NotFound(plan.ticket_id.clone()))?;
    if ticket.project_id != plan.project_id || ticket.revision != plan.ticket_revision {
        return Err(StorageError::Domain(DomainError::BaselineRequired(
            format!(
            "ticket {} is at revision {}, plan was made for revision {}; re-plan before scheduling",
            ticket.id, ticket.revision, plan.ticket_revision
        ),
        )));
    }
    Ok(())
}

/// Startup recovery, after active runs were interrupted in the same
/// transaction. No writer runs before the runtime accepts work, so every lease
/// still active is orphaned whatever its run's state (including a run that
/// settled while its lease release failed, which would otherwise wedge the
/// repository): it is abandoned with its files preserved. Every running
/// attempt is interrupted. The worktree is not read here, so the reason says a
/// commit may exist in it unrecorded; such a commit is never a candidate.
pub(crate) fn abandon_orphaned_work(
    tx: &Transaction<'_>,
    skipped: &mut Vec<SkippedRecord>,
) -> Result<usize> {
    const LEASE_REASON: &str = "runtime restarted while the lease was active; files preserved, and a commit the writer made before the restart may exist in the worktree unrecorded (it is never a candidate); attempt evidence void";
    let mut statement = tx.prepare(
        "SELECT id,run_id,repository_id FROM worktree_leases WHERE state='active' ORDER BY rowid",
    )?;
    let leases = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(statement);
    let released_at = now();
    for (id, run_id, repository_id) in &leases {
        let mut lease = match load_lease(tx, id) {
            Ok(Some(lease)) => lease,
            Ok(None) => return Err(StorageError::NotFound(id.clone())),
            Err(StorageError::Json(error)) => {
                // The state column alone frees the repository's lease slot.
                let reason = format!("{LEASE_REASON}; {}", undecodable(&error));
                tx.execute(
                    "UPDATE worktree_leases SET state='abandoned',released_at=?1 WHERE id=?2",
                    params![released_at, id],
                )?;
                append(
                    tx,
                    run_id,
                    "worktree.released",
                    json!({"lease_id": id, "repository_id": repository_id, "state": LeaseState::Abandoned, "reason": reason}),
                )?;
                skipped.push(SkippedRecord {
                    table: "worktree_leases",
                    id: id.clone(),
                    reason,
                });
                continue;
            }
            Err(error) => return Err(error),
        };
        lease.state = LeaseState::Abandoned;
        lease.reason = Some(LEASE_REASON.into());
        lease.released_at = Some(released_at.clone());
        tx.execute(
            "UPDATE worktree_leases SET state='abandoned',payload=?1,released_at=?2 WHERE id=?3",
            params![serde_json::to_string(&lease)?, released_at, id],
        )?;
        append(
            tx,
            &lease.run_id,
            "worktree.released",
            json!({"lease_id": lease.id, "repository_id": lease.repository_id, "state": LeaseState::Abandoned, "reason": lease.reason}),
        )?;
    }
    let mut statement = tx.prepare(
        "SELECT id,run_id,task_id,number,payload FROM attempts WHERE state='running' ORDER BY rowid",
    )?;
    let attempts = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, u32>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(statement);
    for (id, run_id, task_id, number, payload) in &attempts {
        let previous = match serde_json::from_str::<Value>(payload) {
            Ok(previous) => Some(previous),
            Err(error) => {
                skipped.push(SkippedRecord {
                    table: "attempts",
                    id: id.clone(),
                    reason: format!(
                        "{}; interrupted and charged as if it reported no usage",
                        undecodable(&error)
                    ),
                });
                None
            }
        };
        let usage = reserved_usage(tx, run_id, previous.as_ref());
        let settled = json!({"reason": "runtime restarted before the writer settled; unreported time and provider spend reserve the remaining approved budgets; re-plan required", "usage": usage});
        // Keep the provider/model provenance the attempt started with.
        let payload = match previous.as_ref() {
            Some(previous) => with_launch(previous, settled),
            None => settled,
        };
        tx.execute(
            "UPDATE attempts SET state='interrupted',payload=?1,finished_at=?2 WHERE id=?3",
            params![serde_json::to_string(&payload)?, released_at, id],
        )?;
        append(
            tx,
            run_id,
            "attempt.finished",
            json!({"attempt_id": id, "task_id": task_id, "number": number, "state": AttemptState::Interrupted, "payload": payload}),
        )?;
    }
    // Tasks of a plan graph whose writer was running are interrupted too, so
    // the graph remains visible after the restart and resumable by task ID;
    // completed tasks keep their commits, worktrees and evidence.
    let mut statement =
        tx.prepare("SELECT run_id,task_id FROM run_tasks WHERE state='running' ORDER BY rowid")?;
    let tasks = statement
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(statement);
    for (run_id, task_id) in &tasks {
        settle_task_in(
            tx,
            run_id,
            task_id,
            TaskState::Interrupted,
            Some("runtime restarted while the task's writer was running; resume with POST /api/v1/runs/{id}/implement naming task_id"),
            None,
            None,
        )?;
    }
    Ok(leases.len() + attempts.len() + tasks.len())
}

impl Store {
    /// Every task of the run's plan graph with its durable state, in plan order.
    pub fn run_tasks(&self, run_id: &str) -> Result<Vec<TaskRecord>> {
        run_tasks_in(&self.connection, run_id)
    }

    /// Enter `Implementing` for a plan's task graph (M4), or resume one. The
    /// gate is `begin_implementation`'s (approval, ticket, manifest and base
    /// binding, a passed baseline per repository, free repositories) with one
    /// environment fingerprint per repository, since each repository's
    /// required recipes fingerprint its own tools. A fresh start requires an
    /// `approved` workflow and records every task `queued`; a resume names
    /// a task that is `interrupted`, `failed` or `blocked` on an
    /// `interrupted` or `failed` workflow, requeues every task that did not
    /// complete, and keeps completed tasks' commits. Attempts still count
    /// against the plan budget, which `start_attempt` enforces.
    pub fn begin_graph_implementation(
        &mut self,
        run_id: &str,
        expected_revision: u64,
        environment_fingerprints: &BTreeMap<String, String>,
        resume_task: Option<&str>,
    ) -> Result<(crate::RunRecord, Vec<TaskRecord>)> {
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let run =
            crate::get_run(&tx, run_id)?.ok_or_else(|| StorageError::NotFound(run_id.into()))?;
        let existing = run_tasks_in(&tx, run_id)?;
        match (run.state, resume_task) {
            (RunState::Approved, _) if existing.is_empty() => {}
            (RunState::Interrupted | RunState::Failed, Some(task_id)) => {
                let task = existing
                    .iter()
                    .find(|task| task.task_id == task_id)
                    .ok_or_else(|| {
                        StorageError::Invalid(format!(
                            "task {task_id} has no recorded state in this workflow; only a workflow whose task graph was interrupted or failed can be resumed"
                        ))
                    })?;
                if !matches!(
                    task.state,
                    TaskState::Interrupted | TaskState::Failed | TaskState::Blocked
                ) {
                    return Err(StorageError::Invalid(format!(
                        "task {task_id} is {:?}; only an interrupted, failed or blocked task can be resumed",
                        task.state
                    )));
                }
                if existing.iter().any(|task| task.state == TaskState::Running) {
                    return Err(StorageError::AlreadyExists(
                        "a task of this workflow is still recorded running".into(),
                    ));
                }
            }
            (state, _) => {
                return Err(StorageError::Invalid(format!(
                    "workflow is {state:?}; implementation requires an approved workflow, and resumption an interrupted or failed one with the task to resume named"
                )));
            }
        }
        let approval = crate::authorized_approval(&tx, run_id)?;
        let plan = crate::validation::bound_plan(&tx, run_id)?;
        check_ticket_binding(&tx, &plan)?;
        check_manifest_binding(&tx, &plan)?;
        let mut baselines = BTreeMap::new();
        for repository_id in plan.base_commits.keys() {
            let fingerprint = environment_fingerprints
                .get(repository_id)
                .filter(|f| !f.is_empty())
                .ok_or_else(|| {
                    StorageError::Invalid(format!(
                        "environment fingerprint for repository {repository_id} is required"
                    ))
                })?;
            let evidence = crate::baselines::latest_baseline(&tx, &plan.project_id, repository_id)?
                .ok_or_else(|| {
                    StorageError::Domain(DomainError::BaselineRequired(format!(
                        "no baseline has been recorded for repository {repository_id}"
                    )))
                })?;
            evidence.authorizes(&plan, repository_id, fingerprint)?;
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
        require_repositories_free(&tx, run_id, &plan)?;
        let updated_at = now();
        let mut requeued = Vec::new();
        if existing.is_empty() {
            // The assignment trace (R06): every task of the graph is assigned
            // by the developer's approval of this exact plan revision; the
            // runtime never adds a task of its own.
            let assigned_by = json!({
                "approved_by": approval.approved_by,
                "approval_id": approval.id,
                "approved_at": approval.approved_at,
                "plan_id": plan.id,
                "plan_revision": plan.revision,
                "plan_hash": plan.hash,
            });
            for task in &plan.tasks {
                let overrides: Vec<&DependencyOverride> = plan
                    .dependency_overrides
                    .iter()
                    .filter(|o| o.consumer == task.id || o.producer == task.id)
                    .collect();
                tx.execute(
                    "INSERT INTO run_tasks(run_id,task_id,repository_id,state,depends_on,updated_at,role,assigned_by,derived_from_task,dependency_overrides) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![
                        run_id,
                        task.id,
                        task.repository_id,
                        text(TaskState::Queued)?,
                        serde_json::to_string(&task.depends_on)?,
                        updated_at,
                        task.role,
                        assigned_by.to_string(),
                        task.derived_from_task,
                        serde_json::to_string(&overrides)?,
                    ],
                )?;
                let record = load_task(&tx, run_id, &task.id)?
                    .ok_or_else(|| StorageError::NotFound(format!("{run_id}/{}", task.id)))?;
                append(
                    &tx,
                    run_id,
                    "task.assigned",
                    with_trace(
                        json!({"task_id": task.id, "repository_id": task.repository_id, "depends_on": task.depends_on, "plan_hash": plan.hash}),
                        &record,
                    ),
                )?;
            }
        } else {
            for task in &existing {
                if task.state != TaskState::Completed {
                    tx.execute(
                        "UPDATE run_tasks SET state=?1,attempt_id=NULL,candidate_commit=NULL,candidate_tree=NULL,reason=NULL,advisory_result=NULL,updated_at=?2 WHERE run_id=?3 AND task_id=?4",
                        params![text(TaskState::Queued)?, updated_at, run_id, task.task_id],
                    )?;
                    requeued.push(task.task_id.clone());
                }
            }
        }
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
                "environment_fingerprints": environment_fingerprints,
                "tasks": plan.tasks.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
                "max_workers": plan.budgets.max_workers,
                "resumed_task": resume_task,
                "requeued": requeued,
            }),
        )?;
        let tasks = run_tasks_in(&tx, run_id)?;
        tx.commit()?;
        Ok((
            self.get_run(run_id)?
                .ok_or_else(|| StorageError::NotFound(run_id.into()))?,
            tasks,
        ))
    }

    /// A queued task starts running under the given attempt (`task.scheduled`).
    pub fn schedule_task(
        &mut self,
        run_id: &str,
        task_id: &str,
        attempt_id: &str,
    ) -> Result<TaskRecord> {
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let task = load_task(&tx, run_id, task_id)?
            .ok_or_else(|| StorageError::NotFound(format!("{run_id}/{task_id}")))?;
        if task.state != TaskState::Queued {
            return Err(StorageError::Invalid(format!(
                "task {task_id} is {:?}, not queued",
                task.state
            )));
        }
        tx.execute(
            "UPDATE run_tasks SET state=?1,attempt_id=?2,updated_at=?3 WHERE run_id=?4 AND task_id=?5",
            params![text(TaskState::Running)?, attempt_id, now(), run_id, task_id],
        )?;
        append(
            &tx,
            run_id,
            "task.scheduled",
            with_trace(
                json!({"task_id": task_id, "repository_id": task.repository_id, "attempt_id": attempt_id, "depends_on": task.depends_on}),
                &task,
            ),
        )?;
        let task = load_task(&tx, run_id, task_id)?
            .ok_or_else(|| StorageError::NotFound(format!("{run_id}/{task_id}")))?;
        tx.commit()?;
        Ok(task)
    }

    /// Settle a task: `completed` with its candidate commit and tree, or
    /// `failed`, `interrupted`, `blocked` or `cancelled` with the reason. A
    /// running task settles any way; a queued task may only be blocked,
    /// cancelled or interrupted (it never ran). The event is `task.<state>`.
    pub fn settle_task(
        &mut self,
        run_id: &str,
        task_id: &str,
        state: TaskState,
        reason: Option<&str>,
        candidate: Option<(&str, &str)>,
    ) -> Result<TaskRecord> {
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let task = settle_task_in(&tx, run_id, task_id, state, reason, candidate, None)?;
        tx.commit()?;
        Ok(task)
    }

    /// Complete an advisory task (ADR 0023): its worktree is unchanged, no
    /// commit exists, and its validated structured result is the record.
    pub fn settle_advisory_task(
        &mut self,
        run_id: &str,
        task_id: &str,
        result: &AdvisoryResult,
    ) -> Result<TaskRecord> {
        result.validate()?;
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let task = settle_task_in(
            &tx,
            run_id,
            task_id,
            TaskState::Completed,
            None,
            None,
            Some(&serde_json::to_value(result)?),
        )?;
        tx.commit()?;
        Ok(task)
    }
}

fn settle_task_in(
    tx: &Transaction<'_>,
    run_id: &str,
    task_id: &str,
    state: TaskState,
    reason: Option<&str>,
    candidate: Option<(&str, &str)>,
    advisory: Option<&Value>,
) -> Result<TaskRecord> {
    if !state.is_terminal() {
        return Err(StorageError::Invalid(
            "a settled task needs a terminal state".into(),
        ));
    }
    let task = load_task(tx, run_id, task_id)?
        .ok_or_else(|| StorageError::NotFound(format!("{run_id}/{task_id}")))?;
    let admissible = match task.state {
        TaskState::Running => true,
        TaskState::Queued => matches!(
            state,
            TaskState::Blocked | TaskState::Cancelled | TaskState::Interrupted
        ),
        _ => false,
    };
    if !admissible {
        return Err(StorageError::Invalid(format!(
            "task {task_id} is {:?} and cannot settle as {state:?}",
            task.state
        )));
    }
    let advisory_role = TaskRole::parse(&task.role).is_some_and(TaskRole::is_advisory);
    if state == TaskState::Completed {
        // An implementer completes with its exact commit; an advisory role
        // completes with its structured result and never with a commit.
        match (advisory_role, candidate, advisory) {
            (false, Some((commit, tree)), None) => {
                if !luuna_domain::is_commit_id(commit) || !luuna_domain::is_commit_id(tree) {
                    return Err(StorageError::Invalid(
                        "a completed task needs exact commit and tree identities".into(),
                    ));
                }
            }
            (false, _, _) => {
                return Err(StorageError::Invalid(
                    "a completed task records its candidate commit and tree".into(),
                ))
            }
            (true, None, Some(_)) => {}
            (true, _, _) => {
                return Err(StorageError::Invalid(format!(
                    "task {task_id} has advisory role {} and completes with an advisory result, never a commit",
                    task.role
                )))
            }
        }
    }
    let (commit, tree) = candidate.unzip();
    let advisory_text = advisory.map(Value::to_string);
    tx.execute(
        "UPDATE run_tasks SET state=?1,candidate_commit=?2,candidate_tree=?3,reason=?4,updated_at=?5,advisory_result=?6 WHERE run_id=?7 AND task_id=?8",
        params![text(state)?, commit, tree, reason, now(), advisory_text, run_id, task_id],
    )?;
    let kind = format!("task.{}", text(state)?);
    append(
        tx,
        run_id,
        &kind,
        with_trace(
            json!({"task_id": task_id, "repository_id": task.repository_id, "attempt_id": task.attempt_id, "state": state, "reason": reason, "candidate_commit": commit, "candidate_tree": tree, "depends_on": task.depends_on, "advisory_result": advisory}),
            &task,
        ),
    )?;
    load_task(tx, run_id, task_id)?
        .ok_or_else(|| StorageError::NotFound(format!("{run_id}/{task_id}")))
}

fn run_tasks_in(connection: &Connection, run_id: &str) -> Result<Vec<TaskRecord>> {
    let mut statement =
        connection.prepare("SELECT task_id FROM run_tasks WHERE run_id=?1 ORDER BY rowid")?;
    let ids = statement
        .query_map([run_id], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    ids.into_iter()
        .map(|task_id| {
            load_task(connection, run_id, &task_id)?
                .ok_or_else(|| StorageError::NotFound(format!("{run_id}/{task_id}")))
        })
        .collect()
}

struct TaskRow {
    repository_id: String,
    state: String,
    attempt_id: Option<String>,
    candidate_commit: Option<String>,
    candidate_tree: Option<String>,
    depends_on: String,
    reason: Option<String>,
    updated_at: String,
    role: String,
    assigned_by: Option<String>,
    derived_from_task: Option<String>,
    dependency_overrides: Option<String>,
    advisory_result: Option<String>,
}

fn load_task(connection: &Connection, run_id: &str, task_id: &str) -> Result<Option<TaskRecord>> {
    let row: Option<TaskRow> = connection
        .query_row(
            "SELECT repository_id,state,attempt_id,candidate_commit,candidate_tree,depends_on,reason,updated_at,role,assigned_by,derived_from_task,dependency_overrides,advisory_result FROM run_tasks WHERE run_id=?1 AND task_id=?2",
            params![run_id, task_id],
            |r| {
                Ok(TaskRow {
                    repository_id: r.get(0)?,
                    state: r.get(1)?,
                    attempt_id: r.get(2)?,
                    candidate_commit: r.get(3)?,
                    candidate_tree: r.get(4)?,
                    depends_on: r.get(5)?,
                    reason: r.get(6)?,
                    updated_at: r.get(7)?,
                    role: r.get(8)?,
                    assigned_by: r.get(9)?,
                    derived_from_task: r.get(10)?,
                    dependency_overrides: r.get(11)?,
                    advisory_result: r.get(12)?,
                })
            },
        )
        .optional()?;
    row.map(|row| {
        Ok(TaskRecord {
            run_id: run_id.into(),
            task_id: task_id.into(),
            repository_id: row.repository_id,
            role: row.role,
            state: serde_json::from_value(Value::String(row.state))?,
            attempt_id: row.attempt_id,
            candidate_commit: row.candidate_commit,
            candidate_tree: row.candidate_tree,
            depends_on: serde_json::from_str(&row.depends_on)?,
            reason: row.reason,
            assigned_by: row
                .assigned_by
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?,
            derived_from_task: row.derived_from_task,
            dependency_overrides: row
                .dependency_overrides
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?
                .unwrap_or_default(),
            advisory_result: row
                .advisory_result
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?,
            updated_at: row.updated_at,
        })
    })
    .transpose()
}

/// An attempt lost to a restart reported neither its final time nor its
/// provider spend: they reserve the remaining approved budgets. Usage that
/// cannot be read reserves the full budgets, never less.
fn reserved_usage(tx: &Transaction<'_>, run_id: &str, previous: Option<&Value>) -> Value {
    let Ok(plan) = crate::validation::bound_plan(tx, run_id) else {
        return json!({});
    };
    let previous = previous.map(|value| value.get("usage").unwrap_or(value));
    let reported = |field: &str| {
        previous
            .and_then(|value| value.get(field))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let (elapsed, cost) = crate::validation::workflow_usage(tx, run_id)
        .map(|used| (used.elapsed_seconds, used.cost_usd_micros))
        .unwrap_or((0, 0));
    json!({"elapsed_seconds": reported("elapsed_seconds").saturating_add(plan.budgets.timeout_seconds.saturating_sub(elapsed)),
        "cost_usd_micros": reported("cost_usd_micros").saturating_add(plan.budgets.cost_usd_micros.saturating_sub(cost))})
}

/// A settled attempt's payload keeps the provenance recorded when it started
/// (provider, model, host fingerprint, Pi version, trigger); the settlement's
/// own fields (reason, usage, commit) are added and win on a name clash.
fn with_launch(started: &Value, settled: Value) -> Value {
    match (started, settled) {
        (Value::Object(started), Value::Object(settled)) => {
            let mut merged = started.clone();
            merged.extend(settled);
            Value::Object(merged)
        }
        (_, settled) => settled,
    }
}

fn load_ticket(connection: &Connection, id: &str) -> Result<Option<Ticket>> {
    let payload: Option<String> = connection
        .query_row("SELECT payload FROM tickets WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .optional()?;
    payload
        .map(|value| serde_json::from_str(&value).map_err(Into::into))
        .transpose()
}

fn ticket_by_key(connection: &Connection, project_id: &str, key: &str) -> Result<Option<Ticket>> {
    let payload: Option<String> = connection
        .query_row(
            "SELECT payload FROM tickets WHERE project_id=?1 AND idempotency_key=?2",
            params![project_id, key],
            |r| r.get(0),
        )
        .optional()?;
    payload
        .map(|value| serde_json::from_str(&value).map_err(Into::into))
        .transpose()
}

fn load_lease(connection: &Connection, id: &str) -> Result<Option<WorktreeLease>> {
    let payload: Option<String> = connection
        .query_row(
            "SELECT payload FROM worktree_leases WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    payload
        .map(|value| serde_json::from_str(&value).map_err(Into::into))
        .transpose()
}

fn load_attempt(connection: &Connection, id: &str) -> Result<Option<AttemptRecord>> {
    let row = connection
        .query_row(
            "SELECT run_id,task_id,number,state,payload,started_at,finished_at FROM attempts WHERE id=?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, u32>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<String>>(6)?,
                ))
            },
        )
        .optional()?;
    row.map(
        |(run_id, task_id, number, state, payload, started_at, finished_at)| {
            Ok(AttemptRecord {
                id: id.into(),
                run_id,
                task_id,
                number,
                state: serde_json::from_value(Value::String(state))?,
                payload: serde_json::from_str(&payload)?,
                started_at,
                finished_at,
            })
        },
    )
    .transpose()
}
