//! M5: approved new-project scaffolding (R04, A05).
//!
//! `POST /api/v1/projects/scaffold/propose` returns a deterministic, hashed
//! proposal and writes nothing. `POST /api/v1/projects/scaffold` executes
//! exactly the proposal its hash names, after the developer confirms it:
//! writes the embedded template (or runs the platform generator), commits
//! on `main`, runs the proposal's baseline recipe through the supervised
//! executor as a real check, registers the project through the ordinary
//! registration path, records `project.scaffolded` durably and replays the
//! same result for the same idempotency key. A failing baseline is reported,
//! never hidden, and the directory is left for the developer.
use crate::{
    baselines::recipe_spec,
    internal,
    projects::{self, ProjectView, RegisterProject, RegisterRepository},
    ApiError, AppState,
};
use axum::{extract::State, http::StatusCode, Json};
use luuna_discovery::{
    capabilities::probe_host_cached,
    scaffold::{propose, ScaffoldProposal, ScaffoldStatus, ScaffoldStep},
};
use luuna_domain::{valid_project_id, CheckStatus, Discovery};
use luuna_executor::{is_secret_or_reserved, CommandSpec, Execution};
use luuna_git::strip_verbatim_prefix;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command, sync::watch};

const MAX_PATH_BYTES: usize = 4096;
const GIT_TIMEOUT: Duration = Duration::from_secs(60);
const GIT_OUTPUT_LIMIT: u64 = 64 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposeScaffold {
    pub profile: String,
    pub name: String,
    /// Absolute directory that will contain `<name>/`.
    pub parent_dir: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecuteScaffold {
    pub proposal_hash: String,
    pub idempotency_key: String,
    /// Must be `true`: sending the request is the developer's confirmation of
    /// exactly the proposal `proposal_hash` names.
    pub confirm: bool,
    /// Confirms the proposal's declared network steps (`npm install`,
    /// `flutter create`'s package resolution). Without it such steps are
    /// skipped and the baseline reports what that leaves.
    #[serde(default)]
    pub confirm_network: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactSummary {
    pub name: String,
    pub sha256: String,
    pub bytes: u64,
    pub truncated: bool,
}

/// One supervised command's outcome, as the executor recorded it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckOutcome {
    pub recipe_id: String,
    pub status: CheckStatus,
    pub exit_code: Option<i32>,
    pub reason: Option<String>,
    pub command: Vec<String>,
    pub tool_version: Option<String>,
    pub tests_detected: Option<u64>,
    pub stdout_excerpt: String,
    pub stderr_excerpt: String,
    pub artifacts: Vec<ArtifactSummary>,
}

impl CheckOutcome {
    fn from_execution(recipe_id: &str, execution: Execution) -> Self {
        Self {
            recipe_id: recipe_id.into(),
            status: execution.status,
            exit_code: execution.exit_code,
            reason: execution.reason,
            command: execution.command,
            tool_version: execution.tool_version,
            tests_detected: execution.tests_detected,
            stdout_excerpt: execution.stdout_excerpt,
            stderr_excerpt: execution.stderr_excerpt,
            artifacts: execution
                .artifacts
                .into_iter()
                .map(|a| ArtifactSummary {
                    name: a.name,
                    sha256: a.sha256,
                    bytes: a.bytes,
                    truncated: a.truncated,
                })
                .collect(),
        }
    }

    fn skipped(step: &ScaffoldStep, reason: &str) -> Self {
        let mut command = vec![step.executable.clone()];
        command.extend(step.args.iter().cloned());
        Self {
            recipe_id: step.id.clone(),
            status: CheckStatus::Blocked,
            exit_code: None,
            reason: Some(reason.into()),
            command,
            tool_version: None,
            tests_detected: None,
            stdout_excerpt: String::new(),
            stderr_excerpt: String::new(),
            artifacts: Vec::new(),
        }
    }

    fn summary(&self) -> Value {
        json!({
            "recipe_id": self.recipe_id,
            "status": self.status,
            "exit_code": self.exit_code,
            "reason": self.reason,
            "tests_detected": self.tests_detected,
        })
    }
}

#[derive(Serialize)]
pub struct ScaffoldResult {
    pub proposal_hash: String,
    pub idempotency_key: String,
    pub profile: String,
    pub target_dir: String,
    /// The scaffold commit on `main`.
    pub commit: String,
    pub generator: Option<CheckOutcome>,
    pub preparation: Vec<CheckOutcome>,
    /// The proposal's required baseline recipe, run as a real check.
    pub check: CheckOutcome,
    pub baseline_passed: bool,
    pub project: ProjectView,
    pub discovery: Discovery,
    /// This response was recorded by an earlier request with the same key.
    pub replayed: bool,
}

fn bad_request(message: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, message.into())
}
fn unprocessable(message: impl Into<String>) -> ApiError {
    ApiError(StatusCode::UNPROCESSABLE_ENTITY, message.into())
}
fn conflict(message: impl Into<String>) -> ApiError {
    ApiError(StatusCode::CONFLICT, message.into())
}

