//! Deterministic Luuna contracts. Worker output never authorizes a state change.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

mod baseline;
mod project;
mod store_mode;
mod validation;
mod writer;
pub use baseline::*;
pub use project::*;
pub use store_mode::*;
pub use validation::*;
pub use writer::*;

pub const SCHEMA_VERSION: &str = "0.1";
pub const CANONICALIZATION: &str = "luuna-json-v1";

/// Inclusive plan budget ceilings. Unbounded values could overflow a deadline or
/// exceed what a JSON client shows exactly; zero would refuse every stage.
pub const MAX_PLAN_ATTEMPTS: u32 = 10;
pub const MAX_PLAN_TIMEOUT_SECONDS: u64 = 86_400;
pub const MAX_PLAN_COST_USD_MICROS: u64 = 1_000_000_000;
/// Most writers a plan may run at once (M4). The runtime's global limit of
/// four active runs bounds it from above; a plan never needs more workers
/// than it has tasks.
pub const MAX_PLAN_WORKERS: u32 = 4;

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("invalid contract: {0}")]
    Invalid(String),
    #[error("invalid transition from {from:?} to {to:?}")]
    InvalidTransition { from: RunState, to: RunState },
    #[error("approval does not authorize this exact plan revision and permissions")]
    StaleApproval,
    #[error("repository identity mismatch: {0}")]
    IdentityMismatch(String),
    #[error("baseline evidence required: {0}")]
    BaselineRequired(String),
    #[error("scope violation: {0}")]
    ScopeViolation(String),
    #[error("budget exceeded: {0}")]
    BudgetExceeded(String),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
    Discovered,
    Exploring,
    TestingBlocked,
    PlanReady,
    Approved,
    Implementing,
    Validating,
    Reviewing,
    Reviewed,
    Paused,
}

impl RunState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted | Self::Reviewed
        )
    }

    /// Structural lifecycle only. `Approved`/`Implementing` additionally require
    /// `Approval::authorize`; storage exposes those through its approval gate.
    pub fn transition(self, to: Self) -> Result<Self, DomainError> {
        use RunState::*;
        let valid = matches!(
            (self, to),
            (Queued, Running | Cancelled | Interrupted | Failed)
                | (Running, Completed | Failed | Cancelled | Interrupted)
                | (Discovered, Exploring | Cancelled)
                | (
                    Exploring,
                    PlanReady | TestingBlocked | Failed | Cancelled | Interrupted
                )
                | (TestingBlocked, Exploring | Cancelled)
                | (PlanReady, Approved | Exploring | Cancelled)
                | (Approved, Implementing | Paused | Cancelled)
                | (
                    Implementing,
                    Validating | Paused | Failed | Cancelled | Interrupted
                )
                | (
                    Validating,
                    Reviewing | TestingBlocked | Paused | Failed | Cancelled | Interrupted
                )
                | (
                    Reviewing,
                    Reviewed | Paused | Failed | Cancelled | Interrupted
                )
                | (Paused, Exploring | Cancelled)
                | (Paused | Failed | TestingBlocked, Implementing)
                // Structural only: storage admits `Interrupted -> Implementing`
                // solely to resume an interrupted task of a plan's task graph
                // under the original approval (M4); completed tasks keep
                // their commits and worktrees.
                | (Interrupted, Implementing)
                // Structural only: storage admits `Implementing -> Completed`
                // solely for a plan whose every task is advisory (ADR 0023):
                // no candidate exists, so nothing validates; the next plan
                // revision carries the proposals.
                | (Implementing, Completed)
                | (Failed | TestingBlocked, Paused)
                | (Failed, Cancelled)
                // Structural only: storage admits `Reviewed -> Failed` solely to
                // invalidate a reviewed candidate whose inputs changed after the
                // review (worktree drift found at publication, a CI failure the
                // developer sends to repair); the evidence is marked, never
                // reused, and repair or re-planning follows as for any failure.
                | (Reviewed, Failed)
                // Structural only: storage admits `Failed -> Validating` solely for a
                // candidate whose required checks passed and whose reviewer left no
                // verdict (a fresh generation, never a new revision or budget).
                | (Interrupted | Failed, Validating)
        );
        if valid {
            Ok(to)
        } else {
            Err(DomainError::InvalidTransition { from: self, to })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Read,
    Write,
    Shell,
    Network,
    Publish,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    /// Repository ID to explicit relative path prefixes. `.` means the repository.
    pub repositories: BTreeMap<String, Vec<String>>,
    pub operations: Vec<Operation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budgets {
    pub max_workers: u32,
    pub max_attempts: u32,
    pub timeout_seconds: u64,
    pub cost_usd_micros: u64,
}

/// The process roles a plan task may carry (ADR 0023). The runtime, not the
/// role prompt, decides what a task may do: an advisory role runs read-only
/// and records a structured advisory result instead of a candidate commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskRole {
    /// The writer: one bounded change in one repository worktree.
    Implementer,
    /// Advises on one repository: affected modules, local commands, task
    /// boundaries and check coverage. Never approves scope or publishes.
    RepositoryLead,
    /// Advises on the project: acceptance criteria, cross-repository
    /// contracts, dependency order, risk and integration.
    ProjectOrchestrator,
    /// The testing prerequisite specialist (ADR 0022): a writer that
    /// establishes a repository's test harness under a `testing_setup` plan.
    /// Only the plan's purpose, never this role name, admits a plan without a
    /// passing baseline.
    TestingSetup,
}

impl TaskRole {
    pub const ALL: [TaskRole; 4] = [
        TaskRole::Implementer,
        TaskRole::RepositoryLead,
        TaskRole::ProjectOrchestrator,
        TaskRole::TestingSetup,
    ];

    pub fn parse(role: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == role)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TaskRole::Implementer => "implementer",
            TaskRole::RepositoryLead => "repository_lead",
            TaskRole::ProjectOrchestrator => "project_orchestrator",
            TaskRole::TestingSetup => "testing-setup",
        }
    }

    /// Advisory roles produce findings, proposed task boundaries and risks,
    /// never a candidate commit; their worktree must be unchanged afterwards.
    /// Writers (the implementer and the testing-setup specialist) commit.
    pub fn is_advisory(self) -> bool {
        !matches!(self, TaskRole::Implementer | TaskRole::TestingSetup)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanTask {
    pub id: String,
    pub repository_id: String,
    /// One of [`TaskRole`]; the contract keeps the string so earlier plans
    /// hash unchanged.
    pub role: String,
    pub depends_on: Vec<String>,
    pub paths: Vec<String>,
    pub operations: Vec<Operation>,
    pub acceptance_criteria: Vec<String>,
    pub execution_profile: String,
    /// The advisory task (of an earlier revision of this plan) whose proposal
    /// the developer copied into this task: the parent of the assignment
    /// trace (R06). Absent for a task the developer wrote directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_from_task: Option<String>,
}

impl PlanTask {
    pub fn role(&self) -> Option<TaskRole> {
        TaskRole::parse(&self.role)
    }

    pub fn is_advisory(&self) -> bool {
        self.role().is_some_and(TaskRole::is_advisory)
    }
}

/// How a plan departs from the project's confirmed repository relationships
/// (P44, ADR 0023). Part of the immutable, hashed plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyOverride {
    /// The consumer task (the one that would, or does, depend on `producer`).
    pub consumer: String,
    /// The producer task.
    pub producer: String,
    pub action: OverrideAction,
    /// Why the developer departs from the confirmed graph; never empty.
    pub justification: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverrideAction {
    /// The consumer does not depend on the producer although a confirmed
    /// relationship between their repositories implies it.
    Waive,
    /// The consumer depends on the producer although no confirmed
    /// relationship between their repositories exists.
    Add,
}

impl OverrideAction {
    pub fn as_str(self) -> &'static str {
        match self {
            OverrideAction::Waive => "waive",
            OverrideAction::Add => "add",
        }
    }
}

/// Most entries an advisory result may carry in each of its lists, and the
/// most bytes its JSON may occupy.
pub const MAX_ADVISORY_ENTRIES: usize = 64;
pub const MAX_ADVISORY_BYTES: usize = 64_000;

/// What an advisory task (a project orchestrator or repository lead) returns
/// instead of a candidate commit (ADR 0023): findings, proposed task
/// boundaries and risks. A proposal becomes a task only through a new plan
/// revision the developer creates and approves, naming the advisory task in
/// `derived_from_task`; the runtime never creates tasks from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AdvisoryResult {
    #[serde(default)]
    pub findings: Vec<String>,
    #[serde(default)]
    pub proposed_tasks: Vec<ProposedTask>,
    #[serde(default)]
    pub risks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedTask {
    pub id: String,
    pub repository_id: String,
    #[serde(default = "default_proposed_role")]
    pub role: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub rationale: String,
}

fn default_proposed_role() -> String {
    TaskRole::Implementer.as_str().to_owned()
}

