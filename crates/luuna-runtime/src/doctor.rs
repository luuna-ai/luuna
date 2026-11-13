//! The host diagnostic shared by `luuna-runtime doctor` and `GET /api/v1/doctor`
//! (ADR 0018; ADR 0027 point 4 makes it the clients' "settings" surface): Git,
//! Python, Node, the Pi launcher, the data directory and database, token and
//! credential presence (names only, never values), the service port, the pinned
//! component layout (ADR 0024) and the Bun executable. The report is the same
//! JSON in both places; the route builds it from the running runtime's own
//! configuration, the command from the environment it was started in.
//!
//! Every probe that runs a program is bounded: a program that does not answer
//! within [`PROBE_DEADLINE`] is killed and reported as a failure, so a hung
//! `git --version` cannot stall the request, and the route runs the whole
//! report on a blocking thread under [`ROUTE_DEADLINE`].
use crate::{ApiError, AppState};
use axum::{extract::State, Json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

/// How long one external program may take to answer `--version`.
pub const PROBE_DEADLINE: Duration = Duration::from_secs(10);
/// How long the route waits for the complete report.
pub const ROUTE_DEADLINE: Duration = Duration::from_secs(30);

/// One diagnosed item. `status` is `ok`, `missing` or `warning`; a missing
/// required item fails the report.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DoctorItem {
    pub name: &'static str,
    pub required: bool,
    pub status: &'static str,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub items: Vec<DoctorItem>,
}

impl DoctorReport {
    /// The table `luuna-runtime doctor` prints (the terminal client prints
    /// the same one): one line per item, then the verdict.
    pub fn table(&self) -> String {
        let mut lines: Vec<String> = self
            .items
            .iter()
            .map(|item| {
                format!(
                    "{:<8} {:<20} {}{}",
                    item.status,
                    item.name,
                    item.detail,
                    if item.required { " (required)" } else { "" }
                )
            })
            .collect();
        lines.push(format!("doctor: {}", if self.ok { "ok" } else { "FAILED" }));
        lines.join("\n")
    }
}

type Probe<T> = Box<dyn Fn() -> T + Send + Sync>;