fn scaffold_dir(state: &AppState) -> PathBuf {
    state.config.data_dir.join("scaffolds")
}

fn hex_digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn proposal_path(state: &AppState, hash: &str) -> PathBuf {
    scaffold_dir(state)
        .join("proposals")
        .join(format!("{}.json", hex_digest(hash)))
}

fn result_path(state: &AppState, key: &str) -> PathBuf {
    scaffold_dir(state)
        .join("results")
        .join(format!("{}.json", hex_digest(key)))
}

fn write_private_json(path: &Path, value: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec(value)?)?;
    std::fs::rename(&temporary, path)
}

fn data_dir(state: &AppState) -> Result<PathBuf, ApiError> {
    std::fs::canonicalize(&state.config.data_dir)
        .map(|path| strip_verbatim_prefix(&path))
        .map_err(internal)
}

/// The parent directory, canonical: absolute, existing, not overlapping
/// Luuna's private data directory (which holds Luuna-owned worktrees).
fn resolve_parent(state: &AppState, parent_dir: &str) -> Result<PathBuf, ApiError> {
    if parent_dir.len() > MAX_PATH_BYTES
        || parent_dir.contains('\0')
        || !Path::new(parent_dir).is_absolute()
    {
        return Err(bad_request(
            "parent_dir must be an absolute path to an existing directory",
        ));
    }
    let parent = std::fs::canonicalize(parent_dir)
        .map(|path| strip_verbatim_prefix(&path))
        .map_err(|_| unprocessable(format!("parent directory {parent_dir} does not exist")))?;
    if !parent.is_dir() {
        return Err(unprocessable(format!(
            "parent directory {parent_dir} is not a directory"
        )));
    }
    let data = data_dir(state)?;
    if parent.starts_with(&data) {
        return Err(unprocessable(format!(
            "{} is inside Luuna's data directory {} (LUUNA_DATA_DIR), which holds Luuna-owned worktrees; scaffold the project elsewhere",
            parent.display(),
            data.display()
        )));
    }
    Ok(parent)
}

/// The target must not exist, or must be an empty directory; it must not
/// contain Luuna's data directory either.
fn check_target(state: &AppState, target: &Path) -> Result<(), ApiError> {
    let data = data_dir(state)?;
    if data.starts_with(target) {
        return Err(unprocessable(format!(
            "Luuna's data directory {} (LUUNA_DATA_DIR) is inside {}; a scaffolded repository must not contain it",
            data.display(),
            target.display()
        )));
    }
    match std::fs::symlink_metadata(target) {
        Err(_) => Ok(()),
        Ok(metadata) if metadata.is_dir() => {
            let mut entries = std::fs::read_dir(target).map_err(internal)?;
            if entries.next().is_some() {
                Err(conflict(format!(
                    "{} exists and is not empty; scaffolding never writes into an existing project",
                    target.display()
                )))
            } else {
                Ok(())
            }
        }
        Ok(_) => Err(conflict(format!(
            "{} exists and is not a directory",
            target.display()
        ))),
    }
}

