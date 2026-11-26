//! Ticket, worktree lease and writer-scope contracts (M2c).
//!
//! A ticket is the unit of intent a plan binds to. Explicitly enabled projects
//! use Missions as their authority; legacy projects retain a clearly marked
//! temporary SQLite source until migration (ADR 0003). A lease
//! records Luuna's ownership of one detached worktree for one run. Scope
//! checking is a deterministic runtime decision over the paths a writer
//! actually changed; a prompt promising to obey paths is never evidence.

use crate::{
    invalid, is_commit, path_within, scope_path_problem, unique, Budgets, CheckRecipe, DomainError,
    ManifestCheck, Operation, Permissions, PlanTask, RepositoryManifest, SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};

/// Where a ticket's intent is authoritative. Migration preserves identifiers
/// and revisions while changing authority to Missions (ADR 0003).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketSource {
    LuunaSqliteTemporary,
    /// The compatible Missions record is authoritative; SQLite is its projection.
    Missions,
}

/// What a ticket, and every plan made for it, is for. `Feature` is the
/// default and is omitted from serialization, so records written before the
/// purpose existed keep their bytes and hashes. A `TestingSetup` ticket is the
/// prerequisite workflow of docs/08-validation.md: its plan carries the
/// testing-setup role and the check recipes that will become the repository's
/// baseline, and it is the only plan admitted to implementation without a
/// passing baseline (ADR 0022).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketPurpose {
    #[default]
    Feature,
    TestingSetup,
}

impl TicketPurpose {
    pub fn is_feature(&self) -> bool {
        *self == Self::Feature
    }

    /// The process role (`agents/<role>.md`) a plan task of this purpose carries.
    pub fn role(&self) -> &'static str {
        match self {
            Self::Feature => "implementer",
            Self::TestingSetup => "testing-setup",
        }
    }
}

/// The kind of work a ticket names, as the Missions `type` field names it.
/// `Task` is the default and is omitted from serialization, so records
/// written before the field existed keep their bytes (ADR 0058).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketType {
    #[default]
    Task,
    Bug,
    Feature,
    Epic,
}

impl TicketType {
    pub const ALL: [TicketType; 4] = [Self::Task, Self::Bug, Self::Feature, Self::Epic];
    pub fn is_task(&self) -> bool {
        *self == Self::Task
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Bug => "bug",
            Self::Feature => "feature",
            Self::Epic => "epic",
        }
    }
    /// `task`, `bug`, `feature` or `epic`, ignoring case.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str().eq_ignore_ascii_case(value.trim()))
    }
}

/// Missions priorities run from 0 (critical) to 4 (backlog); 2 is the default.
pub const DEFAULT_TICKET_PRIORITY: u8 = 2;
pub const MAX_TICKET_PRIORITY: u8 = 4;
pub const MAX_TICKET_LABELS: usize = 16;
pub const MAX_TICKET_LABEL_BYTES: usize = 64;
/// Luuna marks the Missions record it delivers with `luuna-ticket-<id>`;
/// a user label never takes that form.
pub const RESERVED_LABEL_PREFIX: &str = "luuna-ticket-";

fn default_priority() -> u8 {
    DEFAULT_TICKET_PRIORITY
}
fn is_default_priority(priority: &u8) -> bool {
    *priority == DEFAULT_TICKET_PRIORITY
}

/// `0`..`4` or `P0`..`P4`, as Missions writes them.
pub fn parse_ticket_priority(value: &str) -> Option<u8> {
    let value = value.trim();
    let digits = value
        .strip_prefix('P')
        .or_else(|| value.strip_prefix('p'))
        .unwrap_or(value);
    digits
        .parse::<u8>()
        .ok()
        .filter(|p| *p <= MAX_TICKET_PRIORITY && digits.len() == 1)
}

/// One label: 1..64 bytes, lowercase (Missions lowercases every label it
/// stores), no whitespace or comma, and not Luuna's own marker.
pub fn valid_ticket_label(label: &str) -> bool {
    !label.is_empty()
        && label.to_lowercase() == label
        && label.len() <= MAX_TICKET_LABEL_BYTES
        && !label
            .chars()
            .any(|c| c.is_whitespace() || c == ',' || c.is_control())
        && !label.starts_with(RESERVED_LABEL_PREFIX)
}

