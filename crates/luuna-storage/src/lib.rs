//! SQLite authority for local state and its durable event stream.
use chrono::{SecondsFormat, Utc};
use luuna_domain::{Approval, DomainError, EventEnvelope, PlanRevision, RunState, SCHEMA_VERSION};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, fs, path::Path, time::Duration};
use thiserror::Error;

mod backup;
mod baselines;
mod components;
mod history;
mod missions;
mod project_files;
mod projects;
mod publication;
mod validation;
mod writer;
pub use backup::{
    backup_database, inspect_database, restore_database, BackupReport, DatabaseStatus,
    RestoreReport,
};
pub use components::*;
pub use missions::MissionsBinding;
pub use project_files::*;
pub use projects::*;
pub use publication::*;
pub use validation::{
    TicketUsage, WorkflowUsageView, COST_UNREPORTED_EVENT, MULTI_REPOSITORY_REPAIR,
    STAGE_HEARTBEAT_SECONDS,
};
pub use writer::{AttemptRecord, PlanOpening, TaskRecord, TicketRevision};

#[derive(Error)]
pub enum StorageError {
    #[error("SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("filesystem: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error("record not found: {0}")]
    NotFound(String),
    #[error("revision conflict: expected {expected}, current {actual}")]
    RevisionConflict { expected: u64, actual: u64 },
    #[error("plan revision already exists and is immutable")]
    ImmutableRevision,
    #[error("record already exists: {0}")]
    AlreadyExists(String),
    #[error("invalid input: {0}")]
    Invalid(String),
    /// Written by a newer Luuna: this build cannot know its invariants.
    #[error("database {path} has schema version {found}, newer than version {supported} supported by this Luuna build; run the Luuna build that wrote it, or stop the runtime and restore a backup this build can read with `luuna-runtime restore --input <backup>` (see crates/luuna-runtime/README.md)")]
    SchemaTooNew {
        path: String,
        found: u32,
        supported: u32,
    },
    /// The file failed SQLite's structural check; nothing is served or written.
    #[error("database {path} failed its integrity check ({detail}); stop the runtime and restore a backup with `luuna-runtime restore --input <backup>`; automatic pre-migration backups are kept in the data directory's backups/ folder (see crates/luuna-runtime/README.md)")]
    Corrupt { path: String, detail: String },
}

/// The derived form, except that a refused database prints its actionable
/// message: the service's `main` reports a startup error with `Debug`.
impl std::fmt::Debug for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SchemaTooNew { .. } | Self::Corrupt { .. } => std::fmt::Display::fmt(self, f),
            Self::Sqlite(error) => f.debug_tuple("Sqlite").field(error).finish(),
            Self::Io(error) => f.debug_tuple("Io").field(error).finish(),
            Self::Json(error) => f.debug_tuple("Json").field(error).finish(),
            Self::Domain(error) => f.debug_tuple("Domain").field(error).finish(),
            Self::NotFound(id) => f.debug_tuple("NotFound").field(id).finish(),
            Self::RevisionConflict { expected, actual } => f
                .debug_struct("RevisionConflict")
                .field("expected", expected)
                .field("actual", actual)
                .finish(),
            Self::ImmutableRevision => f.write_str("ImmutableRevision"),
            Self::AlreadyExists(id) => f.debug_tuple("AlreadyExists").field(id).finish(),
            Self::Invalid(reason) => f.debug_tuple("Invalid").field(reason).finish(),
        }
    }
}

pub type Result<T> = std::result::Result<T, StorageError>;

/// A record that startup recovery could not decode. It is never reported
/// successful: where its table has an authoritative state column, that column
/// is settled and a durable event records the reason; otherwise the record is
/// left unchanged for the developer and only logged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkippedRecord {
    pub table: &'static str,
    pub id: String,
    pub reason: String,
}

/// Outcome of startup recovery.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RecoveryReport {
    /// Runs moved to `interrupted` by this recovery.
    pub interrupted_runs: usize,
    pub skipped: Vec<SkippedRecord>,
}

/// A decode failure described by category and position only: payloads can
/// carry prompts and other private text, which must not reach logs or events.
pub(crate) fn undecodable(error: &serde_json::Error) -> String {
    format!(
        "stored record could not be decoded ({:?} error at line {} column {})",
        error.classify(),
        error.line(),
        error.column()
    )
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    pub project_id: String,
    pub state: RunState,
    pub revision: u64,
    pub metadata: Value,
    pub created_at: String,
    pub updated_at: String,
}

pub struct Store {
    connection: Connection,
}

