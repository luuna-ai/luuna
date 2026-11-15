//! Roots, called natively: the runtime's in-process API over roots-core.
//!
//! Every call reaches the project's own Roots store, which lives in the
//! component store the store resolver picks
//! (`<data_dir>/components/projects/<id>/.luuna/roots`). There is no upward
//! search and no checkout's `.luuna` folder is ever read. This module does
//! not create a Roots store: a project without `.luuna/roots/config.yaml` in
//! its store is refused with 409.
//!
//! Actors. roots-core takes the acting identity as a plain string and does not
//! check it on accept or reject, so this module owns the check. The runtime
//! builds both identities itself and never takes one from the process
//! environment, from git configuration or from a `.luuna` file:
//!
//! - A human operation takes a [`HumanActor`]. Its trust anchor is the
//!   runtime's bearer-authenticated local API: the token is held by the
//!   clients the user drives and is never in an agent process's environment.
//!   [`HumanActor::local_user`] may be called only from a handler behind the
//!   bearer-authenticated router, with the name supplied the way a plan
//!   approval's `approved_by` is supplied today.
//! - An agent operation takes an [`AgentActor`], which only
//!   [`AgentActor::for_run`] builds, from a run the runtime itself recorded.
//!
//! No route reaches this module yet: s2-roots-intent adds the routes, decides
//! where the human name comes from, and must keep the human routes
//! unreachable from agent sessions.
//!
//! Every operation is async and runs roots-core on the blocking pool, because
//! roots-core takes file locks that sleep while they wait. Errors map to
//! statuses as follows: not found 404, conflict 409, guard 403, invalid input
//! 422, anything else 500 with the cause in the runtime log only. No message
//! carries a host path.
use crate::{internal, not_found, ApiError, AppState};
use axum::http::StatusCode;
use roots_core::actor::{is_agent_actor, is_human_actor, parse_actor, ActorKind};
use roots_core::config::{load_config, RootsConfig};
use roots_core::decisions::{accept_proposal, reject_proposal, AcceptOptions, Decision};
use roots_core::errors::{ErrorCode, RootsError, RootsResult};
use roots_core::graph::read_graph;
use roots_core::model::{EdgeRecord, NodeRecord, ProposalKind, ProposalRecord, QuestionRecord};
use roots_core::paths::{roots_paths, RootsPaths};
use roots_core::proposals::{pending_proposals, propose_as_agent, read_proposals, ProposeInput};
use roots_core::questions::{ask_agent_question, read_questions, AskInput};
use roots_core::resolve::{resolve_node, ResolveOptions};
use roots_core::sprouts::{file_sprout, open_sprouts, reject_sprout, SproutInput};
use roots_core::time_crate::OffsetDateTime;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The longest human name accepted, in bytes; the same bound a plan
/// approval's `approved_by` has.
const MAX_HUMAN_NAME_BYTES: usize = 200;

const NOT_SET_UP: &str = "Roots is not set up for this project";

/// A human actor, `human:<name>`, built only by [`HumanActor::local_user`].
///
/// The field is private, so code outside this module cannot make a human out
/// of an arbitrary string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HumanActor(String);

impl HumanActor {
    /// The local user named `name`, which is a plain name (`alice`) or
    /// `human:<name>`; either way the actor is `human:<name>`.
    ///
    /// Call this only from a handler behind the bearer-authenticated router,
    /// with the name supplied the way a plan approval's `approved_by` is
    /// supplied. Never pass a name read from the process environment, from git
    /// configuration, from a `.luuna` file or from an agent session.
    ///
    /// Refuses with 403 anything roots-core classifies as an agent or as
    /// Roots itself (`agent:…`, `roots`, `roots:…`), and with 422 an empty,
    /// over-long or otherwise invalid name.
    pub fn local_user(name: &str) -> Result<Self, ApiError> {
        if name.len() > MAX_HUMAN_NAME_BYTES {
            return Err(invalid_human_name());
        }
        let actor = if name.starts_with("human:") {
            name.to_owned()
        } else {
            match parse_actor(name) {
                Some(parsed) if parsed.kind != ActorKind::Human => return Err(not_a_human()),
                Some(_) => name.to_owned(),
                None if name.contains(':') => {
                    // Something like `agent:` with an invalid name: still not
                    // a plain name, and never a human.
                    return Err(
                        if name.starts_with("agent:") || name.starts_with("roots:") {
                            not_a_human()
                        } else {
                            invalid_human_name()
                        },
                    );
                }
                None => format!("human:{name}"),
            }
        };
        if !is_human_actor(&actor) {
            return Err(invalid_human_name());
        }
        Ok(Self(actor))
    }