pub const MAX_TICKET_TITLE_BYTES: usize = 200;
pub const MAX_TICKET_INTENT_BYTES: usize = 16_000;
pub const MAX_ACCEPTANCE_CRITERIA: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ticket {
    pub schema_version: String,
    pub id: String,
    pub project_id: String,
    /// Source revision a plan binds to; external edits bump it and stale plans fail closed.
    pub revision: u64,
    pub source: TicketSource,
    pub title: String,
    pub intent: String,
    pub acceptance_criteria: Vec<String>,
    /// Registered repositories the ticket may touch.
    pub repository_ids: Vec<String>,
    /// Absent (feature) preserves the bytes of earlier records.
    #[serde(default, skip_serializing_if = "TicketPurpose::is_feature")]
    pub purpose: TicketPurpose,
    /// Missions `type`; absent (task) preserves the bytes of earlier records.
    #[serde(rename = "type", default, skip_serializing_if = "TicketType::is_task")]
    pub ticket_type: TicketType,
    /// 0 (critical) to 4 (backlog); absent means 2.
    #[serde(
        default = "default_priority",
        skip_serializing_if = "is_default_priority"
    )]
    pub priority: u8,
    /// Free labels for sorting and filtering; never Luuna's delivery marker.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl Ticket {
    /// Whether two tickets carry the same developer-written content: every
    /// field a create or an edit can set, ignoring identity, revision,
    /// source and timestamps.
    pub fn same_content(&self, other: &Ticket) -> bool {
        self.title == other.title
            && self.intent == other.intent
            && self.acceptance_criteria == other.acceptance_criteria
            && self.repository_ids == other.repository_ids
            && self.ticket_type == other.ticket_type
            && self.priority == other.priority
            && self.labels == other.labels
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        if self.schema_version != SCHEMA_VERSION {
            return invalid("unsupported ticket schema version");
        }
        if self.id.trim().is_empty() || self.project_id.trim().is_empty() || self.revision == 0 {
            return invalid("ticket identity, project and positive revision are required");
        }
        let title = self.title.trim();
        if title.is_empty() || title.len() > MAX_TICKET_TITLE_BYTES || title != self.title {
            return invalid("ticket title must be 1..200 bytes without surrounding whitespace");
        }
        if self.intent.trim().is_empty() || self.intent.len() > MAX_TICKET_INTENT_BYTES {
            return invalid("ticket intent must be 1..16000 bytes");
        }
        if self.acceptance_criteria.is_empty()
            || self.acceptance_criteria.len() > MAX_ACCEPTANCE_CRITERIA
            || self
                .acceptance_criteria
                .iter()
                .any(|c| c.trim().is_empty() || c.len() > MAX_TICKET_INTENT_BYTES)
        {
            return invalid("ticket needs 1..32 nonempty acceptance criteria");
        }
        if self.repository_ids.is_empty()
            || !unique(&self.repository_ids)
            || self.repository_ids.iter().any(|id| id.trim().is_empty())
        {
            return invalid("ticket needs unique nonempty repository IDs");
        }
        if self.priority > MAX_TICKET_PRIORITY {
            return invalid("ticket priority must be 0 (critical) to 4 (backlog)");
        }
        if self.labels.len() > MAX_TICKET_LABELS
            || !unique(&self.labels)
            || !self.labels.iter().all(|label| valid_ticket_label(label))
        {
            return invalid("ticket labels must be at most 16 unique lowercase 1..64-byte names without spaces or commas, and none may start with luuna-ticket-");
        }
        if self.created_at.is_empty() || self.updated_at.is_empty() {
            return invalid("ticket timestamps are required");
        }
        Ok(())
    }
}