impl Store {
    /// Open the database, creating it if needed. A file that fails SQLite's
    /// quick check or was written by a newer schema is refused before this
    /// build writes anything to it, not even a checkpoint. Only migrations the
    /// file lacks are applied, after an automatic backup of an existing
    /// database under `<directory>/backups/`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let memory = path == Path::new(":memory:");
        if !memory {
            backup::recover_pending_restore(path)?;
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                let mut builder = fs::DirBuilder::new();
                builder.recursive(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                builder.create(parent)?;
            }
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(path) {
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e.into()),
            }
            backup::inspect_read_only(path)?.require_usable()?;
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;",
        )?;
        migrate(&connection, (!memory).then_some(path))?;
        let store = Self { connection };
        store.initialize_component_state()?;
        Ok(store)
    }

    /// A run is one owned Pi attempt in M1. Metadata is descriptive, never authority.
    pub fn create_run(&mut self, id: &str, project_id: &str, metadata: Value) -> Result<RunRecord> {
        self.create_run_with_state(id, project_id, metadata, RunState::Queued)
    }

    pub fn create_workflow_run(
        &mut self,
        id: &str,
        project_id: &str,
        metadata: Value,
    ) -> Result<RunRecord> {
        self.create_run_with_state(id, project_id, metadata, RunState::Discovered)
    }

    fn create_run_with_state(
        &mut self,
        id: &str,
        project_id: &str,
        metadata: Value,
        state: RunState,
    ) -> Result<RunRecord> {
        if id.trim().is_empty() || project_id.trim().is_empty() || !metadata.is_object() {
            return Err(StorageError::Invalid(
                "run IDs and object metadata are required".into(),
            ));
        }
        let now = now();
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("INSERT INTO runs(id,project_id,state,revision,metadata,created_at,updated_at) VALUES (?1,?2,?3,1,?4,?5,?5)",
            params![id, project_id, state_json(state)?, serde_json::to_string(&metadata)?, now])?;
        append(&tx, id, "run.created", serde_json::json!({"state":state}))?;
        tx.commit()?;
        self.get_run(id)?
            .ok_or_else(|| StorageError::NotFound(id.into()))
    }

    pub fn get_run(&self, id: &str) -> Result<Option<RunRecord>> {
        get_run(&self.connection, id)
    }

    /// Startup-only process evidence, independent of damaged run metadata or
    /// unrelated malformed events. Never infer ownership from an undecodable row.
    pub fn recovery_check_events(&self) -> Result<Vec<(String, EventEnvelope)>> {
        let mut query = self.connection.prepare(
            "SELECT e.run_id,e.sequence,e.envelope FROM events e JOIN runs r ON r.id=e.run_id
             WHERE r.state IN ('\"queued\"','\"running\"','\"exploring\"','\"implementing\"','\"validating\"','\"reviewing\"') ORDER BY e.run_id,e.sequence")?;
        let rows = query.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut result = Vec::new();
        for row in rows {
            let (run_id, sequence, encoded) = row?;
            match serde_json::from_str::<EventEnvelope>(&encoded) {
                Ok(event)
                    if event.event_type == "check.spawned"
                        && event.run_id == run_id
                        && event.sequence == sequence =>
                {
                    result.push((run_id, event))
                }
                Ok(_) => {}
                Err(error) => eprintln!(
                    "luuna-storage: startup recovery skipped event {run_id}/{sequence}: {}",
                    undecodable(&error)
                ),
            }
        }
        Ok(result)
    }

    pub fn runs(&self) -> Result<Vec<RunRecord>> {
        let mut query = self.connection.prepare(
            "SELECT id,project_id,state,revision,metadata,created_at,updated_at FROM runs ORDER BY created_at DESC,id",
        )?;
        let rows = query.query_map([], run_row)?;
        rows.map(|row| decode_run(row?)).collect()
    }

    /// State and event commit together; no worker event can invoke this implicitly.
    pub fn transition_run(
        &mut self,
        run_id: &str,
        expected_revision: u64,
        next: RunState,
        event_type: &str,
        payload: Value,
    ) -> Result<RunRecord> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if next == RunState::Approved || next == RunState::PlanReady {
            return Err(StorageError::Invalid(
                "use bind_plan / approve_plan for plan transitions".into(),
            ));
        }
        if next == RunState::Implementing {
            authorize_run(&tx, run_id)?;
            return Err(StorageError::Invalid("baseline evidence required: enter implementation through begin_implementation, which checks the approval, current manifest and passed baselines transactionally".into()));
        }
        if matches!(
            next,
            RunState::Validating | RunState::Reviewing | RunState::Reviewed
        ) {
            return Err(StorageError::Invalid(
                "candidate transitions require candidate-bound validation/review gates".into(),
            ));
        }
        transition(&tx, run_id, expected_revision, next, event_type, payload)?;
        tx.commit()?;
        self.get_run(run_id)?
            .ok_or_else(|| StorageError::NotFound(run_id.into()))
    }

    pub fn append_event(
        &mut self,
        run_id: &str,
        event_type: &str,
        payload: Value,
    ) -> Result<EventEnvelope> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let event = append(&tx, run_id, event_type, payload)?;
        tx.commit()?;
        Ok(event)
    }

    pub fn events_after(&self, run_id: &str, cursor: u64) -> Result<Vec<EventEnvelope>> {
        if self.get_run(run_id)?.is_none() {
            return Err(StorageError::NotFound(run_id.into()));
        }
        let mut statement = self.connection.prepare(
            "SELECT envelope FROM events WHERE run_id=?1 AND sequence>?2 ORDER BY sequence",
        )?;
        let rows = statement.query_map(params![run_id, cursor], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    /// Call at service startup, before accepting work. Both queued and active
    /// attempts are interrupted; they are never silently reported successful.
    /// Records recovery cannot decode are logged to stderr by table and ID
    /// (never content) and skipped; see [`Store::recover_after_restart`].
    pub fn interrupt_active_runs(&mut self) -> Result<usize> {
        let report = self.recover_after_restart()?;
        for skipped in &report.skipped {
            eprintln!(
                "luuna-storage: startup recovery skipped {} {}: {}",
                skipped.table, skipped.id, skipped.reason
            );
        }
        Ok(report.interrupted_runs)
    }

    /// Startup recovery in one write transaction. Active runs are selected by
    /// their state column alone, so damaged metadata cannot block recovery of
    /// every other run; a run whose state cannot be decoded is left unchanged
    /// and reported, never resumed. Leases, attempts, baselines and candidates
    /// that cannot be decoded have their state column settled where possible.
    pub fn recover_after_restart(&mut self) -> Result<RecoveryReport> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut report = RecoveryReport::default();
        let runs = tx
            .prepare("SELECT id,state,revision FROM runs ORDER BY rowid")?
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, u64>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for (id, state, revision) in runs {
            let state: RunState = match serde_json::from_str(&state) {
                Ok(state) => state,
                Err(error) => {
                    report.skipped.push(SkippedRecord {
                        table: "runs",
                        id,
                        reason: format!(
                            "{}; run state unknown, left unchanged",
                            undecodable(&error)
                        ),
                    });
                    continue;
                }
            };
            if !matches!(
                state,
                RunState::Queued
                    | RunState::Running
                    | RunState::Exploring
                    | RunState::Implementing
                    | RunState::Validating
                    | RunState::Reviewing
            ) {
                continue;
            }
            transition(
                &tx,
                &id,
                revision,
                RunState::Interrupted,
                "run.interrupted",
                serde_json::json!({"reason":"runtime restarted before attempt settled"}),
            )?;
            report.interrupted_runs += 1;
        }
        baselines::interrupt_running_baselines(&tx, &mut report.skipped)?;
        validation::interrupt_candidates(&tx, &mut report.skipped)?;
        writer::abandon_orphaned_work(&tx, &mut report.skipped)?;
        tx.commit()?;
        Ok(report)
    }

    /// Append-only revisions. Concurrent writers must supply the next revision.
    pub fn save_plan(&mut self, plan: &PlanRevision) -> Result<()> {
        plan.validate()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: u64 = tx.query_row(
            "SELECT COALESCE(MAX(revision),0) FROM plan_revisions WHERE id=?1",
            [&plan.id],
            |row| row.get(0),
        )?;
        if plan.revision <= current {
            return Err(StorageError::ImmutableRevision);
        }
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
        tx.commit()?;
        Ok(())
    }

    pub fn get_plan(&self, id: &str, revision: u64) -> Result<Option<PlanRevision>> {
        let payload: Option<String> = self
            .connection
            .query_row(
                "SELECT payload FROM plan_revisions WHERE id=?1 AND revision=?2",
                params![id, revision],
                |r| r.get(0),
            )
            .optional()?;
        payload
            .map(|value| serde_json::from_str(&value).map_err(Into::into))
            .transpose()
    }

    /// A plan-ready workflow binds to one immutable project plan. Metadata
    /// supplied to create_run cannot impersonate this transactional binding.
    pub fn bind_plan(
        &mut self,
        run_id: &str,
        expected_run_revision: u64,
        plan_id: &str,
        plan_revision: u64,
    ) -> Result<RunRecord> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let plan = latest_plan(&tx, plan_id)?;
        let run = get_run(&tx, run_id)?.ok_or_else(|| StorageError::NotFound(run_id.into()))?;
        if plan.revision != plan_revision || plan.project_id != run.project_id {
            return Err(StorageError::Invalid(
                "plan revision or project does not match this workflow".into(),
            ));
        }
        tx.execute(
            "UPDATE runs SET plan_id=?1,plan_revision=?2 WHERE id=?3",
            params![plan_id, plan_revision, run_id],
        )?;
        transition(
            &tx,
            run_id,
            expected_run_revision,
            RunState::PlanReady,
            "plan.ready",
            serde_json::json!({"plan_id":plan.id,"plan_revision":plan.revision,"plan_hash":plan.hash}),
        )?;
        tx.commit()?;
        self.get_run(run_id)?
            .ok_or_else(|| StorageError::NotFound(run_id.into()))
    }

    /// Compare current plan revision/hash and persist authorization, state and
    /// event in one write transaction. Browser-supplied permissions cannot widen it.
    pub fn approve_plan(
        &mut self,
        run_id: &str,
        expected_run_revision: u64,
        approval: &Approval,
    ) -> Result<RunRecord> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let plan = latest_plan(&tx, &approval.plan_id)?;
        approval.authorize(&plan)?;
        let registered: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
            [&plan.project_id],
            |row| row.get(0),
        )?;
        if registered {
            writer::check_manifest_binding(&tx, &plan)?;
        }

        let bound: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1 AND plan_id=?2 AND plan_revision=?3 AND project_id=?4)",
            params![run_id,plan.id,plan.revision,plan.project_id], |r| r.get(0))?;
        if !bound {
            return Err(StorageError::Invalid(
                "approval does not match the workflow's bound plan".into(),
            ));
        }
        tx.execute(
            "INSERT INTO approvals(id,plan_id,plan_revision,payload) VALUES (?1,?2,?3,?4)",
            params![
                approval.id,
                approval.plan_id,
                approval.plan_revision,
                serde_json::to_string(approval)?
            ],
        )?;
        tx.execute(
            "UPDATE runs SET approval_id=?1 WHERE id=?2",
            params![approval.id, run_id],
        )?;
        transition(
            &tx,
            run_id,
            expected_run_revision,
            RunState::Approved,
            "plan.approved",
            serde_json::json!({"approval_id":approval.id,"plan_id":plan.id,"plan_revision":plan.revision,"plan_hash":plan.hash}),
        )?;
        tx.commit()?;
        self.get_run(run_id)?
            .ok_or_else(|| StorageError::NotFound(run_id.into()))
    }
}

