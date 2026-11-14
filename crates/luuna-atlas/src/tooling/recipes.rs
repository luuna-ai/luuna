//! User-selected recipe extensions. Built-in setup remains entirely native.
use crate::{
    actions::record::{message, text, Action},
    json, records, Result,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
#[derive(Clone)]
pub(super) struct Provider {
    pub name: String,
    pub source: String,
    pub path: PathBuf,
    pub shadow: String,
}
const BUILTIN: [&str; 3] = ["claude", "codex", "cursor"];
fn command(a: &Action<'_>, program: &str) -> Option<Command> {
    let path = a.ctx.env.look_path(program)?;
    let mut c = Command::new(path);
    c.current_dir(&a.ctx.cwd)
        .env_clear()
        .envs(a.ctx.env.vars().iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Some(c)
}
fn npm(a: &Action<'_>, name: &str) -> Option<PathBuf> {
    let out = command(a, "bun")?
        .args(["-e", include_str!("resolver.js")])
        .env(
            "LUUNA_ATLAS_RECIPE_PACKAGE",
            format!("luuna-atlas-recipe-{name}"),
        )
        .output()
        .ok()?;
    if out.status.success() {
        let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if !text.is_empty() {
            return Some(PathBuf::from(text));
        }
    }
    None
}
pub(super) fn resolve(a: &Action<'_>, name: &str) -> Option<Provider> {
    for ext in ["ts", "sh"] {
        let path = a.ctx.cwd.join(format!(".luuna/atlas/recipes/{name}.{ext}"));
        if path.exists() {
            return Some(Provider {
                name: name.into(),
                source: format!("filesystem-{ext}"),
                path,
                shadow: String::new(),
            });
        }
    }
    if let Some(path) = npm(a, name) {
        return Some(Provider {
            name: name.into(),
            source: "npm".into(),
            path,
            shadow: String::new(),
        });
    }
    BUILTIN.contains(&name).then(|| Provider {
        name: name.into(),
        source: "builtin".into(),
        path: PathBuf::new(),
        shadow: String::new(),
    })
}
pub(super) fn listings(a: &Action<'_>) -> Vec<Provider> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(a.ctx.cwd.join(".luuna/atlas/recipes")) {
        let mut entries: Vec<_> = entries.filter_map(std::result::Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = Path::new(&name);
            if let Some(ext) = path
                .extension()
                .and_then(|e| e.to_str())
                .filter(|e| ["ts", "sh"].contains(e))
            {
                files.push(Provider {
                    name: path.file_stem().unwrap().to_string_lossy().into_owned(),
                    source: format!("filesystem-{ext}"),
                    path: e.path(),
                    shadow: String::new(),
                });
            }
        }
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    let mut out = Vec::new();
    for name in BUILTIN {
        let shadow = files
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.source.clone())
            .unwrap_or_else(|| {
                if npm(a, name).is_some() {
                    "npm".into()
                } else {
                    String::new()
                }
            });
        out.push(Provider {
            name: name.into(),
            source: "builtin".into(),
            path: PathBuf::new(),
            shadow,
        })
    }
    out.extend(files);
    let order = |s: &str| match s {
        "filesystem-ts" => 0,
        "filesystem-sh" => 1,
        "npm" => 2,
        _ => 3,
    };
    out.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| order(&a.source).cmp(&order(&b.source)))
    });
    out
}
pub(super) fn external(a: &mut Action<'_>, r: &Provider, action: &str) -> Result<(bool, String)> {
    if r.source == "filesystem-sh" {
        let mut c = Command::new(&r.path);
        #[cfg(windows)]
        {
            c = Command::new(
                a.ctx
                    .env
                    .look_path("sh")
                    .unwrap_or_else(|| PathBuf::from("sh")),
            );
            c.arg(&r.path);
        }
        let out = c
            .arg(action)
            .current_dir(&a.ctx.cwd)
            .env_clear()
            .envs(a.ctx.env.vars().iter().map(|(k, v)| (k, v)))
            .env("LUUNA_ATLAS_RECIPE_NAME", &r.name)
            .env("LUUNA_ATLAS_RECIPE_ACTION", action)
            .stdin(Stdio::null())
            .output();
        let (ok, mut text, err, code) = match out {
            Ok(out) => (
                out.status.success(),
                String::from_utf8_lossy(&out.stdout).trim().to_owned(),
                String::from_utf8_lossy(&out.stderr).trim().to_owned(),
                out.status.code().unwrap_or(-1),
            ),
            Err(_) => (false, String::new(), String::new(), 1),
        };
        if text.is_empty() {
            text = if ok {
                format!("{} {action} succeeded", r.name)
            } else {
                format!("{} {action} exited with code {code}", r.name)
            };
            if !ok && !err.is_empty() {
                text += &format!(": {err}")
            }
        }
        return Ok((ok, text));
    }
    let Some(mut c) = command(a, "bun") else {
        return Err(message("exec: \"bun\": executable file not found in $PATH"));
    };
    let out = c
        .args(["-e", include_str!("recipe_runner.js")])
        .env("LUUNA_ATLAS_RECIPE_PATH", &r.path)
        .env("LUUNA_ATLAS_RECIPE_NAME", &r.name)
        .env("LUUNA_ATLAS_RECIPE_ACTION", action)
        .env("LUUNA_ATLAS_RECIPE_SOURCE", &r.source)
        .output()
        .map_err(|e| message(e.to_string()))?;
    a.output.stderr += &String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        return Err(message(format!(
            "exit status {}",
            out.status.code().unwrap_or(-1)
        )));
    }
    let raw = String::from_utf8_lossy(&out.stdout);
    let sentinel = "\x1eLUUNA_ATLAS_RECIPE_RESULT:";
    let Some(index) = raw.rfind(sentinel) else {
        return Err(message(format!("recipe \"{}\" returned no result", r.name)));
    };
    a.output.stdout += &raw[..index];
    let payload = json::parse(&raw[index + sentinel.len()..])?;
    let o = records::obj(Some(&payload));
    let resolve = text(o.get("resolveError"));
    if !resolve.is_empty() {
        return Err(message(resolve));
    }
    let threw = text(o.get("threw"));
    if !threw.is_empty() {
        return Ok((
            false,
            format!(
                "recipe \"{}\" {action} threw ({}): {threw}",
                r.name,
                r.path.display()
            ),
        ));
    }
    let result = records::obj(o.get("result"));
    Ok((
        crate::registry::go_bool(result.get("success")),
        text(result.get("message")),
    ))
}
