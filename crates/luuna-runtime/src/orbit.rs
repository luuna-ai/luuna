//! Orbit (ADR 0036): a project's registered repositories seen together. Status
//! is read-only; exec, recipe runs and sync are the developer's own commands in
//! their checkouts, run through the supervised executor or the hardened Git
//! environment. Their results are returned and summarized as project events but
//! are never evidence: nothing here records a `CheckResult` or satisfies a gate.
//! The sandbox route computes the plan `luuna-sbx` applies.
use crate::{internal, projects::storage_error, ApiError, AppState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::{SecondsFormat, Utc};
use luuna_domain::{valid_project_id, CheckStatus, DirtyState, ManifestCheck};
use luuna_executor::CommandSpec;
use luuna_git::{GitError, Upstream};
use luuna_sandbox::{Network, SandboxPlan};
use luuna_storage::RepositoryRegistration;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::BTreeSet,
    path::{Path as FsPath, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{watch, Semaphore};

/// The runtime's four-run limit also bounds parallel Orbit commands.
const MAX_PARALLEL: usize = 4;
const DEFAULT_TIMEOUT_SECONDS: u64 = 300;
const MAX_TIMEOUT_SECONDS: u64 = 3600;
const MAX_ARGS: usize = 64;
const MAX_ARG_BYTES: usize = 4096;
const FETCH_TIMEOUT: Duration = Duration::from_secs(120);
const CLONE_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Serialize)]
pub struct OrbitRepository {
    pub repository_id: String,
    pub local_path: String,
    /// The registered checkout exists on this machine.
    pub present: bool,
    pub branch: Option<String>,
    pub head_commit: Option<String>,
    pub upstream: Option<Upstream>,
    pub dirty: Option<DirtyState>,
    /// Redacted origin URL recorded at registration.
    pub remote: Option<String>,
    /// Confirmed manifest recipe ids this repository can run.
    pub recipes: Vec<String>,
    /// Why the checkout could not be observed, when it could not.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrbitView {
    pub project_id: String,
    pub name: String,
    pub observed_at: String,
    pub manifest_revision: Option<u64>,
    pub repositories: Vec<OrbitRepository>,
    /// Every recipe id any repository defines, sorted.
    pub recipes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OrbitStatus {
    Succeeded,
    Failed,
    Blocked,
    Cancelled,
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrbitResult {
    pub repository_id: String,
    pub status: OrbitStatus,
    /// Sync only: `cloned`, `fetched` or `unchanged`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    pub exit_code: Option<i32>,
    pub reason: Option<String>,
    pub command: Vec<String>,
    pub stdout_excerpt: String,
    pub stderr_excerpt: String,
    pub started_at: String,
    pub finished_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrbitExecution {
    pub id: String,
    pub project_id: String,
    /// `exec`, `recipe` or `sync`.
    pub kind: String,
    /// The command (exec), the recipe id (recipe) or `sync`.
    pub command: Vec<String>,
    pub started_at: String,
    pub finished_at: String,
    pub results: Vec<OrbitResult>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrbitExec {
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Repository ids; empty or omitted selects every registered repository.
    #[serde(default)]
    pub repositories: Vec<String>,
    /// Working directory relative to each repository root (default `.`).
    pub cwd: Option<String>,
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub parallel: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrbitRun {
    pub recipe: String,
    #[serde(default)]
    pub repositories: Vec<String>,
    #[serde(default)]
    pub parallel: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrbitSync {
    #[serde(default)]
    pub repositories: Vec<String>,
    /// Fetch present checkouts (default true); missing ones are always cloned.
    pub fetch: Option<bool>,
}

#[derive(Deserialize)]
pub struct SandboxQuery {
    pub network: Option<String>,
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn unprocessable(message: impl Into<String>) -> ApiError {
    ApiError(StatusCode::UNPROCESSABLE_ENTITY, message.into())
}

fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "project not found".into())
}

/// The project's name, registrations and confirmed manifest checks per repository.
struct Orbit {
    name: String,
    registrations: Vec<RepositoryRegistration>,
    manifest_revision: Option<u64>,
    checks: Vec<(String, Vec<ManifestCheck>)>,
    /// Holds the project's in-flight count until the operation ends.
    _in_flight: InFlight,
}

/// One Orbit operation counted against its project while it may use the
/// project's checkouts; project removal waits while any is counted.
pub(crate) struct InFlight {
    counts: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>,
    project: String,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let mut counts = self
            .counts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(count) = counts.get_mut(&self.project) {
            *count -= 1;
            if *count == 0 {
                counts.remove(&self.project);
            }
        }
    }
}

/// Whether an Orbit operation of the project is in flight. Call with the store
/// lock held, so no operation starts between this check and what follows.
pub(crate) fn in_flight(state: &AppState, project: &str) -> bool {
    state
        .orbit_in_flight
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains_key(project)
}

impl Orbit {
    fn load(state: &AppState, id: &str) -> Result<Self, ApiError> {
        if !valid_project_id(id) {
            return Err(not_found());
        }
        let store = state.store.lock().map_err(internal)?;
        let project = store
            .get_project(id)
            .map_err(internal)?
            .ok_or_else(not_found)?;
        let registrations = store.repositories(id).map_err(internal)?;
        let manifest = store.current_manifest(id).map_err(storage_error)?;
        // Counted while the store lock is held, so removal sees it.
        *state
            .orbit_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(id.to_owned())
            .or_default() += 1;
        let in_flight = InFlight {
            counts: state.orbit_in_flight.clone(),
            project: id.to_owned(),
        };
        let checks = manifest
            .as_ref()
            .map(|m| {
                m.manifest
                    .repositories
                    .iter()
                    .map(|r| (r.id.clone(), r.checks.clone()))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            name: project.name,
            registrations,
            manifest_revision: manifest.map(|m| m.revision),
            checks,
            _in_flight: in_flight,
        })
    }

    fn checks_of(&self, repository_id: &str) -> &[ManifestCheck] {
        self.checks
            .iter()
            .find(|(id, _)| id == repository_id)
            .map(|(_, checks)| checks.as_slice())
            .unwrap_or_default()
    }

    /// The registrations named by `ids` (all when empty); unknown ids are refused.
    fn select(&self, ids: &[String]) -> Result<Vec<RepositoryRegistration>, ApiError> {
        if ids.is_empty() {
            return Ok(self.registrations.clone());
        }
        let wanted: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
        if wanted.len() != ids.len() {
            return Err(unprocessable("repositories must not repeat"));
        }
        let known: BTreeSet<&str> = self
            .registrations
            .iter()
            .map(|r| r.repository_id.as_str())
            .collect();
        if let Some(unknown) = wanted.iter().find(|id| !known.contains(**id)) {
            return Err(unprocessable(format!(
                "repository {unknown} is not registered in this project"
            )));
        }
        Ok(self
            .registrations
            .iter()
            .filter(|r| wanted.contains(r.repository_id.as_str()))
            .cloned()
            .collect())
    }
}

fn present(registration: &RepositoryRegistration) -> bool {
    FsPath::new(&registration.local_path).is_dir()
}

async fn observe(
    state: &AppState,
    orbit: &Orbit,
    registration: RepositoryRegistration,
) -> OrbitRepository {
    let recipes = orbit
        .checks_of(&registration.repository_id)
        .iter()
        .map(|c| c.id.clone())
        .collect();
    let mut repository = OrbitRepository {
        repository_id: registration.repository_id.clone(),
        local_path: registration.local_path.clone(),
        present: present(&registration),
        branch: None,
        head_commit: None,
        upstream: None,
        dirty: None,
        remote: registration.observation.identity.remote.clone(),
        recipes,
        error: None,
    };
    if !repository.present {
        repository.error =
            Some("the registered checkout is missing on this machine; orbit sync clones it".into());
        return repository;
    }
    let root = PathBuf::from(&registration.local_path);
    match state.git.inspect(&root).await {
        Ok(observation) => {
            if let Err(error) = registration
                .observation
                .identity
                .matches(&observation.identity)
            {
                repository.error = Some(format!(
                    "repository identity changed since registration ({error}); rebind it with POST /api/v1/projects/{{id}}/repositories/{}/rebind",
                    registration.repository_id
                ));
            }
            repository.branch = observation.branch;
            repository.head_commit = Some(observation.head_commit);
            repository.dirty = Some(observation.dirty);
            repository.upstream = state.git.upstream(&root).await.ok().flatten();
        }
        Err(error) => repository.error = Some(error.to_string()),
    }
    repository
}

/// `GET /api/v1/projects/{id}/orbit`
pub async fn status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<OrbitView>, ApiError> {
    let orbit = Arc::new(Orbit::load(&state, &id)?);
    let mut tasks = tokio::task::JoinSet::new();
    for (index, registration) in orbit.registrations.iter().cloned().enumerate() {
        let (state, orbit) = (state.clone(), orbit.clone());
        tasks.spawn(async move { (index, observe(&state, &orbit, registration).await) });
    }
    let mut repositories: Vec<(usize, OrbitRepository)> = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        repositories.push(joined.map_err(internal)?);
    }
    repositories.sort_by_key(|(index, _)| *index);
    let recipes: BTreeSet<String> = orbit
        .checks
        .iter()
        .flat_map(|(_, checks)| checks.iter().map(|c| c.id.clone()))
        .collect();
    Ok(Json(OrbitView {
        project_id: id,
        name: orbit.name.clone(),
        observed_at: now(),
        manifest_revision: orbit.manifest_revision,
        repositories: repositories.into_iter().map(|(_, r)| r).collect(),
        recipes: recipes.into_iter().collect(),
    }))
}

fn validate_command(executable: &str, args: &[String]) -> Result<(), ApiError> {
    if executable.trim().is_empty() || executable.len() > 1024 || executable.contains('\0') {
        return Err(unprocessable(
            "executable must be a command name or repository-relative path of 1..1024 bytes",
        ));
    }
    if args.len() > MAX_ARGS
        || args
            .iter()
            .any(|a| a.len() > MAX_ARG_BYTES || a.contains('\0'))
    {
        return Err(unprocessable(format!(
            "at most {MAX_ARGS} arguments of at most {MAX_ARG_BYTES} bytes each"
        )));
    }
    Ok(())
}

fn skipped(repository_id: &str, command: &[String], reason: impl Into<String>) -> OrbitResult {
    let at = now();
    OrbitResult {
        repository_id: repository_id.to_owned(),
        status: OrbitStatus::Skipped,
        action: None,
        exit_code: None,
        reason: Some(reason.into()),
        command: command.to_vec(),
        stdout_excerpt: String::new(),
        stderr_excerpt: String::new(),
        started_at: at.clone(),
        finished_at: at,
    }
}

/// One supervised command in one checkout. The exit code decides: Orbit runs
/// are developer commands, so an exit-0 run with no recognised tests is a
/// success here, not the blocked empty suite a baseline would record.
async fn execute(state: &AppState, repository_id: String, spec: CommandSpec) -> OrbitResult {
    let started_at = now();
    let command: Vec<String> = std::iter::once(spec.executable.clone())
        .chain(spec.args.iter().cloned())
        .collect();
    if let Err(error) = std::fs::create_dir_all(&spec.artifact_dir) {
        return OrbitResult {
            status: OrbitStatus::Blocked,
            reason: Some(format!("cannot create the artifact directory: {error}")),
            ..skipped(&repository_id, &command, "")
        };
    }
    let (_stop, cancel) = watch::channel(false);
    match state.executor.execute(&spec, cancel, None).await {
        Ok(execution) => OrbitResult {
            repository_id,
            status: match (execution.exit_code, execution.status) {
                (Some(0), _) => OrbitStatus::Succeeded,
                (Some(_), _) => OrbitStatus::Failed,
                (None, CheckStatus::Cancelled | CheckStatus::Interrupted) => OrbitStatus::Cancelled,
                (None, CheckStatus::Failed) => OrbitStatus::Failed,
                (None, _) => OrbitStatus::Blocked,
            },
            action: None,
            exit_code: execution.exit_code,
            reason: execution.reason,
            command,
            stdout_excerpt: execution.stdout_excerpt,
            stderr_excerpt: execution.stderr_excerpt,
            started_at,
            finished_at: now(),
        },
        Err(error) => OrbitResult {
            status: OrbitStatus::Blocked,
            reason: Some(error.to_string()),
            started_at,
            ..skipped(&repository_id, &command, "")
        },
    }
}

/// Run `jobs` sequentially or up to [`MAX_PARALLEL`] at once, keeping order.
async fn run_all(
    state: &AppState,
    jobs: Vec<Result<(String, CommandSpec), OrbitResult>>,
    parallel: bool,
) -> Result<Vec<OrbitResult>, ApiError> {
    let gate = Arc::new(Semaphore::new(if parallel { MAX_PARALLEL } else { 1 }));
    let mut tasks = tokio::task::JoinSet::new();
    let mut results: Vec<(usize, OrbitResult)> = Vec::new();
    for (index, job) in jobs.into_iter().enumerate() {
        match job {
            Err(result) => results.push((index, result)),
            Ok((repository_id, spec)) => {
                let (state, gate) = (state.clone(), gate.clone());
                tasks.spawn(async move {
                    let _permit = gate.acquire_owned().await;
                    (index, execute(&state, repository_id, spec).await)
                });
            }
        }
    }
    while let Some(joined) = tasks.join_next().await {
        results.push(joined.map_err(internal)?);
    }
    results.sort_by_key(|(index, _)| *index);
    Ok(results.into_iter().map(|(_, r)| r).collect())
}

fn record(
    state: &AppState,
    project_id: &str,
    event: &str,
    execution: &OrbitExecution,
) -> Result<(), ApiError> {
    let outcomes: Vec<_> = execution
        .results
        .iter()
        .map(|r| json!({"repository_id": r.repository_id, "status": r.status, "action": r.action, "exit_code": r.exit_code}))
        .collect();
    state
        .store
        .lock()
        .map_err(internal)?
        .record_project_event(project_id, event, json!({"id": execution.id, "kind": execution.kind, "command": execution.command, "results": outcomes}))
        .map_err(internal)?;
    Ok(())
}

fn artifacts(state: &AppState, execution: &str, repository_id: &str) -> PathBuf {
    state
        .config
        .data_dir
        .join("orbit")
        .join(execution)
        .join(repository_id)
}

/// `POST /api/v1/projects/{id}/orbit/exec`
pub async fn exec(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<OrbitExec>,
) -> Result<Json<OrbitExecution>, ApiError> {
    let orbit = Orbit::load(&state, &id)?;
    validate_command(&request.executable, &request.args)?;
    let timeout = request.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS);
    if !(1..=MAX_TIMEOUT_SECONDS).contains(&timeout) {
        return Err(unprocessable(format!(
            "timeout_seconds must be 1..={MAX_TIMEOUT_SECONDS}"
        )));
    }
    let selected = orbit.select(&request.repositories)?;
    let execution_id = uuid::Uuid::new_v4().to_string();
    let command: Vec<String> = std::iter::once(request.executable.clone())
        .chain(request.args.iter().cloned())
        .collect();
    let started_at = now();
    let jobs = selected
        .iter()
        .map(|registration| {
            if !present(registration) {
                return Err(skipped(
                    &registration.repository_id,
                    &command,
                    "the checkout is missing on this machine; run orbit sync",
                ));
            }
            Ok((
                registration.repository_id.clone(),
                CommandSpec {
                    recipe_id: "orbit-exec".into(),
                    executable: request.executable.clone(),
                    args: request.args.clone(),
                    repository_root: PathBuf::from(&registration.local_path),
                    cwd: request.cwd.clone().unwrap_or_else(|| ".".into()),
                    timeout: Duration::from_secs(timeout),
                    budget: None,
                    capabilities: Vec::new(),
                    artifact_dir: artifacts(&state, &execution_id, &registration.repository_id),
                    environment: Vec::new(),
                },
            ))
        })
        .collect();
    let results = run_all(&state, jobs, request.parallel).await?;
    let execution = OrbitExecution {
        id: execution_id,
        project_id: id.clone(),
        kind: "exec".into(),
        command,
        started_at,
        finished_at: now(),
        results,
    };
    record(&state, &id, "orbit.executed", &execution)?;
    Ok(Json(execution))
}

/// `POST /api/v1/projects/{id}/orbit/run`
pub async fn run(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<OrbitRun>,
) -> Result<Json<OrbitExecution>, ApiError> {
    let orbit = Orbit::load(&state, &id)?;
    if orbit.manifest_revision.is_none() {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "confirm the project manifest first; recipes come from its confirmed revision".into(),
        ));
    }
    let recipe = request.recipe.trim().to_owned();
    if recipe.is_empty() || recipe.len() > 200 {
        return Err(unprocessable("recipe must be a recipe id of 1..200 bytes"));
    }
    if !orbit
        .checks
        .iter()
        .any(|(_, checks)| checks.iter().any(|c| c.id == recipe))
    {
        return Err(unprocessable(format!(
            "no repository of this project defines the recipe {recipe}"
        )));
    }
    let selected = orbit.select(&request.repositories)?;
    let execution_id = uuid::Uuid::new_v4().to_string();
    let started_at = now();
    let jobs = selected
        .iter()
        .map(|registration| {
            let Some(check) = orbit
                .checks_of(&registration.repository_id)
                .iter()
                .find(|c| c.id == recipe)
            else {
                return Err(skipped(
                    &registration.repository_id,
                    &[],
                    format!("this repository's manifest has no recipe {recipe}"),
                ));
            };
            let command: Vec<String> = std::iter::once(check.executable.clone())
                .chain(check.args.iter().cloned())
                .collect();
            if !present(registration) {
                return Err(skipped(
                    &registration.repository_id,
                    &command,
                    "the checkout is missing on this machine; run orbit sync",
                ));
            }
            Ok((
                registration.repository_id.clone(),
                crate::baselines::recipe_spec(
                    &registration.local_path,
                    check,
                    &artifacts(&state, &execution_id, &registration.repository_id),
                ),
            ))
        })
        .collect();
    let results = run_all(&state, jobs, request.parallel).await?;
    let execution = OrbitExecution {
        id: execution_id,
        project_id: id.clone(),
        kind: "recipe".into(),
        command: vec![recipe],
        started_at,
        finished_at: now(),
        results,
    };
    record(&state, &id, "orbit.recipe_ran", &execution)?;
    Ok(Json(execution))
}