pub(crate) fn get_run(connection: &Connection, id: &str) -> Result<Option<RunRecord>> {
    let row = connection.query_row("SELECT id,project_id,state,revision,metadata,created_at,updated_at FROM runs WHERE id=?1", [id],
        run_row).optional()?;
    row.map(decode_run).transpose()
}

type RunRow = (String, String, String, u64, String, String, String);

fn run_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RunRow> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
    ))
}

fn decode_run(
    (id, project_id, state, revision, metadata, created_at, updated_at): RunRow,
) -> Result<RunRecord> {
    Ok(RunRecord {
        id,
        project_id,
        state: serde_json::from_str(&state)?,
        revision,
        metadata: serde_json::from_str(&metadata)?,
        created_at,
        updated_at,
    })
}

pub(crate) fn transition(
    tx: &Transaction<'_>,
    id: &str,
    expected: u64,
    next: RunState,
    kind: &str,
    payload: Value,
) -> Result<()> {
    // Only the state and revision decide a transition; descriptive metadata
    // is not decoded, so it cannot block one.
    let (state, revision): (String, u64) = tx
        .query_row("SELECT state,revision FROM runs WHERE id=?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?
        .ok_or_else(|| StorageError::NotFound(id.into()))?;
    if revision != expected {
        return Err(StorageError::RevisionConflict {
            expected,
            actual: revision,
        });
    }
    serde_json::from_str::<RunState>(&state)?.transition(next)?;
    tx.execute(
        "UPDATE runs SET state=?1,revision=revision+1,updated_at=?2 WHERE id=?3 AND revision=?4",
        params![state_json(next)?, now(), id, expected],
    )?;
    let mut payload = payload
        .as_object()
        .cloned()
        .ok_or_else(|| StorageError::Invalid("event payload must be an object".into()))?;
    payload.insert("state".into(), serde_json::to_value(next)?);
    payload.insert("revision".into(), serde_json::json!(expected + 1));
    append(tx, id, kind, Value::Object(payload))?;
    Ok(())
}

pub(crate) fn append(
    tx: &Transaction<'_>,
    run_id: &str,
    event_type: &str,
    payload: Value,
) -> Result<EventEnvelope> {
    if event_type.trim().is_empty() || !payload.is_object() {
        return Err(StorageError::Invalid(
            "event type and object payload are required".into(),
        ));
    }
    let project_id: String = tx
        .query_row("SELECT project_id FROM runs WHERE id=?1", [run_id], |r| {
            r.get(0)
        })
        .optional()?
        .ok_or_else(|| StorageError::NotFound(run_id.into()))?;
    let sequence: u64 = tx.query_row(
        "SELECT COALESCE(MAX(sequence),0)+1 FROM events WHERE run_id=?1",
        [run_id],
        |r| r.get(0),
    )?;
    let event = append_envelope(&project_id, run_id, sequence, event_type, payload)?;
    tx.execute(
        "INSERT INTO events(run_id,sequence,event_id,envelope) VALUES (?1,?2,?3,?4)",
        params![
            run_id,
            sequence,
            event.event_id,
            serde_json::to_string(&event)?
        ],
    )?;
    Ok(event)
}

fn latest_plan(tx: &Transaction<'_>, id: &str) -> Result<PlanRevision> {
    let payload: String = tx
        .query_row(
            "SELECT payload FROM plan_revisions WHERE id=?1 ORDER BY revision DESC LIMIT 1",
            [id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| StorageError::NotFound(id.into()))?;
    Ok(serde_json::from_str(&payload)?)
}

pub(crate) fn authorize_run(tx: &Transaction<'_>, run_id: &str) -> Result<()> {
    authorized_approval(tx, run_id).map(|_| ())
}

/// The durably stored approval that authorizes the run's exact plan revision:
/// the developer's assignment of every task of that plan (R06).
pub(crate) fn authorized_approval(tx: &Transaction<'_>, run_id: &str) -> Result<Approval> {
    let payload: String = tx.query_row("SELECT a.payload FROM runs r JOIN approvals a ON r.approval_id=a.id AND r.plan_id=a.plan_id AND r.plan_revision=a.plan_revision WHERE r.id=?1", [run_id], |r| r.get(0))
        .optional()?.ok_or_else(|| StorageError::Invalid("implementation requires a durably stored approval".into()))?;
    let approval: Approval = serde_json::from_str(&payload)?;
    approval.authorize(&latest_plan(tx, &approval.plan_id)?)?;
    Ok(approval)
}

/// Build one durable envelope. `stream_id` is a run ID or a project stream ID.
fn append_envelope(
    project_id: &str,
    stream_id: &str,
    sequence: u64,
    event_type: &str,
    payload: Value,
) -> Result<EventEnvelope> {
    if event_type.trim().is_empty() || !payload.is_object() {
        return Err(StorageError::Invalid(
            "event type and object payload are required".into(),
        ));
    }
    Ok(EventEnvelope {
        schema_version: SCHEMA_VERSION.into(),
        event_id: format!("{stream_id}:{sequence}"),
        sequence,
        timestamp: now(),
        project_id: project_id.into(),
        run_id: stream_id.into(),
        event_type: event_type.into(),
        payload,
    })
}

pub(crate) fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
pub(crate) fn state_json(state: RunState) -> Result<String> {
    Ok(serde_json::to_string(&state)?)
}

/// Ordered, append-only schema migrations. A shipped migration is never
/// edited: databases that recorded its version never run it again, so a change
/// is always a new version. Each script commits its own version row and stays
/// idempotent, because two processes may race to initialize a new file.
const MIGRATIONS: &[(u32, &str)] = &[
    (1, MIGRATION_1),
    (2, MIGRATION_2),
    (3, MIGRATION_3),
    (4, MIGRATION_4),
    (5, MIGRATION_5),
    (6, MIGRATION_6),
    (7, MIGRATION_7),
    (8, MIGRATION_8),
    (9, MIGRATION_9),
    (10, MIGRATION_10),
    (11, MIGRATION_11),
    (12, MIGRATION_12),
];

/// The highest schema version this build creates and understands. A database
/// that records a higher version is refused.
pub const DATABASE_SCHEMA_VERSION: u32 = MIGRATIONS[MIGRATIONS.len() - 1].0;

/// The SQL of one shipped migration, for upgrade drills that rebuild a
/// database at an older schema version.
#[doc(hidden)]
pub fn migration_sql(version: u32) -> Option<&'static str> {
    MIGRATIONS
        .iter()
        .find(|(known, _)| *known == version)
        .map(|(_, sql)| *sql)
}

/// Versions recorded in `schema_migrations`; empty for a new file.
pub(crate) fn applied_versions(connection: &Connection) -> Result<BTreeSet<u32>> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations')",
        [],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(BTreeSet::new());
    }
    let mut statement = connection.prepare("SELECT version FROM schema_migrations")?;
    let versions = statement
        .query_map([], |r| r.get::<_, u32>(0))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(versions)
}

