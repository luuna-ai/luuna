//! Choosing a project's store mode as a product action (ADR 0062): show the
//! recorded mode with what it can be changed to, and write `store.json`.
//!
//! A change is refused when it would leave Luuna-tracked records behind (a
//! ticket handed to Missions or a live memory binding in the current store)
//! or when the target side already holds records: records are never moved or
//! merged. Both mode cannot be chosen until it is usable. The write holds the
//! component lock, so no component operation runs against the old store while
//! the file changes.
use crate::{
    internal, not_found,
    stores::{check_checkout, project_store, StoreRefusal},
    ApiError, AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use luuna_domain::{valid_project_id, StoreMode};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path as FsPath, PathBuf};

/// Component stores below a store root, as ADR 0028 lays them out.
const STORES: [&str; 4] = ["missions", "atlas", "beacon", "roots"];
/// Files a freshly initialised store holds that are not records.
const NOT_RECORDS: [&str; 4] = ["config.yaml", "README.md", ".gitignore", ".gitattributes"];
/// Deepest store folder looked into, and most entries read, before the store
/// is treated as holding records.
const MAX_DEPTH: usize = 8;
const MAX_ENTRIES: usize = 10_000;

fn conflict(message: impl Into<String>) -> ApiError {
    ApiError(StatusCode::CONFLICT, message.into())
}
fn invalid(message: impl Into<String>) -> ApiError {
    ApiError(StatusCode::UNPROCESSABLE_ENTITY, message.into())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetStoreMode {
    pub mode: String,
    #[serde(default)]
    pub repository: Option<String>,
}

fn mode_value(mode: &StoreMode) -> Value {
    json!({"mode": mode.name(), "repository": mode.repository()})
}

/// The recorded mode (null when the file cannot be used), whether the store
/// resolves now and why not, and the modes that can be chosen.
fn describe(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let repositories = state
        .store
        .lock()
        .map_err(internal)?
        .repositories(id)
        .map_err(internal)?;
    let recorded = luuna_storage::read_project_store_mode(&state.config.data_dir, id).ok();
    let problem = match project_store(state, id) {
        Ok(_) => None,
        Err(ApiError(status, message)) if status == StatusCode::CONFLICT => Some(message),
        Err(error) => return Err(error),
    };
    let mut choices = vec![json!({"mode": "runtime", "repository": null})];
    choices.extend(
        repositories
            .iter()
            .map(|r| json!({"mode": "repository", "repository": r.repository_id})),
    );
    Ok(json!({
        "project_id": id,
        "mode": recorded.as_ref().map(StoreMode::name),
        "repository": recorded.as_ref().and_then(StoreMode::repository),
        "usable": problem.is_none(),
        "problem": problem,
        "choices": choices,
    }))
}

fn require_project(state: &AppState, id: &str) -> Result<(), ApiError> {
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

pub async fn show(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_project(&state, &id)?;
    Ok(Json(describe(&state, &id)?))
}

pub async fn set(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SetStoreMode>,
) -> Result<Json<Value>, ApiError> {
    require_project(&state, &id)?;
    let target =
        match (request.mode.as_str(), request.repository) {
            ("runtime", None) => StoreMode::Runtime,
            ("repository", Some(repository)) => StoreMode::Repository { repository },
            ("both", _) => {
                return Err(conflict(
                    "Both mode cannot be chosen yet; choose runtime or repository",
                ))
            }
            _ => return Err(invalid(
                "Choose mode runtime, or mode repository with the id of a registered repository",
            )),
        };
    let _guard = state.component_locks.lock().await;
    let (checkout, has_records) = {
        let store = state.store.lock().map_err(internal)?;
        let checkout = match target.repository() {
            None => None,
            Some(name) => Some(
                store
                    .repositories(&id)
                    .map_err(internal)?
                    .into_iter()
                    .find(|r| r.repository_id == name)
                    .map(|r| PathBuf::from(r.local_path))
                    .ok_or_else(|| invalid("That repository is not registered for this project"))?,
            ),
        };
        let mut tracked = !store
            .memory_bindings(&id, false)
            .map_err(internal)?
            .is_empty();
        for ticket in store.tickets(&id).map_err(internal)? {
            tracked |= store
                .missions_binding(&ticket.id)
                .map_err(internal)?
                .is_some();
        }
        (checkout, tracked)
    };
    let current = luuna_storage::read_project_store_mode(&state.config.data_dir, &id).ok();
    if current.as_ref() == Some(&target) {
        return Ok(Json(describe(&state, &id)?));
    }
    let root = match &checkout {
        Some(checkout) => {
            check_checkout(checkout, &state.config.data_dir).map_err(|refusal| match refusal {
                StoreRefusal::UnusableCheckout => conflict(
                    "That repository's checkout cannot hold its stores: it must be a main checkout with a .git directory, without links, outside the data directory",
                ),
                other => conflict(other.to_string()),
            })?;
            checkout.clone()
        }
        None => crate::stores::resolve_project_store(
            &state.config.data_dir,
            &id,
            &StoreMode::Runtime,
            None,
        )
        .map_err(|_| not_found())?
        .path()
        .to_owned(),
    };
    if has_records {
        return Err(conflict(
            "This project already keeps tickets or memory in its current store, and Luuna never moves or merges records; the store mode cannot change for it",
        ));
    }
    if let Some(component) = STORES
        .iter()
        .find(|component| holds_records(&root.join(".luuna").join(component)))
    {
        return Err(conflict(format!(
            "The chosen side's {component} store already holds records, and Luuna never moves or merges records; empty it or keep the current mode"
        )));
    }
    luuna_storage::write_project_store_mode(&state.config.data_dir, &id, &target)
        .map_err(internal)?;
    let from = current.as_ref().map(mode_value).unwrap_or(Value::Null);
    state
        .store
        .lock()
        .map_err(internal)?
        .record_project_event(
            &id,
            "project.store_mode.changed",
            json!({"from": from, "to": mode_value(&target)}),
        )
        .map_err(internal)?;
    Ok(Json(describe(&state, &id)?))
}

/// Whether a component store holds anything beyond what a fresh
/// initialisation writes: any file other than its configuration, readme and
/// ignore files that has a non-blank byte. A link, an unreadable entry or a
/// store too large to look through counts as holding records.
fn holds_records(store: &FsPath) -> bool {
    fn walk(dir: &FsPath, top: bool, depth: usize, seen: &mut usize) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return true;
        };
        for entry in entries {
            *seen += 1;
            let Ok(entry) = entry else { return true };
            let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
                return true;
            };
            if *seen > MAX_ENTRIES || metadata.file_type().is_symlink() {
                return true;
            }
            let name = entry.file_name();
            if metadata.is_dir() {
                if depth >= MAX_DEPTH || walk(&entry.path(), false, depth + 1, seen) {
                    return true;
                }
            } else if !(top && NOT_RECORDS.iter().any(|n| name == *n)) {
                match std::fs::read(entry.path()) {
                    Ok(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => {}
                    _ => return true,
                }
            }
        }
        false
    }
    match std::fs::symlink_metadata(store) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            walk(store, true, 0, &mut 0)
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::holds_records;
    use std::fs;

    #[test]
    fn only_records_count_as_records() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("missions");
        assert!(!holds_records(&store), "absent");
        fs::create_dir_all(&store).unwrap();
        for (name, text) in [
            ("config.yaml", "project: x\n"),
            ("README.md", "readme"),
            (".gitignore", "*.lock\n"),
            ("issues.jsonl", ""),
            ("plans.jsonl", "\n"),
        ] {
            fs::write(store.join(name), text).unwrap();
        }
        assert!(!holds_records(&store), "freshly initialised");
        fs::write(store.join("issues.jsonl"), "{\"id\":\"x\"}\n").unwrap();
        assert!(holds_records(&store), "one record");
        fs::write(store.join("issues.jsonl"), "").unwrap();
        fs::create_dir_all(store.join("human")).unwrap();
        fs::write(store.join("human/README.md"), "a note").unwrap();
        assert!(holds_records(&store), "a note below the top level");
    }
}
