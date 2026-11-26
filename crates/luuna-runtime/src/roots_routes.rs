//! Bearer-authenticated intent reads and explicit human decisions.
use crate::{
    internal,
    roots::{AcceptRequest, HumanActor, RootsStore},
    ApiError, AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HumanRequest {
    by: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Accept {
    by: String,
    rel: Option<String>,
    keep: Option<String>,
    reason: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Reject {
    by: String,
    reason: Option<String>,
}

// Core errors can include a canonical path different from the resolver's
// lexical root. Only fixed messages cross this API boundary.
fn safe(error: ApiError) -> ApiError {
    // The store resolver's refusals name no path and say what to change, so
    // they reach the developer as written.
    if error.0 == StatusCode::CONFLICT && error.1.starts_with("This project's store mode is") {
        return error;
    }
    let message = match error.0 {
        StatusCode::NOT_FOUND => "not found",
        StatusCode::CONFLICT if error.1 == "Roots is not set up for this project" => {
            "Roots is not set up for this project"
        }
        StatusCode::CONFLICT if error.1 == "Roots is already set up for this project" => {
            "Roots is already set up for this project"
        }
        StatusCode::CONFLICT => "This intent operation conflicts with the current Roots store",
        StatusCode::FORBIDDEN => "Only an explicit human name can decide intent",
        StatusCode::UNPROCESSABLE_ENTITY => "The intent request is invalid",
        _ => "The intent operation failed; see the runtime log",
    };
    ApiError(error.0, message.into())
}
fn response<T: Serialize>(result: Result<T, ApiError>) -> Result<Json<Value>, ApiError> {
    serde_json::to_value(result.map_err(safe)?)
        .map(Json)
        .map_err(internal)
}
pub(crate) async fn show(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let store = RootsStore::for_project(&state, &id).map_err(safe)?;
    let settings = store.config().await.map_err(safe)?;
    let graph = store.graph().await.map_err(safe)?;
    Ok(Json(
        json!({"settings":settings,"nodes":graph.nodes,"edges":graph.edges}),
    ))
}
pub(crate) async fn init(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<HumanRequest>,
) -> Result<Json<Value>, ApiError> {
    let human = HumanActor::local_user(&body.by)?;
    crate::roots_init::init(&state, &id, &human)
        .await
        .map_err(safe)?;
    show(State(state), Path(id)).await
}
pub(crate) async fn proposals(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    response(
        RootsStore::for_project(&state, &id)
            .map_err(safe)?
            .pending_proposals()
            .await,
    )
}
pub(crate) async fn questions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    response(
        RootsStore::for_project(&state, &id)
            .map_err(safe)?
            .questions()
            .await,
    )
}
pub(crate) async fn sprouts(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    response(
        RootsStore::for_project(&state, &id)
            .map_err(safe)?
            .open_sprouts()
            .await,
    )
}
pub(crate) async fn accept(
    State(state): State<AppState>,
    Path((id, proposal)): Path<(String, String)>,
    Json(body): Json<Accept>,
) -> Result<Json<Value>, ApiError> {
    let human = HumanActor::local_user(&body.by)?;
    let _guard = state.component_locks.lock().await;
    response(
        RootsStore::for_project(&state, &id)
            .map_err(safe)?
            .accept_proposal(
                &human,
                &proposal,
                AcceptRequest {
                    rel: body.rel,
                    keep: body.keep,
                    reason: body.reason,
                },
            )
            .await,
    )
}
pub(crate) async fn reject(
    State(state): State<AppState>,
    Path((id, proposal)): Path<(String, String)>,
    Json(body): Json<Reject>,
) -> Result<Json<Value>, ApiError> {
    let human = HumanActor::local_user(&body.by)?;
    let _guard = state.component_locks.lock().await;
    response(
        RootsStore::for_project(&state, &id)
            .map_err(safe)?
            .reject_proposal(&human, &proposal, body.reason)
            .await,
    )
}
pub(crate) async fn reject_sprout(
    State(state): State<AppState>,
    Path((id, sprout)): Path<(String, String)>,
    Json(body): Json<Reject>,
) -> Result<Json<Value>, ApiError> {
    let human = HumanActor::local_user(&body.by)?;
    let _guard = state.component_locks.lock().await;
    response(
        RootsStore::for_project(&state, &id)
            .map_err(safe)?
            .reject_sprout(&human, &sprout, body.reason)
            .await,
    )
}
