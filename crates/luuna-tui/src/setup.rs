//! User-owned first-run prerequisites. No secrets or tools live in the project.
use crate::client::{Client, Signal};
#[cfg(unix)]
use std::io::Read;
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

const PI_PACKAGE: &[u8] = include_bytes!("../../../tools/pi/package.json");
const PI_LOCK: &[u8] = include_bytes!("../../../tools/pi/package-lock.json");

const PROVIDER_KEYS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "MISTRAL_API_KEY",
    "GROQ_API_KEY",
    "XAI_API_KEY",
    "OPENROUTER_API_KEY",
];

pub fn directory() -> io::Result<PathBuf> {
    let config = crate::config::path().ok_or_else(|| {
        io::Error::other("Set HOME or XDG_CONFIG_HOME to a private user directory")
    })?;
    Ok(config
        .parent()
        .ok_or_else(|| io::Error::other("Configuration path has no parent"))?
        .join("private"))
}

fn secure_dir(path: &Path) -> io::Result<()> {
    if path.ancestors().any(|parent| parent.join(".git").exists()) {
        return Err(io::Error::other("Private configuration must be outside every repository; set XDG_CONFIG_HOME to a private user folder"));
    }
    fs::create_dir_all(path)?;
    if path
        .canonicalize()?
        .ancestors()
        .any(|parent| parent.join(".git").exists())
    {
        return Err(io::Error::other(
            "Private configuration resolves inside a repository; choose a private user folder",
        ));
    }
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(io::Error::other(
            "Private configuration directory must not be a symlink",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn allowed(name: &str) -> bool {
    matches!(
        name,
        "LUUNA_API_TOKEN"
            | "LUUNA_DATA_DIR"
            | "LUUNA_PI_COMMAND"
            | "LUUNA_PI_ARGS_JSON"
            | "LUUNA_COMPONENTS_ROOT"
            | "LUUNA_BUN"
    ) || PROVIDER_KEYS.contains(&name)
}

pub fn load(path: &Path) -> io::Result<BTreeMap<String, String>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 64 * 1024 {
        return Err(io::Error::other(
            "Private settings must be a regular file under 64 KiB",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::other(
                "Private settings must have mode 0600; restrict permissions before continuing",
            ));
        }
    }
    let vars: BTreeMap<String, String> = serde_json::from_slice(&fs::read(path)?)
        .map_err(|_| io::Error::other("Private settings are invalid JSON (values hidden)"))?;
    if vars.keys().any(|name| !allowed(name)) {
        return Err(io::Error::other(
            "Private settings contain an unsupported variable",
        ));
    }
    Ok(vars)
}

fn save(path: &Path, vars: &BTreeMap<String, String>) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Missing private directory"))?;
    secure_dir(parent)?;
    let temp = parent.join(format!(".environment-{}.tmp", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    let result = (|| {
        file.write_all(&serde_json::to_vec_pretty(vars).map_err(io::Error::other)?)?;
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

async fn run(
    program: &str,
    args: &[&str],
    cwd: &Path,
    vars: &BTreeMap<String, String>,
) -> io::Result<()> {
    let mut command = Command::new(program);
    for (name, value) in vars {
        if std::env::var_os(name).is_none() {
            command.env(name, value);
        }
    }
    let mut child = command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    match tokio::time::timeout(Duration::from_secs(300), child.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        _ => {
            let _ = child.kill().await;
            Err(io::Error::other(format!(
                "{program} setup failed or timed out; previous private settings were preserved"
            )))
        }
    }
}

/// Prepares interactive init; callers relaunch with the returned environment rather
/// than mutating process environment after Tokio has started worker threads.
pub async fn prepare(root: &Path, vars: &mut BTreeMap<String, String>) -> io::Result<()> {
    let private = directory()?;
    secure_dir(&private)?;
    let tools = private.join("tools");
    secure_dir(&tools)?;
    let current = |name: &str, vars: &BTreeMap<String, String>| {
        std::env::var(name)
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| vars.get(name).cloned())
    };
    if current("LUUNA_API_TOKEN", vars).is_none() {
        #[cfg(unix)]
        {
            let mut bytes = [0u8; 32];
            fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
            vars.insert(
                "LUUNA_API_TOKEN".into(),
                bytes.iter().map(|b| format!("{b:02x}")).collect(),
            );
        }
        #[cfg(not(unix))]
        return Err(io::Error::other(
            "Set LUUNA_API_TOKEN using the platform credential setup",
        ));
    }
    vars.entry("LUUNA_DATA_DIR".into())
        .or_insert_with(|| private.join("data").to_string_lossy().into_owned());
    let pi = tools.join("pi");
    let entry = pi.join("node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js");
    // Never replace a user's Pi installation or its OAuth/configuration stores.
    let owned_pi_missing = std::env::var_os("LUUNA_PI_COMMAND").is_none()
        && vars
            .get("LUUNA_PI_COMMAND")
            .is_some_and(|program| program == "node")
        && vars.get("LUUNA_PI_ARGS_JSON").is_some_and(|args| {
            serde_json::from_str::<Vec<PathBuf>>(args)
                .is_ok_and(|args| args.as_slice() == std::slice::from_ref(&entry))
        })
        && !entry.is_file();
    if current("LUUNA_PI_COMMAND", vars).is_none() || owned_pi_missing {
        eprintln!("Preparing Luuna's pinned Pi 1.3.0 in its private tools folder…");
        fs::create_dir_all(&pi)?;
        // The release contains binaries only. Embed the reviewed lockfile so
        // setup also works after installation without a source checkout.
        fs::write(pi.join("package.json"), PI_PACKAGE)?;
        fs::write(pi.join("package-lock.json"), PI_LOCK)?;
        run(
            "npm",
            &["ci", "--ignore-scripts", "--no-audit", "--no-fund"],
            &pi,
            vars,
        )
        .await?;
        vars.insert("LUUNA_PI_COMMAND".into(), "node".into());
        vars.insert(
            "LUUNA_PI_ARGS_JSON".into(),
            serde_json::to_string(&[entry]).map_err(io::Error::other)?,
        );
    }
    if root.join("components/manifest.json").is_file() {
        let owned_bun = tools.join("bun/node_modules/.bin/bun");
        let owned_bun_missing = std::env::var_os("LUUNA_BUN").is_none()
            && vars
                .get("LUUNA_BUN")
                .is_some_and(|path| Path::new(path) == owned_bun)
            && !owned_bun.is_file();
        if current("LUUNA_BUN", vars).is_none() || owned_bun_missing {
            eprintln!("Preparing Luuna's pinned Bun 1.4.3…");
            let bun = tools.join("bun");
            fs::create_dir_all(&bun)?;
            run(
                "npm",
                &[
                    "install",
                    "--no-audit",
                    "--no-fund",
                    "--save-exact",
                    "bun@1.4.3",
                ],
                &bun,
                vars,
            )
            .await?;
            vars.insert(
                "LUUNA_BUN".into(),
                bun.join("node_modules/.bin/bun")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        eprintln!("Preparing the pinned project components…");
        run("node", &["scripts/components.mjs", "setup"], root, vars).await?;
        vars.insert(
            "LUUNA_COMPONENTS_ROOT".into(),
            root.to_string_lossy().into_owned(),
        );
    }
    if !PROVIDER_KEYS.iter().any(|k| {
        current(k, vars).is_some()
            || std::env::var(format!("LUUNA_PI_{k}")).is_ok_and(|v| !v.is_empty())
    }) && std::env::var_os("LUUNA_PI_AUTH_FILE").is_none()
    {
        eprintln!("Enter an OpenRouter provider key. It is hidden and saved only in Luuna's private configuration.");
        let mut process = crate::cli::Process::real();
        let key = crate::cli::read_secret("Provider key (hidden)", &mut process)
            .map_err(|_| io::Error::other("Provider setup cancelled"))?;
        if key.trim().is_empty() {
            return Err(io::Error::other(
                "A provider key is required; rerun lu init when one is available",
            ));
        }
        vars.insert("OPENROUTER_API_KEY".into(), key);
    }
    save(&private.join("environment.json"), vars)?;
    Ok(())
}

pub async fn ensure_runtime(vars: &BTreeMap<String, String>) -> io::Result<()> {
    let value = |name: &str| std::env::var(name).ok().or_else(|| vars.get(name).cloned());
    let token = value("LUUNA_API_TOKEN").unwrap_or_default();
    let url = value("LUUNA_URL").unwrap_or_else(|| crate::cli::DEFAULT_URL.into());
    let client = Client::new(&url, &token)
        .map_err(|_| io::Error::other("Invalid runtime URL or service token"))?;
    let health = || async {
        tokio::time::timeout(
            Duration::from_secs(2),
            client.get("/health", &Signal::never()),
        )
        .await
        .is_ok_and(|r| r.is_ok())
    };
    if !health().await {
        let executable = std::env::current_exe()?
            .parent()
            .ok_or_else(|| io::Error::other("Missing executable directory"))?
            .join(if cfg!(windows) {
                "luuna-runtime.exe"
            } else {
                "luuna-runtime"
            });
        if !executable.is_file() {
            return Err(io::Error::other("The runtime executable is missing beside luuna; reinstall the complete Luuna release"));
        }
        let private = directory()?;
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(private.join("runtime.log"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                private.join("runtime.log"),
                fs::Permissions::from_mode(0o600),
            )?;
        }
        let mut command = Command::new(executable);
        for (name, value) in vars {
            if std::env::var_os(name).is_none() {
                command.env(name, value);
            }
        }
        command
            .current_dir(&private)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn()?;
        let mut ready = false;
        for _ in 0..100 {
            if child.try_wait()?.is_some() {
                return Err(io::Error::other(
                    "Runtime startup failed; inspect Luuna's private runtime.log",
                ));
            }
            if health().await {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if !ready {
            let _ = child.kill().await;
            return Err(io::Error::other("Runtime did not become ready; check the private log and whether another service owns the port"));
        }
    }
    let doctor = tokio::time::timeout(
        Duration::from_secs(35),
        client.get("/doctor", &Signal::never()),
    )
    .await
    .map_err(|_| io::Error::other("Runtime diagnostics timed out"))?
    .map_err(|_| io::Error::other("Runtime diagnostics failed"))?;
    for item in doctor.get("items").items() {
        if item.get("status").as_str() != Some("ok") {
            eprintln!("{}: {}", item.get("name").js(), item.get("detail").js());
        }
    }
    if doctor.get("ok") != &crate::js::J::Bool(true) {
        return Err(io::Error::other("Runtime diagnostics require attention; run lu doctor for the named missing prerequisites"));
    }
    eprintln!("Luuna runtime diagnostics passed. GitHub publication is optional; configure its token when you want to publish.");
    Ok(())
}

/// Local recovery is explicit and never changes the baseline policy.
pub async fn git(root: &Path, args: &[&str]) -> io::Result<String> {
    use tokio::io::AsyncReadExt;
    let mut child = Command::new("git")
        .arg("--no-optional-locks")
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("Git output unavailable"))?;
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let mut bytes = Vec::new();
        (&mut stdout)
            .take(256 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await?;
        if bytes.len() > 256 * 1024 {
            return Err(io::Error::other(
                "Too many changed paths to display; use your Git client before continuing",
            ));
        }
        if !child.wait().await?.success() {
            return Err(io::Error::other(
                "Git recovery failed; use your Git client before continuing",
            ));
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    })
    .await;
    match result {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => {
            let _ = child.kill().await;
            Err(error)
        }
        Err(_) => {
            let _ = child.kill().await;
            Err(io::Error::other("Git recovery timed out"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_settings_roundtrip_and_refuse_unsafe_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private/environment.json");
        let vars = BTreeMap::from([("OPENROUTER_API_KEY".into(), "synthetic-key".into())]);
        save(&path, &vars).unwrap();
        assert_eq!(load(&path).unwrap(), vars);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(load(&path).is_err());
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        fs::write(&path, r#"{"UNRELATED":"synthetic"}"#).unwrap();
        assert!(load(&path).is_err());
        assert!(!load(&dir.path().join("missing"))
            .unwrap()
            .contains_key("OPENROUTER_API_KEY"));
    }
    #[cfg(unix)]
    #[test]
    fn a_symlinked_parent_cannot_redirect_new_secrets_into_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        let repository = dir.path().join("repository");
        fs::create_dir_all(repository.join(".git")).unwrap();
        std::os::unix::fs::symlink(&repository, dir.path().join("config-link")).unwrap();
        let target = dir.path().join("config-link/private/environment.json");
        assert!(save(&target, &BTreeMap::new()).is_err());
        assert!(!target.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_cannot_supply_private_settings() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("real"), "{}").unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
        assert!(load(&dir.path().join("link")).is_err());
    }
}
