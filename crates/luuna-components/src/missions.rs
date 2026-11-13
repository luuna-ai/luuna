//! Native Missions execution. No component executable or fallback is used.
use super::{
    adopt_legacy_layout, check_project_data_links, prepare_workspace, ComponentError, Result,
    MAX_STDOUT,
};
use luuna_missions::command::{Invocation, Outcome};
use serde_json::Value;
use std::{collections::BTreeMap, fs, io::Read, path::Path, sync::Mutex};

static NATIVE_GATE: Mutex<()> = Mutex::new(());
const MAX_INPUT: usize = 64 * 1024 * 1024;

fn arguments(args: &[String], stdin: &[u8]) -> Result<()> {
    if args.is_empty()
        || args.len() > 128
        || stdin.len() > MAX_STDOUT
        || args
            .iter()
            .any(|a| a.len() > 256 * 1024 || a.contains('\0'))
        || args.iter().map(String::len).sum::<usize>() > MAX_STDOUT
    {
        return Err(ComponentError::Protocol);
    }
    Ok(())
}

/// Full native command contract for the terminal runner. Environment and stdin
/// are explicit; the library never changes the process working directory.
/// The job owns its gate until completion, even if its caller disconnects.
pub async fn run_missions_native(
    cwd: &Path,
    args: &[String],
    env: BTreeMap<String, String>,
    stdin: Vec<u8>,
) -> Result<Outcome> {
    arguments(args, &stdin)?;
    let invocation = Invocation::new(cwd, args.iter().cloned())
        .with_env(env)
        .with_stdin(stdin);
    tokio::task::spawn_blocking(move || {
        let _guard = NATIVE_GATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let outcome =
            luuna_missions::command::run(&invocation).map_err(ComponentError::Missions)?;
        if outcome.stdout.len() > MAX_STDOUT || outcome.stderr.len() > MAX_STDOUT {
            return Err(ComponentError::OutputLimit);
        }
        Ok(outcome)
    })
    .await
    .map_err(|_| ComponentError::Refused)?
}

/// Finite managed-runtime protocol; raw command semantics remain in Missions.
pub async fn call_missions_native(workspace: &Path, args: &[String]) -> Result<Value> {
    arguments(args, &[])?;
    if !matches!(
        args[0].as_str(),
        "init" | "create" | "show" | "list" | "search" | "update" | "block"
    ) {
        return Err(ComponentError::Protocol);
    }
    let root = workspace.to_owned();
    let mut args = args.to_vec();
    if !args.iter().any(|a| a == "--json") {
        args.push("--json".into());
    }
    tokio::task::spawn_blocking(move || {
        let _guard = NATIVE_GATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let root = prepare_workspace(&root)?;
        adopt_legacy_layout(&root)?;
        let initializing = args[0] == "init";
        if !initializing && !root.join(".luuna/missions/config.yaml").is_file() {
            return Err(ComponentError::Workspace);
        }
        check_project_data_links(&root, ".luuna/missions")?;
        preflight(&root, initializing)?;
        let invocation = Invocation::new(&root, args).with_store_dir(root.join(".luuna/missions"));
        let outcome =
            luuna_missions::command::run(&invocation).map_err(|_| ComponentError::Refused)?;
        if outcome.stdout.len() > MAX_STDOUT || outcome.stderr.len() > MAX_STDOUT {
            return Err(ComponentError::OutputLimit);
        }
        if outcome.exit_code != 0 {
            return Err(ComponentError::Refused);
        }
        let value: Value =
            serde_json::from_slice(&outcome.stdout).map_err(|_| ComponentError::Protocol)?;
        if value.get("success").and_then(Value::as_bool) != Some(true) {
            return Err(ComponentError::Protocol);
        }
        Ok(value)
    })
    .await
    .map_err(|_| ComponentError::Refused)?
}

fn preflight(root: &Path, initializing: bool) -> Result<()> {
    let mut remaining = MAX_INPUT;
    let mut records = 0usize;
    for name in [
        "config.yaml",
        "issues.jsonl",
        "templates.jsonl",
        "plans.jsonl",
        ".gitignore",
        ".gitattributes",
    ] {
        if name == ".gitattributes" && !initializing {
            continue;
        }
        let path = if name == ".gitattributes" {
            root.join(name)
        } else {
            root.join(".luuna/missions").join(name)
        };
        let metadata = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(ComponentError::Workspace),
        };
        if !metadata.is_file() {
            return Err(ComponentError::Workspace);
        }
        if metadata.len() > remaining as u64 {
            return Err(ComponentError::OutputLimit);
        }
        let mut file = fs::File::open(&path).map_err(|_| ComponentError::Workspace)?;
        if !file
            .metadata()
            .map_err(|_| ComponentError::Workspace)?
            .is_file()
        {
            return Err(ComponentError::Workspace);
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take(remaining as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ComponentError::Workspace)?;
        if bytes.len() > remaining {
            return Err(ComponentError::OutputLimit);
        }
        remaining -= bytes.len();
        if name.ends_with(".jsonl") {
            for line in bytes.split(|b| *b == b'\n') {
                if std::str::from_utf8(line).is_ok_and(|s| s.trim().is_empty()) {
                    continue;
                }
                records += 1;
                if records > 100_000 || line.len() > MAX_STDOUT {
                    return Err(ComponentError::OutputLimit);
                }
                serde_json::from_slice::<Value>(line).map_err(|_| ComponentError::Protocol)?;
            }
        }
    }
    Ok(())
}
