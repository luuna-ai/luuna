//! Authenticated, loopback-only Luuna runtime. It owns runs, their workers and
//! durable events: repository import, baselines, approved writers, candidate
//! checks, independent review and repair, plus the tool-free Pi demo profile.
//! Writers and checks execute natively with the developer's host permissions;
//! scope guards are runtime policy, not an OS sandbox. Nothing here publishes
//! or merges code.
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Response, Sse,
    },
    routing::{get, post},
    Json, Router,
};
use fs2::FileExt;
use luuna_domain::RunState;
use luuna_engine_pi::{PiConfig, PiProcess, PI_VERSION};
use luuna_executor::Executor;
use luuna_git::GitInspector;
use luuna_storage::{RunRecord, Store};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    convert::Infallible,
    ffi::OsString,
    future::IntoFuture,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use tokio::sync::{watch, Notify, RwLock};
use tower_http::cors::CorsLayer;

/// How long shutdown waits for admitted mutation requests before closing
/// admission anyway, and then for signalled owned work to settle.
const ADMISSION_DRAIN: Duration = Duration::from_secs(5);
/// An export document carries up to 1000 tickets of bounded text; the import
/// route alone accepts it, every other route keeps the 256 KiB limit.
const IMPORT_BODY_LIMIT: usize = 16 * 1024 * 1024;
const SETTLE_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct RuntimeConfig {
    pub data_dir: PathBuf,
    pub pi_program: PathBuf,
    /// Host `git` used for read-only repository inspection.
    pub git_program: PathBuf,
    pub pi_launcher_args: Vec<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub token: String,
    pub max_run_seconds: u64,
}

/// The service binary's settings, read from `LUUNA_*` environment variables.
pub struct ServiceSettings {
    pub runtime: RuntimeConfig,
    pub port: u16,
    /// GitHub publication (ADR 0018): token presence and API base URL.
    pub publication: PublicationSettings,
}

/// A startup failure whose message names the setting at fault and the form it
/// expects. `Debug` is the plain message, so `main` returning it prints one
/// readable line.
pub struct StartupError(pub String);

impl std::fmt::Debug for StartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Display for StartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StartupError {}

impl ServiceSettings {
    /// `var` reads one variable (`std::env::var_os` in the binary). Paths may be
    /// any OS string; every other value must be UTF-8. Errors name the variable
    /// and its expected form; the token's value is never echoed.
    pub fn from_env(var: impl Fn(&str) -> Option<OsString>) -> Result<Self, StartupError> {
        let text = |name: &str| {
            var(name)
                .map(|value| {
                    value
                        .into_string()
                        .map_err(|_| StartupError(format!("{name} must be valid UTF-8")))
                })
                .transpose()
        };
        let data_dir = data_dir::resolve_data_dir(&var, std::env::current_dir)?;
        let token = text("LUUNA_API_TOKEN")?.ok_or_else(|| {
            StartupError(
                "set LUUNA_API_TOKEN to a random token of at least 32 visible ASCII characters"
                    .into(),
            )
        })?;
        let pi_launcher_args = match text("LUUNA_PI_ARGS_JSON")? {
            Some(raw) => serde_json::from_str::<Vec<String>>(&raw).map_err(|error| {
                StartupError(format!(
                    "LUUNA_PI_ARGS_JSON must be a JSON array of strings, for example [\"/path/to/pi/dist/cli.js\"] (line {}, column {})",
                    error.line(),
                    error.column()
                ))
            })?,
            None => Vec::new(),
        };
        let port = match text("LUUNA_PORT")? {
            Some(raw) => raw.parse::<u16>().map_err(|_| {
                StartupError(format!(
                    "LUUNA_PORT must be a whole number from 0 to 65535 (got {raw:?})"
                ))
            })?,
            None => 4242,
        };
        let publication = PublicationSettings::from_env(&var)?;
        let max_run_seconds = match text("LUUNA_RUN_TIMEOUT_SECONDS")? {
            Some(raw) => raw
                .parse::<u64>()
                .ok()
                .filter(|seconds| *seconds > 0)
                .ok_or_else(|| {
                    StartupError(format!(
                        "LUUNA_RUN_TIMEOUT_SECONDS must be a positive whole number of seconds (got {raw:?})"
                    ))
                })?,
            None => 600,
        };
        Ok(Self {
            runtime: RuntimeConfig {
                data_dir,
                pi_program: PathBuf::from(var("LUUNA_PI_COMMAND").unwrap_or_else(|| "pi".into())),
                git_program: PathBuf::from(
                    var("LUUNA_GIT_COMMAND").unwrap_or_else(|| "git".into()),
                ),
                pi_launcher_args,
                provider: text("LUUNA_PI_PROVIDER")?,
                model: text("LUUNA_PI_MODEL")?,
                token,
                max_run_seconds,
            },
            port,
            publication,
        })
    }
}

#[derive(Clone)]
pub struct AppState {
    store: Arc<Mutex<Store>>,
    active: Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
    config: Arc<RuntimeConfig>,
    publication: Arc<PublicationSettings>,
    git: Arc<GitInspector>,
    executor: Arc<Executor>,
    components: Option<Arc<luuna_components::Components>>,
    component_locks: Arc<tokio::sync::Mutex<()>>,
    /// The port this runtime listens on, once the service binary bound it;
    /// `GET /api/v1/doctor` reports it. `None` for an in-process router.
    listen_port: Option<u16>,
    /// Set by explicit shutdown before owned work is signalled to stop.
    shutting_down: Arc<std::sync::atomic::AtomicBool>,
    shutdown_requested: Arc<Notify>,
    admission: Arc<RwLock<()>>,
    request_slots: Arc<tokio::sync::Semaphore>,
    stream_slots: Arc<tokio::sync::Semaphore>,
    _owner_lock: Arc<RuntimeOwnership>,
}

/// The final logical runtime owner releases its lock explicitly. On Unix a
/// concurrent fork may briefly inherit the open file description before exec
/// closes its CLOEXEC descriptor; merely closing our file could otherwise
/// retain the lock until that unrelated child completes exec.
struct RuntimeOwnership(std::fs::File);

impl Drop for RuntimeOwnership {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

/// The private data directory (ADR 0007/0008) must belong to this user and
/// be closed to everyone else; `git worktree add` creates its children under
/// the process umask, so their parent is what keeps them private.
#[cfg(unix)]
fn restrict_to_owner(directory: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let metadata = std::fs::metadata(directory)?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    if metadata.uid() != unsafe { libc::geteuid() } {
        return Err(format!(
            "{} belongs to another user; set LUUNA_DATA_DIR to a directory you own",
            directory.display()
        )
        .into());
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("cannot make {} private: {error}", directory.display()))?;
    }
    Ok(())
}

