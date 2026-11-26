//! M2a: register an existing repository, discover a proposed map read-only, and
//! confirm an immutable manifest revision. Nothing here writes to a repository
//! or executes a proposed command. A registered repository whose checkout moved
//! or whose identity changed is re-pointed only by an explicit rebind request.
use crate::{
    integrations::{self, MemoryRecord, RenderedPrompt},
    internal, ApiError, AppState,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::{SecondsFormat, Utc};
use luuna_discovery::{DiscoveryRequest, RepositoryInput};
use luuna_domain::{
    remotes_equivalent, valid_project_id, Discovery, DomainError, EventEnvelope, ProjectManifest,
    RepositoryIdentity, RepositoryObservation, TestingPrerequisite, Ticket, TicketSource,
    SCHEMA_VERSION,
};
use luuna_git::{strip_verbatim_prefix, GitError};
use luuna_storage::{ManifestRecord, ProjectRecord, RepositoryRegistration, StorageError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

const MAX_REPOSITORIES: usize = 32;
const MAX_PATH_BYTES: usize = 4096;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterRepository {
    pub id: String,
    /// Absolute path to the repository root on this machine. Never stored in the manifest.
    pub path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterProject {
    pub id: Option<String>,
    pub name: String,
    pub repositories: Vec<RegisterRepository>,
}

/// `POST /api/v1/projects/import`: an export document produced by
/// `GET /api/v1/projects/{id}/export` on another machine, plus this machine's
/// checkout of every exported repository.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportProject {
    pub export: Value,
    pub repositories: Vec<RegisterRepository>,
    pub idempotency_key: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmManifest {
    pub expected_revision: u64,
    pub manifest: ProjectManifest,
}

/// An explicit request to re-point a registered repository. Sending it is the
/// developer's confirmation that the observed checkout and identity are intended.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RebindRepository {
    /// New absolute repository root; omitted to re-observe the registered path
    /// (for example after its origin URL was changed on purpose).
    pub local_path: Option<String>,
}

#[derive(Serialize)]
pub struct ProjectView {
    pub project: ProjectRecord,
    pub repositories: Vec<RepositoryRegistration>,
    pub manifest: Option<ManifestRecord>,
    pub discovery: Option<Discovery>,
    /// One offer per registered repository whose latest baseline shows no
    /// usable testing setup (ADR 0022); empty otherwise.
    pub prerequisites: Vec<TestingPrerequisite>,
}

fn bad_request(message: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, message.into())
}

pub(crate) fn storage_error(error: StorageError) -> ApiError {
    match error {
        StorageError::NotFound(_) => ApiError(StatusCode::NOT_FOUND, "project not found".into()),
        StorageError::AlreadyExists(id) => ApiError(
            StatusCode::CONFLICT,
            format!("project {id} is already registered"),
        ),
        StorageError::RevisionConflict { expected, actual } => ApiError(
            StatusCode::CONFLICT,
            format!("revision conflict: expected {expected}, current {actual}"),
        ),
        StorageError::Domain(DomainError::IdentityMismatch(detail)) => ApiError(
            StatusCode::CONFLICT,
            format!("repository identity changed since registration: {detail}"),
        ),
        StorageError::Domain(error) => {
            ApiError(StatusCode::UNPROCESSABLE_ENTITY, error.to_string())
        }
        StorageError::Invalid(detail) => ApiError(StatusCode::UNPROCESSABLE_ENTITY, detail),
        other => internal(other),
    }
}

fn git_error(repository_id: &str, error: GitError) -> ApiError {
    match error {
        GitError::Timeout(_) | GitError::Io(_) => ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("repository {repository_id}: {error}"),
        ),
        GitError::Oversized => ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("repository {repository_id}: {error}; inspection lists every changed and untracked path, so commit, stash or ignore generated files and try again"),
        ),
        _ => ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("repository {repository_id}: {error}"),
        ),
    }
}

fn validate_root_path(path: &str) -> Result<(), ApiError> {
    if path.len() > MAX_PATH_BYTES || !PathBuf::from(path).is_absolute() || path.contains('\0') {
        return Err(bad_request(
            "repository path must be an absolute path to the repository root",
        ));
    }
    Ok(())
}

/// Luuna's private data directory (SQLite, artifacts, owned worktrees) must not
/// be inside a registered checkout, and a registered checkout must not be inside
/// it: a developer checkout is never one of Luuna's own worktrees.
fn reject_data_dir_overlap(
    state: &AppState,
    repository_id: &str,
    observation: &RepositoryObservation,
) -> Result<(), ApiError> {
    let data_dir = std::fs::canonicalize(&state.config.data_dir)
        .map(|path| strip_verbatim_prefix(&path))
        .map_err(internal)?;
    let root = PathBuf::from(&observation.local_path);
    if root.starts_with(&data_dir) {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "repository {repository_id}: {} is inside Luuna's data directory {} (LUUNA_DATA_DIR), which holds Luuna-owned worktrees; register the developer checkout instead",
                root.display(),
                data_dir.display()
            ),
        ));
    }
    if data_dir.starts_with(&root) {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "repository {repository_id}: Luuna's data directory {} (LUUNA_DATA_DIR) is inside this checkout and would put its private database and worktrees in the repository; restart the runtime with LUUNA_DATA_DIR outside {}",
                data_dir.display(),
                root.display()
            ),
        ));
    }
    Ok(())
}