fn sync_result(
    repository_id: &str,
    status: OrbitStatus,
    action: Option<&str>,
    reason: Option<String>,
    started_at: String,
) -> OrbitResult {
    OrbitResult {
        repository_id: repository_id.to_owned(),
        status,
        action: action.map(ToOwned::to_owned),
        exit_code: None,
        reason,
        command: Vec::new(),
        stdout_excerpt: String::new(),
        stderr_excerpt: String::new(),
        started_at,
        finished_at: now(),
    }
}

async fn sync_one(
    state: &AppState,
    registration: &RepositoryRegistration,
    fetch: bool,
) -> OrbitResult {
    let started_at = now();
    let id = registration.repository_id.as_str();
    let root = PathBuf::from(&registration.local_path);
    if present(registration) {
        if !fetch {
            return sync_result(
                id,
                OrbitStatus::Skipped,
                Some("unchanged"),
                Some("present; fetch not requested".into()),
                started_at,
            );
        }
        if registration.observation.identity.remote.is_none() {
            return sync_result(
                id,
                OrbitStatus::Skipped,
                Some("unchanged"),
                Some("no origin remote to fetch from".into()),
                started_at,
            );
        }
        return match state.git.fetch_origin(&root, FETCH_TIMEOUT).await {
            Ok(summary) => sync_result(
                id,
                OrbitStatus::Succeeded,
                Some("fetched"),
                (!summary.is_empty()).then_some(summary),
                started_at,
            ),
            Err(error) => sync_result(
                id,
                OrbitStatus::Failed,
                None,
                Some(error.to_string()),
                started_at,
            ),
        };
    }
    let Some(remote) = registration.observation.identity.remote.clone() else {
        return sync_result(
            id,
            OrbitStatus::Failed,
            None,
            Some("the checkout is missing and the registration has no remote to clone from".into()),
            started_at,
        );
    };
    if let Err(error) = state
        .git
        .clone_repository(&remote, &root, CLONE_TIMEOUT)
        .await
    {
        return sync_result(
            id,
            OrbitStatus::Failed,
            None,
            Some(error.to_string()),
            started_at,
        );
    }
    // The clone is only the registered repository if its roots contain the
    // registered ones; otherwise it is left for the developer to inspect.
    match state.git.inspect(&root).await {
        Ok(observation) => match registration.observation.identity.matches(&observation.identity) {
            Ok(()) => sync_result(id, OrbitStatus::Succeeded, Some("cloned"), Some(format!("cloned {remote}")), started_at),
            Err(error) => sync_result(id, OrbitStatus::Failed, Some("cloned"), Some(format!("cloned {remote}, but it is not the registered repository ({error}); the clone was left at {} for you to inspect", registration.local_path)), started_at),
        },
        Err(GitError::Unborn) => sync_result(id, OrbitStatus::Failed, Some("cloned"), Some("cloned an empty repository; its identity cannot be verified".into()), started_at),
        Err(error) => sync_result(id, OrbitStatus::Failed, Some("cloned"), Some(format!("cloned, but inspection failed: {error}")), started_at),
    }
}