mod baselines;
mod candidates;
pub mod data_dir;
mod dispatch;
pub mod doctor;
mod graph;
mod integrations;
mod missions;
mod orbit;
mod policy;
mod projects;
mod publication;
pub mod roots;
mod scaffold;
pub mod stores;
mod writer;
pub use baselines::{ArtifactId, StartBaseline};
pub use projects::{ConfirmManifest, ProjectView, RegisterProject, RegisterRepository};
pub use publication::{PublicationSettings, PublishRequest, DEFAULT_GITHUB_API_URL};
pub use writer::{
    ApprovePlan, BudgetRequest, CreatePlan, CreateTicket, Implement, PlanApproved, PlanOpened,
    PlanTaskRequest,
};

fn discovered_components() -> Option<Arc<luuna_components::Components>> {
    static COMPONENTS: std::sync::OnceLock<Option<Arc<luuna_components::Components>>> =
        std::sync::OnceLock::new();
    COMPONENTS
        .get_or_init(|| luuna_components::Components::discover().ok().map(Arc::new))
        .clone()
}

impl AppState {
    pub fn open(config: RuntimeConfig) -> Result<Self, Box<dyn std::error::Error>> {
        // A client sends the token in a header value, which must be visible
        // ASCII and loses surrounding whitespace in transit: any other token
        // would start a service that can never authenticate anyone.
        if config.token.len() < 32 || !config.token.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(
                "LUUNA_API_TOKEN must be at least 32 visible ASCII characters with no spaces or line breaks"
                    .into(),
            );
        }
        if config.max_run_seconds == 0 {
            return Err("LUUNA_RUN_TIMEOUT_SECONDS must be positive".into());
        }
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        // Worktrees hold copies of the developer's source and check outputs.
        let worktrees = config.data_dir.join("worktrees");
        for directory in [&config.data_dir, &worktrees] {
            builder
                .create(directory)
                .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
            // The creation mode applies only to new directories; one that
            // already existed (created by hand or by packaging) is tightened.
            #[cfg(unix)]
            restrict_to_owner(directory)?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let owner_lock = options.open(config.data_dir.join("runtime.lock"))?;
        owner_lock
            .try_lock_exclusive()
            .map_err(|_| "another Luuna runtime already owns this data directory")?;
        // Own the acquired lease before fallible state initialization too.
        let owner_lock = RuntimeOwnership(owner_lock);
        let mut store = Store::open(config.data_dir.join("luuna.sqlite3"))?;
        baselines::reap_orphaned_checks(&mut store)?;
        store.interrupt_active_runs()?;
        let git = GitInspector {
            program: config.git_program.clone(),
            ..GitInspector::default()
        };
        Ok(Self {
            store: Arc::new(Mutex::new(store)),
            active: Arc::new(Mutex::new(HashMap::new())),
            config: Arc::new(config),
            publication: Arc::new(PublicationSettings::default()),
            git: Arc::new(git),
            executor: Arc::new(Executor::default()),
            components: discovered_components(),
            component_locks: Arc::new(tokio::sync::Mutex::new(())),
            listen_port: None,
            shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            shutdown_requested: Arc::new(Notify::new()),
            admission: Arc::new(RwLock::new(())),
            request_slots: Arc::new(tokio::sync::Semaphore::new(32)),
            stream_slots: Arc::new(tokio::sync::Semaphore::new(16)),
            _owner_lock: Arc::new(owner_lock),
        })
    }

    /// Publication settings (GitHub token presence, API base URL, deadlines);
    /// the service binary passes what `ServiceSettings::from_env` read.
    pub fn with_publication(mut self, settings: PublicationSettings) -> Self {
        self.publication = Arc::new(settings);
        self
    }

    /// The address the service binary bound, so the doctor route can say
    /// that this runtime is the one listening on it.
    pub fn with_listen_port(mut self, port: u16) -> Self {
        self.listen_port = Some(port);
        self
    }

    /// Override the component toolchain in isolated adapter tests.
    pub fn with_components(mut self, components: luuna_components::Components) -> Self {
        self.components = Some(Arc::new(components));
        self
    }

    /// A runtime without the component toolchain, for synthetic tests whose
    /// outcome must not depend on whether the pinned CLIs are populated.
    pub fn without_components(mut self) -> Self {
        self.components = None;
        self
    }

    /// Signal every owned worker that was not signalled yet; returns how many
    /// were. Repeating it never re-notifies a worker already stopping.
    pub fn cancel_active(&self) -> usize {
        recover(&self.active)
            .values()
            .filter(|signal| signal.send_if_modified(|stop| !std::mem::replace(stop, true)))
            .count()
    }