fn already_registered(path: &str, project_id: &str, repository_id: &str) -> ApiError {
    ApiError(
        StatusCode::CONFLICT,
        format!(
            "checkout {path} is already registered as repository {repository_id} of project {project_id}; use that project, or rebind that repository to another checkout first"
        ),
    )
}

/// An identity mismatch with the recovery route spelled out for this repository.
pub(crate) fn identity_changed(
    project_id: &str,
    repository_id: &str,
    error: DomainError,
) -> ApiError {
    let detail = error
        .to_string()
        .replace("{id}", project_id)
        .replace("{repository_id}", repository_id);
    ApiError(
        StatusCode::CONFLICT,
        format!("repository {repository_id} identity changed since registration: {detail}"),
    )
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn derive_project_id(name: &str) -> String {
    let mut id = String::new();
    for ch in name.chars().take(64) {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_lowercase() || lower.is_ascii_digit() {
            id.push(lower);
        } else if !id.is_empty() && !id.ends_with('-') {
            id.push('-');
        }
    }
    let id = id.trim_end_matches('-').to_owned();
    let mut id = if id.is_empty() || !id.as_bytes()[0].is_ascii_lowercase() {
        format!("p-{id}")
    } else {
        id
    };
    // The prefix may push a 64-character slug past the ID limit.
    id.truncate(64);
    id.trim_end_matches('-').to_owned()
}

fn view(state: &AppState, id: &str) -> Result<ProjectView, ApiError> {
    let store = state.store.lock().map_err(internal)?;
    let project = store
        .get_project(id)
        .map_err(internal)?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "project not found".into()))?;
    let repositories = store.repositories(id).map_err(internal)?;
    let prerequisites = repositories
        .iter()
        .filter_map(|registration| {
            store
                .latest_baseline(id, &registration.repository_id)
                .ok()
                .flatten()
                .and_then(|evidence| evidence.testing_prerequisite())
        })
        .collect();
    Ok(ProjectView {
        repositories,
        manifest: store.current_manifest(id).map_err(internal)?,
        discovery: store.latest_discovery(id).map_err(internal)?,
        prerequisites,
        project,
    })
}

/// Re-observe every registered repository read-only and verify it is still the
/// repository that was registered. Nothing is cached: confirmation always binds
/// to the current state of the checkout.
async fn observe_registered(
    state: &AppState,
    id: &str,
) -> Result<
    (
        Vec<RepositoryRegistration>,
        BTreeMap<String, RepositoryObservation>,
    ),
    ApiError,
> {
    let registrations = {
        let store = state.store.lock().map_err(internal)?;
        store
            .get_project(id)
            .map_err(internal)?
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "project not found".into()))?;
        store.repositories(id).map_err(internal)?
    };
    let mut observations = BTreeMap::new();
    for registration in &registrations {
        let observation = state
            .git
            .inspect(&PathBuf::from(&registration.local_path))
            .await
            .map_err(|e| match e {
                GitError::InvalidPath => ApiError(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!(
                        "repository {repository}: the registered checkout {} no longer exists; move it back, or rebind the repository to its new location with POST /api/v1/projects/{id}/repositories/{repository}/rebind and a local_path",
                        registration.local_path,
                        repository = registration.repository_id
                    ),
                ),
                other => git_error(&registration.repository_id, other),
            })?;
        registration
            .observation
            .identity
            .matches(&observation.identity)
            .map_err(|e| identity_changed(id, &registration.repository_id, e))?;
        observations.insert(registration.repository_id.clone(), observation);
    }
    Ok((registrations, observations))
}

pub async fn list_projects(
    State(state): State<AppState>,
) -> Result<Json<Vec<ProjectRecord>>, ApiError> {
    Ok(Json(
        state
            .store
            .lock()
            .map_err(internal)?
            .projects()
            .map_err(internal)?,
    ))
}

pub async fn get_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ProjectView>, ApiError> {
    if !valid_project_id(&id) {
        return Err(ApiError(StatusCode::NOT_FOUND, "project not found".into()));
    }
    Ok(Json(view(&state, &id)?))
}