/// The probes a report is assembled from; tests inject them, the command and
/// the route run them against the host. `Ok` carries a one-line detail.
pub struct DoctorProbes {
    pub git: Probe<Result<String, String>>,
    pub python: Probe<Result<String, String>>,
    pub node: Probe<Result<String, String>>,
    pub pi: Probe<Result<String, String>>,
    pub data_dir: Probe<Result<String, String>>,
    /// `Ok(None)` when no database exists yet (the runtime creates it).
    pub database: Probe<Result<Option<String>, String>>,
    pub api_token_configured: bool,
    pub github_token_configured: bool,
    /// Provider credential variable names that carry a value (never the values).
    pub provider_keys: Vec<String>,
    /// Provider credential variable names that are set but empty: present in
    /// the environment, yet no model run can use them.
    pub empty_provider_keys: Vec<String>,
    pub port: Probe<PortStatus>,
    /// The pinned component layout: source checkout or portable (ADR 0024).
    pub components: Probe<Result<String, String>>,
    /// The Bun executable the components run under.
    pub bun: Probe<Result<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortStatus {
    Free(u16),
    /// A Luuna runtime already answers `/health` there.
    LuunaRunning(u16),
    /// Something else holds the port, or it could not be probed.
    Busy(u16, String),
    /// The reporting runtime itself is the one listening there (the route).
    Listening(u16),
    /// The reporting runtime answers without a recorded listening address
    /// (an in-process router, as the tests use).
    Unrecorded,
}

fn item(name: &'static str, required: bool, outcome: Result<String, String>) -> DoctorItem {
    match outcome {
        Ok(detail) => DoctorItem {
            name,
            required,
            status: "ok",
            detail,
        },
        Err(detail) => DoctorItem {
            name,
            required,
            status: if required { "missing" } else { "warning" },
            detail,
        },
    }
}

fn presence(name: &'static str, configured: bool, present: &str, absent: &str) -> DoctorItem {
    DoctorItem {
        name,
        required: false,
        status: if configured { "ok" } else { "warning" },
        detail: if configured { present } else { absent }.into(),
    }
}

/// Assemble the report from probe results. Git, Pi and a writable data
/// directory are required; an existing database that is damaged or too new
/// also fails the report, because the service would refuse to start on it.
/// Everything else is advisory.
pub fn doctor_report(probes: &DoctorProbes) -> DoctorReport {
    let database = match (probes.database)() {
        Ok(Some(detail)) => item("database", false, Ok(detail)),
        Ok(None) => item(
            "database",
            false,
            Ok("not created yet; the runtime creates it at first start".into()),
        ),
        Err(detail) => DoctorItem {
            name: "database",
            required: false,
            status: "missing",
            detail,
        },
    };
    let port = match (probes.port)() {
        PortStatus::Free(port) => item("port", false, Ok(format!("127.0.0.1:{port} is free"))),
        PortStatus::LuunaRunning(port) => item(
            "port",
            false,
            Ok(format!(
                "a Luuna runtime already answers http://127.0.0.1:{port}/health"
            )),
        ),
        PortStatus::Busy(port, detail) => item(
            "port",
            false,
            Err(format!(
                "127.0.0.1:{port} is in use and does not answer as Luuna ({detail}); set LUUNA_PORT"
            )),
        ),
        PortStatus::Listening(port) => item(
            "port",
            false,
            Ok(format!(
                "this runtime is the one listening on http://127.0.0.1:{port}"
            )),
        ),
        PortStatus::Unrecorded => item(
            "port",
            false,
            Ok("this runtime is answering; it recorded no listening address".into()),
        ),
    };
    let empty = if probes.empty_provider_keys.is_empty() {
        String::new()
    } else {
        format!(
            "set but empty: {} (an empty variable configures nothing)",
            probes.empty_provider_keys.join(", ")
        )
    };
    let providers = if probes.provider_keys.is_empty() {
        Err(if empty.is_empty() {
            "no provider credential variable is set (model runs fail until one is)".to_owned()
        } else {
            format!(
                "{empty}; no provider credential carries a value (model runs fail until one does)"
            )
        })
    } else {
        Ok(format!(
            "configured: {} (values not shown){}",
            probes.provider_keys.join(", "),
            if empty.is_empty() {
                String::new()
            } else {
                format!("; {empty}")
            }
        ))
    };
    let items = vec![
        item("git", true, (probes.git)()),
        item("python", false, (probes.python)()),
        item("node", false, (probes.node)()),
        item("pi", true, (probes.pi)()),
        item("data_dir", true, (probes.data_dir)()),
        database,
        presence(
            "api_token",
            probes.api_token_configured,
            "LUUNA_API_TOKEN is configured (value not shown)",
            "LUUNA_API_TOKEN is not set; the service refuses to start without it",
        ),
        presence(
            "github_token",
            probes.github_token_configured,
            "LUUNA_GITHUB_TOKEN is configured (value not shown)",
            "LUUNA_GITHUB_TOKEN is not set; publication to GitHub is unavailable",
        ),
        item("provider_credentials", false, providers),
        port,
        item("components", false, (probes.components)()),
        item("bun", false, (probes.bun)()),
    ];
    let ok = items.iter().all(|item| item.status != "missing");
    DoctorReport { ok, items }
}

/// Provider credential variable names by presence: those with a value and
/// those that are set but empty. Values are never read into the report.
pub struct ProviderKeys {
    pub configured: Vec<String>,
    pub empty: Vec<String>,
}

/// `luuna_engine_pi::configured_provider_keys` counts a variable that is set
/// but empty; the report separates the two, because an empty variable
/// configures nothing.
pub fn provider_keys() -> ProviderKeys {
    let mut keys = ProviderKeys {
        configured: Vec::new(),
        empty: Vec::new(),
    };
    for name in luuna_engine_pi::configured_provider_keys() {
        let has_value = [format!("LUUNA_PI_{name}"), name.to_owned()]
            .iter()
            .any(|variable| std::env::var_os(variable).is_some_and(|value| !value.is_empty()));
        if has_value {
            keys.configured.push(name.to_owned());
        } else {
            keys.empty.push(name.to_owned());
        }
    }
    if luuna_engine_pi::openai_oauth_configured() {
        keys.configured
            .push("OpenAI OAuth (LUUNA_PI_AUTH_FILE)".to_owned());
    }
    keys
}

/// Run `program argument` and return its first non-empty output line. The
/// child is killed and reported as a failure when it exceeds
/// [`PROBE_DEADLINE`], so a hung program cannot stall the report.
pub fn version_of(program: &str, argument: &str) -> Result<String, String> {
    version_of_within(program, argument, PROBE_DEADLINE)
}

fn version_of_within(program: &str, argument: &str, deadline: Duration) -> Result<String, String> {
    let mut child = std::process::Command::new(program)
        .arg(argument)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("{program} not found on PATH ({error})"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{program} {argument} did not answer within {:?} and was stopped",
                    deadline
                ));
            }
            Err(error) => {
                let _ = child.kill();
                return Err(format!(
                    "{program} {argument} could not be awaited ({error})"
                ));
            }
        }
    }
    // A version banner is far smaller than the pipe buffer, so the child
    // never blocked on it before exiting; this only collects what it wrote.
    let output = child
        .wait_with_output()
        .map_err(|error| format!("{program} {argument} output could not be read ({error})"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let line = stdout
        .lines()
        .chain(stderr.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_owned();
    if !output.status.success() {
        return Err(format!(
            "{program} {argument} exited with {}",
            output.status
        ));
    }
    Ok(line)
}

/// The Pi launcher the service would start, resolved and version-probed in a
/// private temporary directory (no model call, no user Pi configuration).
/// The engine bounds the probe itself (15 seconds).
pub fn probe_pi(program: &Path, launcher_args: &[String]) -> Result<String, String> {
    let (resolved, args) = luuna_engine_pi::resolve_launcher(program, launcher_args)
        .map_err(|error| error.to_string())?;
    let scratch = std::env::temp_dir().join(format!("luuna-doctor-{}", uuid::Uuid::new_v4()));
    let config = luuna_engine_pi::PiConfig {
        program: program.to_path_buf(),
        launcher_args: launcher_args.to_vec(),
        cwd: scratch.join("cwd"),
        config_dir: scratch.join("config"),
        provider: None,
        model: None,
        profile: luuna_engine_pi::SessionProfile::ToolFree,
    };
    // The caller may be inside a Tokio runtime (the command's `main`, or a
    // blocking thread of the service); the probe gets its own on a plain thread.
    let probe = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())
            .and_then(|runtime| {
                runtime
                    .block_on(luuna_engine_pi::verify_version(&config))
                    .map_err(|error| error.to_string())
            })
    })
    .join()
    .map_err(|_| "the Pi probe panicked".to_owned());
    let _ = std::fs::remove_dir_all(&scratch);
    probe??;
    Ok(format!(
        "pi {} at {}{}",
        luuna_engine_pi::PI_VERSION,
        resolved.display(),
        if args.is_empty() {
            String::new()
        } else {
            format!(" {}", args.join(" "))
        }
    ))
}