    /// The actor string, `human:<name>`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn not_a_human() -> ApiError {
    ApiError(
        StatusCode::FORBIDDEN,
        "only a human can do this; an agent or Roots actor is refused".into(),
    )
}

fn invalid_human_name() -> ApiError {
    ApiError(
        StatusCode::UNPROCESSABLE_ENTITY,
        "a human name must be a plain name or human:<name>: letters, digits, '.', '_' or '-', starting with a letter or digit, at most 200 bytes".into(),
    )
}

/// An agent actor, `agent:run-<run id>`, built only by
/// [`AgentActor::for_run`] from a run the runtime recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentActor {
    actor: String,
    project_id: String,
}

impl AgentActor {
    /// The agent of run `run_id`, which must be recorded in the runtime's own
    /// store for project `project_id`. An unknown run, or a run of another
    /// project, is not found (404). No request text, environment variable or
    /// file can name an agent identity.
    pub fn for_run(state: &AppState, project_id: &str, run_id: &str) -> Result<Self, ApiError> {
        if !luuna_domain::valid_project_id(project_id) {
            return Err(not_found());
        }
        let run = state
            .store
            .lock()
            .map_err(internal)?
            .get_run(run_id)
            .map_err(internal)?
            .ok_or_else(not_found)?;
        if run.project_id != project_id {
            return Err(not_found());
        }
        let actor = format!("agent:run-{}", run.id);
        if !is_agent_actor(&actor) {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "this run's id cannot form a Roots agent actor".into(),
            ));
        }
        Ok(Self {
            actor,
            project_id: project_id.to_owned(),
        })
    }

    /// The actor string, `agent:run-<run id>`.
    pub fn as_str(&self) -> &str {
        &self.actor
    }

    /// The project whose run this agent is.
    pub fn project_id(&self) -> &str {
        &self.project_id
    }
}

/// A project's Roots store, resolved through the store resolver.
#[derive(Clone, Debug)]
pub struct RootsStore {
    project_id: String,
    paths: RootsPaths,
}

/// The graph: every node and every edge.
#[derive(Clone, Debug, Serialize)]
pub struct RootsGraph {
    /// Ideas and sprouts.
    pub nodes: Vec<NodeRecord>,
    /// Accepted relationships between nodes.
    pub edges: Vec<EdgeRecord>,
}

/// The settings of a project's Roots store. The configured agent command is
/// left out: the runtime never runs it.
#[derive(Clone, Debug, Serialize)]
pub struct RootsSettings {
    /// The project name Roots records.
    pub project: String,
    /// The configuration format version.
    pub version: String,
    /// The project's agent tier.
    pub tier: u8,
    /// How long Roots waits for its agent command, in seconds.
    pub agent_timeout_seconds: u64,
    /// The most pending proposals at once.
    pub proposal_limit: u64,
    /// The most open sprouts at once.
    pub sprout_limit: u64,
    /// The most agent questions per session.
    pub questions_per_session: u64,
    /// Days before a pending proposal expires.
    pub proposal_ttl_days: u64,
    /// Days before an open sprout expires.
    pub sprout_ttl_days: u64,
    /// Whether `view` also writes ROOTS.md.
    pub view_write: bool,
    /// Whether `flow` asks the agent where the session is going.
    pub flow_heading: bool,
}