pub async fn register_project(
    State(state): State<AppState>,
    Json(request): Json<RegisterProject>,
) -> Result<(StatusCode, Json<ProjectView>), ApiError> {
    let name = request.name.trim().to_owned();
    if name.is_empty() || name.len() > 200 {
        return Err(bad_request("project name must be 1..200 bytes"));
    }
    let explicit_id = request
        .id
        .as_deref()
        .is_some_and(|id| !id.trim().is_empty());
    let id = request
        .id
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| derive_project_id(&name));
    if !valid_project_id(&id) {
        return Err(bad_request("project ID must match ^[a-z][a-z0-9-]{0,63}$"));
    }
    if request.repositories.is_empty() || request.repositories.len() > MAX_REPOSITORIES {
        return Err(bad_request("register between one and 32 repositories"));
    }
    let mut ids = BTreeSet::new();
    for repository in &request.repositories {
        if !valid_project_id(&repository.id) || !ids.insert(repository.id.as_str()) {
            return Err(bad_request(
                "repository IDs must be unique and match ^[a-z][a-z0-9-]*$",
            ));
        }
        validate_root_path(&repository.path)?;
    }
    let mut observations = Vec::new();
    for repository in &request.repositories {
        let observation = state
            .git
            .inspect_for_registration(&PathBuf::from(&repository.path))
            .await
            .map_err(|e| git_error(&repository.id, e))?;
        reject_data_dir_overlap(&state, &repository.id, &observation)?;
        if observations
            .iter()
            .any(|(_, existing): &(String, RepositoryObservation)| {
                existing.local_path == observation.local_path
            })
        {
            return Err(bad_request(
                "each repository must be a distinct checkout; two IDs point at the same root",
            ));
        }
        observations.push((repository.id.clone(), observation));
    }
    {
        let mut store = state.store.lock().map_err(internal)?;
        if store.get_project(&id).map_err(internal)?.is_some() {
            return Err(storage_error(StorageError::AlreadyExists(id)));
        }
        // One registration per checkout: two projects would otherwise run
        // baselines in, and lease writers against, the same repository.
        for (_, observation) in &observations {
            if let Some((project, repository)) = store
                .registration_at_path(&observation.local_path)
                .map_err(internal)?
            {
                return Err(already_registered(
                    &observation.local_path,
                    &project,
                    &repository,
                ));
            }
        }
        // ADR 0063: registering a removed project's repositories at the same
        // folders again brings it back with its records. Under its own ID any
        // other registration is refused; under a name-derived ID a removed
        // project that registered exactly these folders comes back.
        let same = |registrations: &[RepositoryRegistration]| {
            registrations.len() == observations.len()
                && registrations.iter().all(|r| {
                    observations
                        .iter()
                        .any(|(id, o)| id == &r.repository_id && o.local_path == r.local_path)
                })
        };
        let mut restore = None;
        if let Some((_, registrations)) = store.removed_project(&id).map_err(internal)? {
            if !same(&registrations) {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    format!("project {id} was removed; register the same repositories at the same folders to bring it back, or choose another project ID"),
                ));
            }
            restore = Some(id.clone());
        } else if !explicit_id {
            for candidate in store
                .removed_projects_at_path(&observations[0].1.local_path)
                .map_err(internal)?
            {
                if let Some((_, registrations)) =
                    store.removed_project(&candidate).map_err(internal)?
                {
                    if same(&registrations) {
                        restore = Some(candidate);
                        break;
                    }
                }
            }
        }
        if let Some(restored) = restore {
            store.restore_project(&restored).map_err(storage_error)?;
            drop(store);
            return Ok((StatusCode::OK, Json(view(&state, &restored)?)));
        }
        store
            .register_project(&id, &name, &observations)
            .map_err(storage_error)?;
    }
    Ok((StatusCode::CREATED, Json(view(&state, &id)?)))
}

/// `DELETE /api/v1/projects/{id}` (ADR 0063): take the project off every list
/// and free its checkouts. Its tickets, runs, evidence, events and project file
/// stay on disk, and registering the same repositories at the same folders
/// brings it back. Refused while a run or an Orbit command may be working in
/// one of its checkouts.
pub async fn remove_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if !valid_project_id(&id) {
        return Err(ApiError(StatusCode::NOT_FOUND, "project not found".into()));
    }
    let mut store = state.store.lock().map_err(internal)?;
    if store.get_project(&id).map_err(internal)?.is_none() {
        return Err(ApiError(StatusCode::NOT_FOUND, "project not found".into()));
    }
    if let Some(run) = store
        .active_checkout_runs(&id)
        .map_err(internal)?
        .into_iter()
        .next()
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            format!("run {run} of project {id} may be executing in a registered checkout; wait for it to finish or cancel it before removing the project"),
        ));
    }
    if crate::orbit::in_flight(&state, &id) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            format!("an Orbit command of project {id} is running in its checkouts; wait for it to finish before removing the project"),
        ));
    }
    store.remove_project(&id).map_err(storage_error)?;
    Ok(Json(json!({"removed": id})))
}

