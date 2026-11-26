//! `luuna-tui --guide init` in a real PTY against a mock runtime: the binary opens straight into
//! the guide, the real flow asks its first questions, keys answer them, Esc then Enter leaves the
//! setup (nothing undone), and quitting restores the terminal.
mod support;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde_json::json;
use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use support::{json, serve, Request};

#[test]
fn the_guide_runs_in_a_real_pty_and_answers_with_keys() {
    let mock = serve(|request: &Request| match request.path.as_str() {
        "/api/v1/health" => json(json!({"status":"ok","pi_version":"1.3.0"})),
        _ => json(json!([])),
    });
    let cwd = tempfile::tempdir().unwrap();
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("Real PTY required for the guide audit");
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_luuna-tui"));
    command.args(["--guide", "init"]);
    command.cwd(cwd.path());
    command.env("LUUNA_API_TOKEN", "guide-fixture-private-token");
    command.env("LUUNA_URL", mock.url());
    command.env("LUUNA_CLI_NAME", "lu");
    // The defaults, never the developer's own settings file (ADR 0040).
    let settings = tempfile::tempdir().unwrap();
    command.env("LUUNA_TUI_CONFIG", settings.path().join("tui.json"));
    let mut child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);
    let captured = Arc::new(Mutex::new(Vec::new()));
    let output = captured.clone();
    let mut reader = pair.master.try_clone_reader().unwrap();
    let reader_thread = std::thread::spawn(move || {
        let mut bytes = [0; 4096];
        while let Ok(n) = reader.read(&mut bytes) {
            if n == 0 {
                break;
            }
            let mut output = output.lock().unwrap();
            if output.len() < 4 * 1024 * 1024 {
                output.extend_from_slice(&bytes[..n]);
            }
        }
    });
    let mut writer = pair.master.take_writer().unwrap();
    #[cfg(windows)]
    {
        writer.write_all(b"\x1b[1;1R").unwrap();
        writer.flush().unwrap();
    }
    let wait = |what: &dyn Fn(&str) -> bool, label: &str| {
        let start = Instant::now();
        loop {
            let text = String::from_utf8_lossy(&captured.lock().unwrap()).into_owned();
            if what(&text) {
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "{label} did not happen; recent output: {:?}",
                text.chars()
                    .rev()
                    .take(2500)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>()
            );
            std::thread::sleep(Duration::from_millis(30));
        }
    };
    let mut send = |bytes: &[u8]| {
        writer.write_all(bytes).unwrap();
        writer.flush().unwrap();
    };
    wait(
        &|text| text.contains("INIT") && text.contains("repository") && text.contains("folder"),
        "the first question",
    );
    // Outside a checkout the choices are: browse folders, or type a path.
    send(b"\x1b[B");
    send(b"\r");
    wait(
        &|text| text.contains("Repository") && text.contains("folder"),
        "the typed folder question",
    );
    let demo = tempfile::tempdir().unwrap();
    let root = demo.path().join("demo");
    std::fs::create_dir(&root).unwrap();
    let root = root.to_string_lossy().into_owned();
    send(root.as_bytes());
    send(b"\r");
    wait(
        &|text| text.contains("Project") && text.contains("name"),
        "the project name question",
    );
    assert!(
        mock.take().iter().any(|r| r.path == "/api/v1/projects"),
        "the guide looked for registered projects"
    );
    send(b"\x1b");
    wait(
        &|text| text.contains("Leave the setup?"),
        "the leave confirmation",
    );
    send(b"\r");
    wait(
        &|text| text.contains("Nothing was undone"),
        "the left ending",
    );
    send(b"q");
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if start.elapsed() > Duration::from_secs(8) {
            child.kill().unwrap();
            panic!("the guide did not exit after q");
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    drop(writer);
    drop(pair.master);
    reader_thread.join().unwrap();
    let text = String::from_utf8_lossy(&captured.lock().unwrap()).into_owned();
    assert!(!text.contains("guide-fixture-private-token"));
    assert!(
        text.contains("\x1b[?1049h") && text.contains("\x1b[?1049l"),
        "alternate screen entered and left"
    );
    assert!(text.contains("\x1b[?25h"), "cursor restored");
    assert!(
        !mock.take().iter().any(|r| r.method != "GET"),
        "leaving early sent no mutation"
    );
}