impl From<RootsConfig> for RootsSettings {
    fn from(config: RootsConfig) -> Self {
        Self {
            project: config.project,
            version: config.version,
            tier: config.tier,
            agent_timeout_seconds: config.agent.timeout_seconds,
            proposal_limit: config.limits.proposals,
            sprout_limit: config.limits.sprouts,
            questions_per_session: config.limits.questions_per_session,
            proposal_ttl_days: config.limits.proposal_ttl_days,
            sprout_ttl_days: config.limits.sprout_ttl_days,
            view_write: config.view_write,
            flow_heading: config.flow_heading,
        }
    }
}

/// What an agent proposes about one or two ideas.
#[derive(Clone, Debug, Deserialize)]
pub struct ProposalRequest {
    /// edge, split, merge or compost.
    pub kind: ProposalKind,
    /// The idea references: two for edge and merge, one otherwise.
    pub nodes: Vec<String>,
    /// The relation of an edge proposal.
    #[serde(default)]
    pub rel: Option<String>,
    /// Why, in one line. Required.
    #[serde(default)]
    pub reason: Option<String>,
    /// Citations, each `<id>:<quote>`.
    #[serde(default)]
    pub cites: Vec<String>,
    /// An edge proposed because one idea mentions the other.
    #[serde(default)]
    pub mention: bool,
}

/// A question an agent asks about one idea.
#[derive(Clone, Debug, Deserialize)]
pub struct QuestionRequest {
    /// The idea reference.
    pub node: String,
    /// The question.
    pub text: String,
}

/// A sprout an agent files.
#[derive(Clone, Debug, Deserialize)]
pub struct SproutRequest {
    /// The one-line statement.
    pub statement: String,
    /// The optional body.
    #[serde(default)]
    pub body: Option<String>,
    /// The optional slug.
    #[serde(default)]
    pub slug: Option<String>,
}

/// How a human accepts a proposal.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AcceptRequest {
    /// The relation to record for an edge proposal without one.
    #[serde(default)]
    pub rel: Option<String>,
    /// Which idea a merge keeps.
    #[serde(default)]
    pub keep: Option<String>,
    /// Why.
    #[serde(default)]
    pub reason: Option<String>,
}

/// A question an agent asked.
#[derive(Clone, Debug, Serialize)]
pub struct AskedQuestion {
    /// The recorded question.
    pub question: QuestionRecord,
    /// The live think session it joined, if any.
    pub session: Option<String>,
}

/// A sprout an agent filed. The prose file's location is not reported.
#[derive(Clone, Debug, Serialize)]
pub struct FiledSprout {
    /// The new sprout node.
    pub node: NodeRecord,
    /// The statement as recorded.
    pub statement: String,
    /// Sprouts that expired while this one was filed.
    pub expired: Vec<NodeRecord>,
}

impl RootsStore {
    /// The Roots store of `project_id`: the resolver's component store for
    /// the project (refusals as the resolver gives them, so an invalid or
    /// unregistered project is 404 and a repository or both store mode is
    /// 409), then `.luuna/roots` in it. Refused with 409 when that store has
    /// no `config.yaml`. Creates nothing.
    pub fn for_project(state: &AppState, project_id: &str) -> Result<Self, ApiError> {
        let store = crate::stores::project_store(state, project_id)?;
        // A relative path would be made absolute against the process working
        // directory; the resolver's path is absolute, and this keeps it so.
        if !store.is_absolute() {
            return Err(internal("the project store path is not absolute"));
        }
        let paths = roots_paths(&store);
        if !paths.config.is_file() {
            return Err(ApiError(StatusCode::CONFLICT, NOT_SET_UP.into()));
        }
        Ok(Self {
            project_id: project_id.to_owned(),
            paths,
        })
    }

    /// The project this store belongs to.
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// The store's `.luuna/roots` folder. For the runtime's own use; never
    /// put it in a response.
    pub fn store_dir(&self) -> &Path {
        &self.paths.dir
    }