/// `POST /api/v1/projects/{id}/repositories/{repository_id}/rebind`: re-observe
/// the registered path, or a new `local_path`, and bind the repository to that
/// checkout and identity. This is the only way past an identity mismatch; it is
/// refused while a run of the project may be executing in a checkout. Evidence
/// stays bound to exact commits, and a manifest whose remote no longer matches
/// must be confirmed again before it is accepted.
pub async fn rebind_repository(
    State(state): State<AppState>,
    Path((id, repository_id)): Path<(String, String)>,
    Json(request): Json<RebindRepository>,
) -> Result<Json<ProjectView>, ApiError> {
    if !valid_project_id(&id) {
        return Err(ApiError(StatusCode::NOT_FOUND, "project not found".into()));
    }
    let registration = {
        let store = state.store.lock().map_err(internal)?;
        store
            .get_project(&id)
            .map_err(internal)?
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "project not found".into()))?;
        store
            .repositories(&id)
            .map_err(internal)?
            .into_iter()
            .find(|r| r.repository_id == repository_id)
            .ok_or_else(|| {
                ApiError(
                    StatusCode::NOT_FOUND,
                    "repository is not registered in this project".into(),
                )
            })?
    };
    let path = request
        .local_path
        .unwrap_or_else(|| registration.local_path.clone());
    validate_root_path(&path)?;
    let observation = state
        .git
        .inspect_for_registration(&PathBuf::from(&path))
        .await
        .map_err(|e| git_error(&repository_id, e))?;
    reject_data_dir_overlap(&state, &repository_id, &observation)?;
    {
        let mut store = state.store.lock().map_err(internal)?;
        if let Some(run) = store
            .active_checkout_runs(&id)
            .map_err(internal)?
            .into_iter()
            .next()
        {
            return Err(ApiError(
                StatusCode::CONFLICT,
                format!("run {run} of project {id} may be executing in a registered checkout; wait for it to finish or cancel it before rebinding"),
            ));
        }
        if let Some((project, repository)) = store
            .registration_at_path(&observation.local_path)
            .map_err(internal)?
            .filter(|(project, repository)| *project != id || *repository != repository_id)
        {
            return Err(already_registered(
                &observation.local_path,
                &project,
                &repository,
            ));
        }
        store
            .rebind_repository(&id, &repository_id, &observation)
            .map_err(storage_error)?;
    }
    Ok(Json(view(&state, &id)?))
}