impl AdvisoryResult {
    /// Parse the advisory JSON object from a worker's final message. The
    /// message may surround the object with prose or a code fence; the text
    /// between its first `{` and last `}` must be the object. The worker's
    /// output is data: nothing here grants it authority.
    pub fn parse(text: &str) -> Result<Self, DomainError> {
        let trimmed = text.trim();
        let candidate =
            match (trimmed.find('{'), trimmed.rfind('}')) {
                (Some(start), Some(end)) if start < end => &trimmed[start..=end],
                _ => return invalid(
                    "advisory result must be a JSON object with findings, proposed_tasks and risks",
                ),
            };
        let result: AdvisoryResult = serde_json::from_str(candidate).map_err(|error| {
            DomainError::Invalid(format!(
                "advisory result must be a JSON object with findings, proposed_tasks and risks: {error}"
            ))
        })?;
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        let text = |values: &[String]| {
            values.len() <= MAX_ADVISORY_ENTRIES && values.iter().all(|v| !v.trim().is_empty())
        };
        if !text(&self.findings) || !text(&self.risks) {
            return invalid("advisory findings and risks are non-empty strings, at most 64 each");
        }
        if self.proposed_tasks.len() > MAX_ADVISORY_ENTRIES {
            return invalid("an advisory result proposes at most 64 tasks");
        }
        let mut ids = BTreeSet::new();
        for task in &self.proposed_tasks {
            if task.id.trim().is_empty()
                || !ids.insert(task.id.as_str())
                || task.repository_id.trim().is_empty()
                || TaskRole::parse(&task.role).is_none()
                || task.depends_on.iter().any(|d| d.trim().is_empty())
                || task.paths.iter().any(|p| scope_path_problem(p).is_some())
                || task.acceptance_criteria.iter().any(|c| c.trim().is_empty())
            {
                return invalid(
                    "a proposed task needs a unique id, a repository, a known role and well-formed dependencies, paths and criteria",
                );
            }
        }
        if serde_json::to_vec(self)?.len() > MAX_ADVISORY_BYTES {
            return invalid("advisory result exceeds 64000 bytes");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckRecipe {
    pub id: String,
    pub repository_id: String,
    pub executable: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub timeout_seconds: u64,
    pub required: bool,
    pub capabilities: Vec<String>,
    pub execution_profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanRevision {
    pub schema_version: String,
    pub canonicalization: String,
    pub id: String,
    pub revision: u64,
    pub project_id: String,
    pub ticket_id: String,
    pub ticket_revision: u64,
    pub manifest_hash: String,
    pub base_commits: BTreeMap<String, String>,
    /// If this plan repairs an existing candidate, bind that exact identity too.
    pub candidate_hash: Option<String>,
    pub permissions: Permissions,
    pub tasks: Vec<PlanTask>,
    pub required_checks: Vec<CheckRecipe>,
    pub budgets: Budgets,
    /// Departures from the confirmed repository relationships, each with a
    /// justification (P44). Absence preserves old plan hashes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependency_overrides: Vec<DependencyOverride>,
    /// Explicit, hash-bound component references. Absence preserves old plan hashes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_context: Option<ComponentContext>,
    /// Copied from the ticket when the plan is made and part of the hash; the
    /// default (feature) is omitted so earlier plans keep their hashes. A
    /// `testing_setup` plan carries the testing-setup role and the recipes that
    /// will become the repository's baseline (ADR 0022).
    #[serde(default, skip_serializing_if = "TicketPurpose::is_feature")]
    pub purpose: TicketPurpose,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentContext {
    pub memory: Vec<MemoryReference>,
    pub prompt: Option<ApprovedPrompt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryReference {
    pub id: String,
    pub scope: String,
    pub hash: String,
    pub content: String,
    pub provenance: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedPrompt {
    pub name: String,
    pub version: u64,
    pub hash: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptReference {
    pub name: String,
    pub version: u64,
    pub hash: String,
}

impl PlanRevision {
    /// luuna-json-v1 recursively sorts object keys, uses JSON integer/string
    /// encoding, preserves array order and Unicode bytes, and omits only `hash`.
    /// Inputs must already be normalized; no lossy path/Unicode normalization.
    pub fn computed_hash(&self) -> Result<String, DomainError> {
        let mut value = serde_json::to_value(self)?;
        value
            .as_object_mut()
            .expect("plan is an object")
            .remove("hash");
        hash_value(&value)
    }

    pub fn seal(mut self) -> Result<Self, DomainError> {
        self.validate_payload()?;
        self.hash = self.computed_hash()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        self.validate_payload()?;
        if self.hash != self.computed_hash()? {
            return Err(DomainError::Invalid(
                "plan hash does not match payload".into(),
            ));
        }
        Ok(())
    }

    fn validate_payload(&self) -> Result<(), DomainError> {
        if let Some(context) = &self.component_context {
            let mut ids = std::collections::BTreeSet::new();
            if context.memory.len() > 16
                || context.memory.iter().any(|memory| {
                    memory.id.is_empty()
                        || !ids.insert(memory.id.clone())
                        || !is_hash(&memory.hash)
                        || memory.content.trim().is_empty()
                        || memory.provenance.trim().is_empty()
                        || memory.provenance.len() > 1000
                        || !matches!(memory.scope.as_str(), "project" | "process" | "user")
                })
                || context.prompt.as_ref().is_some_and(|prompt| {
                    prompt.name.is_empty()
                        || prompt.version == 0
                        || !is_hash(&prompt.hash)
                        || prompt.body.trim().is_empty()
                })
                || serde_json::to_vec(context)?.len() > 16_000
            {
                return invalid("invalid or duplicated approved component resources");
            }
        }
        if self.schema_version != SCHEMA_VERSION || self.canonicalization != CANONICALIZATION {
            return invalid("unsupported schema or canonicalization version");
        }
        if self.id.is_empty()
            || self.project_id.is_empty()
            || self.ticket_id.is_empty()
            || self.revision == 0
            || self.ticket_revision == 0
        {
            return invalid("plan and ticket identities and positive revisions are required");
        }
        if !is_hash(&self.manifest_hash)
            || self.candidate_hash.as_deref().is_some_and(|h| !is_hash(h))
        {
            return invalid("invalid manifest or candidate hash");
        }
        if self.base_commits.is_empty()
            || self
                .base_commits
                .iter()
                .any(|(id, commit)| id.is_empty() || !is_commit(commit))
        {
            return invalid("nonempty repository scope and exact commits are required");
        }
        if self.permissions.repositories.keys().collect::<Vec<_>>()
            != self.base_commits.keys().collect::<Vec<_>>()
        {
            return invalid("permission repositories must equal base commit scope");
        }
        if self.permissions.operations.is_empty() || !unique(&self.permissions.operations) {
            return invalid("operations must be nonempty and unique");
        }
        // Task paths first: permissions default to them, so a malformed entry is
        // reported against the field the client actually wrote.
        for task in &self.tasks {
            for path in &task.paths {
                if let Some(problem) = scope_path_problem(path) {
                    return Err(DomainError::Invalid(format!(
                        "task {} path {path:?} {problem}",
                        task.id
                    )));
                }
            }
        }
        for (repository, paths) in &self.permissions.repositories {
            if paths.is_empty() || !unique(paths) {
                return invalid("permission paths must be nonempty and unique");
            }
            for path in paths {
                if let Some(problem) = scope_path_problem(path) {
                    return Err(DomainError::Invalid(format!(
                        "permission path {path:?} for repository {repository} {problem}"
                    )));
                }
            }
        }
        let budgets = &self.budgets;
        // One writer per task at most, and never more than the runtime's own
        // active-work limit: a single-task plan still requires exactly 1.
        let max_workers = (self.tasks.len() as u32).clamp(1, MAX_PLAN_WORKERS);
        if !(1..=max_workers).contains(&budgets.max_workers) {
            return Err(DomainError::Invalid(format!(
                "budgets.max_workers must be between 1 and {max_workers} for a plan with {} task(s)",
                self.tasks.len()
            )));
        }
        if !(1..=MAX_PLAN_ATTEMPTS).contains(&budgets.max_attempts) {
            return Err(DomainError::Invalid(format!(
                "budgets.max_attempts must be between 1 and {MAX_PLAN_ATTEMPTS}"
            )));
        }
        if !(1..=MAX_PLAN_TIMEOUT_SECONDS).contains(&budgets.timeout_seconds) {
            return Err(DomainError::Invalid(format!(
                "budgets.timeout_seconds must be between 1 and {MAX_PLAN_TIMEOUT_SECONDS}"
            )));
        }
        // Zero cost is refused rather than defined: a new model stage needs
        // remaining budget, so a zero-cost plan could never be validated.
        if !(1..=MAX_PLAN_COST_USD_MICROS).contains(&budgets.cost_usd_micros) {
            return Err(DomainError::Invalid(format!(
                "budgets.cost_usd_micros must be between 1 and {MAX_PLAN_COST_USD_MICROS}"
            )));
        }
        if self.tasks.is_empty() {
            return invalid("at least one task is required");
        }
        let mut ids = BTreeSet::new();
        for task in &self.tasks {
            if task.id.is_empty() || !ids.insert(&task.id) {
                return invalid("duplicate or empty task ID");
            }
            let Some(allowed_paths) = self.permissions.repositories.get(&task.repository_id) else {
                return invalid("task references unknown repository");
            };
            let Some(role) = task.role() else {
                return Err(DomainError::Invalid(format!(
                    "task {} role {:?} is not one of implementer, repository_lead, project_orchestrator",
                    task.id, task.role
                )));
            };
            // An advisory task reads; the runtime's scope judgement then
            // treats any change it leaves as out of scope.
            if role.is_advisory() && task.operations != [Operation::Read] {
                return Err(DomainError::Invalid(format!(
                    "task {} has advisory role {} and must carry exactly the read operation",
                    task.id,
                    role.as_str()
                )));
            }
            if let Some(parent) = &task.derived_from_task {
                if parent.is_empty() {
                    return Err(DomainError::Invalid(format!(
                        "task {} derived_from_task must name a task",
                        task.id
                    )));
                }
                if parent == &task.id {
                    return Err(DomainError::Invalid(format!(
                        "task {} derives from itself",
                        task.id
                    )));
                }
                if self.tasks.iter().any(|t| &t.id == parent) {
                    return Err(DomainError::Invalid(format!(
                        "task {} derives from task {parent} of the same plan; a proposal becomes a task only in a later plan revision",
                        task.id
                    )));
                }
            }
            if task.role.is_empty()
                || task.execution_profile.is_empty()
                || task.acceptance_criteria.is_empty()
                || task.acceptance_criteria.iter().any(|s| s.is_empty())
                || task.paths.is_empty()
                || task.paths.iter().any(|p| {
                    scope_path_problem(p).is_some()
                        || !allowed_paths.iter().any(|prefix| path_within(p, prefix))
                })
                || task.operations.is_empty()
                || !unique(&task.operations)
                || task
                    .operations
                    .iter()
                    .any(|op| !self.permissions.operations.contains(op))
            {
                return invalid("task exceeds permissions or omits executable requirements");
            }
        }
        self.validate_task_graph()?;
        self.validate_overrides()?;
        // An advisory-only plan writes no candidate, so it has no check to
        // run; every plan with a writer task needs a required check for the
        // repositories its candidate covers.
        let writes = self.tasks.iter().any(|task| !task.is_advisory());
        if writes
            && (self.required_checks.is_empty() || !self.required_checks.iter().any(|c| c.required))
        {
            return invalid("at least one required check is necessary");
        }
        self.validate_purpose()?;
        // Recipe IDs are unique within their repository (results bind to the
        // pair), so every repository of a multi-repository plan may carry its
        // manifest's `unit` recipe.
        let mut checks = BTreeSet::new();
        for check in &self.required_checks {
            if check.id.is_empty()
                || !checks.insert((&check.repository_id, &check.id))
                || !self.base_commits.contains_key(&check.repository_id)
                || check.executable.is_empty()
                || !valid_relative_path(&check.cwd)
                || check.timeout_seconds == 0
                || check.execution_profile.is_empty()
            {
                return invalid("invalid check recipe or repository");
            }
        }
        Ok(())
    }

    /// A testing-setup plan establishes one repository's baseline: one task,
    /// carrying the testing-setup role, and at least one required recipe for
    /// that repository, since a candidate with nothing to execute could never
    /// become a baseline. Feature plans never carry the testing-setup role,
    /// because only the purpose, not a role name, admits a plan without a
    /// baseline.
    fn validate_purpose(&self) -> Result<(), DomainError> {
        let setup_role = TicketPurpose::TestingSetup.role();
        match self.purpose {
            TicketPurpose::Feature => {
                if self.tasks.iter().any(|task| task.role == setup_role) {
                    return invalid("only a testing_setup plan may carry the testing-setup role");
                }
            }
            TicketPurpose::TestingSetup => {
                let [task] = self.tasks.as_slice() else {
                    return invalid("a testing_setup plan has exactly one task");
                };
                if task.role != setup_role {
                    return invalid("a testing_setup plan's task carries the testing-setup role");
                }
                if self.candidate_hash.is_some() {
                    return invalid("a testing_setup plan does not repair a candidate");
                }
                if !self
                    .required_checks
                    .iter()
                    .any(|check| check.required && check.repository_id == task.repository_id)
                {
                    return invalid(
                        "a testing_setup plan needs at least one required check recipe for its repository",
                    );
                }
            }
        }
        Ok(())
    }

    /// The task DAG (M4): every dependency names another task of this plan,
    /// no task depends on itself or lists a dependency twice, and the
    /// dependencies are acyclic. A refusal names the task and the offending
    /// dependency or the cycle it found. Task IDs are already unique here.
    fn validate_task_graph(&self) -> Result<(), DomainError> {
        let ids: BTreeSet<&str> = self.tasks.iter().map(|task| task.id.as_str()).collect();
        for task in &self.tasks {
            let mut seen = BTreeSet::new();
            for dependency in &task.depends_on {
                if dependency == &task.id {
                    return Err(DomainError::Invalid(format!(
                        "task {} depends on itself",
                        task.id
                    )));
                }
                if !ids.contains(dependency.as_str()) {
                    return Err(DomainError::Invalid(format!(
                        "task {} depends on unknown task {dependency}",
                        task.id
                    )));
                }
                if !seen.insert(dependency.as_str()) {
                    return Err(DomainError::Invalid(format!(
                        "task {} lists dependency {dependency} more than once",
                        task.id
                    )));
                }
            }
        }
        match self.dependency_order() {
            Ok(_) => Ok(()),
            Err(cycle) => Err(DomainError::Invalid(format!(
                "task dependencies contain a cycle: {}",
                cycle.join(" -> ")
            ))),
        }
    }

    /// The override list itself (P44): each names two distinct implementer
    /// tasks of this plan, carries a justification and appears once per
    /// consumer/producer pair. Whether an override applies is decided
    /// against the confirmed relationships by [`Self::check_relationships`].
    fn validate_overrides(&self) -> Result<(), DomainError> {
        let mut pairs = BTreeSet::new();
        for entry in &self.dependency_overrides {
            let label = format!(
                "dependency override {} {} <- {}",
                entry.action.as_str(),
                entry.consumer,
                entry.producer
            );
            let named = |id: &str| -> Result<&PlanTask, DomainError> {
                let task = self.tasks.iter().find(|t| t.id == id).ok_or_else(|| {
                    DomainError::Invalid(format!("{label} names unknown task {id}"))
                })?;
                if task.is_advisory() {
                    return Err(DomainError::Invalid(format!(
                        "{label} names advisory task {id}; overrides apply to implementer tasks only"
                    )));
                }
                Ok(task)
            };
            let consumer = named(&entry.consumer)?;
            let producer = named(&entry.producer)?;
            if consumer.id == producer.id || consumer.repository_id == producer.repository_id {
                return Err(DomainError::Invalid(format!(
                    "{label} must name two tasks in different repositories"
                )));
            }
            if entry.justification.trim().is_empty() || entry.justification.len() > 1000 {
                return Err(DomainError::Invalid(format!(
                    "{label} needs a justification (1 to 1000 bytes)"
                )));
            }
            if !pairs.insert((entry.consumer.as_str(), entry.producer.as_str())) {
                return Err(DomainError::Invalid(format!(
                    "dependency override for {} <- {} is listed more than once",
                    entry.consumer, entry.producer
                )));
            }
        }
        Ok(())
    }

    /// The task graph against the project's confirmed repository relationships
    /// (P44, ADR 0023). Advisory tasks change nothing, so only implementer
    /// tasks take part. For every confirmed `producer -> consumer`
    /// relationship whose two repositories both carry an implementer task,
    /// the consumer task depends on the producer task unless a `waive`
    /// override names the pair; a dependency between implementer tasks whose
    /// repositories have no confirmed relationship needs an `add` override;
    /// an override that applies to nothing is refused, so every recorded
    /// override is a real departure. Proposed (unconfirmed) relationships do
    /// not count. A refusal names the relationship and the override that
    /// would allow the plan.
    pub fn check_relationships(&self, relationships: &[Relationship]) -> Result<(), DomainError> {
        let implementers: BTreeMap<&str, &PlanTask> = self
            .tasks
            .iter()
            .filter(|task| !task.is_advisory())
            .map(|task| (task.repository_id.as_str(), task))
            .collect();
        let confirmed: Vec<&Relationship> = relationships.iter().filter(|r| r.confirmed).collect();
        let mut used: BTreeSet<(&str, &str)> = BTreeSet::new();
        let has_override = |consumer: &str, producer: &str, action: OverrideAction| {
            self.dependency_overrides
                .iter()
                .any(|o| o.consumer == consumer && o.producer == producer && o.action == action)
        };
        for relationship in &confirmed {
            let (Some(producer), Some(consumer)) = (
                implementers.get(relationship.producer.as_str()),
                implementers.get(relationship.consumer.as_str()),
            ) else {
                continue;
            };
            if consumer.depends_on.contains(&producer.id) {
                continue;
            }
            if has_override(&consumer.id, &producer.id, OverrideAction::Waive) {
                used.insert((consumer.id.as_str(), producer.id.as_str()));
                continue;
            }
            let kind = serde_json::to_value(relationship.kind)
                .ok()
                .and_then(|v| v.as_str().map(ToOwned::to_owned))
                .unwrap_or_default();
            return Err(DomainError::Invalid(format!(
                "task {consumer} in repository {consumer_repo} must depend on task {producer} in repository {producer_repo}: confirmed relationship {producer_repo} -> {consumer_repo} ({kind} {contract}); add the dependency, or waive it with dependency_overrides [{{\"consumer\": \"{consumer}\", \"producer\": \"{producer}\", \"action\": \"waive\", \"justification\": \"...\"}}]",
                consumer = consumer.id,
                consumer_repo = relationship.consumer,
                producer = producer.id,
                producer_repo = relationship.producer,
                contract = relationship.contract,
            )));
        }
        for consumer in implementers.values() {
            for dependency in &consumer.depends_on {
                let Some(producer) = implementers.values().find(|t| &t.id == dependency) else {
                    // An advisory dependency orders work; it binds nothing.
                    continue;
                };
                if confirmed.iter().any(|r| {
                    r.producer == producer.repository_id && r.consumer == consumer.repository_id
                }) {
                    continue;
                }
                if has_override(&consumer.id, &producer.id, OverrideAction::Add) {
                    used.insert((consumer.id.as_str(), producer.id.as_str()));
                    continue;
                }
                return Err(DomainError::Invalid(format!(
                    "task {consumer} in repository {consumer_repo} depends on task {producer} in repository {producer_repo}, but no confirmed relationship {producer_repo} -> {consumer_repo} exists; record it with dependency_overrides [{{\"consumer\": \"{consumer}\", \"producer\": \"{producer}\", \"action\": \"add\", \"justification\": \"...\"}}]",
                    consumer = consumer.id,
                    consumer_repo = consumer.repository_id,
                    producer = producer.id,
                    producer_repo = producer.repository_id,
                )));
            }
        }
        if let Some(unused) = self
            .dependency_overrides
            .iter()
            .find(|o| !used.contains(&(o.consumer.as_str(), o.producer.as_str())))
        {
            let why = match unused.action {
                OverrideAction::Waive => "the consumer already depends on the producer, or no confirmed relationship between their repositories implies the dependency",
                OverrideAction::Add => "the consumer does not depend on the producer, or a confirmed relationship between their repositories already covers the dependency",
            };
            return Err(DomainError::Invalid(format!(
                "dependency override {} {} <- {} does not apply: {why}",
                unused.action.as_str(),
                unused.consumer,
                unused.producer
            )));
        }
        Ok(())
    }

    /// Tasks in an order that runs every dependency before its dependents,
    /// stable with respect to the plan's own order among independent tasks.
    /// `Err` carries one dependency cycle (closed, so its first and last
    /// elements are the same task) when the graph has one.
    pub fn dependency_order(&self) -> Result<Vec<&PlanTask>, Vec<String>> {
        let mut ordered: Vec<&PlanTask> = Vec::with_capacity(self.tasks.len());
        let mut placed: BTreeSet<&str> = BTreeSet::new();
        while ordered.len() < self.tasks.len() {
            let before = ordered.len();
            for task in &self.tasks {
                if !placed.contains(task.id.as_str())
                    && task
                        .depends_on
                        .iter()
                        .all(|dependency| placed.contains(dependency.as_str()))
                {
                    placed.insert(task.id.as_str());
                    ordered.push(task);
                }
            }
            if ordered.len() == before {
                return Err(self.find_cycle(&placed));
            }
        }
        Ok(ordered)
    }

    /// One cycle among the tasks that could not be ordered: walk unplaced
    /// dependencies from the first stuck task until a task repeats.
    fn find_cycle(&self, placed: &BTreeSet<&str>) -> Vec<String> {
        let stuck: Vec<&PlanTask> = self
            .tasks
            .iter()
            .filter(|task| !placed.contains(task.id.as_str()))
            .collect();
        let Some(start) = stuck.first() else {
            return Vec::new();
        };
        let mut path: Vec<String> = vec![start.id.clone()];
        let mut current = *start;
        loop {
            let Some(next_id) = current.depends_on.iter().find(|dependency| {
                !placed.contains(dependency.as_str())
                    && stuck.iter().any(|task| &task.id == *dependency)
            }) else {
                // Every unplaced dependency was already reported or missing;
                // the path so far is the best description available.
                return path;
            };
            if let Some(position) = path.iter().position(|id| id == next_id) {
                let mut cycle = path.split_off(position);
                cycle.push(next_id.clone());
                return cycle;
            }
            path.push(next_id.clone());
            current = stuck
                .iter()
                .find(|task| &task.id == next_id)
                .expect("dependency is a stuck task");
        }
    }
}

/// Durable state of one plan task inside a workflow run (M4). A task is one
/// writer attempt in its own worktree; the run records every task's state so
/// a restart can show the graph and resume what was interrupted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Running,
    /// An in-scope candidate commit exists in the task's preserved worktree.
    Completed,
    Failed,
    /// The runtime stopped while the writer ran; resumable by task ID.
    Interrupted,
    /// A dependency failed, was blocked or was interrupted; never started.
    Blocked,
    Cancelled,
}

impl TaskState {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Queued | Self::Running)
    }
}

/// The environment variable prefix under which a consumer task's writer and
/// its repository's checks receive a completed producer's candidate worktree
/// root (`…_PATH`) and commit (`…_COMMIT`): the repository ID is uppercased
/// with dashes replaced by underscores.
pub fn dependency_variable(repository_id: &str, suffix: &str) -> String {
    let name: String = repository_id
        .chars()
        .map(|c| match c {
            '-' => '_',
            other => other.to_ascii_uppercase(),
        })
        .collect();
    format!("LUUNA_DEPENDENCY_{name}_{suffix}")
}

/// The `dependency_bindings` key of a candidate set for one consumer and
/// producer repository pair.
pub fn dependency_binding_key(consumer: &str, producer: &str) -> String {
    format!("{consumer}:{producer}")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub schema_version: String,
    pub id: String,
    pub plan_id: String,
    pub plan_revision: u64,
    pub plan_hash: String,
    pub approved_by: String,
    pub approved_at: String,
    pub permissions: Permissions,
}

impl Approval {
    pub fn authorize(&self, plan: &PlanRevision) -> Result<(), DomainError> {
        plan.validate()?;
        if self.schema_version != SCHEMA_VERSION
            || self.id.is_empty()
            || self.approved_by.is_empty()
            || self.approved_at.is_empty()
            || self.plan_id != plan.id
            || self.plan_revision != plan.revision
            || self.plan_hash != plan.hash
            || self.permissions != plan.permissions
        {
            return Err(DomainError::StaleApproval);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateSet {
    pub schema_version: String,
    pub id: String,
    pub revision: u64,
    pub plan_hash: String,
    pub repository_commits: BTreeMap<String, String>,
    pub dependency_bindings: BTreeMap<String, String>,
    pub hash: String,
}

impl CandidateSet {
    pub fn computed_hash(&self) -> Result<String, DomainError> {
        let mut value = serde_json::to_value(self)?;
        value
            .as_object_mut()
            .expect("candidate object")
            .remove("hash");
        hash_value(&value)
    }
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.schema_version != SCHEMA_VERSION
            || self.id.is_empty()
            || self.revision == 0
            || !is_hash(&self.plan_hash)
            || self.repository_commits.is_empty()
            || self
                .repository_commits
                .iter()
                .any(|(id, c)| id.is_empty() || !is_commit(c))
            || self.hash != self.computed_hash()?
        {
            return invalid("invalid candidate identity");
        }
        // A binding `<consumer>:<producer>` names two distinct repositories of
        // this set and the producer's exact commit, so the bindings the checks
        // and the review saw are the commits the set freezes.
        for (key, commit) in &self.dependency_bindings {
            let bound = key.split_once(':').filter(|(consumer, producer)| {
                consumer != producer
                    && self.repository_commits.contains_key(*consumer)
                    && self.repository_commits.get(*producer) == Some(commit)
            });
            if bound.is_none() {
                return Err(DomainError::Invalid(format!(
                    "candidate dependency binding {key} must name a consumer and a producer repository of this set and the producer's exact commit"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub schema_version: String,
    pub id: String,
    pub task_id: String,
    pub plan_hash: String,
    pub role: String,
    pub repository_id: String,
    pub environment_id: String,
    pub context_manifest: Vec<String>,
    pub tool_profile: String,
}

impl Assignment {
    pub fn validate_for(&self, plan: &PlanRevision) -> Result<(), DomainError> {
        plan.validate()?;
        let task = plan
            .tasks
            .iter()
            .find(|task| task.id == self.task_id)
            .ok_or_else(|| DomainError::Invalid("assignment references unknown task".into()))?;
        if self.schema_version != SCHEMA_VERSION
            || self.id.is_empty()
            || self.plan_hash != plan.hash
            || self.role != task.role
            || self.repository_id != task.repository_id
            || self.environment_id.is_empty()
            || self.context_manifest.is_empty()
            || self.context_manifest.iter().any(|s| s.is_empty())
            || self.tool_profile.is_empty()
        {
            return invalid("assignment does not match plan task or required execution context");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Blocking,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub severity: Severity,
    pub path: String,
    pub line: u64,
    pub explanation: String,
    pub evidence: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    Passed,
    ChangesRequired,
    Blocked,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewResult {
    pub schema_version: String,
    pub id: String,
    pub candidate_hash: String,
    pub reviewer_session: String,
    pub findings: Vec<Finding>,
    pub verdict: ReviewVerdict,
    pub evidence: Vec<String>,
}
impl ReviewResult {
    pub fn validate_for(
        &self,
        candidate: &CandidateSet,
        implementer_session: &str,
    ) -> Result<(), DomainError> {
        candidate.validate()?;
        if self.schema_version != SCHEMA_VERSION
            || self.id.is_empty()
            || self.candidate_hash != candidate.hash
            || self.reviewer_session.is_empty()
            || self.reviewer_session == implementer_session
        {
            return invalid("review must independently match the exact candidate and findings");
        }
        self.check_contract()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationState {
    Pending,
    Pushing,
    PrOpen,
    Failed,
    Integrated,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Publication {
    pub schema_version: String,
    pub ticket_id: String,
    pub candidate_hash: String,
    pub per_repository_branch: BTreeMap<String, String>,
    pub pull_request_urls: BTreeMap<String, String>,
    pub publication_state: PublicationState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventEnvelope {
    pub schema_version: String,
    pub event_id: String,
    pub sequence: u64,
    pub timestamp: String,
    pub project_id: String,
    pub run_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub payload: serde_json::Value,
}

pub(crate) fn invalid<T>(message: &str) -> Result<T, DomainError> {
    Err(DomainError::Invalid(message.into()))
}
pub(crate) fn unique<T: Ord>(values: &[T]) -> bool {
    values.iter().collect::<BTreeSet<_>>().len() == values.len()
}
pub(crate) fn valid_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains(':')
        && !path.bytes().any(|byte| byte < 32 || byte == 127)
        && path.split('/').all(|part| !part.is_empty() && part != "..")
}
/// Why `path` cannot be an approved scope path (a task path or permission
/// prefix), or `None` when it is canonical. Scope is compared as plain strings
/// with the paths Git reports, so each location has exactly one spelling: `.`
/// alone is the whole repository, and no component may be `.`, `..`, empty or
/// Git's repository metadata.
pub fn scope_path_problem(path: &str) -> Option<&'static str> {
    if path == "." {
        return None;
    }
    if path.is_empty() {
        return Some("must not be empty");
    }
    if path.starts_with('/') || path.contains('\\') {
        return Some("must be relative to the repository root and use '/' separators");
    }
    if path.contains(':') || path.bytes().any(|byte| byte < 32 || byte == 127) {
        return Some("must not contain ':' or control characters");
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.iter().any(|part| part.is_empty()) {
        return Some("must not contain empty components or a trailing '/'");
    }
    if parts.contains(&"..") {
        return Some("must not contain '..' components");
    }
    if parts.contains(&".") {
        return Some("must not contain '.' components (write \"src\", not \"./src\" or \"src/.\")");
    }
    if parts.iter().any(|part| is_repository_metadata(part)) {
        return Some("must not name repository metadata (.git)");
    }
    None
}
/// Whether one path component reaches Git's repository metadata as a
/// filesystem may resolve it: `.git` in any ASCII case, with the trailing dots
/// or spaces Windows ignores or an NTFS stream suffix, or its `GIT~1` short name.
pub fn is_repository_metadata(component: &str) -> bool {
    let name = component.split(':').next().unwrap_or(component);
    let name = name.trim_end_matches(['.', ' ']).to_ascii_lowercase();
    name == ".git" || name == "git~1"
}
pub(crate) fn path_within(path: &str, prefix: &str) -> bool {
    prefix == "."
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|p| p.starts_with('/'))
}
/// Whether `value` is an exact Git object identity (40 or 64 lowercase hex digits).
pub fn is_commit_id(value: &str) -> bool {
    is_commit(value)
}
pub(crate) fn is_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(crate) fn is_hash(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
/// Canonical luuna-json-v1 content identity, shared by runtime resource bindings.
pub fn hash_value(value: &serde_json::Value) -> Result<String, DomainError> {
    fn sorted(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let entries: BTreeMap<_, _> =
                    map.iter().map(|(k, v)| (k.clone(), sorted(v))).collect();
                serde_json::to_value(entries).expect("JSON object serialization")
            }
            serde_json::Value::Array(a) => serde_json::Value::Array(a.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    let bytes = serde_json::to_vec(&sorted(value))?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plan() -> PlanRevision {
        serde_json::from_str(include_str!("../../../fixtures/domain/plan.valid.json")).unwrap()
    }
    #[test]
    fn plan_purpose_is_hashed_only_when_it_is_not_the_feature_default() {
        // The fixture predates the purpose: its bytes and hash are unchanged.
        let plan = plan();
        assert_eq!(plan.purpose, TicketPurpose::Feature);
        plan.validate().unwrap();
        let bytes = serde_json::to_string(&plan).unwrap();
        assert!(!bytes.contains("purpose"), "{bytes}");
        let mut setup = plan.clone();
        setup.purpose = TicketPurpose::TestingSetup;
        setup.tasks[0].role = TicketPurpose::TestingSetup.role().into();
        let setup = setup.seal().unwrap();
        assert_ne!(setup.hash, plan.hash);
        assert!(serde_json::to_string(&setup)
            .unwrap()
            .contains("\"purpose\":\"testing_setup\""));
        let reread: PlanRevision =
            serde_json::from_str(&serde_json::to_string(&setup).unwrap()).unwrap();
        assert_eq!(reread.purpose, TicketPurpose::TestingSetup);
        reread.validate().unwrap();
    }

    #[test]
    fn testing_setup_plans_carry_one_setup_task_and_its_recipes() {
        let feature = plan();
        // A feature plan cannot borrow the role that the setup admission names.
        let mut disguised = feature.clone();
        disguised.tasks[0].role = "testing-setup".into();
        assert!(disguised
            .seal()
            .unwrap_err()
            .to_string()
            .contains("only a testing_setup plan"));
        let mut setup = feature.clone();
        setup.purpose = TicketPurpose::TestingSetup;
        assert!(setup
            .clone()
            .seal()
            .unwrap_err()
            .to_string()
            .contains("testing-setup role"));
        setup.tasks[0].role = "testing-setup".into();
        setup.clone().seal().unwrap();
        let mut repairing = setup.clone();
        repairing.candidate_hash = Some(format!("sha256:{}", "c".repeat(64)));
        assert!(repairing
            .seal()
            .unwrap_err()
            .to_string()
            .contains("does not repair"));
        let mut two_tasks = setup.clone();
        let mut second = two_tasks.tasks[0].clone();
        second.id = "second".into();
        two_tasks.tasks.push(second);
        two_tasks.budgets.max_workers = 1;
        assert!(two_tasks
            .seal()
            .unwrap_err()
            .to_string()
            .contains("exactly one task"));
        let mut elsewhere = setup;
        for check in &mut elsewhere.required_checks {
            check.required = false;
        }
        elsewhere.required_checks[0].required = true;
        elsewhere.required_checks[0].repository_id = "app".into();
        elsewhere.clone().seal().unwrap();
        elsewhere.required_checks[0].required = false;
        elsewhere.required_checks.push(CheckRecipe {
            id: "other".into(),
            repository_id: "lib".into(),
            executable: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_seconds: 60,
            required: true,
            capabilities: vec![],
            execution_profile: "native".into(),
        });
        elsewhere.base_commits.insert("lib".into(), "b".repeat(40));
        elsewhere
            .permissions
            .repositories
            .insert("lib".into(), vec!["src".into()]);
        assert!(elsewhere
            .seal()
            .unwrap_err()
            .to_string()
            .contains("required check recipe for its repository"));
    }

    #[test]
    fn approved_component_bytes_are_part_of_the_plan_hash() {
        let mut plan = plan();
        plan.component_context = Some(ComponentContext {
            memory: vec![MemoryReference {
                id: "mem-1".into(),
                scope: "project".into(),
                hash: format!("sha256:{}", "1".repeat(64)),
                content: "Selected content".into(),
                provenance: "Explicit fixture".into(),
            }],
            prompt: Some(ApprovedPrompt {
                name: "guidance".into(),
                version: 1,
                hash: format!("sha256:{}", "2".repeat(64)),
                body: "Selected guidance".into(),
            }),
        });
        let sealed = plan.seal().unwrap();
        let mut changed = sealed.clone();
        changed.component_context.as_mut().unwrap().memory[0]
            .content
            .push('!');
        assert_ne!(
            sealed.computed_hash().unwrap(),
            changed.computed_hash().unwrap()
        );
        assert!(changed.validate().is_err());
        let mut changed = sealed.clone();
        changed
            .component_context
            .as_mut()
            .unwrap()
            .prompt
            .as_mut()
            .unwrap()
            .body
            .push('!');
        assert!(changed.validate().is_err());
    }

    fn approval(plan: &PlanRevision) -> Approval {
        Approval {
            schema_version: SCHEMA_VERSION.into(),
            id: "approval-1".into(),
            plan_id: plan.id.clone(),
            plan_revision: plan.revision,
            plan_hash: plan.hash.clone(),
            approved_by: "human".into(),
            approved_at: "2026-10-16T00:00:00Z".into(),
            permissions: plan.permissions.clone(),
        }
    }
    #[test]
    fn fixture_plan_hash_matches() {
        plan().validate().unwrap();
    }
    #[test]
    fn rejects_invalid_transitions_and_terminal_restarts() {
        assert!(RunState::PlanReady
            .transition(RunState::Implementing)
            .is_err());
        assert!(RunState::Queued.transition(RunState::Completed).is_err());
        assert!(RunState::Completed.transition(RunState::Running).is_err());
        assert_eq!(
            RunState::Running.transition(RunState::Cancelled).unwrap(),
            RunState::Cancelled
        );
        // A fresh validation generation may follow an interrupted or failed
        // stage; paused, cancelled and reviewed candidates never re-enter it.
        assert!(RunState::Failed.transition(RunState::Validating).is_ok());
        assert!(RunState::Interrupted
            .transition(RunState::Validating)
            .is_ok());
        for state in [
            RunState::Paused,
            RunState::Cancelled,
            RunState::Reviewed,
            RunState::TestingBlocked,
        ] {
            assert!(state.transition(RunState::Validating).is_err(), "{state:?}");
        }
    }
    #[test]
    fn permissions_candidate_and_checks_are_hash_bound() {
        let original = plan();
        let mut changed = original.clone();
        changed.permissions.operations.push(Operation::Network);
        assert_ne!(original.hash, changed.computed_hash().unwrap());
        changed = original.clone();
        changed.base_commits.insert("app".into(), "b".repeat(40));
        assert_ne!(original.hash, changed.computed_hash().unwrap());
        changed = original.clone();
        changed.candidate_hash = Some(format!("sha256:{}", "c".repeat(64)));
        assert_ne!(original.hash, changed.computed_hash().unwrap());
        changed = original.clone();
        changed.required_checks[0].args.push("--ignored".into());
        assert_ne!(original.hash, changed.computed_hash().unwrap());
    }
    #[test]
    fn rejects_stale_revision_and_material_reapproval() {
        let original = plan();
        let approval = approval(&original);
        approval.authorize(&original).unwrap();
        let mut changed = original;
        changed.revision += 1;
        let changed = changed.seal().unwrap();
        assert!(matches!(
            approval.authorize(&changed),
            Err(DomainError::StaleApproval)
        ));
    }
    #[test]
    fn denies_permission_escalation_in_approval() {
        let plan = plan();
        let mut approval = approval(&plan);
        approval.permissions.operations.push(Operation::Publish);
        assert!(approval.authorize(&plan).is_err());
    }
    #[test]
    fn rejects_unknown_repositories_cycles_and_escape_paths() {
        let mut unknown = plan();
        unknown.tasks[0].repository_id = "other".into();
        assert!(unknown.seal().is_err());
        let mut cycle = plan();
        cycle.tasks[0].depends_on = vec![cycle.tasks[0].id.clone()];
        assert!(cycle.seal().is_err());
        let mut escape = plan();
        escape.tasks[0].paths = vec!["src/../../secret".into()];
        assert!(escape.seal().is_err());
        let mut sibling = plan();
        sibling
            .permissions
            .repositories
            .insert("app".into(), vec!["src".into()]);
        sibling.tasks[0].paths = vec!["src-escape".into()];
        assert!(sibling.seal().is_err());
    }
    #[test]
    fn scope_paths_have_one_canonical_spelling_and_never_name_repository_metadata() {
        for good in [
            ".",
            "src",
            "src/main.py",
            "docs/a b.md",
            ".github/workflows",
            "a.git",
        ] {
            let mut plan = plan();
            plan.permissions
                .repositories
                .insert("app".into(), vec![good.into()]);
            plan.tasks[0].paths = vec![good.into()];
            plan.seal().unwrap_or_else(|e| panic!("{good}: {e}"));
        }
        for bad in [
            "./src",
            "src/.",
            "a/./b",
            "src/",
            "src//x",
            "/src",
            "a\\b",
            "a:b",
            "",
            "..",
            ".git",
            "src/.git/x",
            ".GIT",
            ".git.",
            ".git ",
            "Git~1",
            "src/.Git/config",
        ] {
            // Permissions default to the task paths, so the error names the task.
            let mut plan = plan();
            plan.permissions
                .repositories
                .insert("app".into(), vec![bad.into()]);
            plan.tasks[0].paths = vec![bad.into()];
            let error = plan.seal().unwrap_err().to_string();
            assert!(
                error.contains(&format!("task implement path {bad:?}")),
                "{bad}: {error}"
            );
            let mut plan = self::plan();
            plan.permissions
                .repositories
                .insert("app".into(), vec![bad.into()]);
            let error = plan.seal().unwrap_err().to_string();
            assert!(
                error.contains(&format!("permission path {bad:?} for repository app")),
                "{bad}: {error}"
            );
        }
        for metadata in [
            ".git",
            ".GIT",
            ".git.",
            ".git ",
            "git~1",
            ".git::$INDEX_ALLOCATION",
        ] {
            assert!(is_repository_metadata(metadata), "{metadata}");
        }
        for ordinary in [".gitignore", ".github", "git", ".", "..", "a.git"] {
            assert!(!is_repository_metadata(ordinary), "{ordinary}");
        }
    }
    #[test]
    fn budgets_are_bounded_and_named_when_refused() {
        let at_limits = |edit: fn(&mut Budgets)| {
            let mut plan = plan();
            edit(&mut plan.budgets);
            plan.seal()
        };
        at_limits(|b| {
            b.max_attempts = MAX_PLAN_ATTEMPTS;
            b.timeout_seconds = MAX_PLAN_TIMEOUT_SECONDS;
            b.cost_usd_micros = MAX_PLAN_COST_USD_MICROS;
        })
        .unwrap();
        at_limits(|b| {
            b.max_attempts = 1;
            b.timeout_seconds = 1;
            b.cost_usd_micros = 1;
        })
        .unwrap();
        for (edit, field) in [
            (
                (|b: &mut Budgets| b.max_workers = 0) as fn(&mut Budgets),
                "budgets.max_workers",
            ),
            (|b| b.max_workers = 2, "budgets.max_workers"),
            (
                |b| b.max_attempts = 0,
                "budgets.max_attempts must be between 1 and 10",
            ),
            (
                |b| b.max_attempts = 11,
                "budgets.max_attempts must be between 1 and 10",
            ),
            (
                |b| b.timeout_seconds = 0,
                "budgets.timeout_seconds must be between 1 and 86400",
            ),
            (|b| b.timeout_seconds = 86_401, "budgets.timeout_seconds"),
            (|b| b.timeout_seconds = u64::MAX, "budgets.timeout_seconds"),
            (
                |b| b.cost_usd_micros = 0,
                "budgets.cost_usd_micros must be between 1 and 1000000000",
            ),
            (
                |b| b.cost_usd_micros = 1_000_000_001,
                "budgets.cost_usd_micros",
            ),
        ] {
            let error = at_limits(edit).unwrap_err().to_string();
            assert!(error.contains(field), "{field}: {error}");
        }
    }
    #[test]
    fn object_key_order_is_irrelevant_but_array_order_is_not() {
        let a = serde_json::json!({"z": {"b":2,"a":1}, "a":[1,2]});
        let b: serde_json::Value =
            serde_json::from_str(r#"{"a":[1,2],"z":{"a":1,"b":2}}"#).unwrap();
        assert_eq!(hash_value(&a).unwrap(), hash_value(&b).unwrap());
        assert_ne!(
            hash_value(&a).unwrap(),
            hash_value(&serde_json::json!({"a":[2,1],"z":{"a":1,"b":2}})).unwrap()
        );
    }
    #[test]
    fn model_claims_do_not_advance_domain_state() {
        let _: EventEnvelope = serde_json::from_value(serde_json::json!({"schema_version":"0.1", "event_id":"e", "sequence":1,
            "timestamp":"2026-10-16T00:00:00Z", "project_id":"p", "run_id":"r", "type":"worker.output", "payload":{"text":"all tests passed"}})).unwrap();
        assert!(RunState::Running.transition(RunState::Reviewed).is_err());
    }

    #[test]
    fn candidate_changes_invalidate_review_and_blocking_findings_cannot_pass() {
        let mut candidate: CandidateSet = serde_json::from_str(include_str!(
            "../../../fixtures/domain/candidate.valid.json"
        ))
        .unwrap();
        let mut review: ReviewResult =
            serde_json::from_str(include_str!("../../../fixtures/domain/review.valid.json"))
                .unwrap();
        review.validate_for(&candidate, "implementer-1").unwrap();
        assert!(review
            .validate_for(&candidate, &review.reviewer_session)
            .is_err());
        // A binding names a producer of the set and its exact commit (M4).
        candidate
            .repository_commits
            .insert("api".into(), "c".repeat(40));
        candidate
            .dependency_bindings
            .insert("app:api".into(), "c".repeat(40));
        candidate.hash = candidate.computed_hash().unwrap();
        assert!(review.validate_for(&candidate, "implementer-1").is_err());
        review.candidate_hash = candidate.hash.clone();
        review.findings.push(Finding {
            severity: Severity::Blocking,
            path: "src/main.rs".into(),
            line: 1,
            explanation: "Known regression".into(),
            evidence: vec!["artifact:failed.log".into()],
        });
        assert!(review.validate_for(&candidate, "implementer-1").is_err());
        review.verdict = ReviewVerdict::ChangesRequired;
        review.validate_for(&candidate, "implementer-1").unwrap();
    }

    #[test]
    fn assignment_requires_matching_plan_task_and_repository() {
        let plan = plan();
        let mut assignment: Assignment = serde_json::from_str(include_str!(
            "../../../fixtures/domain/assignment.valid.json"
        ))
        .unwrap();
        assignment.validate_for(&plan).unwrap();
        assignment.repository_id = "outside-scope".into();
        assert!(assignment.validate_for(&plan).is_err());
    }
}

#[cfg(test)]
mod task_graph_tests {
    use super::*;

    fn plan() -> PlanRevision {
        serde_json::from_str(include_str!("../../../fixtures/domain/plan.valid.json")).unwrap()
    }
    /// A plan whose tasks are `a`, `b` and `c` in one repository, with the
    /// given dependencies; the second repository `lib` is added for `c`.
    fn graph(deps: &[(&str, &[&str])]) -> PlanRevision {
        let mut plan = plan();
        let template = plan.tasks[0].clone();
        plan.base_commits.insert("lib".into(), "b".repeat(40));
        plan.permissions
            .repositories
            .insert("lib".into(), vec!["src".into()]);
        plan.tasks = deps
            .iter()
            .map(|(id, depends_on)| PlanTask {
                id: (*id).to_owned(),
                repository_id: if *id == "c" {
                    "lib".into()
                } else {
                    "app".into()
                },
                depends_on: depends_on.iter().map(|d| (*d).to_owned()).collect(),
                ..template.clone()
            })
            .collect();
        plan
    }

    #[test]
    fn single_task_plans_hash_exactly_as_before() {
        // The fixture hash was sealed before M4: the graph rules add no field.
        let plan = plan();
        plan.validate().unwrap();
        assert_eq!(plan.computed_hash().unwrap(), plan.hash);
    }

    #[test]
    fn a_dag_is_ordered_with_dependencies_first_and_workers_bounded_by_tasks() {
        let mut plan = graph(&[("c", &["b"]), ("a", &[]), ("b", &["a"])]);
        plan.budgets.max_workers = 3;
        let sealed = plan.seal().unwrap();
        let order: Vec<&str> = sealed
            .dependency_order()
            .unwrap()
            .iter()
            .map(|task| task.id.as_str())
            .collect();
        assert_eq!(order, ["a", "b", "c"]);
        let mut too_many = sealed.clone();
        too_many.budgets.max_workers = 4;
        let error = too_many.seal().unwrap_err().to_string();
        assert!(
            error.contains("budgets.max_workers must be between 1 and 3"),
            "{error}"
        );
        let mut single = plan_with_one_task();
        single.budgets.max_workers = 2;
        assert!(single
            .seal()
            .unwrap_err()
            .to_string()
            .contains("between 1 and 1"));
    }

    fn plan_with_one_task() -> PlanRevision {
        plan()
    }

    #[test]
    fn self_unknown_duplicate_and_cyclic_dependencies_are_named() {
        let error = graph(&[("a", &["a"])]).seal().unwrap_err().to_string();
        assert!(error.contains("task a depends on itself"), "{error}");
        let error = graph(&[("a", &["zzz"])]).seal().unwrap_err().to_string();
        assert!(
            error.contains("task a depends on unknown task zzz"),
            "{error}"
        );
        let error = graph(&[("a", &[]), ("b", &["a", "a"])])
            .seal()
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("task b lists dependency a more than once"),
            "{error}"
        );
        let error = graph(&[("a", &["c"]), ("b", &["a"]), ("c", &["b"])])
            .seal()
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("task dependencies contain a cycle: a -> c -> b -> a"),
            "{error}"
        );
        // A task on a repository outside the permissions is refused as before.
        let mut outside = graph(&[("a", &[])]);
        outside.tasks[0].repository_id = "other".into();
        assert!(outside
            .seal()
            .unwrap_err()
            .to_string()
            .contains("unknown repository"));
    }

    #[test]
    fn dependency_bindings_must_name_set_repositories_and_exact_producer_commits() {
        let mut candidate: CandidateSet = serde_json::from_str(include_str!(
            "../../../fixtures/domain/candidate.valid.json"
        ))
        .unwrap();
        candidate.validate().unwrap();
        candidate
            .repository_commits
            .insert("lib".into(), "c".repeat(40));
        candidate
            .dependency_bindings
            .insert(dependency_binding_key("app", "lib"), "c".repeat(40));
        candidate.hash = candidate.computed_hash().unwrap();
        candidate.validate().unwrap();
        for (key, commit) in [
            ("app:lib", "d".repeat(40)),
            ("app:app", "b".repeat(40)),
            ("app:zzz", "c".repeat(40)),
            ("lib", "c".repeat(40)),
        ] {
            let mut wrong = candidate.clone();
            wrong.dependency_bindings = BTreeMap::from([(key.to_owned(), commit)]);
            wrong.hash = wrong.computed_hash().unwrap();
            let error = wrong.validate().unwrap_err().to_string();
            assert!(error.contains("dependency binding"), "{key}: {error}");
        }
        assert_eq!(
            dependency_variable("shared-lib", "PATH"),
            "LUUNA_DEPENDENCY_SHARED_LIB_PATH"
        );
        assert_eq!(
            dependency_variable("api", "COMMIT"),
            "LUUNA_DEPENDENCY_API_COMMIT"
        );
    }
}

#[cfg(test)]
mod role_and_override_tests {
    use super::*;

    fn plan() -> PlanRevision {
        serde_json::from_str(include_str!("../../../fixtures/domain/plan.valid.json")).unwrap()
    }

    /// `(id, repository, role, depends_on)` tasks over repositories `app`,
    /// `lib` and `web`.
    fn graph(tasks: &[(&str, &str, &str, &[&str])]) -> PlanRevision {
        let mut plan = plan();
        let template = plan.tasks[0].clone();
        for repository in ["lib", "web"] {
            plan.base_commits.insert(repository.into(), "b".repeat(40));
            plan.permissions
                .repositories
                .insert(repository.into(), vec!["src".into()]);
        }
        plan.tasks = tasks
            .iter()
            .map(|(id, repository, role, depends_on)| PlanTask {
                id: (*id).to_owned(),
                repository_id: (*repository).to_owned(),
                role: (*role).to_owned(),
                operations: if TaskRole::parse(role).is_some_and(TaskRole::is_advisory) {
                    vec![Operation::Read]
                } else {
                    template.operations.clone()
                },
                depends_on: depends_on.iter().map(|d| (*d).to_owned()).collect(),
                ..template.clone()
            })
            .collect();
        plan
    }

    fn relationship(producer: &str, consumer: &str, confirmed: bool) -> Relationship {
        Relationship {
            producer: producer.into(),
            consumer: consumer.into(),
            kind: RelationshipKind::Package,
            contract: format!("npm:@fixture/{producer}"),
            confirmed,
            evidence: vec![],
        }
    }

    fn waive(consumer: &str, producer: &str) -> DependencyOverride {
        DependencyOverride {
            consumer: consumer.into(),
            producer: producer.into(),
            action: OverrideAction::Waive,
            justification: "independent this time".into(),
        }
    }

    fn add(consumer: &str, producer: &str) -> DependencyOverride {
        DependencyOverride {
            consumer: consumer.into(),
            producer: producer.into(),
            action: OverrideAction::Add,
            justification: "shares a generated schema".into(),
        }
    }

    #[test]
    fn a_default_role_task_serializes_exactly_as_before() {
        // The fixture already names `implementer`; the new optional fields
        // stay absent, so the sealed hash is the one the fixture records.
        let plan = plan();
        plan.validate().unwrap();
        let value = serde_json::to_value(&plan).unwrap();
        assert!(value.get("dependency_overrides").is_none());
        assert!(value["tasks"][0].get("derived_from_task").is_none());
        assert_eq!(plan.computed_hash().unwrap(), plan.hash);
        assert_eq!(plan.tasks[0].role(), Some(TaskRole::Implementer));
        assert!(!plan.tasks[0].is_advisory());
    }

    #[test]
    fn roles_are_explicit_and_advisory_roles_read_only() {
        let error = graph(&[("a", "app", "wizard", &[])])
            .seal()
            .unwrap_err()
            .to_string();
        assert!(error.contains("role \"wizard\" is not one of"), "{error}");
        let mut writing = graph(&[("a", "app", "project_orchestrator", &[])]);
        writing.tasks[0].operations = vec![Operation::Read, Operation::Write];
        let error = writing.seal().unwrap_err().to_string();
        assert!(
            error.contains(
                "advisory role project_orchestrator and must carry exactly the read operation"
            ),
            "{error}"
        );
        let sealed = graph(&[
            ("plan", "app", "project_orchestrator", &[]),
            ("lead", "lib", "repository_lead", &["plan"]),
            ("a", "app", "implementer", &["plan"]),
            ("b", "lib", "implementer", &["lead", "a"]),
        ])
        .seal()
        .unwrap();
        assert!(sealed.tasks[0].is_advisory() && sealed.tasks[1].is_advisory());
        let value = serde_json::to_value(&sealed).unwrap();
        assert_eq!(value["tasks"][0]["role"], "project_orchestrator");
        assert_eq!(
            TaskRole::parse("repository_lead"),
            Some(TaskRole::RepositoryLead)
        );
        assert_eq!(TaskRole::parse("Implementer"), None);
    }

    #[test]
    fn a_task_never_derives_from_itself_or_from_a_task_of_the_same_plan() {
        let mut own = graph(&[("a", "app", "implementer", &[])]);
        own.tasks[0].derived_from_task = Some("a".into());
        let error = own.seal().unwrap_err().to_string();
        assert!(error.contains("task a derives from itself"), "{error}");
        let mut same = graph(&[
            ("plan", "app", "project_orchestrator", &[]),
            ("b", "lib", "implementer", &["plan"]),
        ]);
        same.tasks[1].derived_from_task = Some("plan".into());
        let error = same.seal().unwrap_err().to_string();
        assert!(
            error.contains("derives from task plan of the same plan"),
            "{error}"
        );
        let mut empty = graph(&[("a", "app", "implementer", &[])]);
        empty.tasks[0].derived_from_task = Some(String::new());
        assert!(empty.seal().is_err());
        // Deriving from an earlier revision's advisory task is the trace.
        let mut earlier = graph(&[("a", "app", "implementer", &[])]);
        earlier.tasks[0].derived_from_task = Some("orchestrate".into());
        let sealed = earlier.seal().unwrap();
        let value = serde_json::to_value(&sealed).unwrap();
        assert_eq!(value["tasks"][0]["derived_from_task"], "orchestrate");
        let reread: PlanRevision = serde_json::from_value(value).unwrap();
        reread.validate().unwrap();
    }

    #[test]
    fn override_entries_name_implementer_tasks_once_with_a_justification() {
        let base = graph(&[
            ("a", "app", "implementer", &[]),
            ("b", "lib", "implementer", &[]),
            ("lead", "web", "repository_lead", &[]),
        ]);
        let cases: Vec<(Vec<DependencyOverride>, &str)> = vec![
            (vec![waive("b", "zzz")], "names unknown task zzz"),
            (vec![waive("b", "lead")], "names advisory task lead"),
            (vec![waive("b", "b")], "two tasks in different repositories"),
            (
                vec![DependencyOverride {
                    justification: "  ".into(),
                    ..waive("b", "a")
                }],
                "needs a justification",
            ),
            (
                vec![waive("b", "a"), add("b", "a")],
                "listed more than once",
            ),
        ];
        for (overrides, expected) in cases {
            let mut plan = base.clone();
            plan.dependency_overrides = overrides;
            let error = plan.seal().unwrap_err().to_string();
            assert!(error.contains(expected), "{expected}: {error}");
        }
        let mut plan = base.clone();
        plan.dependency_overrides = vec![waive("b", "a")];
        let sealed = plan.seal().unwrap();
        let value = serde_json::to_value(&sealed).unwrap();
        assert_eq!(value["dependency_overrides"][0]["action"], "waive");
        // The overrides are part of the hashed plan.
        let mut changed = sealed.clone();
        changed.dependency_overrides[0].justification = "another reason".into();
        assert_ne!(changed.computed_hash().unwrap(), sealed.hash);
        assert!(changed.validate().is_err());
    }

    #[test]
    fn confirmed_relationships_require_the_dependency_unless_waived() {
        let relationships = [
            relationship("app", "lib", true),
            relationship("lib", "web", false),
        ];
        let missing = graph(&[
            ("a", "app", "implementer", &[]),
            ("b", "lib", "implementer", &[]),
        ])
        .seal()
        .unwrap();
        let error = missing
            .check_relationships(&relationships)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("task b in repository lib must depend on task a in repository app"),
            "{error}"
        );
        assert!(
            error.contains("confirmed relationship app -> lib (package npm:@fixture/app)"),
            "{error}"
        );
        assert!(error.contains("\"action\": \"waive\""), "{error}");
        let mut waived = missing.clone();
        waived.dependency_overrides = vec![waive("b", "a")];
        waived
            .seal()
            .unwrap()
            .check_relationships(&relationships)
            .unwrap();
        let with_edge = graph(&[
            ("a", "app", "implementer", &[]),
            ("b", "lib", "implementer", &["a"]),
        ])
        .seal()
        .unwrap();
        with_edge.check_relationships(&relationships).unwrap();
        // A waive that applies to nothing is refused rather than recorded.
        let mut idle = with_edge.clone();
        idle.dependency_overrides = vec![waive("b", "a")];
        let error = idle
            .seal()
            .unwrap()
            .check_relationships(&relationships)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("dependency override waive b <- a does not apply"),
            "{error}"
        );
        // An unconfirmed relationship implies nothing.
        graph(&[
            ("b", "lib", "implementer", &[]),
            ("c", "web", "implementer", &[]),
        ])
        .seal()
        .unwrap()
        .check_relationships(&relationships)
        .unwrap();
        // Advisory tasks take no part: an orchestrator in the producer's
        // repository implies no dependency, and depending on it needs no override.
        graph(&[
            ("plan", "app", "project_orchestrator", &[]),
            ("b", "lib", "implementer", &["plan"]),
        ])
        .seal()
        .unwrap()
        .check_relationships(&relationships)
        .unwrap();
    }

    #[test]
    fn a_dependency_without_a_confirmed_relationship_needs_an_add_override() {
        let relationships = [relationship("app", "lib", true)];
        let unrelated = graph(&[
            ("a", "app", "implementer", &[]),
            ("b", "lib", "implementer", &["a"]),
            ("c", "web", "implementer", &["a"]),
        ])
        .seal()
        .unwrap();
        let error = unrelated
            .check_relationships(&relationships)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("task c in repository web depends on task a in repository app, but no confirmed relationship app -> web exists"),
            "{error}"
        );
        assert!(error.contains("\"action\": \"add\""), "{error}");
        let mut added = unrelated.clone();
        added.dependency_overrides = vec![add("c", "a")];
        added
            .seal()
            .unwrap()
            .check_relationships(&relationships)
            .unwrap();
        // An add override for a pair the confirmed graph already covers does
        // not apply.
        let mut covered = unrelated.clone();
        covered.dependency_overrides = vec![add("b", "a"), add("c", "a")];
        let error = covered
            .seal()
            .unwrap()
            .check_relationships(&relationships)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("dependency override add b <- a does not apply"),
            "{error}"
        );
    }

    #[test]
    fn advisory_results_are_bounded_structured_data() {
        let parsed = AdvisoryResult::parse(
            "Here is my advice:\n```json\n{\"findings\": [\"the consumer imports helper\"], \"proposed_tasks\": [{\"id\": \"api\", \"repository_id\": \"api\", \"paths\": [\"src\"], \"acceptance_criteria\": [\"tests pass\"], \"rationale\": \"first\"}], \"risks\": [\"rename breaks consumer\"]}\n```\n",
        )
        .unwrap();
        assert_eq!(parsed.findings.len(), 1);
        assert_eq!(parsed.proposed_tasks[0].role, "implementer");
        assert_eq!(parsed.risks[0], "rename breaks consumer");
        for text in [
            "no object here",
            "{\"findings\": [\"\"]}",
            "{\"proposed_tasks\": [{\"id\": \"\", \"repository_id\": \"api\"}]}",
            "{\"proposed_tasks\": [{\"id\": \"a\", \"repository_id\": \"api\", \"role\": \"wizard\"}]}",
            "{\"proposed_tasks\": [{\"id\": \"a\", \"repository_id\": \"api\", \"paths\": [\"../x\"]}]}",
            "{\"findings\": [], \"other\": 1}",
        ] {
            let error = AdvisoryResult::parse(text).unwrap_err().to_string();
            assert!(error.contains("advisory") || error.contains("proposed task"), "{text}: {error}");
        }
        let huge = AdvisoryResult {
            findings: vec!["x".repeat(MAX_ADVISORY_BYTES)],
            ..Default::default()
        };
        assert!(huge.validate().unwrap_err().to_string().contains("64000"));
        assert_eq!(
            serde_json::to_value(AdvisoryResult::default()).unwrap(),
            serde_json::json!({"findings": [], "proposed_tasks": [], "risks": []})
        );
    }

    #[test]
    fn an_advisory_only_plan_may_complete_without_a_candidate() {
        assert_eq!(
            RunState::Implementing
                .transition(RunState::Completed)
                .unwrap(),
            RunState::Completed
        );
    }
}