    /// True once explicit shutdown has begun. Owned work then settles as
    /// `interrupted` (resumable after restart) rather than `cancelled`: the
    /// developer did not cancel it, the service stopped (ADR 0004).
    pub(crate) fn shutting_down(&self) -> bool {
        self.shutting_down.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The terminal run state for work stopped by the cancellation signal.
    pub(crate) fn stop_run_state(&self) -> RunState {
        if self.shutting_down() {
            RunState::Interrupted
        } else {
            RunState::Cancelled
        }
    }

    pub(crate) fn stop_attempt_state(&self) -> luuna_domain::AttemptState {
        if self.shutting_down() {
            luuna_domain::AttemptState::Interrupted
        } else {
            luuna_domain::AttemptState::Cancelled
        }
    }

    /// The recorded reason for work stopped by the cancellation signal.
    pub(crate) fn stop_reason(&self, reason: &str) -> String {
        if self.shutting_down() {
            format!("interrupted by runtime shutdown: {reason}")
        } else {
            reason.to_owned()
        }
    }

    /// Close admission, signal every owned worker and wait (bounded) for
    /// them to settle as `interrupted`; restart recovery reconciles any that
    /// do not. Each step is bounded, so an orderly stop always ends.
    pub async fn shutdown(&self) {
        self.close_admission().await;
        let signalled = self.cancel_active();
        eprintln!("luuna-runtime: shutting down; signalled {signalled} owned run(s) to stop");
        let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
        loop {
            let remaining = recover(&self.active).len();
            if remaining == 0 {
                eprintln!("luuna-runtime: owned work settled");
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                eprintln!(
                    "luuna-runtime: {remaining} owned run(s) still settling after {}s; restart recovery will reconcile them",
                    SETTLE_DEADLINE.as_secs()
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
            // A request that outlived the admission drain may register work
            // after the first broadcast; it is stopped as soon as it appears.
            self.cancel_active();
        }
    }

    /// Request the orderly stop, as `POST /api/v1/shutdown` or a termination
    /// signal does; whoever serves the router performs it (see [`serve`]).
    pub fn request_shutdown(&self) {
        self.shutdown_requested.notify_one();
    }

    /// One main-process waiter; Notify retains an early request until it waits.
    pub async fn wait_for_shutdown_request(&self) {
        self.shutdown_requested.notified().await;
    }

    /// Drain admitted mutation requests, then refuse new ones. A stalled
    /// request (a client that never finishes its body) cannot hold the stop
    /// open: after the bound admission closes regardless, and work such a
    /// request admits later is still signalled by the settle loop.
    async fn close_admission(&self) {
        let write = self.admission.write();
        tokio::pin!(write);
        let drained = tokio::time::timeout(ADMISSION_DRAIN, write.as_mut()).await;
        self.shutting_down
            .store(true, std::sync::atomic::Ordering::SeqCst);
        // Requests queued behind a writer that gave up are released only when
        // `write` drops, after the flag is set, so they are refused too.
        drop(drained);
    }

    /// Run an owned worker on its own task under supervision. However the
    /// worker ends, its `active` entry is removed only after the worker's
    /// `AppState` clones have dropped, so an empty map still means
    /// data-directory ownership is released. A panic first settles the run as
    /// failed, rather than leaving it active with no worker, a leaked slot
    /// and a cancel that reaches nothing.
    pub(crate) fn spawn_supervised<F>(&self, run_id: String, worker: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        // The caller registered the worker before calling. A request admitted
        // before admission closed can register it after the shutdown
        // broadcast, even after the settle loop gave up; it then starts
        // already signalled and settles as interrupted instead of running.
        if self.shutting_down() {
            self.cancel_active();
        }
        let supervisor = self.clone();
        tokio::spawn(async move {
            // The task drops the worker future, and every clone it owns,
            // before its handle resolves.
            // Workers run on the bounded blocking pool (ADR 0015) because
            // they hold synchronous SQLite transactions.
            if let Err(error) = dispatch::worker(worker).await {
                if error.is_panic() {
                    eprintln!(
                        "luuna-runtime: the worker for run {run_id} panicked; settling the run as failed"
                    );
                    supervisor.settle_panicked_worker(&run_id);
                }
            }
            let active = supervisor.active.clone();
            drop(supervisor);
            recover(&active).remove(&run_id);
        });
    }

    /// Settle the durable record of a run whose worker panicked. Spend the
    /// worker did not report is treated as restart recovery treats it: a
    /// running attempt reserves what remains of the approved time and cost, a
    /// candidate stage keeps the time it took and a review also reserves the
    /// remaining cost, so a retry can never exceed the approved budgets. An
    /// active lease is abandoned with its files preserved.
    fn settle_panicked_worker(&self, run_id: &str) {
        const REASON: &str =
            "internal error: the runtime worker stopped unexpectedly; see the runtime log";
        // A panic while the store was held rolled its SQLite transaction back
        // during unwinding, so the connection itself is consistent.
        let mut store = recover(&self.store);
        let Ok(Some(run)) = store.get_run(run_id) else {
            return;
        };
        let budgets = run
            .metadata
            .get("plan_id")
            .and_then(Value::as_str)
            .zip(run.metadata.get("plan_revision").and_then(Value::as_u64))
            .and_then(|(id, revision)| store.get_plan(id, revision).ok().flatten())
            .map(|plan| plan.budgets);
        let unspent = |store: &Store| match (&budgets, store.workflow_usage(run_id)) {
            (Some(budgets), Ok(used)) => (
                budgets.timeout_seconds.saturating_sub(used.elapsed_seconds),
                budgets.cost_usd_micros.saturating_sub(used.cost_usd_micros),
            ),
            (Some(budgets), Err(_)) => (budgets.timeout_seconds, budgets.cost_usd_micros),
            (None, _) => (0, 0),
        };
        for attempt in store.attempts(run_id).unwrap_or_default() {
            if attempt.state == luuna_domain::AttemptState::Running {
                let (elapsed, cost) = unspent(&store);
                let _ = store.finish_attempt(
                    &attempt.id,
                    luuna_domain::AttemptState::Failed,
                    json!({"reason": REASON, "usage": {"elapsed_seconds": elapsed, "cost_usd_micros": cost}}),
                );
            }
        }
        for lease in store.worktree_leases(run_id).unwrap_or_default() {
            if lease.state == luuna_domain::LeaseState::Active {
                let _ = store.release_worktree_lease(
                    &lease.id,
                    luuna_domain::LeaseState::Abandoned,
                    None,
                    Some("the runtime worker stopped unexpectedly; files preserved, attempt evidence void"),
                );
            }
        }
        if run.metadata.get("kind").and_then(Value::as_str) == Some("baseline") {
            for baseline in store.baselines(&run.project_id).unwrap_or_default() {
                if baseline.run_id == run_id
                    && baseline.status == luuna_domain::BaselineStatus::Running
                {
                    let _ = store.finish_baseline(
                        &baseline.id,
                        luuna_domain::BaselineStatus::Failed,
                        Some(REASON),
                    );
                }
            }
        }
        let Ok(Some(run)) = store.get_run(run_id) else {
            return;
        };
        if matches!(run.state, RunState::Validating | RunState::Reviewing) {
            if let Ok(Some(evidence)) = store.current_candidate(run_id) {
                use luuna_domain::CandidateStatus;
                if matches!(
                    evidence.status,
                    CandidateStatus::Pending
                        | CandidateStatus::Validating
                        | CandidateStatus::Reviewing
                ) {
                    let hash = evidence.candidate.hash.clone();
                    let stage_seconds = evidence
                        .stage_started_at
                        .as_deref()
                        .and_then(|started| chrono::DateTime::parse_from_rfc3339(started).ok())
                        .map(|started| {
                            (chrono::Utc::now() - started.with_timezone(&chrono::Utc))
                                .num_seconds()
                                .max(0) as u64
                        })
                        .unwrap_or(0);
                    let elapsed = evidence
                        .stage_start_elapsed_seconds
                        .saturating_add(stage_seconds)
                        .saturating_sub(evidence.elapsed_seconds);
                    let cost = if evidence.status == CandidateStatus::Reviewing {
                        unspent(&store).1
                    } else {
                        0
                    };
                    if store
                        .fail_candidate_stage(run_id, run.revision, &hash, RunState::Failed, REASON)
                        .is_ok()
                    {
                        let _ = store.charge_candidate_usage(run_id, &hash, elapsed, cost);
                        return;
                    }
                }
            }
        }
        if matches!(
            run.state,
            RunState::Queued
                | RunState::Running
                | RunState::Exploring
                | RunState::Implementing
                | RunState::Validating
                | RunState::Reviewing
        ) {
            let _ = store.transition_run(
                run_id,
                run.revision,
                RunState::Failed,
                "run.failed",
                json!({"reason": REASON}),
            );
        }
    }

    fn event(&self, id: &str, kind: &str, payload: Value) -> Result<(), ApiError> {
        self.store
            .lock()
            .map_err(internal)?
            .append_event(id, kind, payload)
            .map_err(internal)?;
        Ok(())
    }

    fn finish(&self, id: &str, state: RunState, payload: Value) -> Result<(), ApiError> {
        let mut store = self.store.lock().map_err(internal)?;
        let current = store.get_run(id).map_err(internal)?.ok_or_else(not_found)?;
        if !current.state.is_terminal() {
            let kind = format!(
                "run.{}",
                serde_json::to_value(state)
                    .map_err(internal)?
                    .as_str()
                    .unwrap_or("failed")
            );
            store
                .transition_run(id, current.revision, state, &kind, payload)
                .map_err(internal)?;
        }
        Ok(())
    }
}

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route(
            "/api/v1/projects/{id}/components",
            get(integrations::status).post(integrations::enable),
        )
        .route(
            "/api/v1/projects/{id}/memory",
            get(integrations::list_memory).post(integrations::create_memory),
        )
        .route(
            "/api/v1/projects/{id}/memory/{memory_id}",
            axum::routing::delete(integrations::delete_memory).put(integrations::update_memory),
        )
        .route(
            "/api/v1/projects/{id}/memory/retention",
            get(integrations::memory_retention).put(integrations::set_memory_retention),
        )
        .route(
            "/api/v1/projects/{id}/memory/retention/apply",
            post(integrations::apply_memory_retention),
        )
        .route(
            "/api/v1/memory/user/export",
            get(integrations::export_user_memory),
        )
        .route(
            "/api/v1/projects/{id}/memory/{memory_id}/promote",
            post(integrations::promote_memory),
        )
        .route(
            "/api/v1/projects/{id}/export",
            get(integrations::export_project),
        )
        .route(
            "/api/v1/projects/{id}/prompts",
            get(integrations::list_prompts).post(integrations::create_prompt),
        )
        .route(
            "/api/v1/projects/{id}/prompts/{name}/render",
            get(integrations::render_prompt),
        )
        .route("/api/v1/tickets/{id}/source", get(writer::ticket_source))
        .route(
            "/api/v1/tickets/{id}/reconcile",
            post(writer::reconcile_ticket),
        )
        .route("/api/v1/shutdown", post(request_shutdown))
        .route("/api/v1/doctor", get(doctor::doctor))
        .route("/api/v1/runs", get(list_runs).post(start_run))
        .route("/api/v1/runs/{id}", get(get_run))
        .route("/api/v1/runs/{id}/events", get(events))
        .route("/api/v1/runs/{id}/stream", get(stream))
        .route("/api/v1/runs/{id}/cancel", post(cancel))
        .route("/api/v1/runs/{id}/implement", post(writer::implement))
        .route("/api/v1/runs/{id}/candidate", get(candidates::candidate))
        .route("/api/v1/runs/{id}/diff", get(candidates::diff))
        .route("/api/v1/runs/{id}/validate", post(candidates::validate))
        .route("/api/v1/runs/{id}/repair", post(writer::repair))
        .route("/api/v1/runs/{id}/publish", post(publication::publish))
        .route("/api/v1/runs/{id}/publication", get(publication::get))
        .route(
            "/api/v1/runs/{id}/publication/reconcile",
            post(publication::reconcile),
        )
        .route(
            "/api/v1/runs/{id}/publication/repair",
            post(publication::repair),
        )
        .route("/api/v1/runs/{id}/worktrees", get(writer::list_worktrees))
        .route("/api/v1/runs/{id}/attempts", get(writer::list_attempts))
        .route("/api/v1/runs/{id}/tasks", get(graph::list_tasks))
        .route(
            "/api/v1/projects/{id}/tickets",
            get(writer::list_tickets).post(writer::create_ticket),
        )
        .route(
            "/api/v1/tickets/{id}",
            get(writer::get_ticket).put(writer::update_ticket),
        )
        .route(
            "/api/v1/tickets/{id}/plans",
            get(writer::list_plans).post(writer::create_plan),
        )
        .route("/api/v1/plans/{id}", get(writer::get_plan))
        .route("/api/v1/plans/{id}/approvals", post(writer::approve_plan))
        .route(
            "/api/v1/projects",
            get(projects::list_projects).post(projects::register_project),
        )
        .route("/api/v1/projects/{id}", get(projects::get_project))
        .route(
            "/api/v1/projects/import",
            post(projects::import_project)
                .layer(axum::extract::DefaultBodyLimit::max(IMPORT_BODY_LIMIT)),
        )
        .route(
            "/api/v1/projects/scaffold/propose",
            post(scaffold::propose_scaffold),
        )
        .route(
            "/api/v1/projects/scaffold",
            post(scaffold::execute_scaffold),
        )
        .route("/api/v1/projects/{id}/discover", post(projects::discover))
        .route(
            "/api/v1/projects/{id}/repositories/{repository_id}/rebind",
            post(projects::rebind_repository),
        )
        .route(
            "/api/v1/projects/{id}/manifest",
            axum::routing::put(projects::confirm_manifest),
        )
        .route(
            "/api/v1/projects/{id}/events",
            get(projects::project_events),
        )
        .route("/api/v1/projects/{id}/orbit", get(orbit::status))
        .route("/api/v1/projects/{id}/orbit/exec", post(orbit::exec))
        .route("/api/v1/projects/{id}/orbit/run", post(orbit::run))
        .route("/api/v1/projects/{id}/orbit/sync", post(orbit::sync))
        .route("/api/v1/projects/{id}/orbit/sandbox", get(orbit::sandbox))
        .route(
            "/api/v1/projects/{id}/baselines",
            get(baselines::list_baselines).post(baselines::start_baseline),
        )
        .route(
            "/api/v1/runs/{id}/artifacts/{artifact_id}",
            get(baselines::artifact),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));
    let cors = CorsLayer::new()
        .allow_origin([
            "http://localhost:1420".parse().unwrap(),
            "http://127.0.0.1:1420".parse().unwrap(),
            "tauri://localhost".parse().unwrap(),
            "http://tauri.localhost".parse().unwrap(),
            "https://tauri.localhost".parse().unwrap(),
        ])
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::DELETE,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::HeaderName::from_static("last-event-id"),
        ]);
    Router::new()
        .merge(protected)
        .route("/health", get(health))
        .route("/api/v1/health", get(health))
        .layer(middleware::from_fn(json_errors))
        .layer(axum::extract::DefaultBodyLimit::max(256 * 1024))
        .layer(cors)
        .with_state(state)
}