/// Lifecycle of Luuna's ownership of one detached worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseState {
    /// A writer may run in the worktree; no other writer may lease this repository.
    Active,
    /// The attempt settled; files are preserved for inspection or validation.
    Released,
    /// The runtime stopped while the lease was active; files are preserved,
    /// evidence from the attempt is void.
    Abandoned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeLease {
    pub id: String,
    pub run_id: String,
    pub project_id: String,
    pub repository_id: String,
    /// Absolute path under the private data directory; never the developer checkout.
    pub path: String,
    pub base_commit: String,
    pub state: LeaseState,
    /// Commit recorded in the worktree when an in-scope attempt completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_commit: Option<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl WorktreeLease {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.id.is_empty()
            || self.run_id.is_empty()
            || self.project_id.is_empty()
            || self.repository_id.is_empty()
            || self.path.is_empty()
            || !is_commit(&self.base_commit)
            || self
                .candidate_commit
                .as_deref()
                .is_some_and(|c| !is_commit(c))
            || self.created_at.is_empty()
        {
            return invalid("invalid worktree lease");
        }
        Ok(())
    }
}

/// Lifecycle of one writer attempt inside a leased worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptState {
    Running,
    /// In-scope changes were committed as a candidate in the worktree.
    Completed,
    Failed,
    Cancelled,
    /// The runtime stopped mid-attempt; the worktree is preserved, nothing is evidence.
    Interrupted,
}

impl AttemptState {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

/// One path a writer changed in its worktree, as reported by the repository,
/// never by the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedPath {
    pub path: String,
    /// Porcelain status such as `M`, `A`, `D`, `R` or `?`.
    pub status: String,
    /// Original path of a rename or copy; it must also be inside scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_path: Option<String>,
}

/// Reject any change outside the task's declared paths, outside the plan's
/// permitted prefixes for that repository, or without the `write` operation.
pub fn check_scope(
    changes: &[ChangedPath],
    task: &PlanTask,
    permissions: &Permissions,
) -> Result<(), DomainError> {
    let violation = |message: String| Err(DomainError::ScopeViolation(message));
    if changes.is_empty() {
        return Ok(());
    }
    if !task.operations.contains(&Operation::Write) {
        return violation(format!(
            "task {} is not permitted to write but changed {} path(s)",
            task.id,
            changes.len()
        ));
    }
    let Some(permitted) = permissions.repositories.get(&task.repository_id) else {
        return violation(format!(
            "repository {} is outside the approved permissions",
            task.repository_id
        ));
    };
    for change in changes {
        for path in std::iter::once(&change.path).chain(change.original_path.iter()) {
            // A changed path is never the whole repository, and repository
            // metadata is never task content, whatever the approved scope.
            if let Some(problem) =
                scope_path_problem(path).or((path == ".").then_some("is not a file"))
            {
                return violation(format!("changed path {path:?} {problem}"));
            }
            if !task.paths.iter().any(|prefix| path_within(path, prefix)) {
                return violation(format!(
                    "changed path {path} is outside task {} paths {:?}",
                    task.id, task.paths
                ));
            }
            if !permitted.iter().any(|prefix| path_within(path, prefix)) {
                return violation(format!(
                    "changed path {path} is outside the approved permissions for repository {}",
                    task.repository_id
                ));
            }
        }
    }
    Ok(())
}

/// Operations this runtime can actually grant a writer: Pi's file tools. The
/// contract keeps `shell`, `network` and `publish` for later milestones, but a
/// plan must not record authority that nothing enforces or implements.
pub const GRANTED_OPERATIONS: [Operation; 2] = [Operation::Read, Operation::Write];

pub fn require_granted_operations(operations: &[Operation]) -> Result<(), DomainError> {
    match operations
        .iter()
        .find(|op| !GRANTED_OPERATIONS.contains(op))
    {
        Some(op) => Err(DomainError::Invalid(format!(
            "operation {} is not granted by this runtime; a writer task may only read and write files",
            format!("{op:?}").to_lowercase()
        ))),
        None => Ok(()),
    }
}

/// Resources an attempt has consumed, measured by the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BudgetUsage {
    pub attempts: u32,
    pub elapsed_seconds: u64,
    pub cost_usd_micros: u64,
    pub workers: u32,
}