fn probe_data_dir(data_dir: &Path) -> Result<String, String> {
    if !data_dir.is_dir() {
        return Err(format!(
            "{} does not exist; the runtime creates it at first start if its parent is writable",
            data_dir.display()
        ));
    }
    let probe = data_dir.join(format!(".doctor-{}", uuid::Uuid::new_v4()));
    std::fs::write(&probe, b"doctor")
        .map_err(|error| format!("{} is not writable: {error}", data_dir.display()))?;
    let _ = std::fs::remove_file(&probe);
    Ok(format!("{} exists and is writable", data_dir.display()))
}

fn probe_database(path: &Path) -> Result<Option<String>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let status = luuna_storage::inspect_database(path).map_err(|error| error.to_string())?;
    status.require_usable().map_err(|error| error.to_string())?;
    Ok(Some(format!(
        "{} schema version {} (this build supports up to {}), integrity ok",
        status.path.display(),
        status.schema_version,
        status.supported_version
    )))
}

/// Bind the port; when it is taken, ask whoever holds it for `/health` with a
/// plain HTTP/1.0 request under a short deadline.
pub fn probe_port(port: u16) -> PortStatus {
    use std::io::{Read, Write};
    let address = std::net::SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, port);
    if std::net::TcpListener::bind(address).is_ok() {
        return PortStatus::Free(port);
    }
    let answer = (|| -> Result<String, String> {
        let deadline = Duration::from_secs(2);
        let mut stream = std::net::TcpStream::connect_timeout(&address.into(), deadline)
            .map_err(|error| error.to_string())?;
        let _ = stream.set_read_timeout(Some(deadline));
        let _ = stream.set_write_timeout(Some(deadline));
        stream
            .write_all(b"GET /health HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")
            .map_err(|error| error.to_string())?;
        let mut buffer = Vec::new();
        let _ = stream.take(64 * 1024).read_to_end(&mut buffer);
        Ok(String::from_utf8_lossy(&buffer).into_owned())
    })();
    match answer {
        Ok(text) if text.starts_with("HTTP/1.") && text.contains("\"pi_version\"") => {
            PortStatus::LuunaRunning(port)
        }
        Ok(text) => PortStatus::Busy(
            port,
            text.lines()
                .next()
                .filter(|line| !line.is_empty())
                .unwrap_or("no HTTP answer")
                .chars()
                .take(80)
                .collect(),
        ),
        Err(detail) => PortStatus::Busy(port, detail),
    }
}

