use serde::{Deserialize, Serialize};

// No Debug: a connection contains the bearer token. Never log this value.
#[derive(Deserialize, Serialize)]
pub struct LocalConnection {
    url: String,
    token: String,
}

fn validate_connection(bytes: &[u8]) -> Result<LocalConnection, String> {
    let connection: LocalConnection = serde_json::from_slice(bytes)
        .map_err(|_| "The local startup helper returned an invalid connection.".to_owned())?;
    if connection.url != "http://127.0.0.1:4242"
        || connection.token.len() != 64
        || !connection.token.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("The local startup helper returned an unexpected connection.".to_owned());
    }
    Ok(connection)
}

#[cfg(target_os = "windows")]
fn bootstrap() -> Result<LocalConnection, String> {
    use std::{
        io::{BufRead, BufReader, Read},
        os::windows::process::CommandExt,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let root = install_root()?;
    let powershell = std::path::PathBuf::from(
        std::env::var_os("SystemRoot").ok_or("Windows directory unavailable.")?,
    )
    .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    // Only this fixed, repository-owned helper may run. No command/path/URL inputs from the view.
    let mut child = Command::new(powershell)
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(root.join("scripts/ensure-windows-runtime.ps1"))
        .arg("-EmitConnection")
        .current_dir(root)
        .creation_flags(0x08000000) // CREATE_NO_WINDOW; the runtime is an independent hidden process.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Could not start the Windows runtime helper.".to_owned())?;
    let deadline = Instant::now() + Duration::from_secs(75);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                // Only reap the helper. A service already launched must survive the client.
                let _ = child.kill();
                let _ = child.wait();
                return Err(
                    "Local startup timed out. Retry to reconnect to any runtime already started."
                        .to_owned(),
                );
            }
        }
    };
    if !status.success() {
        return Err("Local runtime startup failed. Check that port 4242 is free, the runtime is built, and your key is saved with Configure Luuna.cmd. Diagnostics are in your .luuna/logs folder.".to_owned());
    }
    // Windows descendants can retain an inherited pipe handle even with redirected
    // standard streams. Read the bounded JSON line, never wait for runtime-lifetime EOF.
    let stdout = child
        .stdout
        .take()
        .ok_or("The startup connection pipe is unavailable.")?;
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = BufReader::new(stdout.take(1024)).read_until(b'\n', &mut bytes);
        let _ = sender.send(result.map(|_| bytes));
    });
    let bytes = receiver
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| "The startup helper did not return its connection in time.".to_owned())?
        .map_err(|_| "Could not read the local connection.".to_owned())?;
    validate_connection(&bytes)
}

/// Where the fixed startup helper lives. A packaged installation
/// (`scripts/package-windows.ps1`) carries the helper, the runtime and the
/// pinned Pi tree in a `luuna` folder beside the executable; a development
/// executable uses its own checkout. Nothing from the view or the environment
/// chooses between them: the installed layout wins whenever it is present.
#[cfg(target_os = "windows")]
fn install_root() -> Result<std::path::PathBuf, String> {
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            let installed = directory.join("luuna");
            if installed
                .join("scripts")
                .join("ensure-windows-runtime.ps1")
                .is_file()
                && installed.join("luuna-runtime.exe").is_file()
            {
                return Ok(installed);
            }
        }
    }
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| "Cannot locate the development checkout.".to_owned())
}

/// The installed Luuna release's terminal executable, from the installer's
/// fixed layout (ADR 0048): `<home>/versions/<current>/bin/luuna-tui`. The
/// home is `LUUNA_INSTALL_HOME`, else `${XDG_DATA_HOME:-$HOME/.local/share}/luuna-install`.
#[cfg(unix)]
fn installed_terminal(
    home: &std::path::Path,
    install_home: Option<std::ffi::OsString>,
    data_home: Option<std::ffi::OsString>,
) -> Option<std::path::PathBuf> {
    let absolute = |value: std::ffi::OsString| {
        let path = std::path::PathBuf::from(value);
        path.is_absolute().then_some(path)
    };
    let root = install_home.and_then(absolute).unwrap_or_else(|| {
        data_home
            .and_then(absolute)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("luuna-install")
    });
    let current = std::fs::read_to_string(root.join("current")).ok()?;
    let version = current.trim();
    // The installer writes a bare version; anything that could leave `versions/` is refused.
    if version.is_empty()
        || version.len() > 64
        || !version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
        || version.starts_with('.')
    {
        return None;
    }
    let terminal = root.join("versions").join(version).join("bin/luuna-tui");
    terminal.is_file().then_some(terminal)
}