impl Budgets {
    /// Fail closed the moment any declared budget is exceeded.
    pub fn check(&self, usage: BudgetUsage) -> Result<(), DomainError> {
        let exceeded = |message: String| Err(DomainError::BudgetExceeded(message));
        if usage.attempts > self.max_attempts {
            return exceeded(format!(
                "attempt {} exceeds max_attempts {}",
                usage.attempts, self.max_attempts
            ));
        }
        if usage.workers > self.max_workers {
            return exceeded(format!(
                "{} workers exceed max_workers {}",
                usage.workers, self.max_workers
            ));
        }
        if usage.elapsed_seconds > self.timeout_seconds {
            return exceeded(format!(
                "{}s exceeds timeout_seconds {}",
                usage.elapsed_seconds, self.timeout_seconds
            ));
        }
        if usage.cost_usd_micros > self.cost_usd_micros {
            return exceeded(format!(
                "cost {} µUSD exceeds cost_usd_micros {}",
                usage.cost_usd_micros, self.cost_usd_micros
            ));
        }
        Ok(())
    }
}

impl CheckRecipe {
    /// A plan's required check is the confirmed manifest recipe, never client text.
    pub fn from_manifest(repository: &RepositoryManifest, check: &ManifestCheck) -> Self {
        Self {
            id: check.id.clone(),
            repository_id: repository.id.clone(),
            executable: check.executable.clone(),
            args: check.args.clone(),
            cwd: check.cwd.clone(),
            timeout_seconds: check.timeout_seconds,
            required: check.required,
            capabilities: check.capabilities.clone(),
            execution_profile: serde_json::to_value(repository.execution)
                .ok()
                .and_then(|v| v.as_str().map(ToOwned::to_owned))
                .unwrap_or_else(|| "native".into()),
        }
    }