    /// Run `work` on the blocking pool and map its outcome.
    async fn run<T, F>(&self, work: F) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: FnOnce(&RootsPaths) -> RootsResult<T> + Send + 'static,
    {
        let paths = self.paths.clone();
        match tokio::task::spawn_blocking(move || work(&paths)).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(roots_error(error, &self.paths.root)),
            Err(error) => Err(internal(error)),
        }
    }

    /// Every node and edge of the graph.
    pub async fn graph(&self) -> Result<RootsGraph, ApiError> {
        self.run(|paths| {
            let graph = read_graph(paths)?;
            Ok(RootsGraph {
                nodes: graph.nodes,
                edges: graph.edges,
            })
        })
        .await
    }

    /// The proposals still waiting for a human: pending, not expired and not
    /// moot.
    pub async fn pending_proposals(&self) -> Result<Vec<ProposalRecord>, ApiError> {
        self.run(|paths| {
            let graph = read_graph(paths)?;
            let rows = read_proposals(paths)?;
            Ok(
                pending_proposals(&rows, OffsetDateTime::now_utc(), Some(&graph))
                    .into_iter()
                    .cloned()
                    .collect(),
            )
        })
        .await
    }

    /// Every recorded question.
    pub async fn questions(&self) -> Result<Vec<QuestionRecord>, ApiError> {
        self.run(read_questions).await
    }

    /// The sprouts still open.
    pub async fn open_sprouts(&self) -> Result<Vec<NodeRecord>, ApiError> {
        self.run(|paths| {
            let graph = read_graph(paths)?;
            Ok(open_sprouts(&graph, OffsetDateTime::now_utc())
                .into_iter()
                .cloned()
                .collect())
        })
        .await
    }

    /// The store's settings.
    pub async fn config(&self) -> Result<RootsSettings, ApiError> {
        self.run(|paths| load_config(paths).map(RootsSettings::from))
            .await
    }

    /// The agent's identity string, checked against this store's project and
    /// roots-core's agent rule.
    fn agent_by(&self, agent: &AgentActor) -> Result<String, ApiError> {
        if agent.project_id != self.project_id {
            return Err(ApiError(
                StatusCode::FORBIDDEN,
                "this agent's run belongs to another project".into(),
            ));
        }
        if !is_agent_actor(agent.as_str()) {
            return Err(ApiError(
                StatusCode::FORBIDDEN,
                "only an agent can do this".into(),
            ));
        }
        Ok(agent.as_str().to_owned())
    }

    /// The human's identity string, checked against roots-core's human rule.
    fn human_by(human: &HumanActor) -> Result<String, ApiError> {
        if !is_human_actor(human.as_str()) {
            return Err(not_a_human());
        }
        Ok(human.as_str().to_owned())
    }

    /// File a proposal as `agent`. It stays pending until a human decides it.
    pub async fn propose(
        &self,
        agent: &AgentActor,
        request: ProposalRequest,
    ) -> Result<ProposalRecord, ApiError> {
        let by = self.agent_by(agent)?;
        self.run(move |paths| {
            propose_as_agent(
                paths,
                ProposeInput {
                    kind: request.kind,
                    nodes: request.nodes.iter().map(String::as_str).collect(),
                    rel: request.rel.as_deref(),
                    reason: request.reason.as_deref(),
                    cites: request.cites.iter().map(String::as_str).collect(),
                    mention: request.mention,
                    by: &by,
                    now: None,
                },
            )
        })
        .await
    }

    /// Ask a question about one idea as `agent`.
    pub async fn ask(
        &self,
        agent: &AgentActor,
        request: QuestionRequest,
    ) -> Result<AskedQuestion, ApiError> {
        let by = self.agent_by(agent)?;
        self.run(move |paths| {
            let graph = read_graph(paths)?;
            let node = resolve_node(&graph.nodes, &request.node, ResolveOptions::default())?;
            let asked = ask_agent_question(
                paths,
                AskInput {
                    node,
                    text: &request.text,
                    by: &by,
                    session: None,
                    now: None,
                },
            )?;
            Ok(AskedQuestion {
                question: asked.question,
                session: asked.session,
            })
        })
        .await
    }

    /// File a sprout as `agent`. It stays open until a human adopts or
    /// rejects it.
    pub async fn file_sprout(
        &self,
        agent: &AgentActor,
        request: SproutRequest,
    ) -> Result<FiledSprout, ApiError> {
        let by = self.agent_by(agent)?;
        self.run(move |paths| {
            let filed = file_sprout(
                paths,
                SproutInput {
                    statement: &request.statement,
                    body: request.body.as_deref(),
                    slug: request.slug.as_deref(),
                    by: &by,
                    now: None,
                },
            )?;
            Ok(FiledSprout {
                node: filed.node,
                statement: filed.statement,
                expired: filed.expired,
            })
        })
        .await
    }

    /// Accept a pending proposal as `human`.
    pub async fn accept_proposal(
        &self,
        human: &HumanActor,
        proposal_id: &str,
        request: AcceptRequest,
    ) -> Result<Decision, ApiError> {
        let by = Self::human_by(human)?;
        let id = proposal_id.to_owned();
        self.run(move |paths| {
            accept_proposal(
                paths,
                &by,
                &id,
                AcceptOptions {
                    rel: request.rel,
                    keep: request.keep,
                    reason: request.reason,
                },
                None,
            )
        })
        .await
    }

    /// Reject a pending proposal as `human`. The rows are kept, so an agent
    /// cannot file the same proposal again at once.
    pub async fn reject_proposal(
        &self,
        human: &HumanActor,
        proposal_id: &str,
        reason: Option<String>,
    ) -> Result<Decision, ApiError> {
        let by = Self::human_by(human)?;
        let id = proposal_id.to_owned();
        self.run(move |paths| reject_proposal(paths, &by, &id, reason.as_deref(), None))
            .await
    }

    /// Reject an open sprout as `human`.
    pub async fn reject_sprout(
        &self,
        human: &HumanActor,
        sprout_id: &str,
        reason: Option<String>,
    ) -> Result<NodeRecord, ApiError> {
        let by = Self::human_by(human)?;
        let id = sprout_id.to_owned();
        self.run(move |paths| reject_sprout(paths, &by, &id, reason.as_deref(), None))
            .await
    }
}