/// Apply the migrations this database lacks, in order. An existing database
/// is copied to `<directory>/backups/` first; `path` is `None` in memory.
fn migrate(connection: &Connection, path: Option<&Path>) -> Result<()> {
    let applied = applied_versions(connection)?;
    let current = applied.last().copied().unwrap_or(0);
    if current > DATABASE_SCHEMA_VERSION {
        return Err(StorageError::SchemaTooNew {
            path: path.unwrap_or(Path::new(":memory:")).display().to_string(),
            found: current,
            supported: DATABASE_SCHEMA_VERSION,
        });
    }
    let missing: Vec<_> = MIGRATIONS
        .iter()
        .filter(|(version, _)| !applied.contains(version))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    if let (Some(path), true) = (path, current > 0) {
        backup::pre_migration_backup(connection, path, current)?;
    }
    for (version, sql) in missing {
        connection.execute_batch(sql)?;
        if !applied_versions(connection)?.contains(version) {
            return Err(StorageError::Invalid(format!(
                "migration {version} did not record its schema version"
            )));
        }
    }
    Ok(())
}

const MIGRATION_1: &str = "
BEGIN IMMEDIATE;
CREATE TABLE IF NOT EXISTS schema_migrations(version INTEGER PRIMARY KEY);
CREATE TABLE IF NOT EXISTS plan_revisions(id TEXT NOT NULL, revision INTEGER NOT NULL CHECK(revision>0), hash TEXT NOT NULL,
 payload TEXT NOT NULL, PRIMARY KEY(id,revision));