pub async fn discover(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Discovery>), ApiError> {
    if !valid_project_id(&id) {
        return Err(ApiError(StatusCode::NOT_FOUND, "project not found".into()));
    }
    let project = {
        let store = state.store.lock().map_err(internal)?;
        store
            .get_project(&id)
            .map_err(internal)?
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "project not found".into()))?
    };
    let (registrations, mut observations) = observe_registered(&state, &id).await?;
    let repositories = registrations
        .iter()
        .map(|registration| RepositoryInput {
            repository_id: registration.repository_id.clone(),
            root: PathBuf::from(&registration.local_path),
            observation: observations
                .remove(&registration.repository_id)
                .expect("observed every registration"),
        })
        .collect();
    let request = DiscoveryRequest {
        id: uuid::Uuid::new_v4().to_string(),
        project_id: id.clone(),
        project_name: project.name.clone(),
        manifest_revision: project.revision + 1,
        created_at: now(),
        repositories,
        host_capabilities: None,
    };
    // The host probe (M5) annotates every proposed check available or missing.
    let discovery = tokio::task::spawn_blocking(move || {
        let mut request = request;
        request.host_capabilities = Some(luuna_discovery::capabilities::probe_host_cached());
        luuna_discovery::discover(request)
    })
    .await
    .map_err(internal)?
    .map_err(|e| ApiError(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;
    state
        .store
        .lock()
        .map_err(internal)?
        .record_discovery(&discovery)
        .map_err(storage_error)?;
    Ok((StatusCode::CREATED, Json(discovery)))
}

pub async fn confirm_manifest(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<ConfirmManifest>,
) -> Result<Json<ProjectView>, ApiError> {
    if !valid_project_id(&id) {
        return Err(ApiError(StatusCode::NOT_FOUND, "project not found".into()));
    }
    confirm_observed(&state, &id, request.expected_revision, &request.manifest).await?;
    Ok(Json(view(&state, &id)?))
}

/// Re-observe every registered checkout and confirm `manifest` as revision
/// `expected_revision + 1`, with the validation every confirmation gets.
async fn confirm_observed(
    state: &AppState,
    id: &str,
    expected_revision: u64,
    manifest: &ProjectManifest,
) -> Result<(), ApiError> {
    manifest
        .validate()
        .map_err(|e| ApiError(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;
    let (registrations, observations) = observe_registered(state, id).await?;
    // A default branch that exists only as `origin/<name>` (a fresh clone checks
    // out just one branch) is confirmed from an exact remote-tracking ref.
    let mut remote_branches = BTreeMap::new();
    for repository in &manifest.repositories {
        let (Some(registration), Some(observation)) = (
            registrations
                .iter()
                .find(|r| r.repository_id == repository.id),
            observations.get(&repository.id),
        ) else {
            continue;
        };
        if !observation.has_branch(&repository.default_branch)
            && state
                .git
                .has_remote_branch(
                    &PathBuf::from(&registration.local_path),
                    &repository.default_branch,
                )
                .await
                .map_err(|e| git_error(&repository.id, e))?
        {
            remote_branches.insert(
                repository.id.clone(),
                BTreeSet::from([repository.default_branch.clone()]),
            );
        }
    }
    state
        .store
        .lock()
        .map_err(internal)?
        .confirm_manifest_with_remote_branches(
            id,
            expected_revision,
            manifest,
            &observations,
            &remote_branches,
        )
        .map_err(storage_error)?;
    Ok(())
}

const MAX_IMPORT_TICKETS: usize = 1000;
const MAX_IMPORT_MEMORY: usize = 1000;
const MAX_IMPORT_PROMPTS: usize = 64;

fn import_refused(detail: impl std::fmt::Display) -> ApiError {
    ApiError(
        StatusCode::UNPROCESSABLE_ENTITY,
        format!("Import refused: {detail}"),
    )
}

/// A validated export document (see `integrations::export_project`).
struct ExportDocument {
    hash: String,
    exported_at: String,
    id: String,
    name: String,
    /// The exported manifest, renumbered to revision 1 for this machine.
    manifest: ProjectManifest,
    exported_manifest_revision: u64,
    identities: BTreeMap<String, RepositoryIdentity>,
    tickets: Vec<Ticket>,
    memory: Vec<MemoryRecord>,
    prompts: Vec<RenderedPrompt>,
}

fn parse_export(value: &Value) -> Result<ExportDocument, ApiError> {
    let fields = value
        .as_object()
        .ok_or_else(|| import_refused("the export must be a JSON object"))?;
    if fields.get("kind").and_then(Value::as_str) != Some(integrations::EXPORT_KIND) {
        return Err(import_refused(format!(
            "the document is not a {} export",
            integrations::EXPORT_KIND
        )));
    }
    if fields.get("schema_version").and_then(Value::as_str) != Some(SCHEMA_VERSION) {
        return Err(import_refused(format!(
            "the export schema version is not {SCHEMA_VERSION}"
        )));
    }
    let hash = fields
        .get(integrations::EXPORT_HASH_FIELD)
        .and_then(Value::as_str)
        .ok_or_else(|| {
            import_refused(
                "the export carries no export_hash; export it again with this runtime version",
            )
        })?;
    let computed = integrations::export_hash(value).map_err(internal)?;
    if computed != hash {
        return Err(import_refused(
            "the export hash does not match its content; the document was edited or truncated in transit, so export it again",
        ));
    }
    let project = fields
        .get("project")
        .and_then(Value::as_object)
        .ok_or_else(|| import_refused("the export names no project"))?;
    let id = project
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| valid_project_id(id))
        .ok_or_else(|| import_refused("the exported project ID must match ^[a-z][a-z0-9-]{0,63}$"))?
        .to_owned();
    let name = project
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty() && name.len() <= 200)
        .ok_or_else(|| import_refused("the exported project name must be 1..200 bytes"))?
        .to_owned();
    let mut manifest: ProjectManifest = fields
        .get("manifest")
        .filter(|manifest| !manifest.is_null())
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| import_refused(format!("the exported manifest is invalid: {e}")))?
        .ok_or_else(|| {
            import_refused("the export carries no confirmed manifest; confirm one and export again")
        })?;
    manifest
        .validate()
        .map_err(|e| import_refused(format!("the exported manifest is invalid: {e}")))?;
    if manifest.id != id {
        return Err(import_refused(
            "the exported manifest names a different project",
        ));
    }
    // Revisions are per machine: the import confirms the manifest as this
    // runtime's first revision, under the hashing every confirmation uses.
    let exported_manifest_revision = manifest.revision;
    manifest.revision = 1;
    let identities: BTreeMap<String, RepositoryIdentity> = fields
        .get("repository_identities")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| import_refused(format!("the exported repository identities are invalid: {e}")))?
        .ok_or_else(|| {
            import_refused("the export carries no repository identities; export it again with this runtime version")
        })?;
    let identity_ids: BTreeSet<&str> = identities.keys().map(String::as_str).collect();
    if identity_ids != manifest.repository_ids() {
        return Err(import_refused(
            "the exported repository identities do not name the manifest's repositories",
        ));
    }
    for identity in identities.values() {
        identity.validate().map_err(|e| {
            import_refused(format!("an exported repository identity is invalid: {e}"))
        })?;
    }
    let tickets: Vec<Ticket> = fields
        .get("tickets")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| import_refused(format!("the exported tickets are invalid: {e}")))?
        .unwrap_or_default();
    if tickets.len() > MAX_IMPORT_TICKETS {
        return Err(import_refused(format!(
            "an import carries at most {MAX_IMPORT_TICKETS} tickets"
        )));
    }
    let mut ticket_ids = BTreeSet::new();
    for ticket in &tickets {
        if ticket.project_id != id || !ticket_ids.insert(ticket.id.as_str()) {
            return Err(import_refused(
                "every exported ticket must belong to the exported project and have a unique ID",
            ));
        }
    }
    let memory: Vec<MemoryRecord> = fields
        .get("memory")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| import_refused(format!("the exported memory is invalid: {e}")))?
        .unwrap_or_default();
    if memory.len() > MAX_IMPORT_MEMORY {
        return Err(import_refused(format!(
            "an import carries at most {MAX_IMPORT_MEMORY} memory records"
        )));
    }
    let prompts: Vec<RenderedPrompt> = fields
        .get("prompts")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| import_refused(format!("the exported prompts are invalid: {e}")))?
        .unwrap_or_default();
    if prompts.len() > MAX_IMPORT_PROMPTS {
        return Err(import_refused(format!(
            "an import carries at most {MAX_IMPORT_PROMPTS} prompts"
        )));
    }
    let mut prompt_names = BTreeSet::new();
    if prompts
        .iter()
        .any(|prompt| !prompt_names.insert(prompt.name.as_str()))
    {
        return Err(import_refused("exported prompt names must be unique"));
    }
    Ok(ExportDocument {
        hash: hash.to_owned(),
        exported_at: fields
            .get("exported_at")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        id,
        name,
        manifest,
        exported_manifest_revision,
        identities,
        tickets,
        memory,
        prompts,
    })
}

