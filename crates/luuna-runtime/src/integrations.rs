//! Explicit component activation, private memory and versioned prompt resources.
use crate::{internal, not_found, ApiError, AppState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use luuna_domain::{
    hash_value, valid_project_id, ApprovedPrompt, ComponentContext, MemoryReference,
    PromptReference,
};
use luuna_storage::{MemoryBinding, MemoryRetentionPolicy};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path as FsPath, PathBuf};

pub(crate) fn conflict(message: impl Into<String>) -> ApiError {
    ApiError(StatusCode::CONFLICT, message.into())
}
fn invalid(message: impl Into<String>) -> ApiError {
    ApiError(StatusCode::UNPROCESSABLE_ENTITY, message.into())
}
fn available(state: &AppState) -> Result<&luuna_components::Components, ApiError> {
    state.components.as_deref().ok_or_else(|| ApiError(StatusCode::SERVICE_UNAVAILABLE,
        "Pinned component toolchain unavailable; run npm run components:setup and restart the runtime".into()))
}
fn project(state: &AppState, id: &str) -> Result<(), ApiError> {
    if !valid_project_id(id) {
        return Err(not_found());
    }
    state
        .store
        .lock()
        .map_err(internal)?
        .get_project(id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    Ok(())
}
/// The project's component store, as the store resolver decides it.
pub(crate) fn workspace(state: &AppState, id: &str) -> Result<PathBuf, ApiError> {
    crate::stores::project_store(state, id)
}
pub(crate) fn enabled(state: &AppState, id: &str) -> Result<bool, ApiError> {
    project(state, id)?;
    match state
        .store
        .lock()
        .map_err(internal)?
        .component_state(id)
        .map_err(internal)?
        .as_deref()
    {
        Some("enabled") => Ok(true),
        Some("initializing") => Err(conflict(
            "Component activation is incomplete; retry activation before changing this project",
        )),
        _ => Ok(false),
    }
}
fn require_enabled(state: &AppState, id: &str) -> Result<(), ApiError> {
    if !enabled(state, id)? {
        return Err(conflict(
            "Enable project components before using memory or prompts",
        ));
    }
    available(state)?;
    Ok(())
}
fn private_directory(path: &FsPath, anchor: &FsPath) -> Result<(), ApiError> {
    let relative = path
        .strip_prefix(anchor)
        .map_err(|_| invalid("Invalid component workspace"))?;
    for ancestor in anchor.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        if let Ok(metadata) = std::fs::symlink_metadata(ancestor) {
            let link = metadata.file_type().is_symlink();
            #[cfg(windows)]
            let link = {
                use std::os::windows::fs::MetadataExt;
                link || metadata.file_attributes() & 0x400 != 0
            };
            if link || !metadata.is_dir() {
                return Err(conflict(
                    "Component data directory contains a link or non-directory",
                ));
            }
        }
    }
    let mut current = anchor.to_owned();
    for part in relative.components() {
        current.push(part);
        if let Ok(metadata) = std::fs::symlink_metadata(&current) {
            let link = metadata.file_type().is_symlink();
            #[cfg(windows)]
            let link = {
                use std::os::windows::fs::MetadataExt;
                link || metadata.file_attributes() & 0x400 != 0
            };
            if link || !metadata.is_dir() {
                return Err(conflict(
                    "Component workspace contains a link or non-directory",
                ));
            }
        }
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|_| invalid("Cannot create private component workspace"))?;
    Ok(())
}
async fn call(
    state: &AppState,
    component: &str,
    directory: &FsPath,
    args: Vec<String>,
) -> Result<Value, ApiError> {
    available(state)?.call(component,directory,&args).await.map_err(|error| {
        // Diagnostic data for the operator's log only; the API answer stays
        // generic so component output never reaches a client.
        eprintln!("luuna-runtime: {component} call failed: {error}");
        ApiError(StatusCode::SERVICE_UNAVAILABLE,
        format!("{component} refused or unavailable; verify the pinned toolchain and managed store"))
    })
}
pub async fn status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let path = workspace(&state, &id)?;
    let phase = state
        .store
        .lock()
        .map_err(internal)?
        .component_state(&id)
        .map_err(internal)?;
    let reason = if state.components.is_none() {
        Some("Pinned component toolchain unavailable; run npm run components:setup and restart")
    } else if phase.as_deref() == Some("initializing") {
        Some("Activation is incomplete; retry to reconcile it")
    } else {
        None
    };
    Ok(Json(
        json!({"enabled":phase.as_deref()==Some("enabled"),"workspace_path":path,"reason":reason}),
    ))
}
pub async fn enable(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let _guard = state.component_locks.lock().await;
    enable_project(&state, &id).await?;
    drop(_guard);
    status(State(state), Path(id)).await
}
/// Activation itself, for a caller that already holds the component lock:
/// initializes the three private stores and migrates legacy tickets while the
/// project is frozen in `initializing`. Repeating it is harmless.
pub(crate) async fn enable_project(state: &AppState, id: &str) -> Result<(), ApiError> {
    let directory = workspace(state, id)?;
    available(state)?;
    state
        .store
        .lock()
        .map_err(internal)?
        .begin_component_enable(id)
        .map_err(internal)?;
    private_directory(&directory, &state.config.data_dir)?;
    // Stores created before ADR 0028 move to `.luuna/<component>` first.
    luuna_components::adopt_legacy_layout(&directory).map_err(internal)?;
    for (component, marker) in [
        ("missions", ".luuna/missions/config.yaml"),
        ("atlas", ".luuna/atlas/config.yaml"),
        ("beacon", ".luuna/beacon/config.yaml"),
    ] {
        if !directory.join(marker).exists() {
            call(state, component, &directory, vec!["init".into()]).await?;
        }
    }
    crate::missions::migrate_project(state, id).await?;
    state
        .store
        .lock()
        .map_err(internal)?
        .finish_component_enable(id)
        .map_err(internal)?;
    Ok(())
}

