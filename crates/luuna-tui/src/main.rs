//! `luuna-tui` (installed as `lu` and `luuna`): the Luuna terminal client. With a command it runs
//! one cataloged action (crate::cli); without one on a terminal, or for `init`, it opens the
//! interactive workbench, whose actions run in process (crate::bridge).
use luuna_tui::{
    cli::{self, Next, Process},
    protocol::safe_text,
};
use std::io::{self, IsTerminal};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // Component aliases run before private runtime settings are loaded. Generated
    // hooks need only this executable and must never inherit saved credentials.
    let executable = std::env::args().next().unwrap_or_default();
    let basename = std::path::Path::new(&executable)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let alias = luuna_tui::components::component_alias(basename);
    let named = argv
        .first()
        .and_then(|s| luuna_tui::components::component_alias(s));
    if let Some(name) = alias.or(named) {
        let args = if alias.is_some() {
            &argv[..]
        } else {
            &argv[1..]
        };
        let code = luuna_tui::components::dispatch(name, args).await;
        return exit_code(code);
    }
    // The workbench's settings file (keys, colors): print every default, or where it is read.
    match argv.first().map(String::as_str) {
        Some("--print-config") => {
            print!("{}", luuna_tui::config::defaults_json());
            return std::process::ExitCode::SUCCESS;
        }
        Some("--config-path") => {
            match luuna_tui::config::path() {
                Some(path) => println!("{}", path.display()),
                None => println!("(no home directory; set LUUNA_TUI_CONFIG)"),
            }
            return std::process::ExitCode::SUCCESS;
        }
        _ => {}
    }
    // The desktop app's local start (ADR 0065): one JSON line on stdout, errors on stderr.
    if argv.first().map(String::as_str) == Some("desktop-connection") && argv.len() == 1 {
        return match luuna_tui::setup::desktop_connection().await {
            Ok(line) => {
                println!("{line}");
                std::process::ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("Luuna desktop connection: {error}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    if argv.first().map(String::as_str) == Some("config") {
        let result = (|| {
            let path = luuna_tui::config::path()
                .ok_or_else(|| io::Error::other("Set HOME or LUUNA_TUI_CONFIG"))?;
            if argv.len() == 1 {
                println!("Settings: {}\nChange a setting: lu config set <name> <JSON value>\nExample: lu config set keyboard_enhancement false", path.display());
                return Ok(());
            }
            if argv.len() != 4 || argv[1] != "set" {
                return Err(io::Error::other("Usage: lu config set <name> <JSON value>"));
            }
            let value = serde_json::from_str(&argv[3]).map_err(io::Error::other)?;
            luuna_tui::config::set(&path, &argv[2], value)?;
            println!(
                "Settings saved to {}. They load when the workbench opens.",
                path.display()
            );
            Ok::<_, io::Error>(())
        })();
        return match result {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Luuna settings: {error}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    // Load saved private settings by relaunching, without changing environment
    // variables from a multi-threaded async process. Explicit overrides win.
    if std::env::var_os("LUUNA_PRIVATE_SETTINGS_LOADED").is_none() {
        let init = argv.first().map(String::as_str) == Some("init")
            && io::stdin().is_terminal()
            && io::stdout().is_terminal();
        let prepared = async {
            let mut vars = match luuna_tui::setup::directory() {
                Ok(directory) => luuna_tui::setup::load(&directory.join("environment.json"))?,
                Err(error) if init => return Err(error),
                Err(_) => std::collections::BTreeMap::new(),
            };
            if init {
                luuna_tui::setup::prepare(&luuna_tui::components::install_root(), &mut vars)
                    .await?;
                luuna_tui::setup::ensure_runtime(&vars).await?;
            }
            if init || !vars.is_empty() {
                let mut command = tokio::process::Command::new(std::env::current_exe()?);
                command
                    .args(&argv)
                    .env("LUUNA_PRIVATE_SETTINGS_LOADED", "1");
                for (name, value) in vars {
                    if std::env::var_os(&name).is_none() {
                        command.env(name, value);
                    }
                }
                return command.status().await.map(Some);
            }
            Ok::<_, io::Error>(None)
        }
        .await;
        match prepared {
            Ok(Some(status)) => {
                let code = status.code().unwrap_or(1);
                return exit_code(code);
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("Luuna setup: {error}");
                return std::process::ExitCode::FAILURE;
            }
        }
    }
    // `--guide init [--guide-flag key=value]...` opens the guide directly (the tests' entry).
    let next = if argv.first().map(String::as_str) == Some("--guide") {
        match guide_arguments(&argv) {
            Ok(flags) => Next::Workbench { guide: Some(flags) },
            Err(message) => {
                eprintln!("Luuna terminal: {message}");
                return std::process::ExitCode::from(2);
            }
        }
    } else {
        let mut process = Process::real();
        cli::main(argv, &mut process).await
    };
    match next {
        Next::Exit(code) => exit_code(code),
        Next::Workbench { guide } => match luuna_tui::workbench::run(guide).await {
            Ok(code) => std::process::ExitCode::from(code),
            Err(error) => {
                eprintln!("Luuna terminal: {}", safe_text(&error.to_string()));
                std::process::ExitCode::FAILURE
            }
        },
    }
}

fn guide_arguments(argv: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut args = argv.iter().skip(1);
    if args.next().map(String::as_str) != Some("init") {
        return Err("The only guide is init.".into());
    }
    let mut flags = Vec::new();
    while let Some(arg) = args.next() {
        if arg != "--guide-flag" {
            return Err(format!("Unknown terminal option {arg}."));
        }
        let spec = args.next().ok_or("--guide-flag needs key=value")?;
        match spec.split_once('=') {
            Some((key @ ("path" | "project"), value)) if !value.is_empty() => {
                flags.push((key.to_string(), value.to_string()))
            }
            _ => return Err("--guide-flag takes path=<dir> or project=<id>".into()),
        }
    }
    Ok(flags)
}

/// The process exit code; codes beyond a byte (a sandboxed command's, on Windows) leave unchanged,
/// as Node's did.
fn exit_code(code: i32) -> std::process::ExitCode {
    match u8::try_from(code) {
        Ok(code) => std::process::ExitCode::from(code),
        Err(_) => std::process::exit(code),
    }
}