/// How an orderly stop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// Owned work settled and every connection drained.
    Drained,
    /// Owned work settled, but connections were still open at the drain
    /// deadline; the caller exits anyway.
    DeadlineExceeded,
}

/// Serve the API until a shutdown is requested (`POST /api/v1/shutdown` or
/// [`AppState::request_shutdown`]), then stop owned work (each step bounded,
/// see [`AppState::shutdown`]) and drain open connections for at most
/// `drain`. Every event stream ends when shutdown begins, so only a stalled
/// client can reach the deadline.
pub async fn serve(
    listener: tokio::net::TcpListener,
    state: AppState,
    drain: Duration,
) -> std::io::Result<Stopped> {
    let stopping = state.clone();
    let (settled, draining) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(listener, router(state)).with_graceful_shutdown(async move {
        stopping.wait_for_shutdown_request().await;
        stopping.shutdown().await;
        let _ = settled.send(());
    });
    let deadline = async move {
        match draining.await {
            Ok(()) => tokio::time::sleep(drain).await,
            Err(_) => std::future::pending().await,
        }
    };
    tokio::select! {
        served = server.into_future() => served.map(|()| Stopped::Drained),
        () = deadline => Ok(Stopped::DeadlineExceeded),
    }
}

async fn health() -> Json<Value> {
    // `native_execution`: approved writers and required checks run with the
    // developer's host permissions; the tool-free demo is one run profile.
    Json(
        json!({"status":"ok","pi_version":PI_VERSION,"pi_version_status":"required; checked on every run","native_execution":true,"build_identity":env!("LUUNA_BUILD_IDENTITY"),"build_rustc":env!("LUUNA_BUILD_RUSTC"),"build_rustc_sha256":env!("LUUNA_BUILD_RUSTC_SHA256"),"build_rustc_wrapper":env!("LUUNA_BUILD_RUSTC_WRAPPER"),"build_flags":env!("LUUNA_BUILD_FLAGS")}),
    )
}