CREATE TRIGGER IF NOT EXISTS immutable_plans_update BEFORE UPDATE ON plan_revisions BEGIN SELECT RAISE(ABORT,'immutable plan revision'); END;
CREATE TRIGGER IF NOT EXISTS immutable_plans_delete BEFORE DELETE ON plan_revisions BEGIN SELECT RAISE(ABORT,'immutable plan revision'); END;
CREATE TABLE IF NOT EXISTS approvals(id TEXT PRIMARY KEY, plan_id TEXT NOT NULL, plan_revision INTEGER NOT NULL, payload TEXT NOT NULL,
 FOREIGN KEY(plan_id,plan_revision) REFERENCES plan_revisions(id,revision));
CREATE TRIGGER IF NOT EXISTS immutable_approvals_update BEFORE UPDATE ON approvals BEGIN SELECT RAISE(ABORT,'immutable approval'); END;
CREATE TRIGGER IF NOT EXISTS immutable_approvals_delete BEFORE DELETE ON approvals BEGIN SELECT RAISE(ABORT,'immutable approval'); END;
CREATE TABLE IF NOT EXISTS runs(id TEXT PRIMARY KEY, project_id TEXT NOT NULL, state TEXT NOT NULL, revision INTEGER NOT NULL CHECK(revision>0),
 metadata TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, approval_id TEXT REFERENCES approvals(id),
 plan_id TEXT, plan_revision INTEGER, FOREIGN KEY(plan_id,plan_revision) REFERENCES plan_revisions(id,revision));