/// `POST /api/v1/projects/{id}/orbit/sync`
pub async fn sync(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<OrbitSync>,
) -> Result<Json<OrbitExecution>, ApiError> {
    let orbit = Orbit::load(&state, &id)?;
    let selected = orbit.select(&request.repositories)?;
    let started_at = now();
    let mut results = Vec::new();
    for registration in &selected {
        results.push(sync_one(&state, registration, request.fetch.unwrap_or(true)).await);
    }
    let execution = OrbitExecution {
        id: uuid::Uuid::new_v4().to_string(),
        project_id: id.clone(),
        kind: "sync".into(),
        command: vec!["sync".into()],
        started_at,
        finished_at: now(),
        results,
    };
    record(&state, &id, "orbit.synced", &execution)?;
    Ok(Json(execution))
}

/// `GET /api/v1/projects/{id}/orbit/sandbox?network=open|none`: what a sandbox
/// for this project may reach. Only present, registered checkouts are listed;
/// the home lives in the runtime's data directory and is created here.
pub async fn sandbox(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<SandboxQuery>,
) -> Result<Json<SandboxPlan>, ApiError> {
    let orbit = Orbit::load(&state, &id)?;
    let network = match query.network.as_deref() {
        None => Network::Open,
        Some(value) => {
            Network::parse(value).ok_or_else(|| unprocessable("network must be open or none"))?
        }
    };
    let repositories: Vec<(String, PathBuf)> = orbit
        .registrations
        .iter()
        .filter(|r| present(r))
        .filter_map(|r| {
            std::fs::canonicalize(&r.local_path).ok().map(|p| {
                (
                    r.repository_id.clone(),
                    PathBuf::from(luuna_sandbox::display(&p)),
                )
            })
        })
        .collect();
    let data_dir = std::fs::canonicalize(&state.config.data_dir)
        .map(|p| PathBuf::from(luuna_sandbox::display(&p)))
        .map_err(internal)?;
    // Registration already refuses a checkout overlapping the data directory;
    // the plan re-checks it because the sandbox would otherwise expose it.
    if repositories
        .iter()
        .any(|(_, path)| path.starts_with(&data_dir) || data_dir.starts_with(path))
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "a repository overlaps the runtime data directory and cannot be sandboxed".into(),
        ));
    }
    let sandbox_dir = data_dir.join("sandboxes").join(&id);
    std::fs::create_dir_all(sandbox_dir.join("home")).map_err(internal)?;
    Ok(Json(luuna_sandbox::plan(
        &id,
        &repositories,
        &sandbox_dir,
        network,
        luuna_sandbox::host_backend(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_in_flight_count_is_released_when_each_operation_ends() {
        let counts = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        counts.lock().unwrap().insert("app".to_string(), 2);
        let first = InFlight {
            counts: counts.clone(),
            project: "app".into(),
        };
        let second = InFlight {
            counts: counts.clone(),
            project: "app".into(),
        };
        drop(first);
        assert_eq!(counts.lock().unwrap().get("app"), Some(&1));
        drop(second);
        assert!(counts.lock().unwrap().is_empty());
    }
}