async fn authenticate(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    // The scheme name is case-insensitive (RFC 9110); the token is not.
    let supplied = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, token)| token);
    if !supplied.is_some_and(|value| {
        constant_time_eq::constant_time_eq(value.as_bytes(), state.config.token.as_bytes())
    }) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"authentication required"})),
        )
            .into_response();
    }
    dispatch::request(state, request, next).await
}

/// Authenticated, idempotent service stop. Client disconnect is never shutdown.
async fn request_shutdown(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    // Start the stop at once, then close admission before answering, so a
    // client that saw 202 can never start new work.
    state.request_shutdown();
    state.close_admission().await;
    (
        StatusCode::ACCEPTED,
        Json(json!({"status":"shutting_down"})),
    )
}

tokio::task_local! {
    /// The request an internal error is reported against in the runtime log.
    static REQUEST: String;
}

/// Every 4xx/5xx body is `{"error": reason}` with the original status.
/// Handlers answer with [`ApiError`]; this maps the responses produced
/// elsewhere (axum's rejections of malformed JSON, unknown or mistyped
/// fields, a missing content type, an oversized body or a bad query, and
/// unmatched routes or methods), so a client can show the runtime's reason.
async fn json_errors(request: axum::extract::Request, next: Next) -> Response {
    let route = format!("{} {}", request.method(), request.uri().path());
    let response = REQUEST.scope(route, next.run(request)).await;
    let status = response.status();
    let json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    if json || !(status.is_client_error() || status.is_server_error()) {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let text = axum::body::to_bytes(body, 64 * 1024)
        .await
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    let reason = one_line(&text, 280).unwrap_or_else(|| {
        status
            .canonical_reason()
            .unwrap_or("request failed")
            .to_ascii_lowercase()
    });
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Response::from_parts(parts, Body::from(json!({"error": reason}).to_string()))
}

/// `text` as one line without control characters, clipped to `max` UTF-16
/// units (the client's display bound counts those); `None` when blank.
fn one_line(text: &str, max: usize) -> Option<String> {
    let line = text
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if line.is_empty() {
        return None;
    }
    if line.encode_utf16().count() <= max {
        return Some(line);
    }
    let mut clipped = String::new();
    let mut units = 0;
    for c in line.chars() {
        units += c.len_utf16();
        if units > max - 1 {
            break;
        }
        clipped.push(c);
    }
    clipped.push('…');
    Some(clipped)
}

#[derive(Debug)]
pub struct ApiError(pub StatusCode, pub String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}

impl ApiError {
    /// A refusal whose body carries a structured field beside `error`, such
    /// as the prerequisite offer of a baseline gate refusal (ADR 0022).
    pub fn with_detail(self, field: &'static str, value: Value) -> ApiDetailError {
        ApiDetailError {
            status: self.0,
            error: self.1,
            field,
            value,
        }
    }
}

/// `{"error": ..., "<field>": ...}` with the refusal's status.
pub struct ApiDetailError {
    status: StatusCode,
    error: String,
    field: &'static str,
    value: Value,
}

impl From<ApiError> for ApiDetailError {
    fn from(error: ApiError) -> Self {
        ApiDetailError {
            status: error.0,
            error: error.1,
            field: "detail",
            value: Value::Null,
        }
    }
}

impl IntoResponse for ApiDetailError {
    fn into_response(self) -> Response {
        let mut body = json!({"error": self.error});
        if !self.value.is_null() {
            body[self.field] = self.value;
        }
        (self.status, Json(body)).into_response()
    }
}
/// A storage or process failure. The cause goes to the runtime log with the
/// request it failed (or "background work" for a worker); the HTTP body stays
/// generic. Causes are storage and process errors, never request payloads,
/// prompts or credentials.
fn internal<T: std::fmt::Display>(error: T) -> ApiError {
    let context = REQUEST
        .try_with(Clone::clone)
        .unwrap_or_else(|_| "background work".into());
    let cause = one_line(&error.to_string(), 1000).unwrap_or_default();
    eprintln!("luuna-runtime: {context} failed: {cause}");
    ApiError(
        StatusCode::INTERNAL_SERVER_ERROR,
        "runtime storage or process operation failed; see the runtime log".into(),
    )
}
fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "run not found".into())
}

/// Lock a mutex even after a panicking holder poisoned it: the store's open
/// SQLite transaction rolls back on unwind and the active map holds only stop
/// signals, so one worker's panic must not become a permanent outage.
fn recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| {
        mutex.clear_poison();
        poisoned.into_inner()
    })
}

/// `now + duration`, saturating at a distant instant rather than panicking
/// when an approved budget exceeds what the platform clock can represent.
pub(crate) fn deadline_after(duration: Duration) -> tokio::time::Instant {
    let now = tokio::time::Instant::now();
    now.checked_add(duration)
        .or_else(|| now.checked_add(Duration::from_secs(30 * 365 * 86_400)))
        .unwrap_or(now)
}