/// Why a local checkout is not the repository the export describes. The rules
/// are registration's: an equivalent remote spelling is the same remote, and
/// the observed roots must contain every exported root.
fn checkout_mismatch(
    exported: &RepositoryIdentity,
    observed: &RepositoryIdentity,
) -> Option<String> {
    match (&exported.remote, &observed.remote) {
        (Some(expected), Some(actual)) if !remotes_equivalent(expected, actual) => {
            return Some(format!(
                "its origin remote {actual} is not the exported remote {expected}"
            ));
        }
        (Some(expected), None) => {
            return Some(format!(
                "it has no origin remote, while the export names {expected}"
            ));
        }
        (None, Some(_)) => {
            return Some("it has an origin remote, while the exported repository had none".into());
        }
        _ => {}
    }
    if !exported.shallow
        && !observed.shallow
        && !exported
            .root_commits
            .iter()
            .all(|root| observed.root_commits.contains(root))
    {
        return Some(
            "its root commits do not contain the exported root commits; this is a different repository or rewritten history".into(),
        );
    }
    if (exported.shallow || observed.shallow)
        && (exported.remote.is_none() || observed.remote.is_none())
    {
        return Some("a shallow clone needs a matching origin remote to be identified".into());
    }
    None
}

/// The `guidance` section text of a rendered prompt body (`## guidance\n…`),
/// or the whole rendered body as one guidance section when it had several.
fn prompt_guidance(body: &str) -> (String, bool) {
    let mut sections: Vec<(&str, String)> = Vec::new();
    for line in body.split_inclusive('\n') {
        if let Some(name) = line.strip_prefix("## ") {
            sections.push((name.trim_end_matches('\n'), String::new()));
        } else if let Some((_, text)) = sections.last_mut() {
            text.push_str(line);
        } else {
            return (body.to_owned(), true);
        }
    }
    match sections.as_slice() {
        [(name, text)] if *name == "guidance" => (text.trim_end_matches('\n').to_owned(), false),
        _ => (body.to_owned(), true),
    }
}

fn ticket_error(error: StorageError) -> ApiError {
    match error {
        StorageError::AlreadyExists(detail) => ApiError(
            StatusCode::CONFLICT,
            format!("ticket import conflict: {detail}"),
        ),
        other => storage_error(other),
    }
}

