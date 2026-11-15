//! Bounded in-process Atlas. Managed store roots never depend on a source checkout.
use super::{
    adopt_legacy_layout, check_project_data_links, prepare_workspace, ComponentError, Result,
    MAX_STDOUT,
};
use luuna_atlas::{
    config, git,
    json::Value as AtlasValue,
    managed::{self, YamlLimits},
    options::{Context, Env, GlobalOptions},
    tooling, CommandOutput,
};
use serde_json::Value;
use std::{collections::BTreeMap, fs, io::Read, path::Path, sync::Mutex};
static NATIVE_GATE: Mutex<()> = Mutex::new(());
const MAX_INPUT: usize = 64 * 1024 * 1024;
const MAX_ARGUMENTS: usize = 256 * 1024;
fn arguments(args: &[String], stdin: &[u8]) -> Result<()> {
    if args.is_empty()
        || args.len() > 128
        || stdin.len() > MAX_STDOUT
        || args
            .iter()
            .any(|a| a.len() > MAX_ARGUMENTS || a.contains('\0'))
        || args.iter().map(String::len).sum::<usize>() > MAX_ARGUMENTS
    {
        return Err(ComponentError::Protocol);
    }
    Ok(())
}
fn outcome(out: CommandOutput) -> Result<CommandOutput> {
    if out.stdout.len() > MAX_STDOUT || out.stderr.len() > MAX_STDOUT {
        return Err(ComponentError::OutputLimit);
    }
    Ok(out)
}
fn environment(mut env: BTreeMap<String, String>, args: &[String]) -> Env {
    super::retain_component_env(&mut env);
    Env::fixed(env, args.to_vec(), false).with_managed_children()
}
/// Full terminal command result, with explicit environment and stdin. Admission is
/// owned by the blocking job so dropping an awaiting future never admits a second writer.
pub async fn run_atlas_native(
    cwd: &Path,
    args: &[String],
    env: BTreeMap<String, String>,
    stdin: Vec<u8>,
) -> Result<CommandOutput> {
    arguments(args, &stdin)?;
    // The private raw IPC API accepts caller-selected file paths and bypasses
    // terminal command admission. It is available only through the raw library.
    if env.get("LUUNA_ATLAS_API_BRIDGE").is_some_and(|v| v == "1") {
        return Err(ComponentError::Protocol);
    }
    let cwd = cwd.to_path_buf();
    let args = args.to_vec();
    tokio::task::spawn_blocking(move || {
        let _guard = NATIVE_GATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        managed::with_yaml_limits(YamlLimits::default(), || {
            let cwd = fs::canonicalize(cwd).map_err(|_| ComponentError::Workspace)?;
            let env = environment(env, &args);
            let root = git::resolve_store_root(&cwd, &env);
            let ctx = Context {
                root,
                cwd,
                env,
                globals: GlobalOptions::default(),
            };
            outcome(tooling::run_in_store_checked(
                &ctx,
                &args,
                &stdin,
                &|ctx, _| {
                    preflight(&ctx.root, false).map_err(|e| {
                        luuna_atlas::AtlasError::Message(format!(
                            "Refusing managed Atlas input: {e}"
                        ))
                    })
                },
            ))
        })
    })
    .await
    .map_err(|_| ComponentError::Refused)?
}
/// Finite managed-runtime protocol. Configured executable hooks remain unauthorized.
pub async fn call_atlas_native(workspace: &Path, args: &[String]) -> Result<Value> {
    arguments(args, &[])?;
    if !matches!(
        args[0].as_str(),
        "init" | "add" | "record" | "edit" | "delete" | "query" | "search" | "show" | "config"
    ) {
        return Err(ComponentError::Protocol);
    }
    if args[0] == "config" && args.get(1).map(String::as_str) != Some("show") {
        return Err(ComponentError::Protocol);
    }
    let root = workspace.to_path_buf();
    let mut args = args.to_vec();
    if !args.iter().any(|s| s == "--json") {
        args.push("--json".into())
    }
    if !args.iter().any(|s| s == "--quiet" || s == "-q") {
        args.push("--quiet".into())
    }
    tokio::task::spawn_blocking(move || {
        let _guard = NATIVE_GATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        managed::with_yaml_limits(YamlLimits::default(), || {
            let root = prepare_workspace(&root)?;
            adopt_legacy_layout(&root)?;
            check_project_data_links(&root, ".luuna/atlas")?;
            preflight(&root, true)?;
            if args[0] != "init" {
                let cfg = config::read_config(&root.join(".luuna/atlas"))
                    .map_err(|_| ComponentError::Refused)?;
                if cfg.get("hooks").is_some_and(nonempty_hooks) {
                    return Err(ComponentError::Hooks);
                }
            }
            let env = Env::fixed(
                [
                    ("NO_COLOR", "1"),
                    ("FORCE_COLOR", "0"),
                    ("LUUNA_ATLAS_LOG_LEVEL", "silent"),
                ],
                args.clone(),
                false,
            );
            let ctx = Context {
                root: root.clone(),
                cwd: root,
                env,
                globals: GlobalOptions::default(),
            };
            let out = outcome(tooling::run_in_store(&ctx, &args, &[]))?;
            if out.exit_code != 0 {
                return Err(ComponentError::Refused);
            }
            let value: Value =
                serde_json::from_str(&out.stdout).map_err(|_| ComponentError::Protocol)?;
            if value.get("success").and_then(Value::as_bool) != Some(true) {
                return Err(ComponentError::Protocol);
            }
            Ok(value)
        })
    })
    .await
    .map_err(|_| ComponentError::Refused)?
}
fn nonempty_hooks(value: &AtlasValue) -> bool {
    let mut stack = vec![value];
    while let Some(v) = stack.pop() {
        match v {
            AtlasValue::Null => (),
            AtlasValue::Object(o) => stack.extend(o.iter().map(|(_, v)| v)),
            AtlasValue::Array(a) => {
                if !a.is_empty() {
                    return true;
                }
            }
            _ => return true,
        }
    }
    false
}
fn preflight(root: &Path, strict: bool) -> Result<()> {
    let dir = root.join(".luuna/atlas");
    let mut paths = vec![dir.join("config.yaml"), root.join(".gitattributes")];
    for name in ["expertise", "archive"] {
        if let Ok(entries) = fs::read_dir(dir.join(name)) {
            let mut entries: Vec<_> = entries
                .filter_map(std::result::Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
                .collect();
            entries.sort();
            paths.extend(entries)
        }
    }
    let mut remaining = MAX_INPUT;
    let mut records = 0usize;
    let mut total_nodes = 0usize;
    for path in paths {
        let metadata = match if strict {
            fs::symlink_metadata(&path)
        } else {
            fs::metadata(&path)
        } {
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
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        if strict {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(&path).map_err(|_| ComponentError::Workspace)?;
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
        if path.file_name().is_some_and(|n| n == "config.yaml") {
            let text = std::str::from_utf8(&bytes).map_err(|_| ComponentError::Protocol)?;
            if let Err(error) = luuna_atlas::yaml::parse(text) {
                if error.to_string().contains("managed YAML") {
                    return Err(ComponentError::OutputLimit);
                }
                if strict {
                    return Err(ComponentError::Protocol);
                }
            }
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            for raw in bytes.split(|b| *b == b'\n') {
                let view = String::from_utf8_lossy(raw);
                let trimmed = view.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                records += 1;
                if records > 100_000 || raw.len() > MAX_STDOUT {
                    return Err(ComponentError::OutputLimit);
                }
                let left = view.len() - view.trim_start().len();
                let right = view.len() - view.trim_end().len();
                let raw = &raw[left..raw.len() - right];
                if strict {
                    let parsed: Value =
                        serde_json::from_slice(raw).map_err(|_| ComponentError::Protocol)?;
                    total_nodes = total_nodes.saturating_add(json_budget(&parsed)?);
                } else if let Ok(parsed) = luuna_atlas::json::parse_bytes(raw) {
                    managed::with_yaml_limits(
                        YamlLimits {
                            max_text_bytes: MAX_STDOUT / 2,
                            ..YamlLimits::default()
                        },
                        || managed::check_value(&parsed),
                    )
                    .map_err(|_| ComponentError::OutputLimit)?;
                    total_nodes = total_nodes.saturating_add(atlas_nodes(&parsed));
                }
                if total_nodes > 100_000 {
                    return Err(ComponentError::OutputLimit);
                }
            }
        }
    }
    Ok(())
}
pub(super) fn json_budget(value: &Value) -> Result<usize> {
    let mut stack = vec![(value, 0usize)];
    let mut cost = 0usize;
    let mut nodes = 0usize;
    while let Some((v, depth)) = stack.pop() {
        nodes += 1;
        let bytes = match v {
            Value::String(s) => s.len(),
            Value::Array(a) => {
                stack.extend(a.iter().map(|v| (v, depth + 1)));
                0
            }
            Value::Object(o) => {
                for (k, v) in o {
                    cost = cost.saturating_add(k.len().saturating_mul(6) + depth * 4 + 24);
                    stack.push((v, depth + 1));
                }
                0
            }
            _ => 32,
        };
        cost = cost.saturating_add(bytes.saturating_mul(6) + depth * 4 + 24);
        if depth > 96 || cost > MAX_STDOUT / 2 || nodes > 100_000 {
            return Err(ComponentError::OutputLimit);
        }
    }
    Ok(nodes)
}
fn atlas_nodes(value: &AtlasValue) -> usize {
    let mut count = 0usize;
    let mut stack = vec![value];
    while let Some(v) = stack.pop() {
        count = count.saturating_add(1);
        match v {
            AtlasValue::Object(o) => stack.extend(o.iter().map(|(_, v)| v)),
            AtlasValue::Array(a) => stack.extend(a),
            _ => (),
        }
    }
    count
}