/// The largest page a client may request; omitting `limit` stays unbounded.
const MAX_PAGE: u64 = 10_000;

/// Bounded history reads (ADR 0015) answer at most this many runs or events
/// per page, and run events at most about 1 MiB, whatever `limit` asked for;
/// clients continue from the last record until a page comes back empty.
const HISTORY_PAGE: usize = 200;

/// Event replay paging for run and project streams: `?after=` and `?limit=`.
#[derive(Deserialize)]
pub(crate) struct EventPage {
    #[serde(default)]
    after: u64,
    limit: Option<u64>,
    /// Read the page that ends just before this sequence (newest history first).
    before: Option<u64>,
}

impl EventPage {
    /// The validated cursor and page size, checked before any read.
    pub(crate) fn bounds(&self) -> Result<(u64, Option<usize>), ApiError> {
        Ok((cursor(self.after)?, page_limit(self.limit)?))
    }

    /// The validated `before` cursor; it is only meaningful with `limit`.
    pub(crate) fn before(&self) -> Result<Option<u64>, ApiError> {
        match self.before {
            Some(before) if before == 0 || before > i64::MAX as u64 => Err(ApiError(
                StatusCode::BAD_REQUEST,
                format!("before must be an integer from 1 to {}", i64::MAX),
            )),
            Some(_) if self.limit.is_none() => Err(ApiError(
                StatusCode::BAD_REQUEST,
                "before requires limit".into(),
            )),
            before => Ok(before),
        }
    }
}

/// Event sequences are SQLite integers; a larger cursor is a client error,
/// never a storage failure.
fn cursor(after: u64) -> Result<u64, ApiError> {
    if after > i64::MAX as u64 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            format!("event cursor must be an integer from 0 to {}", i64::MAX),
        ));
    }
    Ok(after)
}

fn page_limit(limit: Option<u64>) -> Result<Option<usize>, ApiError> {
    match limit {
        None => Ok(None),
        Some(limit @ 1..=MAX_PAGE) => Ok(Some(limit as usize)),
        Some(_) => Err(ApiError(
            StatusCode::BAD_REQUEST,
            format!("limit must be an integer from 1 to {MAX_PAGE}"),
        )),
    }
}

#[derive(Deserialize)]
struct RunPage {
    limit: Option<u64>,
    /// Keyset cursor: the id of the last run of the previous page.
    before: Option<String>,
    project: Option<String>,
}

/// Run records, newest first. `?limit=N` reads one bounded keyset page
/// (`before`, `project`) of at most `HISTORY_PAGE` runs (ADR 0015); without
/// `limit` every run is returned.
async fn list_runs(
    State(state): State<AppState>,
    Query(page): Query<RunPage>,
) -> Result<Json<Vec<RunRecord>>, ApiError> {
    let limit = page_limit(page.limit)?;
    let store = state.store.lock().map_err(internal)?;
    Ok(Json(match limit {
        Some(limit) => store
            .runs_page(page.before.as_deref(), page.project.as_deref(), limit)
            .map_err(|error| match error {
                luuna_storage::StorageError::NotFound(_) => ApiError(
                    StatusCode::BAD_REQUEST,
                    "history cursor names no run".into(),
                ),
                error => internal(error),
            })?,
        None if page.before.is_some() || page.project.is_some() => {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "before and project require limit".into(),
            ))
        }
        None => store.runs().map_err(internal)?,
    }))
}
async fn get_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RunRecord>, ApiError> {
    Ok(Json(
        state
            .store
            .lock()
            .map_err(internal)?
            .get_run(&id)
            .map_err(internal)?
            .ok_or_else(not_found)?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartRun {
    pub prompt: String,
    pub provider: Option<String>,
    pub model: Option<String>,
}

async fn start_run(
    State(state): State<AppState>,
    Json(request): Json<StartRun>,
) -> Result<(StatusCode, Json<RunRecord>), ApiError> {
    if request.prompt.trim().is_empty()
        || request.prompt.len() > 16_000
        || request.prompt.trim_start().starts_with('/')
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "prompt must be 1..16000 bytes and cannot invoke a slash command".into(),
        ));
    }
    let provider = request
        .provider
        .or_else(|| state.config.provider.clone())
        .filter(|s| !s.is_empty());
    let model = request
        .model
        .or_else(|| state.config.model.clone())
        .filter(|s| !s.is_empty());
    if model.is_none()
        || provider.is_none()
        || model.as_ref().is_some_and(|s| s.len() > 256)
        || provider.as_ref().is_some_and(|s| s.len() > 128)
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "an explicit provider and model are required".into(),
        ));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = watch::channel(false);
    let mut active = state.active.lock().map_err(internal)?;
    // The four slots are shared by demo runs, baselines, writers and validations.
    if active.len() >= 4 {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "four runs are already active".into(),
        ));
    }
    let metadata = json!({"prompt":request.prompt,"provider":provider,"model":model,"demo_only":true,"tools":[]});
    let run = state
        .store
        .lock()
        .map_err(internal)?
        .create_run(&id, "demo", metadata)
        .map_err(internal)?;
    active.insert(id.clone(), tx);
    drop(active);
    state.spawn_supervised(
        id.clone(),
        run_worker(state.clone(), id, request.prompt, provider, model, rx),
    );
    Ok((StatusCode::ACCEPTED, Json(run)))
}