#[derive(Deserialize)]
pub struct MemoryQuery {
    #[serde(default)]
    pub include_user: bool,
    pub q: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateMemory {
    pub idempotency_key: String,
    pub scope: String,
    pub content: String,
    pub provenance: String,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct MemoryRecord {
    pub id: String,
    pub scope: String,
    pub content: String,
    pub provenance: String,
    pub hash: String,
    pub created_at: String,
}
fn memory_workspace(state: &AppState, binding: &MemoryBinding) -> Result<PathBuf, ApiError> {
    if binding.scope == "user" {
        Ok(crate::stores::user_store(state))
    } else {
        workspace(state, &binding.project_id)
    }
}
async fn atlas_records(state: &AppState, directory: &FsPath) -> Result<Vec<Value>, ApiError> {
    let result = call(
        state,
        "atlas",
        directory,
        vec!["search".into(), "--type".into(), "convention".into()],
    )
    .await?;
    let domains = result["domains"]
        .as_array()
        .ok_or_else(|| invalid("Atlas returned an incompatible search contract"))?;
    let mut records = Vec::new();
    for domain in domains {
        for record in domain["matches"]
            .as_array()
            .ok_or_else(|| invalid("Atlas returned incompatible records"))?
        {
            records.push(json!({"domain":domain["domain"],"record":record}));
        }
    }
    Ok(records)
}
async fn resolved_memory(
    state: &AppState,
    binding: &MemoryBinding,
) -> Result<MemoryRecord, ApiError> {
    let directory = memory_workspace(state, binding)?;
    let rows = atlas_records(state, &directory).await?;
    memory_from_rows(binding, &rows)
}
fn memory_from_rows(binding: &MemoryBinding, rows: &[Value]) -> Result<MemoryRecord, ApiError> {
    let record = rows
        .iter()
        .find(|row| {
            row["record"]["id"].as_str() == Some(&binding.source_id)
                && row["domain"].as_str() == Some(&binding.domain)
        })
        .ok_or_else(|| {
            conflict("Atlas memory was removed or moved externally; refresh before continuing")
        })?;
    let raw = &record["record"];
    let envelope: Value = serde_json::from_str(
        raw["content"]
            .as_str()
            .ok_or_else(|| conflict("Atlas memory schema changed"))?,
    )
    .map_err(|_| conflict("Atlas memory schema changed externally"))?;
    if envelope["id"].as_str() != Some(&binding.id)
        || envelope["scope"].as_str() != Some(&binding.scope)
    {
        return Err(conflict(
            "Atlas memory identity or scope changed externally",
        ));
    }
    Ok(MemoryRecord {
        id: binding.id.clone(),
        scope: binding.scope.clone(),
        content: envelope["content"]
            .as_str()
            .ok_or_else(|| invalid("Memory content is invalid"))?
            .into(),
        provenance: envelope["provenance"]
            .as_str()
            .ok_or_else(|| invalid("Memory provenance is invalid"))?
            .into(),
        hash: hash_value(raw).map_err(internal)?,
        created_at: binding.created_at.clone(),
    })
}
async fn memory_records(
    state: &AppState,
    project_id: &str,
    include_user: bool,
) -> Result<Vec<MemoryRecord>, ApiError> {
    let bindings = state
        .store
        .lock()
        .map_err(internal)?
        .memory_bindings(project_id, include_user)
        .map_err(internal)?;
    let mut records = Vec::new();
    let mut snapshots = std::collections::BTreeMap::new();
    for binding in bindings {
        let directory = memory_workspace(state, &binding)?;
        if !snapshots.contains_key(&directory) {
            snapshots.insert(directory.clone(), atlas_records(state, &directory).await?);
        }
        records.push(memory_from_rows(&binding, &snapshots[&directory])?);
    }
    Ok(records)
}
pub async fn list_memory(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MemoryQuery>,
) -> Result<Json<Vec<MemoryRecord>>, ApiError> {
    require_enabled(&state, &id)?;
    let _guard = state.component_locks.lock().await;
    let mut records = memory_records(&state, &id, query.include_user).await?;
    if let Some(query) = query.q {
        if query.len() > 500 {
            return Err(invalid("Memory queries are bounded to 500 bytes"));
        }
        let needle = query.to_lowercase();
        records.retain(|record| {
            record.content.to_lowercase().contains(&needle)
                || record.provenance.to_lowercase().contains(&needle)
        });
    }
    Ok(Json(records))
}
pub async fn create_memory(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<CreateMemory>,
) -> Result<Json<MemoryRecord>, ApiError> {
    require_enabled(&state, &id)?;
    validate_memory_request(&request.scope, &request.content, &request.provenance)?;
    let _guard = state.component_locks.lock().await;
    Ok(Json(
        create_memory_locked(
            &state,
            &id,
            &request.idempotency_key,
            &request.scope,
            &request.content,
            &request.provenance,
        )
        .await?,
    ))
}
pub(crate) fn validate_memory_request(
    scope: &str,
    content: &str,
    provenance: &str,
) -> Result<(), ApiError> {
    if !matches!(scope, "project" | "process" | "user")
        || content.trim().is_empty()
        || content.len() > 16_000
        || provenance.trim().is_empty()
        || provenance.len() > 1000
    {
        return Err(invalid(
            "Memory requires a valid scope, bounded content and explicit provenance",
        ));
    }
    Ok(())
}
/// Creates one memory under the caller's component lock. Durable intent is
/// recorded before the Atlas write and reconciled by identity on retry.
pub(crate) async fn create_memory_locked(
    state: &AppState,
    id: &str,
    idempotency_key: &str,
    scope: &str,
    content: &str,
    provenance: &str,
) -> Result<MemoryRecord, ApiError> {
    let request_value = json!({"scope":scope,"content":content,"provenance":provenance});
    let operation = state
        .store
        .lock()
        .map_err(internal)?
        .begin_component_operation(
            id,
            idempotency_key,
            "memory.create",
            &request_value,
            &format!("mem-{}", uuid::Uuid::new_v4()),
        )
        .map_err(|_| conflict("Memory idempotency key conflicts with a previous request"))?;
    if let Some(response) = operation.response {
        return serde_json::from_value(response).map_err(internal);
    }
    let directory = if scope == "user" {
        crate::stores::user_store(state)
    } else {
        workspace(state, id)?
    };
    private_directory(&directory, &state.config.data_dir)?;
    luuna_components::adopt_legacy_layout(&directory).map_err(internal)?;
    if !directory.join(".luuna/atlas/config.yaml").exists() {
        call(state, "atlas", &directory, vec!["init".into()]).await?;
    }
    let domain = format!("luuna-{scope}");
    let payload =
        json!({"id":operation.record_id,"scope":scope,"content":content,"provenance":provenance});
    let rows = atlas_records(state, &directory).await?;
    let matching: Vec<_> = rows
        .iter()
        .filter(|row| {
            row["record"]["content"]
                .as_str()
                .and_then(|text| serde_json::from_str::<Value>(text).ok())
                .is_some_and(|value| value["id"] == operation.record_id)
        })
        .collect();
    if matching.len() > 1 {
        return Err(conflict(
            "Duplicate Atlas delivery identities require source reconciliation",
        ));
    }
    let raw = if let Some(found) = matching.first() {
        if found["domain"].as_str() != Some(&domain)
            || found["record"]["content"].as_str() != Some(&payload.to_string())
        {
            return Err(conflict(
                "Uncertain memory write differs from the original request",
            ));
        }
        found["record"].clone()
    } else {
        if operation.attempted {
            return Err(conflict("Uncertain Atlas write cannot be reconciled by its identity; inspect the managed source before retrying"));
        }
        state
            .store
            .lock()
            .map_err(internal)?
            .mark_component_operation_attempted(id, idempotency_key)
            .map_err(internal)?;
        let response = call(
            state,
            "atlas",
            &directory,
            vec![
                "record".into(),
                domain.clone(),
                payload.to_string(),
                "--type".into(),
                "convention".into(),
                "--tags".into(),
                operation.record_id.clone(),
                // Luuna records carry their own identity inside the
                // envelope; Atlas's content-similarity guard compares the
                // envelopes and would refuse any two short memories. Explicit
                // developer records are never duplicates by Luuna's policy.
                "--force".into(),
            ],
        )
        .await?;
        response["record"].clone()
    };
    let source_id = raw["id"]
        .as_str()
        .ok_or_else(|| invalid("Atlas did not identify its created record"))?;
    let created = chrono::Utc::now().to_rfc3339();
    let binding = MemoryBinding {
        id: operation.record_id,
        project_id: id.into(),
        scope: scope.into(),
        source_id: source_id.into(),
        domain,
        created_at: created,
        deleted: false,
    };
    state
        .store
        .lock()
        .map_err(internal)?
        .record_memory_binding(&binding)
        .map_err(internal)?;
    let result = resolved_memory(state, &binding).await?;
    state
        .store
        .lock()
        .map_err(internal)?
        .complete_component_operation(
            id,
            idempotency_key,
            &serde_json::to_value(&result).map_err(internal)?,
        )
        .map_err(internal)?;
    Ok(result)
}
#[derive(Deserialize)]
pub struct DeleteMemoryQuery {
    pub expected_hash: String,
}
pub async fn delete_memory(
    State(state): State<AppState>,
    Path((project_id, id)): Path<(String, String)>,
    Query(query): Query<DeleteMemoryQuery>,
) -> Result<Json<Value>, ApiError> {
    require_enabled(&state, &project_id)?;
    let _guard = state.component_locks.lock().await;
    Ok(Json(
        delete_memory_locked(&state, &project_id, &id, &query.expected_hash).await?,
    ))
}
fn valid_memory_hash(hash: &str) -> bool {
    hash.starts_with("sha256:")
        && hash.len() == 71
        && hash[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
}
/// Deletes one memory under the caller's component lock; repeated calls with
/// the same displayed hash are idempotent even after the binding is tombstoned.
async fn delete_memory_locked(
    state: &AppState,
    project_id: &str,
    id: &str,
    expected_hash: &str,
) -> Result<Value, ApiError> {
    let binding = state
        .store
        .lock()
        .map_err(internal)?
        .memory_binding(project_id, id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    if !valid_memory_hash(expected_hash) {
        return Err(invalid("A valid displayed memory hash is required"));
    }
    let key = format!("memory-delete:{id}:{expected_hash}");
    let operation = state
        .store
        .lock()
        .map_err(internal)?
        .begin_component_operation(
            project_id,
            &key,
            "memory.delete",
            &json!({"id":id,"expected_hash":expected_hash}),
            id,
        )
        .map_err(internal)?;
    if let Some(response) = operation.response {
        return Ok(response);
    }
    let directory = memory_workspace(state, &binding)?;
    let rows = atlas_records(state, &directory).await?;
    let present = rows.iter().any(|row| {
        row["record"]["id"].as_str() == Some(&binding.source_id)
            && row["domain"].as_str() == Some(&binding.domain)
    });
    if present {
        let current = resolved_memory(state, &binding).await?;
        if current.hash != expected_hash {
            return Err(conflict(
                "Memory changed since it was displayed; refresh before deleting",
            ));
        }
        state
            .store
            .lock()
            .map_err(internal)?
            .mark_component_operation_attempted(project_id, &key)
            .map_err(internal)?;
        call(
            state,
            "atlas",
            &directory,
            vec!["delete".into(), binding.domain, binding.source_id],
        )
        .await?;
    }
    // An absent bound source is already deleted, including an acknowledged CLI
    // delete whose response or local tombstone was lost during a restart.
    state
        .store
        .lock()
        .map_err(internal)?
        .delete_memory_binding(id)
        .map_err(internal)?;
    let result = json!({"deleted":true,"id":id});
    state
        .store
        .lock()
        .map_err(internal)?
        .complete_component_operation(project_id, &key, &result)
        .map_err(internal)?;
    Ok(result)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateMemory {
    pub idempotency_key: String,
    /// The hash the client displayed; a changed memory is not corrected blindly.
    pub expected_hash: String,
    pub content: String,
}
/// `PUT /api/v1/projects/{id}/memory/{memory_id}`: correct a memory record's
/// content in place. The record keeps its identity and scope; its provenance
/// names the correction and the hash it replaced, so the history of the fact
/// stays readable. The write goes through the Atlas CLI under the durable
/// component-operation ledger: a retry after an uncertain write finds the
/// corrected envelope in the store instead of writing twice. A corrected record
/// that an approved plan selected no longer matches that plan's
/// `component_context`, so the existing source checks refuse implementation,
/// validation and publication of that plan.
pub async fn update_memory(
    State(state): State<AppState>,
    Path((project_id, id)): Path<(String, String)>,
    Json(request): Json<UpdateMemory>,
) -> Result<Json<MemoryRecord>, ApiError> {
    require_enabled(&state, &project_id)?;
    if !valid_memory_hash(&request.expected_hash) {
        return Err(invalid("A valid displayed memory hash is required"));
    }
    if request.content.trim().is_empty() || request.content.len() > 16_000 {
        return Err(invalid("Memory requires bounded, nonempty content"));
    }
    let _guard = state.component_locks.lock().await;
    let binding = state
        .store
        .lock()
        .map_err(internal)?
        .memory_binding(&project_id, &id)
        .map_err(internal)?
        .filter(|binding| !binding.deleted)
        .ok_or_else(not_found)?;
    let request_value =
        json!({"id":id,"expected_hash":request.expected_hash,"content":request.content});
    let operation = state
        .store
        .lock()
        .map_err(internal)?
        .begin_component_operation(
            &project_id,
            &request.idempotency_key,
            "memory.update",
            &request_value,
            &id,
        )
        .map_err(|_| conflict("Memory idempotency key conflicts with a previous request"))?;
    if let Some(response) = operation.response {
        return Ok(Json(serde_json::from_value(response).map_err(internal)?));
    }
    let directory = memory_workspace(&state, &binding)?;
    let current = resolved_memory(&state, &binding).await?;
    let correction = format!("corrected from {}", request.expected_hash);
    // An acknowledged CLI write whose response was lost already holds the
    // corrected content under this correction's provenance.
    let written = current.content == request.content && current.provenance.starts_with(&correction);
    if !written {
        if current.hash != request.expected_hash {
            return Err(conflict(
                "Memory changed since it was displayed; refresh before correcting it",
            ));
        }
        if operation.attempted {
            return Err(conflict("Uncertain Atlas write cannot be reconciled by its identity; inspect the managed source before retrying"));
        }
        let provenance = clip_provenance(&format!("{correction}: {}", current.provenance));
        let payload = json!({"id":binding.id,"scope":binding.scope,"content":request.content,"provenance":provenance});
        state
            .store
            .lock()
            .map_err(internal)?
            .mark_component_operation_attempted(&project_id, &request.idempotency_key)
            .map_err(internal)?;
        call(
            &state,
            "atlas",
            &directory,
            vec![
                "edit".into(),
                binding.domain.clone(),
                binding.source_id.clone(),
                "--content".into(),
                payload.to_string(),
            ],
        )
        .await?;
    }
    let result = resolved_memory(&state, &binding).await?;
    if result.content != request.content || result.scope != binding.scope {
        return Err(conflict(
            "Uncertain memory write differs from the original request",
        ));
    }
    state
        .store
        .lock()
        .map_err(internal)?
        .complete_component_operation(
            &project_id,
            &request.idempotency_key,
            &serde_json::to_value(&result).map_err(internal)?,
        )
        .map_err(internal)?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromoteMemory {
    pub idempotency_key: String,
    /// The hash the client displayed; a changed memory is not promoted blindly.
    pub expected_hash: String,
    /// `project` for a process memory, `user` for a project memory.
    pub to_scope: String,
}
/// Promotion widens a memory's scope one step (process -> project -> user) by
/// creating the record in the wider store with provenance naming its origin,
/// then deleting the narrower record. Both steps are reconciled through the
/// durable intent ledger, so a retry after a crash finishes rather than
/// duplicates. Demotion does not exist: narrowing is a delete and a new record.
pub async fn promote_memory(
    State(state): State<AppState>,
    Path((project_id, id)): Path<(String, String)>,
    Json(request): Json<PromoteMemory>,
) -> Result<Json<Value>, ApiError> {
    require_enabled(&state, &project_id)?;
    if !valid_memory_hash(&request.expected_hash) {
        return Err(invalid("A valid displayed memory hash is required"));
    }
    let _guard = state.component_locks.lock().await;
    let binding = state
        .store
        .lock()
        .map_err(internal)?
        .memory_binding(&project_id, &id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    let allowed = matches!(
        (binding.scope.as_str(), request.to_scope.as_str()),
        ("process", "project") | ("project", "user")
    );
    if !allowed {
        return Err(invalid(format!(
            "Memory promotes one step at a time: process to project, or project to user; {} memory cannot be promoted to {}",
            binding.scope, request.to_scope
        )));
    }
    let request_value =
        json!({"id":id,"expected_hash":request.expected_hash,"to_scope":request.to_scope});
    let operation = state
        .store
        .lock()
        .map_err(internal)?
        .begin_component_operation(
            &project_id,
            &request.idempotency_key,
            "memory.promote",
            &request_value,
            &id,
        )
        .map_err(|_| conflict("Memory idempotency key conflicts with a previous request"))?;
    if let Some(response) = operation.response {
        return Ok(Json(response));
    }
    // A tombstoned source with an unfinished promotion is a crashed promotion
    // being resumed: the wider record already exists under the derived key.
    let current = if binding.deleted {
        if !operation.attempted {
            return Err(not_found());
        }
        None
    } else {
        let current = resolved_memory(&state, &binding).await?;
        if current.hash != request.expected_hash {
            return Err(conflict(
                "Memory changed since it was displayed; refresh before promoting",
            ));
        }
        Some(current)
    };
    state
        .store
        .lock()
        .map_err(internal)?
        .mark_component_operation_attempted(&project_id, &request.idempotency_key)
        .map_err(internal)?;
    let create_key = format!("{}:promote-create", request.idempotency_key);
    let promoted = match current {
        Some(current) => {
            let origin = format!(
                "promoted from {} memory {}: {}",
                binding.scope, binding.id, current.provenance
            );
            create_memory_locked(
                &state,
                &project_id,
                &create_key,
                &request.to_scope,
                &current.content,
                &clip_provenance(&origin),
            )
            .await?
        }
        None => {
            // The source is gone: only a recorded create can finish this promotion.
            let finished = state
                .store
                .lock()
                .map_err(internal)?
                .component_operation_response(&project_id, &create_key)
                .map_err(internal)?;
            match finished {
                Some(response) => serde_json::from_value(response).map_err(internal)?,
                None => {
                    return Err(conflict(
                        "Promotion cannot be reconciled: the source memory is gone and no promoted record was recorded; create the memory again in the wider scope",
                    ))
                }
            }
        }
    };
    if !binding.deleted {
        delete_memory_locked(&state, &project_id, &id, &request.expected_hash).await?;
    }
    let result = json!({"promoted":promoted,"from":{"id":id,"scope":binding.scope}});
    state
        .store
        .lock()
        .map_err(internal)?
        .complete_component_operation(&project_id, &request.idempotency_key, &result)
        .map_err(internal)?;
    Ok(Json(result))
}
fn clip_provenance(text: &str) -> String {
    if text.len() <= 1000 {
        return text.to_owned();
    }
    let mut end = 997;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

/// A project export carries what another machine or a teammate may receive:
/// the confirmed machine-independent manifest, ticket projections, project and
/// process memory and the current prompts. It never carries user memory,
/// credentials or this host's absolute paths; an export that would is refused
/// rather than redacted.
pub async fn export_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    project(&state, &id)?;
    let (record, manifest, tickets, local_paths, identities) = {
        let store = state.store.lock().map_err(internal)?;
        let record = store
            .get_project(&id)
            .map_err(internal)?
            .ok_or_else(not_found)?;
        let manifest = store.current_manifest(&id).map_err(internal)?;
        let tickets = store.tickets(&id).map_err(internal)?;
        let registrations = store.repositories(&id).map_err(internal)?;
        let local_paths: Vec<String> = registrations
            .iter()
            .map(|registration| registration.local_path.clone())
            .collect();
        // The registered identity (remote and root commits) is machine
        // independent; an import compares it with the checkout it is given.
        let identities: std::collections::BTreeMap<String, luuna_domain::RepositoryIdentity> =
            registrations
                .into_iter()
                .map(|registration| {
                    (
                        registration.repository_id,
                        registration.observation.identity,
                    )
                })
                .collect();
        (record, manifest, tickets, local_paths, identities)
    };
    let components_enabled = enabled(&state, &id)? && state.components.is_some();
    let (memory, prompts) = if components_enabled {
        let _guard = state.component_locks.lock().await;
        let mut memory = memory_records(&state, &id, false).await?;
        memory.retain(|record| record.scope != "user");
        let names: Vec<String> = prompt_records(&state, &id)
            .await?
            .iter()
            .filter_map(|value| value["name"].as_str().map(str::to_owned))
            .take(64)
            .collect();
        let mut prompts = Vec::new();
        for name in names {
            prompts.push(render(&state, &id, &name, None).await?);
        }
        (memory, prompts)
    } else {
        (Vec::new(), Vec::new())
    };
    let mut export = json!({
        "schema_version": luuna_domain::SCHEMA_VERSION,
        "kind": EXPORT_KIND,
        "exported_at": chrono::Utc::now().to_rfc3339(),
        "project": {"id": record.id, "name": record.name, "manifest_revision": record.revision, "manifest_hash": record.manifest_hash},
        "manifest": manifest.map(|m| m.manifest),
        "repository_identities": identities,
        "tickets": tickets,
        "memory": memory,
        "prompts": prompts,
        "omitted": ["user memory", "credentials", "machine registry paths", "worktrees and private artifacts"],
    });
    let forbidden = forbidden_values(&state, local_paths);
    export_guard(&export, &forbidden).map_err(|what| {
        conflict(format!(
            "Export refused: it would include {what}; remove it from the ticket or memory text first"
        ))
    })?;
    // The content hash covers every other field; an import recomputes it and
    // refuses a document that was edited in transit.
    let hash = export_hash(&export).map_err(internal)?;
    export[EXPORT_HASH_FIELD] = Value::String(hash);
    Ok(Json(export))
}

pub(crate) const EXPORT_KIND: &str = "luuna-project-export";
pub(crate) const EXPORT_HASH_FIELD: &str = "export_hash";

/// The `luuna-json-v1` hash of an export document without its own hash field.
pub(crate) fn export_hash(export: &Value) -> Result<String, luuna_domain::DomainError> {
    let mut body = export.clone();
    if let Some(fields) = body.as_object_mut() {
        fields.remove(EXPORT_HASH_FIELD);
    }
    hash_value(&body)
}

/// Exact values no export may carry and no import may bring in: this host's
/// credential values, its runtime data directory, the given checkout paths and
/// the home directory.
pub(crate) fn forbidden_values(
    state: &AppState,
    local_paths: Vec<String>,
) -> Vec<(String, &'static str)> {
    let mut forbidden: Vec<(String, &str)> = crate::writer::known_secrets(state)
        .into_iter()
        .map(|secret| (secret, "a credential value"))
        .collect();
    forbidden.push((
        state.config.data_dir.to_string_lossy().into_owned(),
        "the runtime data directory path",
    ));
    forbidden.extend(
        local_paths
            .into_iter()
            .map(|path| (path, "a registered checkout path")),
    );
    for name in ["HOME", "USERPROFILE"] {
        if let Some(home) = std::env::var_os(name) {
            forbidden.push((
                home.to_string_lossy().into_owned(),
                "the home directory path",
            ));
        }
    }
    forbidden
}

/// The exact values no export may carry: every known credential value, the
/// runtime data directory, every registered checkout path of every project and
/// the home directory.
fn forbidden_export_values(state: &AppState) -> Result<Vec<(String, &'static str)>, ApiError> {
    let mut forbidden: Vec<(String, &str)> = crate::writer::known_secrets(state)
        .into_iter()
        .map(|secret| (secret, "a credential value"))
        .collect();
    forbidden.push((
        state.config.data_dir.to_string_lossy().into_owned(),
        "the runtime data directory path",
    ));
    let store = state.store.lock().map_err(internal)?;
    for project in store.projects().map_err(internal)? {
        for registration in store.repositories(&project.id).map_err(internal)? {
            forbidden.push((registration.local_path, "a registered checkout path"));
        }
    }
    drop(store);
    for name in ["HOME", "USERPROFILE"] {
        if let Some(home) = std::env::var_os(name) {
            forbidden.push((
                home.to_string_lossy().into_owned(),
                "the home directory path",
            ));
        }
    }
    Ok(forbidden)
}

/// `GET /api/v1/memory/user/export`: the personal export the project export
/// refuses to carry. It returns every user-scope record with its provenance
/// from the one private user store (`<data_dir>/components/user`), never a
/// project's memory, and is refused outright when a record carries a
/// credential value or a path of this host, exactly as the project export is
/// (ADR 0021 point 2), because a personal export still travels between
/// machines.
pub async fn export_user_memory(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    available(&state)?;
    let _guard = state.component_locks.lock().await;
    let bindings = state
        .store
        .lock()
        .map_err(internal)?
        .user_memory_bindings()
        .map_err(internal)?;
    let mut memory = Vec::new();
    if !bindings.is_empty() {
        let directory = crate::stores::user_store(&state);
        let rows = atlas_records(&state, &directory).await?;
        for binding in &bindings {
            memory.push(memory_from_rows(binding, &rows)?);
        }
    }
    let export = json!({
        "schema_version": luuna_domain::SCHEMA_VERSION,
        "kind": "luuna-user-memory-export",
        "exported_at": chrono::Utc::now().to_rfc3339(),
        "scope": "user",
        "memory": memory,
        "omitted": ["project and process memory", "tickets and prompts", "credentials", "machine registry paths", "worktrees and private artifacts"],
    });
    let forbidden = forbidden_export_values(&state)?;
    export_guard_with(&export, &forbidden, true).map_err(|what| {
        conflict(format!(
            "Export refused: it would include {what}; correct or delete that memory first"
        ))
    })?;
    Ok(Json(export))
}

/// `GET /api/v1/projects/{id}/memory/retention`: the project's policy.
pub async fn memory_retention(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MemoryRetentionPolicy>, ApiError> {
    project(&state, &id)?;
    Ok(Json(
        state
            .store
            .lock()
            .map_err(internal)?
            .memory_retention(&id)
            .map_err(internal)?,
    ))
}

/// Longest retention a policy may name, in days.
const MAX_RETENTION_DAYS: u32 = 3650;

/// `PUT /api/v1/projects/{id}/memory/retention`: replace the policy. Setting
/// it deletes nothing; `POST …/retention/apply` does, explicitly.
pub async fn set_memory_retention(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(policy): Json<MemoryRetentionPolicy>,
) -> Result<Json<MemoryRetentionPolicy>, ApiError> {
    project(&state, &id)?;
    for days in [policy.process_max_age_days, policy.project_max_age_days]
        .into_iter()
        .flatten()
    {
        if days == 0 || days > MAX_RETENTION_DAYS {
            return Err(invalid(format!(
                "a retention limit is between 1 and {MAX_RETENTION_DAYS} days, or null to keep records"
            )));
        }
    }
    Ok(Json(
        state
            .store
            .lock()
            .map_err(internal)?
            .set_memory_retention(&id, &policy)
            .map_err(crate::projects::storage_error)?,
    ))
}

/// Whether a record of `scope` created at `created_at` is older than the
/// policy allows at `now`. An unparseable timestamp never expires a record.
pub(crate) fn retention_expired(
    policy: &MemoryRetentionPolicy,
    scope: &str,
    created_at: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    let Some(days) = policy.max_age_days(scope) else {
        return false;
    };
    let Ok(created) = chrono::DateTime::parse_from_rfc3339(created_at) else {
        return false;
    };
    now.signed_duration_since(created.with_timezone(&chrono::Utc))
        > chrono::Duration::days(i64::from(days))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyRetention {
    pub idempotency_key: String,
}
/// `POST /api/v1/projects/{id}/memory/retention/apply`: delete every project
/// or process record older than the policy allows, each through the
/// reconciled delete of ADR 0014 under the intent ledger, and record
/// `memory.retention_applied` naming what was deleted. User memory is never
/// touched by a project policy. Replaying the key returns the recorded result;
/// a retry after a crash deletes only what is still present.
pub async fn apply_memory_retention(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<ApplyRetention>,
) -> Result<Json<Value>, ApiError> {
    require_enabled(&state, &id)?;
    let _guard = state.component_locks.lock().await;
    let (policy, bindings) = {
        let store = state.store.lock().map_err(internal)?;
        (
            store.memory_retention(&id).map_err(internal)?,
            store.memory_bindings(&id, false).map_err(internal)?,
        )
    };
    let operation = state
        .store
        .lock()
        .map_err(internal)?
        .begin_component_operation(
            &id,
            &request.idempotency_key,
            "memory.retention.apply",
            &json!({"policy": policy}),
            &id,
        )
        .map_err(|_| conflict("Retention idempotency key conflicts with a previous request"))?;
    if let Some(response) = operation.response {
        return Ok(Json(response));
    }
    let now = chrono::Utc::now();
    let mut deleted = Vec::new();
    for binding in bindings
        .iter()
        .filter(|binding| retention_expired(&policy, &binding.scope, &binding.created_at, now))
    {
        let current = resolved_memory(&state, binding).await?;
        delete_memory_locked(&state, &id, &binding.id, &current.hash).await?;
        deleted.push(json!({"id": binding.id, "scope": binding.scope, "created_at": binding.created_at, "hash": current.hash}));
    }
    let result = json!({"applied_at": now.to_rfc3339(), "policy": policy, "deleted": deleted});
    state
        .store
        .lock()
        .map_err(internal)?
        .record_project_event(&id, "memory.retention_applied", result.clone())
        .map_err(internal)?;
    state
        .store
        .lock()
        .map_err(internal)?
        .complete_component_operation(&id, &request.idempotency_key, &result)
        .map_err(internal)?;
    Ok(Json(result))
}

/// Fails closed on any string that carries a forbidden exact value, an
/// absolute path of this host's shapes, or a user-scoped memory.
pub(crate) fn export_guard(value: &Value, forbidden: &[(String, &str)]) -> Result<(), String> {
    export_guard_with(value, forbidden, false)
}

/// [`export_guard`] with the user-scope refusal optional: the personal user
/// memory export carries user memory by definition and still refuses
/// credential values and host paths.
pub(crate) fn export_guard_with(
    value: &Value,
    forbidden: &[(String, &str)],
    allow_user_scope: bool,
) -> Result<(), String> {
    fn absolute_path(text: &str) -> bool {
        let bytes = text.as_bytes();
        let drive = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/');
        drive
            || text.starts_with("\\\\")
            || [
                "/home/",
                "/Users/",
                "/root/",
                "/tmp/",
                "/var/",
                "/mnt/",
                "/workspace/",
            ]
            .iter()
            .any(|prefix| text.starts_with(prefix))
    }
    fn walk(
        value: &Value,
        forbidden: &[(String, &str)],
        allow_user_scope: bool,
        under_scope: bool,
    ) -> Result<(), String> {
        match value {
            Value::String(text) => {
                for (needle, label) in forbidden {
                    if !needle.is_empty() && text.contains(needle.as_str()) {
                        return Err((*label).to_owned());
                    }
                }
                if under_scope && !allow_user_scope && text == "user" {
                    return Err("user memory".to_owned());
                }
                for token in
                    text.split(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '`')
                {
                    if absolute_path(token) {
                        return Err("an absolute path".to_owned());
                    }
                }
                Ok(())
            }
            Value::Array(items) => items
                .iter()
                .try_for_each(|item| walk(item, forbidden, allow_user_scope, false)),
            Value::Object(fields) => fields.iter().try_for_each(|(key, item)| {
                walk(item, forbidden, allow_user_scope, key == "scope")
            }),
            _ => Ok(()),
        }
    }
    walk(value, forbidden, allow_user_scope, false)
}

#[cfg(test)]
mod export_tests {
    use super::export_guard;
    use serde_json::json;

    #[test]
    fn export_guard_refuses_user_memory_paths_and_secrets() {
        let forbidden = vec![
            ("super-secret-token-value".to_owned(), "a credential value"),
            (
                "C:\\Users\\dev\\repo".to_owned(),
                "a registered checkout path",
            ),
        ];
        let clean = json!({"memory":[{"scope":"project","content":"use the shared logger","provenance":"team convention"}],"tickets":[{"intent":"relative paths like src/main.py are fine"}]});
        assert!(export_guard(&clean, &forbidden).is_ok());
        let user = json!({"memory":[{"scope":"user","content":"private"}]});
        assert_eq!(export_guard(&user, &forbidden).unwrap_err(), "user memory");
        let secret = json!({"tickets":[{"intent":"key super-secret-token-value leaked"}]});
        assert_eq!(
            export_guard(&secret, &forbidden).unwrap_err(),
            "a credential value"
        );
        let registered = json!({"note":"see C:\\Users\\dev\\repo\\x"});
        assert_eq!(
            export_guard(&registered, &forbidden).unwrap_err(),
            "a registered checkout path"
        );
        for text in [
            "D:\\other\\place",
            "/home/dev/other",
            "/Users/dev/x",
            "\\\\server\\share",
        ] {
            let value = json!({"tickets":[{"intent":format!("look in {text}")}]});
            assert_eq!(
                export_guard(&value, &forbidden).unwrap_err(),
                "an absolute path",
                "{text}"
            );
        }
        // Only a `scope` field says "user" about a memory; the word elsewhere is text.
        assert!(export_guard(&json!({"tags":["user"]}), &forbidden).is_ok());
        // The personal export carries user memory by definition and still refuses the rest.
        assert!(super::export_guard_with(&user, &forbidden, true).is_ok());
        assert_eq!(
            super::export_guard_with(&secret, &forbidden, true).unwrap_err(),
            "a credential value"
        );
        assert_eq!(
            super::export_guard_with(
                &json!({"memory":[{"scope":"user","content":"see /home/dev/notes"}]}),
                &forbidden,
                true
            )
            .unwrap_err(),
            "an absolute path"
        );
    }

    #[test]
    fn retention_expires_only_the_scopes_the_policy_names_and_never_user_memory() {
        use super::retention_expired;
        use luuna_storage::MemoryRetentionPolicy;
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-22T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let policy = MemoryRetentionPolicy {
            process_max_age_days: Some(30),
            project_max_age_days: None,
        };
        assert!(retention_expired(
            &policy,
            "process",
            "2026-09-01T00:00:00Z",
            now
        ));
        assert!(!retention_expired(
            &policy,
            "process",
            "2026-10-01T00:00:00Z",
            now
        ));
        assert!(!retention_expired(
            &policy,
            "project",
            "2020-01-01T00:00:00Z",
            now
        ));
        assert!(!retention_expired(
            &policy,
            "user",
            "2020-01-01T00:00:00Z",
            now
        ));
        assert!(!retention_expired(
            &policy,
            "process",
            "not a timestamp",
            now
        ));
        assert!(!retention_expired(
            &MemoryRetentionPolicy::default(),
            "process",
            "2020-01-01T00:00:00Z",
            now
        ));
        let both = MemoryRetentionPolicy {
            process_max_age_days: Some(1),
            project_max_age_days: Some(400),
        };
        assert!(retention_expired(
            &both,
            "project",
            "2025-01-01T00:00:00Z",
            now
        ));
        assert!(!retention_expired(
            &both,
            "project",
            "2026-01-01T00:00:00Z",
            now
        ));
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePrompt {
    pub idempotency_key: String,
    pub name: String,
    pub body: String,
    #[serde(default)]
    pub expected_version: Option<u64>,
}
#[derive(Deserialize)]
pub struct PromptQuery {
    pub version: Option<u64>,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct RenderedPrompt {
    pub name: String,
    pub version: u64,
    pub hash: String,
    pub body: String,
}
fn valid_prompt_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}
async fn prompt_records(state: &AppState, id: &str) -> Result<Vec<Value>, ApiError> {
    let value = call(state, "beacon", &workspace(state, id)?, vec!["list".into()]).await?;
    value["prompts"]
        .as_array()
        .cloned()
        .ok_or_else(|| invalid("Beacon returned an incompatible prompt list"))
}
async fn render(
    state: &AppState,
    id: &str,
    name: &str,
    version: Option<u64>,
) -> Result<RenderedPrompt, ApiError> {
    if !valid_prompt_name(name) {
        return Err(invalid(
            "Prompt names must be bounded letters, digits, dashes or underscores",
        ));
    }
    // A historical pin is not permission to silently ignore a changed current prompt.
    let records = prompt_records(state, id).await?;
    let current = records
        .iter()
        .find(|value| value["name"].as_str() == Some(name))
        .ok_or_else(not_found)?;
    let current_version = current["version"]
        .as_u64()
        .ok_or_else(|| invalid("Beacon prompt version is invalid"))?;
    if version.is_some_and(|expected| expected != current_version) {
        return Err(conflict(
            "Beacon prompt changed; preview and select its current version",
        ));
    }
    let raw = call(
        state,
        "beacon",
        &workspace(state, id)?,
        vec!["render".into(), format!("{name}@{current_version}")],
    )
    .await?;
    // Beacon render resolves content without applying its optional schemas.
    // Bind every contributing source and schema so changes that preserve the
    // displayed text still invalidate the selected prompt's approval evidence.
    let chain = raw["resolvedFrom"]
        .as_array()
        .filter(|chain| !chain.is_empty() && chain.len() <= 64)
        .ok_or_else(|| conflict("Beacon returned an incompatible prompt source chain"))?;
    let directory = workspace(state, id)?;
    let mut sources = Vec::new();
    let mut schemas = std::collections::BTreeMap::new();
    for source_name in chain {
        let source_name = source_name
            .as_str()
            .ok_or_else(|| conflict("Beacon prompt source identity is invalid"))?;
        if sources
            .iter()
            .any(|source: &Value| source["name"].as_str() == Some(source_name))
        {
            continue;
        }
        let source=records.iter().find(|source| source["name"].as_str()==Some(source_name))
            .ok_or_else(|| conflict("Beacon prompt inheritance changed or contains an archived source; refresh before continuing"))?;
        if let Some(schema) = source.get("schema").filter(|schema| !schema.is_null()) {
            let schema_name = schema
                .as_str()
                .filter(|name| !name.is_empty() && name.len() <= 100 && !name.starts_with('-'))
                .ok_or_else(|| conflict("Beacon prompt schema identity is invalid"))?;
            let validation=call(state,"beacon",&directory,vec!["validate".into(),source_name.into()]).await
                .map_err(|_| conflict("Beacon prompt schema validation failed; repair the managed prompt or schema before continuing"))?;
            if validation["valid"] != true
                || !validation["errors"].as_array().is_some_and(Vec::is_empty)
                || !validation["warnings"].as_array().is_some_and(Vec::is_empty)
            {
                return Err(conflict("Beacon prompt schema validation returned errors or warnings; repair it before continuing"));
            }
            if !schemas.contains_key(schema_name) {
                let shown = call(
                    state,
                    "beacon",
                    &directory,
                    vec!["schema".into(), "show".into(), schema_name.into()],
                )
                .await
                .map_err(|_| {
                    conflict("Beacon prompt schema is unavailable; repair it before continuing")
                })?;
                let schema = shown["schema"]
                    .as_object()
                    .filter(|schema| {
                        schema.get("name").and_then(Value::as_str) == Some(schema_name)
                    })
                    .ok_or_else(|| conflict("Beacon returned an incompatible schema record"))?;
                schemas.insert(schema_name.to_owned(), Value::Object(schema.clone()));
            }
        }
        sources.push(source.clone());
    }
    let sections = raw["sections"]
        .as_array()
        .ok_or_else(|| invalid("Beacon rendered an incompatible prompt"))?;
    let mut body = String::new();
    for section in sections {
        let name = section["name"]
            .as_str()
            .ok_or_else(|| invalid("Beacon section name is invalid"))?;
        let text = section["body"]
            .as_str()
            .ok_or_else(|| invalid("Beacon section body is invalid"))?;
        body.push_str(&format!("## {name}\n{text}\n"));
    }
    if body.len() > 24_000 {
        return Err(invalid(
            "Rendered prompt exceeds the approved resource limit",
        ));
    }
    Ok(RenderedPrompt {
        name: name.into(),
        version: current_version,
        hash: hash_value(&json!({"rendered":raw,"sources":sources,"schemas":schemas}))
            .map_err(internal)?,
        body,
    })
}
pub async fn list_prompts(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_enabled(&state, &id)?;
    let _guard = state.component_locks.lock().await;
    let rows = prompt_records(&state, &id).await?;
    Ok(Json(
        rows.into_iter()
            .map(|value| json!({"name":value["name"],"version":value["version"]}))
            .collect(),
    ))
}
pub async fn render_prompt(
    State(state): State<AppState>,
    Path((id, name)): Path<(String, String)>,
    Query(query): Query<PromptQuery>,
) -> Result<Json<RenderedPrompt>, ApiError> {
    require_enabled(&state, &id)?;
    let _guard = state.component_locks.lock().await;
    Ok(Json(render(&state, &id, &name, query.version).await?))
}
pub async fn create_prompt(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<CreatePrompt>,
) -> Result<Json<RenderedPrompt>, ApiError> {
    require_enabled(&state, &id)?;
    valid_prompt_request(&request.name, &request.body)?;
    let _guard = state.component_locks.lock().await;
    Ok(Json(
        create_prompt_locked(
            &state,
            &id,
            &request.idempotency_key,
            &request.name,
            &request.body,
            request.expected_version,
        )
        .await?,
    ))
}
pub(crate) fn valid_prompt_request(name: &str, body: &str) -> Result<(), ApiError> {
    if !valid_prompt_name(name) || body.trim().is_empty() || body.len() > 16_000 {
        return Err(invalid(
            "Prompts require a bounded name and nonempty content",
        ));
    }
    Ok(())
}
/// Creates or updates one prompt under the caller's component lock. Durable
/// intent is recorded before the Beacon write and reconciled on retry through
/// the `luuna_request` frontmatter identity.
pub(crate) async fn create_prompt_locked(
    state: &AppState,
    id: &str,
    idempotency_key: &str,
    name: &str,
    body: &str,
    expected_version: Option<u64>,
) -> Result<RenderedPrompt, ApiError> {
    let request = CreatePrompt {
        idempotency_key: idempotency_key.to_owned(),
        name: name.to_owned(),
        body: body.to_owned(),
        expected_version,
    };
    let operation=state.store.lock().map_err(internal)?.begin_component_operation(id,&request.idempotency_key,"prompt.create",&json!({"name":request.name,"body":request.body,"expected_version":request.expected_version}),&format!("prompt-{}",uuid::Uuid::new_v4())).map_err(|_| conflict("Prompt idempotency key conflicts with a previous request"))?;
    if let Some(response) = operation.response {
        return serde_json::from_value(response).map_err(internal);
    }
    let records = prompt_records(state, id).await?;
    if let Some(existing) = records
        .iter()
        .find(|value| value["name"].as_str() == Some(&request.name))
    {
        if existing["frontmatter"]["luuna_request"].as_str() != Some(&operation.record_id) {
            if operation.attempted {
                return Err(conflict("Uncertain Beacon update cannot be reconciled; inspect the managed source before retrying"));
            }
            let expected=request.expected_version.filter(|version| *version>0).ok_or_else(|| conflict("A Beacon prompt with that name already exists; preview its current version before updating"))?;
            if existing["version"].as_u64() != Some(expected) {
                return Err(conflict(
                    "Beacon prompt changed since preview; refresh before updating",
                ));
            }
            state
                .store
                .lock()
                .map_err(internal)?
                .mark_component_operation_attempted(id, &request.idempotency_key)
                .map_err(internal)?;
            call(
                state,
                "beacon",
                &workspace(state, id)?,
                vec![
                    "update".into(),
                    request.name.clone(),
                    "--section".into(),
                    format!("guidance={}", request.body),
                    "--fm".into(),
                    format!("luuna_request={}", operation.record_id),
                ],
            )
            .await?;
        }
    } else {
        if request.expected_version.is_some_and(|version| version != 0) {
            return Err(conflict("The Beacon prompt to update no longer exists"));
        }
        if operation.attempted {
            return Err(conflict("Uncertain Beacon create cannot be reconciled; inspect the managed source before retrying"));
        }
        state
            .store
            .lock()
            .map_err(internal)?
            .mark_component_operation_attempted(id, &request.idempotency_key)
            .map_err(internal)?;
        call(
            state,
            "beacon",
            &workspace(state, id)?,
            vec![
                "create".into(),
                "--name".into(),
                request.name.clone(),
                "--section".into(),
                format!("guidance={}", request.body),
                "--fm".into(),
                format!("luuna_request={}", operation.record_id),
            ],
        )
        .await?;
    }
    let result = render(state, id, &request.name, None).await?;
    if result.version != request.expected_version.unwrap_or(0).saturating_add(1) {
        return Err(conflict(
            "Beacon advanced during the operation; refresh and reconcile before retrying",
        ));
    }
    let records = prompt_records(state, id).await?;
    let written = records
        .iter()
        .find(|record| record["name"].as_str() == Some(&request.name))
        .ok_or_else(|| conflict("Beacon prompt disappeared during the operation"))?;
    if written["frontmatter"]["luuna_request"].as_str() != Some(&operation.record_id)
        || !written["sections"].as_array().is_some_and(|sections| {
            sections.iter().any(|section| {
                section["name"] == "guidance" && section["body"].as_str() == Some(&request.body)
            })
        })
    {
        return Err(conflict(
            "Uncertain prompt write differs from the original request",
        ));
    }
    state
        .store
        .lock()
        .map_err(internal)?
        .complete_component_operation(
            id,
            &request.idempotency_key,
            &serde_json::to_value(&result).map_err(internal)?,
        )
        .map_err(internal)?;
    Ok(result)
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SelectContext {
    #[serde(default)]
    pub memory_ids: Vec<String>,
    pub prompt: Option<PromptReference>,
}
pub(crate) async fn resolve_selection(
    state: &AppState,
    id: &str,
    selection: Option<&SelectContext>,
) -> Result<Option<ComponentContext>, ApiError> {
    let Some(selection) = selection else {
        return Ok(None);
    };
    require_enabled(state, id)?;
    if selection.memory_ids.len() > 16 {
        return Err(invalid("At most sixteen memories may be selected"));
    }
    let bindings = state
        .store
        .lock()
        .map_err(internal)?
        .memory_bindings(id, true)
        .map_err(internal)?;
    let mut memory = Vec::new();
    for selected in &selection.memory_ids {
        if memory
            .iter()
            .any(|value: &MemoryReference| &value.id == selected)
        {
            return Err(invalid("Duplicate memory selection"));
        }
        let binding = bindings
            .iter()
            .find(|value| &value.id == selected)
            .ok_or_else(|| invalid("Selected memory is absent or belongs to another project"))?;
        let record = resolved_memory(state, binding).await?;
        memory.push(MemoryReference {
            id: record.id,
            scope: record.scope,
            hash: record.hash,
            content: record.content,
            provenance: record.provenance,
        });
    }
    let prompt = if let Some(reference) = &selection.prompt {
        let rendered = render(state, id, &reference.name, Some(reference.version)).await?;
        if rendered.hash != reference.hash {
            return Err(conflict(
                "Beacon prompt changed since preview; select it again",
            ));
        }
        Some(ApprovedPrompt {
            name: rendered.name,
            version: rendered.version,
            hash: rendered.hash,
            body: rendered.body,
        })
    } else {
        None
    };
    let context = ComponentContext { memory, prompt };
    if serde_json::to_vec(&context).map_err(internal)?.len() > 16_000 {
        return Err(invalid(
            "Selected component context exceeds the 16000-byte session resource limit",
        ));
    }
    Ok(Some(context))
}
pub(crate) async fn resolve_approved(
    state: &AppState,
    id: &str,
    context: Option<&ComponentContext>,
) -> Result<Option<Value>, ApiError> {
    let Some(context) = context else {
        return Ok(None);
    };
    require_enabled(state, id)?;
    let bindings = state
        .store
        .lock()
        .map_err(internal)?
        .memory_bindings(id, context.memory.iter().any(|value| value.scope == "user"))
        .map_err(internal)?;
    let mut selected = Vec::new();
    for reference in &context.memory {
        let binding = bindings
            .iter()
            .find(|value| value.id == reference.id)
            .ok_or_else(|| conflict("Approved memory was deleted or moved; create a new plan"))?;
        let record = resolved_memory(state, binding).await?;
        if record.hash != reference.hash
            || record.scope != reference.scope
            || record.content != reference.content
            || record.provenance != reference.provenance
        {
            return Err(conflict(
                "Approved Atlas memory changed; create and approve a new plan",
            ));
        }
        selected.push(json!({"id":record.id,"scope":record.scope,"provenance":record.provenance,"content":record.content,"hash":record.hash}));
    }
    let prompt = if let Some(reference) = &context.prompt {
        let rendered = render(state, id, &reference.name, Some(reference.version)).await?;
        if rendered.hash != reference.hash || rendered.body != reference.body {
            return Err(conflict(
                "Approved Beacon prompt changed; create and approve a new plan",
            ));
        }
        Some(rendered)
    } else {
        None
    };
    let value = json!({"memory":selected,"prompt":prompt});
    if value.to_string().len() > 16_000 {
        return Err(invalid(
            "Selected component context exceeds the 16000-byte session resource limit",
        ));
    }
    Ok(Some(value))
}

pub(crate) async fn verify_plan_sources(
    state: &AppState,
    plan: &luuna_domain::PlanRevision,
) -> Result<Option<Value>, ApiError> {
    let _guard = state.component_locks.lock().await;
    let ticket = state
        .store
        .lock()
        .map_err(internal)?
        .get_ticket(&plan.ticket_id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    crate::missions::ensure_current(state, &ticket).await?;
    if ticket.revision != plan.ticket_revision {
        return Err(conflict(
            "Ticket changed since plan approval; create a new plan",
        ));
    }
    resolve_approved(state, &plan.project_id, plan.component_context.as_ref()).await
}