/// `POST /api/v1/projects/import`: register the exported project from this
/// machine's checkouts, confirm the exported manifest as revision 1, activate
/// its components and bring the exported tickets, project/process memory and
/// prompts in through the ordinary adapters under their intent ledgers. The
/// export's refusal rules apply to the document; an already registered project
/// ID is refused (v1 does not merge); the same key replays the recorded result
/// and resumes an import that stopped between steps.
pub async fn import_project(
    State(state): State<AppState>,
    Json(request): Json<ImportProject>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let key = request.idempotency_key.trim();
    if key.is_empty() || key.len() > 200 || key.chars().any(char::is_control) {
        return Err(bad_request(
            "an idempotency key of 1..200 bytes is required",
        ));
    }
    // Every per-item key derived below is bounded by hashing the parent key,
    // so a long import key cannot fail after the project was registered.
    let step_key = {
        use sha2::{Digest, Sha256};
        let digest = format!("{:x}", Sha256::digest(key.as_bytes()));
        let parent = digest[..24].to_owned();
        move |kind: &str, id: &str| -> String {
            let derived = format!("import-{parent}:{kind}:{id}");
            if derived.len() <= 200 {
                derived
            } else {
                format!("import-{parent}:{kind}:{:x}", Sha256::digest(id.as_bytes()))
            }
        }
    };
    let export = parse_export(&request.export)?;
    let id = export.id.clone();
    if request.repositories.is_empty() || request.repositories.len() > MAX_REPOSITORIES {
        return Err(bad_request("give between one and 32 repository checkouts"));
    }
    let mut ids = BTreeSet::new();
    for repository in &request.repositories {
        if !valid_project_id(&repository.id) || !ids.insert(repository.id.as_str()) {
            return Err(bad_request(
                "repository IDs must be unique and match ^[a-z][a-z0-9-]*$",
            ));
        }
        validate_root_path(&repository.path)?;
    }
    if ids != export.manifest.repository_ids() {
        return Err(import_refused(format!(
            "give a local checkout for exactly the exported repositories {:?}; got {:?}",
            export.manifest.repository_ids(),
            ids
        )));
    }
    // The export's own rules, with this machine's secrets, data directory,
    // home and the given checkout paths as the forbidden values.
    let forbidden = integrations::forbidden_values(
        &state,
        request
            .repositories
            .iter()
            .map(|repository| repository.path.clone())
            .collect(),
    );
    integrations::export_guard(&request.export, &forbidden)
        .map_err(|what| import_refused(format!("the export contains {what}")))?;
    if export.memory.iter().any(|record| record.scope == "user") {
        return Err(import_refused("the export contains user memory"));
    }
    for record in &export.memory {
        integrations::validate_memory_request(&record.scope, &record.content, &record.provenance)?;
    }
    let mut prompt_bodies = Vec::with_capacity(export.prompts.len());
    for prompt in &export.prompts {
        let (body, flattened) = prompt_guidance(&prompt.body);
        integrations::valid_prompt_request(&prompt.name, &body)?;
        prompt_bodies.push((body, flattened));
    }
    if state.components.is_none() && !(export.memory.is_empty() && export.prompts.is_empty()) {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "the export carries memory or prompts, and the pinned component toolchain is unavailable; run npm run components:setup and restart the runtime before importing".into(),
        ));
    }
    let mut observations = Vec::new();
    for repository in &request.repositories {
        let observation = state
            .git
            .inspect_for_registration(&PathBuf::from(&repository.path))
            .await
            .map_err(|e| match e {
                GitError::InvalidPath => import_refused(format!(
                    "repository {}: {} is not an existing Git repository root",
                    repository.id, repository.path
                )),
                other => git_error(&repository.id, other),
            })?;
        reject_data_dir_overlap(&state, &repository.id, &observation)?;
        if observations
            .iter()
            .any(|(_, existing): &(String, RepositoryObservation)| {
                existing.local_path == observation.local_path
            })
        {
            return Err(bad_request(
                "each repository must be a distinct checkout; two IDs point at the same root",
            ));
        }
        let exported = &export.identities[&repository.id];
        if let Some(reason) = checkout_mismatch(exported, &observation.identity) {
            return Err(import_refused(format!(
                "repository {}: the checkout at {} is not the exported repository: {reason}",
                repository.id, repository.path
            )));
        }
        observations.push((repository.id.clone(), observation));
    }
    // Serialize with every other component mutation for the whole import.
    let _components = state.component_locks.lock().await;
    let request_value = json!({
        "export_hash": export.hash,
        "repositories": observations.iter().map(|(id, o)| json!({"id": id, "path": o.local_path})).collect::<Vec<_>>(),
    });
    let ledger_conflict = || {
        ApiError(
            StatusCode::CONFLICT,
            format!("idempotency key {key} was already used for a different export or different checkouts of project {id}"),
        )
    };
    {
        let mut store = state.store.lock().map_err(internal)?;
        if store.get_project(&id).map_err(internal)?.is_some() {
            if store
                .component_operation(&id, key)
                .map_err(internal)?
                .is_none()
            {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    format!("project {id} is already registered; an import never merges into an existing project, so register the export under a new ID or remove the project first"),
                ));
            }
            let operation = store
                .begin_component_operation(&id, key, "project.import", &request_value, "import")
                .map_err(|_| ledger_conflict())?;
            if let Some(mut response) = operation.response {
                response["replayed"] = Value::Bool(true);
                return Ok((StatusCode::OK, Json(response)));
            }
            // The intent is recorded but a step died: resume below; every step
            // replays or reconciles by its derived key.
        } else {
            for (_, observation) in &observations {
                if let Some((project, repository)) = store
                    .registration_at_path(&observation.local_path)
                    .map_err(internal)?
                {
                    return Err(already_registered(
                        &observation.local_path,
                        &project,
                        &repository,
                    ));
                }
            }
            store
                .register_project(&id, &export.name, &observations)
                .map_err(storage_error)?;
            store
                .begin_component_operation(&id, key, "project.import", &request_value, "import")
                .map_err(|_| ledger_conflict())?;
        }
    }
    let manifest_hash = export.manifest.computed_hash().map_err(internal)?;
    let project = {
        let store = state.store.lock().map_err(internal)?;
        store
            .get_project(&id)
            .map_err(internal)?
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "project not found".into()))?
    };
    if project.revision == 0 {
        confirm_observed(&state, &id, 0, &export.manifest).await?;
    } else if project.manifest_hash.as_deref() != Some(manifest_hash.as_str()) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            format!("project {id} already has a confirmed manifest that is not the exported one"),
        ));
    }
    let mut ticket_mapping = Vec::new();
    for exported in &export.tickets {
        let ticket = Ticket {
            schema_version: SCHEMA_VERSION.into(),
            id: exported.id.clone(),
            project_id: id.clone(),
            revision: 1,
            source: TicketSource::LuunaSqliteTemporary,
            title: exported.title.clone(),
            intent: exported.intent.clone(),
            acceptance_criteria: exported.acceptance_criteria.clone(),
            repository_ids: exported.repository_ids.clone(),
            purpose: exported.purpose,
            ticket_type: exported.ticket_type,
            priority: exported.priority,
            labels: exported.labels.clone(),
            created_at: exported.created_at.clone(),
            updated_at: exported.updated_at.clone(),
        };
        ticket
            .validate()
            .map_err(|e| import_refused(format!("ticket {}: {e}", exported.id)))?;
        state
            .store
            .lock()
            .map_err(internal)?
            .create_ticket(&ticket, &step_key("ticket", &exported.id))
            .map_err(ticket_error)?;
        if exported.revision != 1 {
            ticket_mapping.push(json!({"id": exported.id, "exported_revision": exported.revision, "imported_revision": 1}));
        }
    }
    let mut memory_mapping = Vec::new();
    let mut prompt_mapping = Vec::new();
    let components_enabled = if state.components.is_some() {
        integrations::enable_project(&state, &id).await?;
        for record in &export.memory {
            let created = integrations::create_memory_locked(
                &state,
                &id,
                &step_key("memory", &record.id),
                &record.scope,
                &record.content,
                &record.provenance,
            )
            .await?;
            memory_mapping.push(json!({"exported_id": record.id, "imported_id": created.id, "scope": record.scope, "exported_hash": record.hash, "imported_hash": created.hash}));
        }
        for (prompt, (body, flattened)) in export.prompts.iter().zip(&prompt_bodies) {
            let created = integrations::create_prompt_locked(
                &state,
                &id,
                &step_key("prompt", &prompt.name),
                &prompt.name,
                body,
                None,
            )
            .await?;
            prompt_mapping.push(json!({"name": prompt.name, "exported_version": prompt.version, "imported_version": created.version, "exported_hash": prompt.hash, "imported_hash": created.hash, "sections_flattened": flattened}));
        }
        true
    } else {
        false
    };
    let counts = json!({"tickets": export.tickets.len(), "memory": export.memory.len(), "prompts": export.prompts.len()});
    let mapping = json!({
        "manifest": {"exported_revision": export.exported_manifest_revision, "imported_revision": 1},
        "tickets": ticket_mapping,
        "memory": memory_mapping,
        "prompts": prompt_mapping,
    });
    let mut response = json!({
        "export_hash": export.hash,
        "exported_at": export.exported_at,
        "manifest_hash": manifest_hash,
        "components_enabled": components_enabled,
        "counts": counts,
        "mapping": mapping,
        "replayed": false,
    });
    {
        let mut store = state.store.lock().map_err(internal)?;
        store
            .record_project_event(&id, "project.imported", response.clone())
            .map_err(storage_error)?;
    }
    response["project"] = serde_json::to_value(view(&state, &id)?).map_err(internal)?;
    state
        .store
        .lock()
        .map_err(internal)?
        .complete_component_operation(&id, key, &response)
        .map_err(internal)?;
    Ok((StatusCode::CREATED, Json(response)))
}