fn project_id_taken(state: &AppState, name: &str) -> Result<(), ApiError> {
    let store = state.store.lock().map_err(internal)?;
    if store.get_project(name).map_err(internal)?.is_some() {
        return Err(conflict(format!(
            "project {name} is already registered; choose another name"
        )));
    }
    // A removed project keeps its ID (ADR 0063), so registration would refuse it.
    if store.removed_project(name).map_err(internal)?.is_some() {
        return Err(conflict(format!(
            "project {name} was removed and keeps its ID; choose another name"
        )));
    }
    Ok(())
}

/// `POST /api/v1/projects/scaffold/propose`
pub async fn propose_scaffold(
    State(state): State<AppState>,
    Json(request): Json<ProposeScaffold>,
) -> Result<Json<ScaffoldProposal>, ApiError> {
    let name = request.name.trim().to_owned();
    if !valid_project_id(&name) {
        return Err(bad_request(
            "name must match ^[a-z][a-z0-9-]{0,63}$; it names the directory, the package and the project",
        ));
    }
    let parent = resolve_parent(&state, &request.parent_dir)?;
    check_target(&state, &parent.join(&name))?;
    project_id_taken(&state, &name)?;
    let profile = request.profile.trim().to_owned();
    let proposal = tokio::task::spawn_blocking(move || {
        let host = probe_host_cached();
        propose(&profile, &name, &parent, &host)
    })
    .await
    .map_err(internal)?
    .map_err(|e| unprocessable(e.to_string()))?;
    write_private_json(
        &proposal_path(&state, &proposal.hash),
        &serde_json::to_value(&proposal).map_err(internal)?,
    )
    .map_err(internal)?;
    Ok(Json(proposal))
}

fn load_proposal(state: &AppState, hash: &str) -> Result<ScaffoldProposal, ApiError> {
    let mismatch = || {
        conflict(format!(
            "proposal_hash {hash} does not name a current proposal; request a new proposal and confirm its hash"
        ))
    };
    if hash.is_empty() || hash.len() > 128 || !hash.starts_with("sha256:") {
        return Err(mismatch());
    }
    let bytes = std::fs::read(proposal_path(state, hash)).map_err(|_| mismatch())?;
    let proposal: ScaffoldProposal = serde_json::from_slice(&bytes).map_err(|_| mismatch())?;
    if proposal.hash != hash || proposal.computed_hash().map_err(internal)? != hash {
        return Err(mismatch());
    }
    Ok(proposal)
}

fn relative_file_path(path: &str) -> Result<PathBuf, ApiError> {
    let mut out = PathBuf::new();
    if path.is_empty() || path.len() > 512 || path.starts_with('/') || path.contains('\\') {
        return Err(internal(format!("template path {path} is not relative")));
    }
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains(':') {
            return Err(internal(format!("template path {path} is not relative")));
        }
        out.push(part);
    }
    Ok(out)
}

fn write_template(target: &Path, proposal: &ScaffoldProposal) -> Result<(), ApiError> {
    std::fs::create_dir_all(target).map_err(internal)?;
    for file in &proposal.files {
        let path = target.join(relative_file_path(&file.path)?);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(internal)?;
        }
        std::fs::write(&path, file.contents.as_bytes()).map_err(internal)?;
    }
    Ok(())
}

/// Run the host `git` in `cwd` with hooks, signing and inherited `GIT_*`
/// variables neutralised, a deadline and bounded output.
async fn git(state: &AppState, cwd: &Path, args: &[&str]) -> Result<String, ApiError> {
    let mut command = Command::new(&state.config.git_program);
    command
        .env_clear()
        .envs(
            std::env::vars_os().filter(|(name, _)| !is_secret_or_reserved(&name.to_string_lossy())),
        )
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "--no-pager",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
        ])
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let failure = |detail: String| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "git {} failed in {}: {detail}; the scaffolded files were left in place",
                args.first().copied().unwrap_or(""),
                cwd.display()
            ),
        )
    };
    let mut child = command
        .spawn()
        .map_err(|e| failure(format!("could not start git: {}", e.kind())))?;
    let mut stdout = child.stdout.take().expect("piped").take(GIT_OUTPUT_LIMIT);
    let mut stderr = child.stderr.take().expect("piped").take(GIT_OUTPUT_LIMIT);
    let capture = async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let (status, _, _) = tokio::try_join!(
            child.wait(),
            stdout.read_to_end(&mut out),
            stderr.read_to_end(&mut err)
        )?;
        Ok::<_, std::io::Error>((status, out, err))
    };
    let (status, out, err) = match tokio::time::timeout(GIT_TIMEOUT, capture).await {
        Ok(Ok(captured)) => captured,
        Ok(Err(error)) => return Err(failure(format!("I/O: {}", error.kind()))),
        Err(_) => {
            return Err(failure(format!(
                "timed out after {}s",
                GIT_TIMEOUT.as_secs()
            )))
        }
    };
    if !status.success() {
        let detail = String::from_utf8_lossy(&err);
        let detail = detail
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(300)
            .collect::<String>();
        return Err(failure(format!(
            "exit status {:?}: {detail}",
            status.code()
        )));
    }
    Ok(String::from_utf8_lossy(&out).trim().to_owned())
}

