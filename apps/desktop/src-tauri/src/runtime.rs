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

#[cfg(target_os = "macos")]
fn bootstrap() -> Result<LocalConnection, String> {
    use std::{
        io::Read,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    // Fixed local helper and an anonymous pipe: no credential in argv, a URL,
    // web storage, or a generated frontend asset. The helper owns service setup.
    let home = std::env::var_os("HOME").ok_or("Your home directory is unavailable.")?;
    let helper = std::path::PathBuf::from(home).join(".config/luuna/launch.py");
    if !helper.is_file() {
        return Err("The local Luuna launcher is missing. Configure just lu, or use the manual connection form.".to_owned());
    }
    let mut child = Command::new("/usr/bin/python3")
        .arg(helper)
        .arg("connection")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Could not start the local Luuna helper.".to_owned())?;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return Err(
                "Local runtime startup failed. Run just lu-check or just lu-restart in a terminal."
                    .to_owned(),
            ),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(
                    "Local runtime startup timed out. Check just lu-log, then retry.".to_owned(),
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

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn bootstrap() -> Result<LocalConnection, String> {
    Err("Automatic runtime startup is currently available on Windows. Use the manual connection form.".to_owned())
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
}