pub async fn project_events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(page): Query<crate::EventPage>,
) -> Result<Json<Vec<EventEnvelope>>, ApiError> {
    if !valid_project_id(&id) {
        return Err(ApiError(StatusCode::NOT_FOUND, "project not found".into()));
    }
    let (after, limit) = page.bounds()?;
    Ok(Json(
        state
            .store
            .lock()
            .map_err(internal)?
            .project_events_page(&id, after, limit.map(|limit| limit as u64))
            .map_err(storage_error)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::{checkout_mismatch, derive_project_id, prompt_guidance};
    use luuna_domain::RepositoryIdentity;

    #[test]
    fn prompt_guidance_extracts_one_section_and_flattens_several() {
        assert_eq!(
            prompt_guidance("## guidance\nBe brief.\n"),
            ("Be brief.".into(), false)
        );
        let several = "## guidance\nA\n## rules\nB\n";
        assert_eq!(prompt_guidance(several), (several.into(), true));
        assert_eq!(prompt_guidance("plain"), ("plain".into(), true));
    }

    #[test]
    fn checkout_mismatch_names_remote_and_root_differences() {
        let exported = RepositoryIdentity {
            remote: Some("https://example.invalid/fixture.git".into()),
            push_remote: None,
            root_commits: vec!["1".repeat(40)],
            shallow: false,
        };
        let same = RepositoryIdentity {
            remote: Some("ssh://git@example.invalid/fixture".into()),
            ..exported.clone()
        };
        // An SSH spelling is a different transport, so a different remote.
        assert!(checkout_mismatch(&exported, &same)
            .unwrap()
            .contains("origin remote"));
        let spelled = RepositoryIdentity {
            remote: Some("http://example.invalid/fixture".into()),
            ..exported.clone()
        };
        assert_eq!(checkout_mismatch(&exported, &spelled), None);
        let rewritten = RepositoryIdentity {
            root_commits: vec!["2".repeat(40)],
            ..exported.clone()
        };
        assert!(checkout_mismatch(&exported, &rewritten)
            .unwrap()
            .contains("root commits"));
        let grown = RepositoryIdentity {
            root_commits: vec!["1".repeat(40), "2".repeat(40)],
            ..exported.clone()
        };
        assert_eq!(checkout_mismatch(&exported, &grown), None);
    }

    #[test]
    fn project_ids_derive_from_names_safely() {
        assert_eq!(derive_project_id("My App 2"), "my-app-2");
        assert_eq!(derive_project_id("42 things"), "p-42-things");
        assert_eq!(derive_project_id("***"), "p");
        let digits = derive_project_id(&"7".repeat(80));
        assert_eq!(digits.len(), 64);
        assert!(luuna_domain::valid_project_id(&digits));
        let long = derive_project_id(&format!("{}-x", "a".repeat(63)));
        assert!(luuna_domain::valid_project_id(&long));
    }
}