/// Initialise `main`, commit every template file and return the commit.
async fn commit_scaffold(
    state: &AppState,
    target: &Path,
    proposal: &ScaffoldProposal,
) -> Result<String, ApiError> {
    git(state, target, &["init", "-q", "-b", "main"]).await?;
    git(state, target, &["add", "-A", "--", "."]).await?;
    // The developer's identity when one is configured; a scaffold identity otherwise.
    let identity_args: &[&str] = if git(state, target, &["config", "--get", "user.email"])
        .await
        .is_ok()
    {
        &[]
    } else {
        &[
            "-c",
            "user.name=Luuna scaffold",
            "-c",
            "user.email=scaffold@luuna.invalid",
        ]
    };
    let message = format!(
        "Scaffold {} project {} with Luuna\n\nProposal {}\nBaseline: {}",
        proposal.profile,
        proposal.name,
        proposal.hash,
        proposal
            .baseline_check()
            .map(|c| format!("{} {}", c.executable, c.args.join(" ")))
            .unwrap_or_default()
    );
    let mut args: Vec<&str> = identity_args.to_vec();
    args.extend(["commit", "-q", "--no-verify", "-m", &message]);
    git(state, target, &args).await?;
    git(state, target, &["rev-parse", "HEAD"]).await
}

fn step_spec(step: &ScaffoldStep, root: &Path, artifact_dir: &Path) -> CommandSpec {
    CommandSpec {
        recipe_id: step.id.clone(),
        executable: step.executable.clone(),
        args: step.args.clone(),
        repository_root: root.to_path_buf(),
        cwd: ".".into(),
        timeout: Duration::from_secs(1800),
        budget: None,
        capabilities: step.capabilities.clone(),
        artifact_dir: artifact_dir.to_path_buf(),
        environment: Vec::new(),
    }
}

async fn supervised(state: &AppState, spec: &CommandSpec) -> Result<CheckOutcome, ApiError> {
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let execution = state
        .executor
        .execute(spec, cancel_rx, None)
        .await
        .map_err(internal)?;
    Ok(CheckOutcome::from_execution(&spec.recipe_id, execution))
}