CREATE TABLE IF NOT EXISTS events(run_id TEXT NOT NULL REFERENCES runs(id), sequence INTEGER NOT NULL CHECK(sequence>0), event_id TEXT NOT NULL UNIQUE,
 envelope TEXT NOT NULL, PRIMARY KEY(run_id,sequence));
INSERT OR IGNORE INTO schema_migrations(version) VALUES (1);
COMMIT;
";

const MIGRATION_2: &str = "
BEGIN IMMEDIATE;
CREATE TABLE IF NOT EXISTS projects(id TEXT PRIMARY KEY, name TEXT NOT NULL, revision INTEGER NOT NULL CHECK(revision>=0),
 manifest_hash TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS manifest_revisions(project_id TEXT NOT NULL REFERENCES projects(id), revision INTEGER NOT NULL CHECK(revision>0),
 hash TEXT NOT NULL, payload TEXT NOT NULL, base_commits TEXT NOT NULL, confirmed_at TEXT NOT NULL, PRIMARY KEY(project_id,revision));
CREATE TRIGGER IF NOT EXISTS immutable_manifests_update BEFORE UPDATE ON manifest_revisions BEGIN SELECT RAISE(ABORT,'immutable manifest revision'); END;
CREATE TRIGGER IF NOT EXISTS immutable_manifests_delete BEFORE DELETE ON manifest_revisions BEGIN SELECT RAISE(ABORT,'immutable manifest revision'); END;
CREATE TABLE IF NOT EXISTS repositories(project_id TEXT NOT NULL REFERENCES projects(id), id TEXT NOT NULL, local_path TEXT NOT NULL,
 observation TEXT NOT NULL, registered_at TEXT NOT NULL, PRIMARY KEY(project_id,id));
CREATE TABLE IF NOT EXISTS discoveries(id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id), created_at TEXT NOT NULL, payload TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS project_events(project_id TEXT NOT NULL REFERENCES projects(id), sequence INTEGER NOT NULL CHECK(sequence>0),
 event_id TEXT NOT NULL UNIQUE, envelope TEXT NOT NULL, PRIMARY KEY(project_id,sequence));
INSERT OR IGNORE INTO schema_migrations(version) VALUES (2);
COMMIT;
";

const MIGRATION_3: &str = "
BEGIN IMMEDIATE;
CREATE TABLE IF NOT EXISTS baselines(id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id), repository_id TEXT NOT NULL,
 run_id TEXT NOT NULL REFERENCES runs(id), status TEXT NOT NULL, payload TEXT NOT NULL, started_at TEXT NOT NULL, finished_at TEXT);
CREATE TABLE IF NOT EXISTS check_results(id TEXT PRIMARY KEY, baseline_id TEXT NOT NULL REFERENCES baselines(id), recipe_id TEXT NOT NULL, payload TEXT NOT NULL);
CREATE TRIGGER IF NOT EXISTS immutable_check_results_update BEFORE UPDATE ON check_results BEGIN SELECT RAISE(ABORT,'immutable check result'); END;
CREATE TRIGGER IF NOT EXISTS immutable_check_results_delete BEFORE DELETE ON check_results BEGIN SELECT RAISE(ABORT,'immutable check result'); END;
INSERT OR IGNORE INTO schema_migrations(version) VALUES (3);
COMMIT;
";