#[derive(Deserialize)]
struct Cursor {
    #[serde(default)]
    after: u64,
}
async fn events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(page): Query<EventPage>,
) -> Result<Json<Vec<luuna_domain::EventEnvelope>>, ApiError> {
    let (after, limit) = page.bounds()?;
    let store = state.store.lock().map_err(internal)?;
    store
        .get_run(&id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    Ok(Json(match limit {
        Some(limit) => store
            .events_page(&id, after, page.before()?, limit)
            .map_err(internal)?,
        None => {
            page.before()?;
            store.events_after(&id, after).map_err(internal)?
        }
    }))
}

/// Whether a run's event stream is complete, so its SSE follower may stop:
/// the run can never change state again and no owned worker can append to
/// it. A failed or interrupted workflow is settled but resumable (repair,
/// re-validation and cancellation continue on the same run id, including an
/// automatic repair inside an active validation), and a reviewed workflow
/// still publishes, reconciles and can have its candidate invalidated after
/// the review (drift found at publication, a CI failure sent to repair), so
/// their followers stay attached; demo and baseline runs never leave a
/// terminal state.
fn stream_complete(run: &RunRecord, owned: bool) -> bool {
    if run.metadata.get("kind").and_then(Value::as_str) == Some("workflow") {
        !owned && matches!(run.state, RunState::Completed | RunState::Cancelled)
    } else {
        run.state.is_terminal()
    }
}

async fn stream(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<Cursor>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let stream_permit = state
        .stream_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "event stream capacity is busy; retry shortly".into(),
            )
        })?;
    state
        .store
        .lock()
        .map_err(internal)?
        .get_run(&id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    let mut after = cursor(
        headers
            .get("last-event-id")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(query.after),
    )?;
    let records = async_stream::stream! {
        let _stream_permit = stream_permit;
        loop {
            // Observe ownership before the read: a worker persists its last
            // event before it leaves the active map.
            let owned = recover(&state.active).contains_key(&id);
            let Ok(permit) = state.request_slots.clone().acquire_owned().await else { break; };
            let database = state.store.clone();
            let run_id = id.clone();
            let batch = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let store = database.lock();
                store.ok().and_then(|store| {
                    let run = store.get_run(&run_id).ok().flatten()?;
                    Some((store.events_page(&run_id, after, None, HISTORY_PAGE).ok()?, stream_complete(&run, owned)))
                })
            }).await.ok().flatten();
            let Some((events, complete)) = batch else { break; };
            let empty = events.is_empty();
            for record in events {
                after = record.sequence;
                let event = Event::default().id(after.to_string()).event("run_event").json_data(record);
                match event { Ok(event) => yield Ok::<Event, Infallible>(event), Err(_) => break }
            }
            // A complete stream still drains every remaining page; shutdown
            // ends every stream, so graceful HTTP shutdown never waits on a
            // follower of an idle or resumable run.
            if (complete && empty) || state.shutting_down() { break; }
            if !empty { continue; }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    };
    Ok(Sse::new(records).keep_alive(KeepAlive::new().interval(Duration::from_secs(10))))
}