/// Serialises scaffold executions: idempotency is judged against the
/// recorded result, and two requests must not race on the same directory.
fn scaffold_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// `POST /api/v1/projects/scaffold`
pub async fn execute_scaffold(
    State(state): State<AppState>,
    Json(request): Json<ExecuteScaffold>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    if !request.confirm {
        return Err(bad_request(
            "confirm must be true: the request executes exactly the proposal its hash names",
        ));
    }
    let key = request.idempotency_key.trim().to_owned();
    if key.is_empty() || key.len() > 200 {
        return Err(bad_request(
            "an idempotency key of 1..200 bytes is required",
        ));
    }
    let _serial = scaffold_lock().lock().await;
    if let Ok(bytes) = std::fs::read(result_path(&state, &key)) {
        let mut recorded: Value = serde_json::from_slice(&bytes).map_err(internal)?;
        if recorded["proposal_hash"] != request.proposal_hash {
            return Err(conflict(format!(
                "idempotency key {key} was already used for proposal {}",
                recorded["proposal_hash"]
            )));
        }
        recorded["replayed"] = Value::Bool(true);
        return Ok((StatusCode::OK, Json(recorded)));
    }
    let proposal = load_proposal(&state, &request.proposal_hash)?;
    if proposal.status == ScaffoldStatus::Blocked {
        return Err(unprocessable(format!(
            "proposal {} is blocked: {}",
            proposal.hash,
            proposal
                .blocked_reason
                .as_deref()
                .unwrap_or("its platform capability is missing")
        )));
    }
    if !proposal.network_steps.is_empty() && !request.confirm_network {
        return Err(unprocessable(format!(
            "proposal {} declares network steps that need confirm_network: true: {}",
            proposal.hash,
            proposal.network_steps.join("; ")
        )));
    }
    let Some(baseline) = proposal.baseline_check().cloned() else {
        return Err(unprocessable("proposal has no required baseline recipe"));
    };
    let parent = resolve_parent(&state, &proposal.parent_dir)?;
    let target = parent.join(&proposal.name);
    check_target(&state, &target)?;
    project_id_taken(&state, &proposal.name)?;

    let artifact_dir = state
        .config
        .data_dir
        .join("artifacts")
        .join(format!("scaffold-{}", &hex_digest(&proposal.hash)[..16]));
    let generator = match &proposal.generator {
        Some(step) => {
            let outcome = supervised(&state, &step_spec(step, &parent, &artifact_dir)).await?;
            if outcome.status != CheckStatus::Passed || !target.is_dir() {
                return Err(unprocessable(format!(
                    "generator {} did not create {}: {} ({})",
                    step.id,
                    target.display(),
                    serde_json::to_string(&outcome.status).unwrap_or_default(),
                    outcome.reason.as_deref().unwrap_or("no reason recorded")
                )));
            }
            Some(outcome)
        }
        None => {
            write_template(&target, &proposal)?;
            None
        }
    };
    let commit = commit_scaffold(&state, &target, &proposal).await?;
    let mut preparation = Vec::new();
    for step in &proposal.preparation {
        preparation.push(if step.network && !request.confirm_network {
            CheckOutcome::skipped(step, "network step not confirmed (confirm_network)")
        } else {
            supervised(&state, &step_spec(step, &target, &artifact_dir)).await?
        });
    }
    let target_text = target.display().to_string();
    let check = supervised(&state, &recipe_spec(&target_text, &baseline, &artifact_dir)).await?;

    let (_, Json(project)) = projects::register_project(
        State(state.clone()),
        Json(RegisterProject {
            id: Some(proposal.name.clone()),
            name: proposal.name.clone(),
            repositories: vec![RegisterRepository {
                id: "app".into(),
                path: target_text.clone(),
            }],
        }),
    )
    .await?;
    let (_, Json(discovery)) = projects::discover(
        State(state.clone()),
        axum::extract::Path(proposal.name.clone()),
    )
    .await?;
    let baseline_passed = check.status == CheckStatus::Passed;
    state
        .store
        .lock()
        .map_err(internal)?
        .record_project_event(
            &proposal.name,
            "project.scaffolded",
            json!({
                "proposal_hash": proposal.hash,
                "idempotency_key": key,
                "profile": proposal.profile,
                "target_dir": target_text,
                "commit": commit,
                "generator": generator.as_ref().map(CheckOutcome::summary),
                "preparation": preparation.iter().map(CheckOutcome::summary).collect::<Vec<_>>(),
                "check": check.summary(),
                "baseline_passed": baseline_passed,
                "discovery_id": discovery.id,
            }),
        )
        .map_err(projects::storage_error)?;
    let result = ScaffoldResult {
        proposal_hash: proposal.hash.clone(),
        idempotency_key: key.clone(),
        profile: proposal.profile.clone(),
        target_dir: target_text,
        commit,
        generator,
        preparation,
        check,
        baseline_passed,
        project,
        discovery,
        replayed: false,
    };
    let value = serde_json::to_value(&result).map_err(internal)?;
    write_private_json(&result_path(&state, &key), &value).map_err(internal)?;
    Ok((StatusCode::CREATED, Json(value)))
}