/// The fixed local start on macOS and Linux (ADR 0065). The installed release's
/// `luuna-tui desktop-connection` starts the runtime from the private settings
/// `lu init` saved. A machine without an installed release may still have the
/// older personal launcher at `~/.config/luuna/launch.py`. Either way the
/// connection arrives on an anonymous pipe: no credential in argv, a URL, web
/// storage, or a generated frontend asset. Nothing from the view chooses the helper.
#[cfg(unix)]
fn bootstrap() -> Result<LocalConnection, String> {
    use std::{
        io::Read,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let home = std::path::PathBuf::from(
        std::env::var_os("HOME").ok_or("Your home directory is unavailable.")?,
    );
    let legacy = home.join(".config/luuna/launch.py");
    let mut command = match installed_terminal(
        &home,
        std::env::var_os("LUUNA_INSTALL_HOME"),
        std::env::var_os("XDG_DATA_HOME"),
    ) {
        Some(terminal) => {
            let mut command = Command::new(terminal);
            command.arg("desktop-connection");
            command
        }
        None if cfg!(target_os = "macos") && legacy.is_file() => {
            let mut command = Command::new("/usr/bin/python3");
            command.arg(legacy).arg("connection");
            command
        }
        None => {
            return Err("Luuna is not installed for this user. Install a Luuna release and run lu init, or use the manual connection form.".to_owned())
        }
    };
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Could not start the local Luuna helper.".to_owned())?;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => {
                return Err(
                    "Local runtime startup failed. Run lu init, then lu doctor, in a terminal."
                        .to_owned(),
                );
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(
                    "Local runtime startup timed out. Run lu doctor in a terminal, then retry."
                        .to_owned(),
                );
            }
        }
    }
    let mut bytes = Vec::new();
    child
        .stdout
        .take()
        .ok_or("The local connection pipe is unavailable.")?
        .take(1024)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read the local connection.".to_owned())?;
    validate_connection(&bytes)
}

#[cfg(not(any(target_os = "windows", unix)))]
fn bootstrap() -> Result<LocalConnection, String> {
    Err("Automatic runtime startup is not available on this platform. Use the manual connection form.".to_owned())
}

#[tauri::command]
pub async fn ensure_local_runtime() -> Result<LocalConnection, String> {
    tauri::async_runtime::spawn_blocking(bootstrap)
        .await
        .map_err(|_| "The local startup helper could not finish.".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_remote_endpoints_and_malformed_tokens_without_echoing_secrets() {
        let token = "a".repeat(64);
        assert!(
            validate_connection(
                format!(r#"{{"url":"http://127.0.0.1:4242","token":"{token}"}}"#).as_bytes()
            )
            .is_ok()
        );
        for url in [
            "https://example.com",
            "http://localhost:4242",
            "http://127.0.0.1:4242/redirect",
        ] {
            let error =
                validate_connection(format!(r#"{{"url":"{url}","token":"{token}"}}"#).as_bytes())
                    .err()
                    .unwrap();
            assert!(!error.contains(&token));
        }
        assert!(
            validate_connection(br#"{"url":"http://127.0.0.1:4242","token":"short"}"#).is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn finds_the_installed_terminal_only_inside_the_installer_layout() {
        let dir = std::env::temp_dir().join(format!("luuna-desktop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("data/luuna-install");
        std::fs::create_dir_all(root.join("versions/0.1.0/bin")).unwrap();
        std::fs::write(root.join("versions/0.1.0/bin/luuna-tui"), "").unwrap();
        // No `current` file: nothing is installed.
        assert_eq!(
            installed_terminal(&dir, None, Some(dir.join("data").into())),
            None
        );
        std::fs::write(root.join("current"), "0.1.0\n").unwrap();
        let found = installed_terminal(&dir, None, Some(dir.join("data").into())).unwrap();
        assert_eq!(found, root.join("versions/0.1.0/bin/luuna-tui"));
        // LUUNA_INSTALL_HOME wins, and the default sits under ~/.local/share.
        assert_eq!(
            installed_terminal(&dir, Some(root.clone().into()), None),
            Some(found)
        );
        assert_eq!(installed_terminal(&dir, None, None), None);
        // A relative override is ignored rather than resolved against the working folder.
        assert_eq!(
            installed_terminal(&dir, Some("relative".into()), None),
            None
        );
        for bad in ["../../etc", ".hidden", "", "a/b"] {
            std::fs::write(root.join("current"), bad).unwrap();
            assert_eq!(
                installed_terminal(&dir, Some(root.clone().into()), None),
                None,
                "{bad}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