async fn cancel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<RunRecord>), ApiError> {
    let run = state
        .store
        .lock()
        .map_err(internal)?
        .get_run(&id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    // A worker that already returned has dropped its receiver while its
    // supervisor removes the entry: nothing is left to stop, so the settled
    // state answers, as a repeated cancel always does (ADR 0004).
    if recover(&state.active)
        .get(&id)
        .is_some_and(|sender| sender.send(true).is_ok())
    {
        return Ok((StatusCode::ACCEPTED, Json(run)));
    }
    let run = state
        .store
        .lock()
        .map_err(internal)?
        .get_run(&id)
        .map_err(internal)?
        .ok_or_else(not_found)?;
    if run.state.is_terminal() {
        return Ok((StatusCode::OK, Json(run)));
    }
    Err(ApiError(
        StatusCode::CONFLICT,
        "run has no active local process".into(),
    ))
}

async fn run_worker(
    state: AppState,
    id: String,
    prompt: String,
    provider: Option<String>,
    model: Option<String>,
    mut cancel: watch::Receiver<bool>,
) {
    if *cancel.borrow() {
        let _ = state.finish(
            &id,
            state.stop_run_state(),
            json!({"reason": state.stop_reason("cancelled before startup")}),
        );
        return;
    }
    let running = (|| -> Result<(), ApiError> {
        let mut store = state.store.lock().map_err(internal)?;
        let run = store
            .get_run(&id)
            .map_err(internal)?
            .ok_or_else(not_found)?;
        store
            .transition_run(
                &id,
                run.revision,
                RunState::Running,
                "run.running",
                json!({"pi_version":PI_VERSION}),
            )
            .map_err(internal)?;
        Ok(())
    })();
    if running.is_err() {
        let _ = state.finish(
            &id,
            RunState::Failed,
            json!({"reason":"cannot persist startup"}),
        );
        return;
    }
    let config = PiConfig {
        program: state.config.pi_program.clone(),
        launcher_args: state.config.pi_launcher_args.clone(),
        cwd: state
            .config
            .data_dir
            .join("attempts")
            .join(&id)
            .join("work"),
        config_dir: state.config.data_dir.join("attempts").join(&id).join("pi"),
        provider,
        model,
        profile: luuna_engine_pi::SessionProfile::ToolFree,
    };
    let spawned = tokio::select! {
        biased;
        _ = cancel.changed() => { let _ = state.finish(&id, state.stop_run_state(), json!({"reason": state.stop_reason("cancelled during startup")})); return; },
        spawned = PiProcess::spawn(&config) => spawned,
    };
    let mut process = match spawned {
        Ok(process) => process,
        Err(error) => {
            let _ = state.finish(&id, RunState::Failed, json!({"reason":error.to_string()}));
            return;
        }
    };
    // The owned child's PID lets operators and smoke tests verify it was reaped
    // after the terminal event. Credential names are recorded, never values.
    if state
        .event(
            &id,
            "pi.spawned",
            json!({"pid":process.pid(),"pi_version":PI_VERSION,"provider_credentials":luuna_engine_pi::configured_provider_keys()}),
        )
        .is_err()
    {
        let _ = process.stop().await;
        let _ = state.finish(
            &id,
            RunState::Failed,
            json!({"reason":"cannot persist process start"}),
        );
        return;
    }
    let operation = async {
        process
            .command("state-1", "get_state", json!({}))
            .await
            .map_err(|e| e.to_string())?;
        let handshake = tokio::time::timeout(Duration::from_secs(15), process.next())
            .await
            .map_err(|_| "Pi startup timed out".to_owned())?
            .map_err(|e| e.to_string())?;
        if handshake.kind != "response"
            || handshake.raw["id"] != "state-1"
            || handshake.raw["success"] != true
        {
            return Err("Pi startup handshake rejected".to_owned());
        }
        state
            .event(&id, "pi.response", handshake.raw)
            .map_err(|_| "cannot persist handshake".to_owned())?;
        process
            .command("prompt-1", "prompt", json!({"message":prompt}))
            .await
            .map_err(|e| e.to_string())?;
        let mut failure: Option<String> = None;
        let mut accepted = false;
        loop {
            let record = process.next().await.map_err(|e| e.to_string())?;
            state
                .event(&id, &format!("pi.{}", record.kind), record.raw.clone())
                .map_err(|_| "cannot persist event".to_owned())?;
            if record.kind == "response" && record.raw["id"] == "prompt-1" {
                if record.raw["success"] != true {
                    return Err(record.raw["error"]
                        .as_str()
                        .unwrap_or("prompt rejected")
                        .to_owned());
                }
                if record.raw["data"]["disposition"] == "handled" {
                    return Err("prompt was handled without starting model work".to_owned());
                }
                accepted = true;
            }
            if record.kind == "message_end" && record.raw["message"]["role"] == "assistant" {
                failure = record.model_error();
            }
            if record.kind == "agent_end" && record.raw["willRetry"] != true {
                if let Some(error) = record.model_error() {
                    failure = Some(error);
                }
            }
            if record.kind == "extension_ui_request" {
                return Err("unexpected extension UI request in tool-free demo".to_owned());
            }
            if record.kind == "agent_settled" {
                if !accepted {
                    return Err("model settled before prompt acknowledgement".to_owned());
                }
                if record.raw["aborted"] == true {
                    return Err("provider run aborted".to_owned());
                }
                return failure.map_or(Ok(()), Err);
            }
        }
    };
    enum Outcome {
        Completed,
        Failed(String),
        Cancelled,
    }
    let outcome = tokio::select! {
        biased;
        _ = cancel.changed() => Outcome::Cancelled,
        result = tokio::time::timeout(Duration::from_secs(state.config.max_run_seconds), operation) => match result {
            Ok(Ok(())) => Outcome::Completed, Ok(Err(error)) => Outcome::Failed(error), Err(_) => Outcome::Failed("run deadline exceeded".into()),
        },
    };
    // The terminal event is persisted only after the child is stopped/reaped.
    let stopped = process.stop().await;
    let (terminal, payload) = match (outcome, stopped) {
        (_, Err(error)) => (
            RunState::Failed,
            json!({"reason":format!("process cleanup failed: {error}")}),
        ),
        (Outcome::Completed, _) => (
            RunState::Completed,
            json!({"reason":"Pi agent_settled; no repository validation performed"}),
        ),
        (Outcome::Failed(reason), _) => (RunState::Failed, json!({"reason":reason})),
        (Outcome::Cancelled, _) => (
            state.stop_run_state(),
            json!({"reason": state.stop_reason("abort requested; subprocess reaped")}),
        ),
    };
    let _ = state.finish(&id, terminal, payload);
}

#[cfg(test)]
mod integrations_tests;

#[cfg(test)]
mod lifecycle_tests;

#[cfg(test)]
mod ownership_tests {
    use super::*;

    fn config(root: &std::path::Path) -> RuntimeConfig {
        RuntimeConfig {
            data_dir: root.into(),
            pi_program: "node".into(),
            git_program: "git".into(),
            pi_launcher_args: vec![],
            provider: None,
            model: None,
            token: "synthetic-ownership-token-at-least-32-bytes".into(),
            max_run_seconds: 10,
        }
    }

    #[tokio::test]
    async fn blocked_database_does_not_block_io_and_admission_is_bounded() {
        use axum::{body::Body, http::Request};
        use tower::ServiceExt;
        let root = tempfile::tempdir().unwrap();
        let state = AppState::open(config(root.path())).unwrap();
        let app = router(state.clone());
        let request = |path: &str, method: &str| {
            Request::builder()
                .uri(path)
                .method(method)
                .header("authorization", format!("Bearer {}", state.config.token))
                .body(Body::empty())
                .unwrap()
        };
        let database = state.store.clone();
        let (held_tx, held_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let _guard = database.lock().unwrap();
            held_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        held_rx.await.unwrap();
        let blocked = tokio::spawn(
            app.clone()
                .oneshot(request("/api/v1/runs?limit=200", "GET")),
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
        let health = tokio::time::timeout(
            Duration::from_millis(250),
            app.clone().oneshot(request("/health", "GET")),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(health.status(), StatusCode::OK);
        assert!(!blocked.is_finished());
        release_tx.send(()).unwrap();
        assert_eq!(blocked.await.unwrap().unwrap().status(), StatusCode::OK);
        thread.join().unwrap();
        let _capacity = state.request_slots.acquire_many(32).await.unwrap();
        assert_eq!(
            app.clone()
                .oneshot(request("/api/v1/runs", "GET"))
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            app.oneshot(request("/api/v1/shutdown", "POST"))
                .await
                .unwrap()
                .status(),
            StatusCode::ACCEPTED
        );
    }

    #[test]
    fn runtime_ownership_is_retained_until_the_last_state_clone_drops() {
        let root = tempfile::tempdir().unwrap();
        let settings = config(root.path());
        let state = AppState::open(settings.clone()).unwrap();
        let active_owner = state.clone();
        drop(state);
        assert!(AppState::open(settings.clone()).is_err());
        drop(active_owner);
        assert!(AppState::open(settings).is_ok());
    }

    #[tokio::test]
    async fn terminal_sse_drains_all_pages_and_http_pages_preserve_cursors() {
        use axum::{
            body::{to_bytes, Body},
            http::Request,
        };
        use tower::ServiceExt;
        let root = tempfile::tempdir().unwrap();
        let state = AppState::open(config(root.path())).unwrap();
        let count = {
            let mut store = state.store.lock().unwrap();
            let run = store.create_run("paged", "p", json!({})).unwrap();
            for _ in 0..250 {
                store.append_event("paged", "fixture", json!({})).unwrap();
            }
            let run = store
                .transition_run(
                    "paged",
                    run.revision,
                    RunState::Running,
                    "run.running",
                    json!({}),
                )
                .unwrap();
            store
                .transition_run(
                    "paged",
                    run.revision,
                    RunState::Completed,
                    "run.completed",
                    json!({}),
                )
                .unwrap();
            store.events_after("paged", 0).unwrap().len()
        };
        let app = router(state.clone());
        let request = |path: &str| {
            Request::builder()
                .uri(path)
                .header("authorization", format!("Bearer {}", state.config.token))
                .body(Body::empty())
                .unwrap()
        };
        let response = app
            .clone()
            .oneshot(request("/api/v1/runs/paged/events?after=0&limit=200"))
            .await
            .unwrap();
        let values: Vec<Value> =
            serde_json::from_slice(&to_bytes(response.into_body(), 2_000_000).await.unwrap())
                .unwrap();
        assert_eq!(values.len(), 200);
        assert_eq!(values[199]["sequence"], 200);
        let response = app
            .oneshot(request("/api/v1/runs/paged/stream"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = tokio::time::timeout(
            Duration::from_secs(5),
            to_bytes(response.into_body(), 2_000_000),
        )
        .await
        .unwrap()
        .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(body.matches("event: run_event").count(), count);
    }

    #[test]
    #[cfg(unix)]
    fn runtime_final_drop_unlocks_even_when_a_fork_style_duplicate_stays_open() {
        let root = tempfile::tempdir().unwrap();
        let settings = config(root.path());
        let state = AppState::open(settings.clone()).unwrap();
        // dup/fork descriptors share the same open file description/flock.
        // Keep one alive deterministically instead of racing fork and exec.
        let inherited = state._owner_lock.0.try_clone().unwrap();
        assert!(AppState::open(settings.clone()).is_err());
        drop(state);
        let replacement = AppState::open(settings)
            .expect("a stale inherited descriptor must not extend logical runtime ownership");
        drop(inherited);
        drop(replacement);
    }
}