const MIGRATION_4: &str = "
BEGIN IMMEDIATE;
CREATE TABLE IF NOT EXISTS tickets(id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id), revision INTEGER NOT NULL CHECK(revision>0),
 idempotency_key TEXT NOT NULL, payload TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE UNIQUE INDEX IF NOT EXISTS tickets_idempotency ON tickets(project_id, idempotency_key);
CREATE TABLE IF NOT EXISTS worktree_leases(id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), project_id TEXT NOT NULL REFERENCES projects(id),
 repository_id TEXT NOT NULL, path TEXT NOT NULL UNIQUE, base_commit TEXT NOT NULL, state TEXT NOT NULL, payload TEXT NOT NULL, created_at TEXT NOT NULL, released_at TEXT);
CREATE UNIQUE INDEX IF NOT EXISTS one_active_lease_per_repository ON worktree_leases(project_id, repository_id) WHERE state='active';
CREATE TABLE IF NOT EXISTS attempts(id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), task_id TEXT NOT NULL, number INTEGER NOT NULL CHECK(number>0),
 state TEXT NOT NULL, payload TEXT NOT NULL, started_at TEXT NOT NULL, finished_at TEXT, UNIQUE(run_id, number));
INSERT OR IGNORE INTO schema_migrations(version) VALUES (4);
COMMIT;
";

const MIGRATION_5: &str = "
BEGIN IMMEDIATE;
CREATE TABLE IF NOT EXISTS candidates(hash TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), id TEXT NOT NULL,
 revision INTEGER NOT NULL CHECK(revision>0), status TEXT NOT NULL, payload TEXT NOT NULL, created_at TEXT NOT NULL,
 UNIQUE(run_id, revision));
CREATE TRIGGER IF NOT EXISTS immutable_candidate_identity BEFORE UPDATE ON candidates
 WHEN NEW.hash != OLD.hash OR NEW.id != OLD.id OR NEW.run_id != OLD.run_id OR NEW.revision != OLD.revision
 OR json_extract(NEW.payload,'$.candidate') != json_extract(OLD.payload,'$.candidate')
 OR json_extract(NEW.payload,'$.repository_trees') != json_extract(OLD.payload,'$.repository_trees')
 OR json_extract(NEW.payload,'$.implementer_session') != json_extract(OLD.payload,'$.implementer_session')
 BEGIN SELECT RAISE(ABORT,'immutable candidate identity'); END;
CREATE TABLE IF NOT EXISTS candidate_check_results(id TEXT PRIMARY KEY, candidate_hash TEXT NOT NULL REFERENCES candidates(hash),
 generation INTEGER NOT NULL, recipe_id TEXT NOT NULL, payload TEXT NOT NULL, UNIQUE(candidate_hash,generation,recipe_id));
CREATE TRIGGER IF NOT EXISTS immutable_candidate_checks_update BEFORE UPDATE ON candidate_check_results BEGIN SELECT RAISE(ABORT,'immutable candidate check'); END;
CREATE TRIGGER IF NOT EXISTS immutable_candidate_checks_delete BEFORE DELETE ON candidate_check_results BEGIN SELECT RAISE(ABORT,'immutable candidate check'); END;
CREATE TABLE IF NOT EXISTS candidate_reviews(id TEXT PRIMARY KEY, candidate_hash TEXT NOT NULL REFERENCES candidates(hash), generation INTEGER NOT NULL,
 payload TEXT NOT NULL, UNIQUE(candidate_hash,generation));
CREATE TRIGGER IF NOT EXISTS immutable_candidate_reviews_update BEFORE UPDATE ON candidate_reviews BEGIN SELECT RAISE(ABORT,'immutable candidate review'); END;
CREATE TRIGGER IF NOT EXISTS immutable_candidate_reviews_delete BEFORE DELETE ON candidate_reviews BEGIN SELECT RAISE(ABORT,'immutable candidate review'); END;
INSERT OR IGNORE INTO schema_migrations(version) VALUES (5);
COMMIT;
";

const MIGRATION_6: &str = "
BEGIN IMMEDIATE;
CREATE TABLE IF NOT EXISTS ticket_sources(
 ticket_id TEXT PRIMARY KEY REFERENCES tickets(id), project_id TEXT NOT NULL REFERENCES projects(id),
 source_id TEXT, source_revision TEXT, original_payload TEXT NOT NULL,
 UNIQUE(project_id,source_id));
CREATE TABLE IF NOT EXISTS ticket_delivery_attempts(
 ticket_id TEXT PRIMARY KEY REFERENCES ticket_sources(ticket_id));
INSERT OR IGNORE INTO schema_migrations(version) VALUES (6);
COMMIT;
";

