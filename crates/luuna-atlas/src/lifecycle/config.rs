//! Config paths, schema defaults, mutations and exact plain-error output.
use super::contract::config_schema;
use crate::{
    actions::record::{get, has, message, text, Action},
    config,
    json::{Object, Value},
    records, schema, store, yaml, CommandOutput, Result,
};
fn segments(path: &str) -> Vec<String> {
    path.split('.')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
fn descend(parts: &[String]) -> Option<&'static Object> {
    let mut node = config_schema();
    for part in parts {
        let props = records::obj(node.get("properties"));
        if let Some(next) = props.get(part) {
            node = records::obj(Some(next));
        } else if let Some(Value::Object(next)) = node.get("additionalProperties") {
            node = next;
        } else {
            return None;
        }
    }
    Some(node)
}
fn defaults(node: &Object) -> Option<Value> {
    if let Some(value) = node.get("default") {
        return Some(value.clone());
    }
    let mut values = Object::new();
    for (key, node) in records::obj(node.get("properties")).iter() {
        if let Some(value) = defaults(records::obj(Some(node))) {
            values.insert_unordered(key, value);
        }
    }
    if values.is_empty() {
        None
    } else {
        Some(Value::Object(values))
    }
}
fn walk<'a>(cfg: &'a Object, parts: &[String]) -> Option<&'a Value> {
    let mut current = cfg;
    for (i, part) in parts.iter().enumerate() {
        let value = current.get(part)?;
        if i + 1 == parts.len() {
            return Some(value);
        }
        current = value.as_object()?;
    }
    None
}
fn validate_path(parts: &[String]) -> Result<()> {
    let mut node = config_schema();
    for (n, part) in parts.iter().enumerate() {
        let props = records::obj(node.get("properties"));
        if let Some(next) = props.get(part) {
            node = records::obj(Some(next));
            continue;
        }
        if let Some(Value::Object(next)) = node.get("additionalProperties") {
            node = next;
            continue;
        }
        let known = props.keys().collect::<Vec<_>>().join(", ");
        let known = if known.is_empty() { "(none)" } else { &known };
        let parent = if n == 0 {
            "<root>".into()
        } else {
            parts[..n].join(".")
        };
        return Err(message(format!(
            "Unknown config path: '{}' is not a known knob. Known keys at '{parent}': {known}.",
            parts[..n + 1].join(".")
        )));
    }
    Ok(())
}
fn emit(a: &mut Action<'_>, value: &Value) {
    let out = if let Value::Object(object) = value {
        CommandOutput::emit(object)
    } else {
        CommandOutput::json(value)
    };
    a.output.stdout.push_str(&out.stdout);
}
fn mutate(cfg: &mut Object, parts: &[String], value: Option<Value>) -> bool {
    let unset = value.is_none();
    let mut current = std::mem::take(cfg);
    let mut ancestors = Vec::new();
    for part in &parts[..parts.len() - 1] {
        let mut child = match current.remove(part) {
            Some(child) => child,
            None if unset => return false,
            None => Value::Object(Object::new()),
        };
        let child = match child.as_object_mut() {
            Some(object) => std::mem::take(object),
            None if unset => return false,
            None => Object::new(),
        };
        ancestors.push((current, part.clone()));
        current = child;
    }
    let leaf = parts.last().expect("nonempty validated config path");
    if let Some(value) = value {
        current.insert_unordered(leaf, value);
    } else if current.remove(leaf).is_none() {
        return false;
    }
    while let Some((mut parent, key)) = ancestors.pop() {
        let n = ancestors.len() + 1;
        let required = descend(&parts[..n - 1])
            .map(|node| records::list(node.get("required")))
            .unwrap_or_default()
            .contains(&key);
        if !unset || !current.is_empty() || required {
            parent.insert_unordered(key, Value::Object(current));
        }
        current = parent;
    }
    *cfg = current;
    true
}
fn execute(a: &mut Action<'_>, args: &[String], flags: &[(String, String)]) -> Result<()> {
    match args.first().map(String::as_str).unwrap_or("show") {
        "schema" => {
            emit(a, &Value::Object(config_schema().clone()));
            Ok(())
        }
        "show" => {
            let cfg = a.config()?;
            if !has(flags, "path") {
                emit(a, &Value::Object(cfg));
                return Ok(());
            }
            let path = get(flags, "path");
            let parts = segments(&path);
            if parts.is_empty() {
                return Err(message("--path must not be empty."));
            }
            if let Some(value) = walk(&cfg, &parts) {
                emit(a, value);
                return Ok(());
            }
            if let Some(value) = descend(&parts).and_then(defaults) {
                emit(a, &value);
                return Ok(());
            }
            Err(message(format!(
                "Path '{path}' not found in config and has no schema default."
            )))
        }
        action @ ("set" | "unset") => {
            let path = args.get(1).map(String::as_str).unwrap_or_default();
            let parts = segments(path);
            if parts.is_empty() {
                return Err(message("<path> must not be empty."));
            }
            let file = a.ctx.root.join(".luuna/atlas/config.yaml");
            if !file.exists() {
                let tick = char::from(96);
                return Err(message(format!("No .luuna/atlas/ directory found. Run {tick}luuna-atlas init{tick} to set up this project before {tick}luuna-atlas config {action}{tick}.")));
            }
            validate_path(&parts)?;
            let value = if action == "unset" {
                None
            } else {
                let raw = args
                    .get(2)
                    .ok_or_else(|| message("Missing config value."))?;
                Some(
                    yaml::parse(raw)
                        .map_err(|e| message(format!("Invalid YAML for <value>: {e}")))?,
                )
            };
            store::with_file_lock(&file, || {
                let mut cfg = a.config()?;
                if !mutate(&mut cfg, &parts, value) {
                    return Ok(());
                }
                let issues =
                    schema::validate_json_schema(&Value::Object(cfg.clone()), config_schema())?;
                if !issues.is_empty() {
                    let mut lines = Vec::new();
                    for issue in issues {
                        let path = if issue.path.is_empty() {
                            "<root>"
                        } else {
                            &issue.path
                        };
                        let segments = issue
                            .path
                            .trim_start_matches('/')
                            .split('/')
                            .map(str::to_owned)
                            .collect::<Vec<_>>();
                        let title = descend(&segments)
                            .map(|node| text(node.get("title")))
                            .unwrap_or_default();
                        let tail = if title.is_empty() {
                            String::new()
                        } else {
                            format!(" ({title})")
                        };
                        lines.push(format!("  - {path}: {}{tail}", issue.message));
                    }
                    return Err(message(format!(
                        "Invalid config after {action}:\n{}",
                        lines.join("\n")
                    )));
                }
                config::check_lossless_rewrite(
                    &a.ctx.root.join(".luuna/atlas"),
                    &cfg,
                    std::slice::from_ref(&parts),
                )?;
                let data = yaml::to_yaml_string(&Value::Object(cfg))?;
                config::write_atomic(&file, data.as_bytes())
            })
        }
        other => Err(message(format!("Unknown config operation: {other}"))),
    }
}
pub(crate) fn run(a: &mut Action<'_>, args: &[String], flags: &[(String, String)]) {
    if let Err(error) = execute(a, args, flags) {
        a.output.exit_code = 1;
        a.output.stderr.push_str(&format!("{error}\n"));
    }
}