    /// The recipe as a manifest check: what a testing-setup plan proposes as
    /// the repository's baseline, and what the host is fingerprinted for when
    /// such a plan is scheduled without one.
    pub fn to_manifest_check(&self) -> ManifestCheck {
        ManifestCheck {
            id: self.id.clone(),
            executable: self.executable.clone(),
            args: self.args.clone(),
            required: self.required,
            timeout_seconds: self.timeout_seconds,
            capabilities: self.capabilities.clone(),
            cwd: self.cwd.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn ticket() -> Ticket {
        Ticket {
            schema_version: SCHEMA_VERSION.into(),
            id: "ticket-1".into(),
            project_id: "project".into(),
            revision: 1,
            source: TicketSource::LuunaSqliteTemporary,
            title: "Add greeting".into(),
            intent: "Print a greeting from main.".into(),
            acceptance_criteria: vec!["main prints hello".into()],
            repository_ids: vec!["app".into()],
            purpose: TicketPurpose::Feature,
            ticket_type: Default::default(),
            priority: DEFAULT_TICKET_PRIORITY,
            labels: vec![],
            created_at: "2026-10-17T00:00:00.000Z".into(),
            updated_at: "2026-10-17T00:00:00.000Z".into(),
        }
    }

    fn task() -> PlanTask {
        PlanTask {
            id: "implement".into(),
            repository_id: "app".into(),
            role: "implementer".into(),
            depends_on: vec![],
            paths: vec!["src".into(), "docs/CHANGELOG.md".into()],
            operations: vec![Operation::Read, Operation::Write],
            acceptance_criteria: vec!["ok".into()],
            execution_profile: "native".into(),
            derived_from_task: None,
        }
    }

    fn permissions(prefixes: &[&str]) -> Permissions {
        Permissions {
            repositories: BTreeMap::from([(
                "app".to_owned(),
                prefixes.iter().map(|p| (*p).to_owned()).collect(),
            )]),
            operations: vec![Operation::Read, Operation::Write],
        }
    }

    fn change(path: &str) -> ChangedPath {
        ChangedPath {
            path: path.into(),
            status: "M".into(),
            original_path: None,
        }
    }

    #[test]
    fn type_priority_and_labels_default_to_absent_and_are_bounded() {
        // Defaults are omitted, so records written before ADR 0058 keep their bytes.
        let plain = serde_json::to_value(ticket()).unwrap();
        for key in ["type", "priority", "labels"] {
            assert!(plain.get(key).is_none(), "{key}");
        }
        let old: Ticket = serde_json::from_value(plain).unwrap();
        assert_eq!(old.ticket_type, TicketType::Task);
        assert_eq!(old.priority, DEFAULT_TICKET_PRIORITY);
        assert!(old.labels.is_empty());
        let mut triaged = ticket();
        triaged.ticket_type = TicketType::Bug;
        triaged.priority = 0;
        triaged.labels = vec!["ui".into(), "needs-review".into()];
        triaged.validate().unwrap();
        let value = serde_json::to_value(&triaged).unwrap();
        assert_eq!(value["type"], "bug");
        assert_eq!(value["priority"], 0);
        assert_eq!(value["labels"], serde_json::json!(["ui", "needs-review"]));
        assert!(!triaged.same_content(&ticket()));
        for labels in [
            vec!["has space"],
            vec!["UPPER"],
            vec!["a,b"],
            vec![""],
            vec!["dup", "dup"],
            vec!["luuna-ticket-ticket-1"],
        ] {
            let mut bad = ticket();
            bad.labels = labels.iter().map(|l| (*l).to_owned()).collect();
            assert!(bad.validate().is_err(), "{labels:?}");
        }
        let mut many = ticket();
        many.labels = (0..17).map(|i| format!("l{i}")).collect();
        assert!(many.validate().is_err());
        let mut urgent = ticket();
        urgent.priority = 5;
        assert!(urgent.validate().is_err());
        assert_eq!(parse_ticket_priority("P0"), Some(0));
        assert_eq!(parse_ticket_priority("p4"), Some(4));
        assert_eq!(parse_ticket_priority("3"), Some(3));
        assert_eq!(parse_ticket_priority("P5"), None);
        assert_eq!(parse_ticket_priority("P10"), None);
        assert_eq!(TicketType::parse("Epic"), Some(TicketType::Epic));
        assert_eq!(TicketType::parse("story"), None);
        assert!(serde_json::from_value::<Ticket>(serde_json::json!({
            "schema_version": SCHEMA_VERSION, "id": "t", "project_id": "p", "revision": 1,
            "source": "luuna_sqlite_temporary", "title": "T", "intent": "I",
            "acceptance_criteria": ["c"], "repository_ids": ["app"], "type": "story",
            "created_at": "x", "updated_at": "x"
        }))
        .is_err());
    }

    #[test]
    fn tickets_are_temporary_local_records_with_bounded_fields() {
        ticket().validate().unwrap();
        assert_eq!(
            serde_json::to_value(TicketSource::LuunaSqliteTemporary).unwrap(),
            "luuna_sqlite_temporary"
        );
        let mut untitled = ticket();
        untitled.title = " ".into();
        assert!(untitled.validate().is_err());
        let mut padded = ticket();
        padded.title = " x".into();
        assert!(padded.validate().is_err());
        let mut no_criteria = ticket();
        no_criteria.acceptance_criteria.clear();
        assert!(no_criteria.validate().is_err());
        let mut duplicate_repo = ticket();
        duplicate_repo.repository_ids = vec!["app".into(), "app".into()];
        assert!(duplicate_repo.validate().is_err());
        let mut stale = ticket();
        stale.revision = 0;
        assert!(stale.validate().is_err());
    }

    #[test]
    fn scope_accepts_only_changes_inside_task_paths_and_permissions() {
        let task = task();
        let approved = permissions(&["src", "docs"]);
        check_scope(&[], &task, &approved).unwrap();
        check_scope(
            &[change("src/main.rs"), change("docs/CHANGELOG.md")],
            &task,
            &approved,
        )
        .unwrap();
        for outside in [
            "README.md",
            "docs/other.md",
            "srcs/main.rs",
            "src/../Cargo.toml",
        ] {
            assert!(
                matches!(
                    check_scope(&[change(outside)], &task, &approved),
                    Err(DomainError::ScopeViolation(_))
                ),
                "{outside}"
            );
        }
        // A rename must keep both ends inside scope.
        let rename = ChangedPath {
            path: "src/new.rs".into(),
            status: "R".into(),
            original_path: Some("tests/old.rs".into()),
        };
        assert!(check_scope(std::slice::from_ref(&rename), &task, &approved).is_err());
        // Permissions narrower than the task: the approval wins.
        assert!(check_scope(
            &[change("docs/CHANGELOG.md")],
            &task,
            &permissions(&["src"])
        )
        .is_err());
        // Without the write operation nothing may change.
        let mut read_only = task.clone();
        read_only.operations = vec![Operation::Read];
        assert!(check_scope(&[change("src/main.rs")], &read_only, &approved).is_err());
        // An unrelated repository has no permitted prefixes.
        let mut other_repo = task;
        other_repo.repository_id = "other".into();
        assert!(check_scope(&[change("src/main.rs")], &other_repo, &approved).is_err());
    }

    #[test]
    fn repository_metadata_is_never_in_scope_even_for_the_whole_repository() {
        let mut whole = task();
        whole.paths = vec![".".into()];
        let everything = permissions(&["."]);
        check_scope(
            &[change("src/main.rs"), change("README.md")],
            &whole,
            &everything,
        )
        .unwrap();
        for metadata in [
            ".git",
            ".GIT/config",
            "src/.git/HEAD",
            "vendor/.git.",
            "git~1/x",
            ".",
        ] {
            assert!(
                matches!(
                    check_scope(&[change(metadata)], &whole, &everything),
                    Err(DomainError::ScopeViolation(_))
                ),
                "{metadata}"
            );
        }
    }

    #[test]
    fn only_operations_the_runtime_grants_may_be_planned() {
        require_granted_operations(&[Operation::Read]).unwrap();
        require_granted_operations(&[Operation::Write, Operation::Read]).unwrap();
        for op in [Operation::Shell, Operation::Network, Operation::Publish] {
            let error = require_granted_operations(&[Operation::Read, op.clone()])
                .unwrap_err()
                .to_string();
            assert!(
                error.contains(&format!("operation {}", format!("{op:?}").to_lowercase())),
                "{error}"
            );
        }
    }

    #[test]
    fn budgets_fail_closed_on_any_exceeded_dimension() {
        let budgets = Budgets {
            max_workers: 1,
            max_attempts: 2,
            timeout_seconds: 60,
            cost_usd_micros: 1_000,
        };
        budgets
            .check(BudgetUsage {
                attempts: 2,
                elapsed_seconds: 60,
                cost_usd_micros: 1_000,
                workers: 1,
            })
            .unwrap();
        for usage in [
            BudgetUsage {
                attempts: 3,
                ..Default::default()
            },
            BudgetUsage {
                elapsed_seconds: 61,
                ..Default::default()
            },
            BudgetUsage {
                cost_usd_micros: 1_001,
                ..Default::default()
            },
            BudgetUsage {
                workers: 2,
                ..Default::default()
            },
        ] {
            assert!(matches!(
                budgets.check(usage),
                Err(DomainError::BudgetExceeded(_))
            ));
        }
    }

    #[test]
    fn required_checks_derive_from_the_confirmed_manifest() {
        let repository = RepositoryManifest {
            id: "app".into(),
            remote: "local:fixture".into(),
            default_branch: "main".into(),
            profiles: vec!["python".into()],
            execution: crate::ExecutionProfile::Native,
            checks: vec![ManifestCheck {
                id: "unit".into(),
                executable: "python3".into(),
                args: vec!["-m".into(), "pytest".into()],
                required: true,
                timeout_seconds: 60,
                capabilities: vec!["python".into()],
                cwd: ".".into(),
            }],
        };
        let recipe = CheckRecipe::from_manifest(&repository, &repository.checks[0]);
        assert_eq!(recipe.execution_profile, "native");
        assert_eq!(recipe.repository_id, "app");
        assert_eq!(recipe.args, vec!["-m", "pytest"]);
    }
}