/// Lookup indexes for per-baseline check results, the latest baseline per
/// repository, a plan's workflow run, a run's leases and the latest discovery.
const MIGRATION_7: &str = "
BEGIN IMMEDIATE;
CREATE INDEX IF NOT EXISTS check_results_baseline ON check_results(baseline_id);
CREATE INDEX IF NOT EXISTS baselines_repository ON baselines(project_id, repository_id);
CREATE INDEX IF NOT EXISTS runs_plan ON runs(plan_id, plan_revision);
CREATE INDEX IF NOT EXISTS worktree_leases_run ON worktree_leases(run_id);
CREATE INDEX IF NOT EXISTS discoveries_project ON discoveries(project_id);
INSERT OR IGNORE INTO schema_migrations(version) VALUES (7);
COMMIT;
";

/// Plan requests a client may replay under a per-ticket idempotency key: the
/// key returns the plan revision and workflow its first delivery created.
const MIGRATION_8: &str = "
BEGIN IMMEDIATE;
CREATE TABLE IF NOT EXISTS plan_requests(ticket_id TEXT NOT NULL REFERENCES tickets(id), idempotency_key TEXT NOT NULL,
 request_hash TEXT NOT NULL, plan_id TEXT NOT NULL, plan_revision INTEGER NOT NULL, run_id TEXT NOT NULL REFERENCES runs(id),
 created_at TEXT NOT NULL, PRIMARY KEY(ticket_id,idempotency_key), FOREIGN KEY(plan_id,plan_revision) REFERENCES plan_revisions(id,revision));
INSERT OR IGNORE INTO schema_migrations(version) VALUES (8);
COMMIT;
";

/// Indexes for bounded, newest-first run history pages (ADR 0015).
const MIGRATION_9: &str = "
BEGIN IMMEDIATE;
CREATE INDEX IF NOT EXISTS runs_history ON runs(created_at DESC,id);
CREATE INDEX IF NOT EXISTS runs_project_history ON runs(project_id,created_at DESC,id);
INSERT OR IGNORE INTO schema_migrations(version) VALUES (9);
COMMIT;
";

/// Per-task workflow state of a run's plan task graph (M4, ADR 0017): one
/// row per plan task, settled transactionally with the attempt that ran it
/// and queried by run. Events `task.scheduled`, `task.completed`,
/// `task.failed`, `task.interrupted`, `task.blocked` and `task.cancelled`
/// carry the reason in the run's stream.
const MIGRATION_10: &str = "
BEGIN IMMEDIATE;
CREATE TABLE IF NOT EXISTS run_tasks(run_id TEXT NOT NULL REFERENCES runs(id), task_id TEXT NOT NULL, repository_id TEXT NOT NULL,
 state TEXT NOT NULL, attempt_id TEXT, candidate_commit TEXT, candidate_tree TEXT, depends_on TEXT NOT NULL, reason TEXT,
 updated_at TEXT NOT NULL, PRIMARY KEY(run_id, task_id));
INSERT OR IGNORE INTO schema_migrations(version) VALUES (10);
COMMIT;
";

/// Publication records (one per run and candidate hash) and the intent
/// ledger of their external side effects (ADR 0018). Outcomes are recorded in
/// the payload; `state` columns are indexes for recovery and display.
/// ADR 0023: delegated roles and assignment traces on the task graph. Each
/// task records its role, who assigned it (the approval of the exact plan),
/// the advisory task it was derived from, the dependency overrides that
/// name it and, for an advisory role, its structured result.
const MIGRATION_12: &str = "
BEGIN IMMEDIATE;
ALTER TABLE run_tasks ADD COLUMN role TEXT NOT NULL DEFAULT 'implementer';
ALTER TABLE run_tasks ADD COLUMN assigned_by TEXT;
ALTER TABLE run_tasks ADD COLUMN derived_from_task TEXT;
ALTER TABLE run_tasks ADD COLUMN dependency_overrides TEXT;
ALTER TABLE run_tasks ADD COLUMN advisory_result TEXT;
INSERT OR IGNORE INTO schema_migrations(version) VALUES (12);
COMMIT;
";
const MIGRATION_11: &str = "
BEGIN IMMEDIATE;
CREATE TABLE IF NOT EXISTS publications(run_id TEXT NOT NULL REFERENCES runs(id), candidate_hash TEXT NOT NULL, ticket_id TEXT NOT NULL,
 idempotency_key TEXT NOT NULL, state TEXT NOT NULL, payload TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 PRIMARY KEY(run_id,candidate_hash));
CREATE TABLE IF NOT EXISTS publication_intents(key TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), candidate_hash TEXT NOT NULL,
 repository_id TEXT NOT NULL, step TEXT NOT NULL, state TEXT NOT NULL, payload TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 FOREIGN KEY(run_id,candidate_hash) REFERENCES publications(run_id,candidate_hash));
CREATE INDEX IF NOT EXISTS publication_intents_publication ON publication_intents(run_id,candidate_hash);
INSERT OR IGNORE INTO schema_migrations(version) VALUES (11);
COMMIT;
";