/// How the `port` item is established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Port {
    /// The command: bind the configured port, or ask whoever holds it.
    Probe(u16),
    /// The route: this runtime is the one listening there.
    Listening(u16),
    /// The route without a recorded listening address (an in-process router).
    Unrecorded,
}

/// What the host probes are built from: the command passes what it read from
/// the environment, the route what the running runtime was started with.
pub struct Host {
    pub data_dir: PathBuf,
    pub git_program: PathBuf,
    pub pi_program: PathBuf,
    pub pi_launcher_args: Vec<String>,
    pub port: Port,
    /// The toolchain the running runtime discovered at start; `None`
    /// discovers now (a portable layout verifies every staged digest, which
    /// is the expensive part, so it happens once for both component items).
    pub components: Option<Arc<luuna_components::Components>>,
    pub api_token_configured: bool,
    pub github_token_configured: bool,
}

/// The probes of a real host. Only presence is read from credentials.
pub fn host_probes(host: Host) -> DoctorProbes {
    let Host {
        data_dir,
        git_program,
        pi_program,
        pi_launcher_args,
        port,
        components,
        api_token_configured,
        github_token_configured,
    } = host;
    let database_path = data_dir.join("luuna.sqlite3");
    let python = if cfg!(windows) { "python" } else { "python3" };
    let components = match components {
        Some(components) => Ok(components),
        None => luuna_components::Components::discover()
            .map(Arc::new)
            .map_err(|error| error.to_string()),
    };
    let components_for_bun = components.clone();
    let keys = provider_keys();
    DoctorProbes {
        git: Box::new(move || version_of(&git_program.to_string_lossy(), "--version")),
        python: Box::new(move || version_of(python, "--version")),
        node: Box::new(|| version_of("node", "--version")),
        pi: Box::new(move || probe_pi(&pi_program, &pi_launcher_args)),
        data_dir: Box::new(move || probe_data_dir(&data_dir)),
        database: Box::new(move || probe_database(&database_path)),
        api_token_configured,
        github_token_configured,
        provider_keys: keys.configured,
        empty_provider_keys: keys.empty,
        port: Box::new(move || match port {
            Port::Probe(port) => probe_port(port),
            Port::Listening(port) => PortStatus::Listening(port),
            Port::Unrecorded => PortStatus::Unrecorded,
        }),
        components: Box::new(move || {
            components
                .as_ref()
                .map(|components| {
                    format!(
                        "pinned Missions/Atlas toolchain discovered: {}; Beacon runs natively",
                        components.describe()
                    )
                })
                .map_err(|error| {
                    format!("component toolchain unavailable: {error}; set LUUNA_COMPONENTS_ROOT to a source checkout or a packaged layout")
                })
        }),
        bun: Box::new(move || {
            match &components_for_bun {
            Ok(components) => Ok(format!(
                "bun {} at {} (revalidated before every legacy component call)",
                components.runtime_version(),
                components.bun().display()
            )),
            Err(_) => version_of("bun", "--version")
                .map(|version| {
                    format!("bun {version} on PATH; the component layout was not discovered")
                })
                .map_err(|_| {
                    "bun not found; install Bun 1.4.3 (npm install bun@1.4.3) or use the packaged layout (LUUNA_BUN)".to_owned()
                }),
        }
        }),
    }
}