/// The status a roots-core error answers with.
fn roots_status(code: ErrorCode) -> StatusCode {
    match code {
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Conflict | ErrorCode::NotInitialized => StatusCode::CONFLICT,
        ErrorCode::Guard => StatusCode::FORBIDDEN,
        ErrorCode::Usage | ErrorCode::Ambiguous | ErrorCode::Validation => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        ErrorCode::Error | ErrorCode::Cancelled | ErrorCode::Config => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// Map a roots-core error to the runtime's error. A 500 goes through
/// `internal`, so its cause reaches only the runtime log. A message that
/// names the store's location, and the not-initialised message (which names
/// a folder), are replaced by a fixed text.
fn roots_error(error: RootsError, root: &Path) -> ApiError {
    let status = roots_status(error.code);
    if status == StatusCode::INTERNAL_SERVER_ERROR {
        return internal(error);
    }
    if error.code == ErrorCode::NotInitialized {
        return ApiError(status, NOT_SET_UP.into());
    }
    let root = root.to_string_lossy();
    if !root.is_empty() && error.message.contains(root.as_ref()) {
        let text = match status {
            StatusCode::NOT_FOUND => "Roots could not find what was asked for",
            StatusCode::CONFLICT => "Roots refused the request because of a conflict",
            StatusCode::FORBIDDEN => "Roots refused the request",
            _ => "Roots refused the request as invalid",
        };
        return ApiError(status, text.into());
    }
    ApiError(status, error.message)
}

#[cfg(test)]
#[path = "roots_tests.rs"]
mod tests;