/// `GET /api/v1/doctor`: the report for the running runtime, from its own
/// configuration (ADR 0027 point 4: the clients' settings surface). Values
/// of the token and credentials never appear; the probes run on a blocking
/// thread and the request fails with 503 at [`ROUTE_DEADLINE`].
pub async fn doctor(State(state): State<AppState>) -> Result<Json<DoctorReport>, ApiError> {
    let config = state.config.clone();
    let host = Host {
        data_dir: config.data_dir.clone(),
        git_program: config.git_program.clone(),
        pi_program: config.pi_program.clone(),
        pi_launcher_args: config.pi_launcher_args.clone(),
        port: match state.listen_port {
            Some(port) => Port::Listening(port),
            None => Port::Unrecorded,
        },
        components: state.components.clone(),
        api_token_configured: !config.token.is_empty(),
        github_token_configured: state.publication.github_token_configured(),
    };
    let probing = tokio::task::spawn_blocking(move || doctor_report(&host_probes(host)));
    match tokio::time::timeout(ROUTE_DEADLINE, probing).await {
        Ok(Ok(report)) => Ok(Json(report)),
        Ok(Err(_)) => Err(ApiError(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "the doctor probes panicked".into(),
        )),
        Err(_) => Err(ApiError(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            format!(
                "the doctor probes did not finish within {}s; run luuna-runtime doctor on the host to see which program hangs",
                ROUTE_DEADLINE.as_secs()
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probes(pi: Result<String, String>, data_dir: Result<String, String>) -> DoctorProbes {
        DoctorProbes {
            git: Box::new(|| Ok("git version 2.55.0".into())),
            python: Box::new(|| Err("python not found".into())),
            node: Box::new(|| Ok("v26.0.0".into())),
            pi: Box::new(move || pi.clone()),
            data_dir: Box::new(move || data_dir.clone()),
            database: Box::new(|| Ok(None)),
            api_token_configured: true,
            github_token_configured: false,
            provider_keys: vec!["OPENROUTER_API_KEY".into()],
            empty_provider_keys: vec![],
            port: Box::new(|| PortStatus::LuunaRunning(4242)),
            components: Box::new(|| Err("component toolchain unavailable".into())),
            bun: Box::new(|| Err("bun not found".into())),
        }
    }

    #[test]
    fn doctor_report_fails_only_on_missing_required_items() {
        let report = doctor_report(&probes(
            Ok("pi 1.3.0 at C:/pi".into()),
            Ok("C:/data exists and is writable".into()),
        ));
        assert!(report.ok, "{report:?}");
        let by_name = |name: &str| report.items.iter().find(|i| i.name == name).unwrap();
        assert_eq!(by_name("python").status, "warning");
        assert_eq!(by_name("github_token").status, "warning");
        assert!(by_name("github_token").detail.contains("not set"));
        assert_eq!(by_name("provider_credentials").status, "ok");
        assert!(by_name("provider_credentials")
            .detail
            .contains("OPENROUTER_API_KEY"));
        assert!(by_name("port").detail.contains("already answers"));
        assert_eq!(by_name("components").status, "warning");
        assert_eq!(by_name("bun").status, "warning");
        assert!(!by_name("bun").required);
        assert_eq!(
            by_name("database").detail,
            "not created yet; the runtime creates it at first start"
        );
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["items"][0]["name"], "git");

        let failed = doctor_report(&probes(
            Err("unsupported Pi version".into()),
            Ok("ok".into()),
        ));
        assert!(!failed.ok);
        assert_eq!(
            failed.items.iter().find(|i| i.name == "pi").unwrap().status,
            "missing"
        );
        let unwritable = doctor_report(&probes(Ok("pi".into()), Err("read-only".into())));
        assert!(!unwritable.ok);
    }

    #[test]
    fn doctor_reports_the_component_layout_kind_and_bun_as_advisory_items() {
        let mut probes = probes(Ok("pi".into()), Ok("ok".into()));
        probes.components = Box::new(|| {
            Ok("pinned Missions/Atlas/Beacon toolchain discovered: portable layout at C:/Luuna/luuna (distribution manifest digests verified)".into())
        });
        probes.bun = Box::new(|| Ok("bun 1.4.3 at C:/Luuna/luuna/bun/bun.exe".into()));
        let report = doctor_report(&probes);
        assert!(report.ok);
        let by_name = |name: &str| report.items.iter().find(|i| i.name == name).unwrap();
        assert_eq!(by_name("components").status, "ok");
        assert!(by_name("components").detail.contains("portable layout"));
        assert_eq!(by_name("bun").status, "ok");
        assert!(by_name("bun").detail.contains("1.4.3"));
        let names: Vec<&str> = report.items.iter().map(|i| i.name).collect();
        assert_eq!(&names[names.len() - 2..], ["components", "bun"]);
    }

    #[test]
    fn a_damaged_existing_database_fails_the_report() {
        let mut probes = probes(Ok("pi".into()), Ok("ok".into()));
        probes.database = Box::new(|| Err("database failed its integrity check".into()));
        let report = doctor_report(&probes);
        assert!(!report.ok);
        let database = report.items.iter().find(|i| i.name == "database").unwrap();
        assert_eq!(database.status, "missing");
        assert!(!database.required);
    }

    #[test]
    fn an_empty_provider_variable_is_reported_as_set_but_empty_not_configured() {
        let mut probes = probes(Ok("pi".into()), Ok("ok".into()));
        probes.empty_provider_keys = vec!["ANTHROPIC_API_KEY".into()];
        let report = doctor_report(&probes);
        let providers = |report: &DoctorReport| {
            report
                .items
                .iter()
                .find(|i| i.name == "provider_credentials")
                .unwrap()
                .clone()
        };
        let both = providers(&report);
        assert_eq!(both.status, "ok");
        assert_eq!(
            both.detail,
            "configured: OPENROUTER_API_KEY (values not shown); set but empty: ANTHROPIC_API_KEY (an empty variable configures nothing)"
        );
        probes.provider_keys = vec![];
        let only_empty = providers(&doctor_report(&probes));
        assert_eq!(only_empty.status, "warning");
        assert!(only_empty
            .detail
            .starts_with("set but empty: ANTHROPIC_API_KEY"));
        assert!(only_empty
            .detail
            .contains("no provider credential carries a value"));
    }

    #[test]
    fn the_runtime_reports_its_own_port_and_the_table_matches_the_command() {
        let mut probes = probes(Ok("pi".into()), Ok("ok".into()));
        probes.port = Box::new(|| PortStatus::Listening(4242));
        let report = doctor_report(&probes);
        let port = report.items.iter().find(|i| i.name == "port").unwrap();
        assert_eq!(
            port.detail,
            "this runtime is the one listening on http://127.0.0.1:4242"
        );
        let table = report.table();
        assert!(table.starts_with("ok       git                  git version 2.55.0 (required)\n"));
        assert!(table.ends_with("\ndoctor: ok"));
        assert_eq!(table.lines().count(), report.items.len() + 1);
    }

    #[test]
    fn a_program_answers_with_its_first_line_or_is_named_as_missing() {
        let answered = version_of("git", "--version").unwrap();
        assert!(answered.starts_with("git version"), "{answered}");
        let missing = version_of("luuna-no-such-program-for-doctor", "--version").unwrap_err();
        assert!(missing.contains("not found on PATH"), "{missing}");
    }

    #[cfg(unix)]
    #[test]
    fn a_hung_program_is_stopped_at_the_probe_deadline() {
        let started = Instant::now();
        let hung = version_of_within("sleep", "30", Duration::from_millis(200)).unwrap_err();
        assert!(hung.contains("did not answer within"), "{hung}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
