use crate::{
    actions::record::{boolean, message, Action},
    assets,
    config::Config,
    json::Object,
    Result,
};
use std::{fs, path::Path};
pub const MARKER_START: &str = "<!-- luuna-atlas:start -->";
pub const MARKER_END: &str = "<!-- luuna-atlas:end -->";
const PAIRS: [(&str, &str); 2] = [
    (MARKER_START, MARKER_END),
    ("<!-- mulch:start -->", "<!-- mulch:end -->"),
];
pub fn has_marker_section(content: &str) -> bool {
    PAIRS.iter().any(|(start, _)| content.contains(start))
}
pub fn replace_marker_section(content: &str, section: &str) -> (String, bool) {
    for (a, b) in PAIRS {
        if let (Some(start), Some(end)) = (content.find(a), content.find(b)) {
            return (
                format!(
                    "{}{section}{}",
                    &content[..start],
                    &content[end + b.len()..]
                ),
                true,
            );
        }
    }
    (content.into(), false)
}
pub(super) fn collapse(content: &str) -> String {
    let mut out = String::new();
    let mut n = 0;
    for c in content.chars() {
        if c == '\n' {
            n += 1;
            if n > 2 {
                continue;
            }
        } else {
            n = 0
        }
        out.push(c)
    }
    out
}
pub fn remove_marker_section(content: &str) -> String {
    let (next, found) = replace_marker_section(content, "");
    if found {
        format!("{}\n", collapse(&next).trim())
    } else {
        content.into()
    }
}
pub fn wrap_in_markers(snippet: &str) -> String {
    format!("{MARKER_START}\n{snippet}{MARKER_END}")
}
pub(super) fn footer(key: &str, cfg: &Config) -> String {
    assets::get(key).replace(
        assets::get("close_conditional_embedded"),
        &crate::prime::format::session_end_reminder("embedded", cfg),
    )
}
pub(super) fn snippet(cfg: &Config) -> String {
    wrap_in_markers(&assets::get("onboard").replace(
        "__ATLAS_SESSION_CLOSE__",
        &crate::prime::format::session_end_reminder("embedded", cfg),
    ))
}
pub(super) fn inspect(path: &Path) -> String {
    super::text_bytes::encode(&fs::read(path).unwrap_or_default())
}
pub(super) fn read(path: &Path) -> Result<String> {
    match fs::read(path) {
        Ok(b) => Ok(super::text_bytes::encode(&b)),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) || path.is_dir() =>
        {
            Ok(String::new())
        }
        Err(e) => Err(message(format!(
            "Refusing to replace unreadable file {}: {e}",
            path.display()
        ))),
    }
}
pub(super) fn write(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| crate::AtlasError::PathOp {
            op: "mkdir",
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    crate::commands::write_file(path, &super::text_bytes::decode(content))
}
const HEADER: &str = "## Project Expertise (Atlas)";
const TAIL:&str="luuna-atlas validate && git add .luuna/atlas/ && git commit -m \"luuna-atlas: record learnings\"";
pub(super) fn legacy(content: &str, section: &str) -> Result<String> {
    let Some(start) = content.find(HEADER) else {
        return Ok(content.into());
    };
    let Some(tail) = content[start..].find(TAIL) else {
        return Err(message("Refusing to replace unbounded legacy onboarding section: closing command/fence missing; unrelated instructions could be lost."));
    };
    let tail_end = start + tail + TAIL.len();
    let fence = String::from_utf8(vec![96; 3]).unwrap();
    let Some(end) = content[tail_end..].find(&fence) else {
        return Err(message("Refusing to replace unbounded legacy onboarding section: closing fence missing; unrelated instructions could be lost."));
    };
    let mut end = tail_end + end + 3;
    while content.as_bytes().get(end) == Some(&b'\n') {
        end += 1;
    }
    Ok(format!("{}{section}{}", &content[..start], &content[end..]))
}
pub(crate) fn run(a: &mut Action<'_>, flags: &[(String, String)]) -> Result<()> {
    let cfg = a.config().unwrap_or_default();
    let snippet = snippet(&cfg);
    if boolean(flags, "stdout") {
        a.output.stdout += &snippet;
        return Ok(());
    }
    let candidates = ["CLAUDE.md", ".claude/CLAUDE.md", "AGENTS.md"];
    let found: Vec<_> = candidates
        .iter()
        .filter(|n| {
            fs::read(a.ctx.cwd.join(n)).ok().is_some_and(|b| {
                let c = String::from_utf8_lossy(&b);
                has_marker_section(&c) || c.contains(HEADER)
            })
        })
        .copied()
        .collect();
    let name = found.first().copied().unwrap_or_else(|| {
        if a.ctx.cwd.join("CLAUDE.md").exists() {
            "CLAUDE.md"
        } else if a.ctx.cwd.join("AGENTS.md").exists() {
            "AGENTS.md"
        } else {
            "CLAUDE.md"
        }
    });
    let path = a.ctx.cwd.join(name);
    let exists = path.exists();
    let content = if boolean(flags, "check") {
        fs::read(&path)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    } else {
        read(&path)?
    };
    let current =
        has_marker_section(&content) && content.contains("<!-- atlas-onboard:v0.11.0 -->");
    let check = boolean(flags, "check");
    let mut next = content.clone();
    let action = if check {
        if !exists {
            "not_installed"
        } else if has_marker_section(&content) {
            if current {
                "up_to_date"
            } else {
                "outdated"
            }
        } else if content.contains(HEADER) {
            "legacy"
        } else {
            "not_installed"
        }
    } else if !exists {
        next = format!("{snippet}\n");
        "created"
    } else if has_marker_section(&content) {
        if current {
            "up_to_date"
        } else {
            next = replace_marker_section(&content, &snippet).0;
            "updated"
        }
    } else if content.contains(HEADER) {
        next = legacy(&content, &format!("{snippet}\n"))?;
        "migrated"
    } else {
        next = format!(
            "{}\n\n{snippet}\n",
            content.trim_end_matches([' ', '\r', '\n', '\t'])
        );
        "appended"
    };
    if !check && action != "up_to_date" {
        write(&path, &next)?
    }
    if a.ctx.globals.json {
        let mut o = Object::new();
        o.insert("file", name);
        o.insert("action", action);
        a.json("onboard", o)
    } else {
        let text=match (check,action){(true,"not_installed")=>format!("Atlas snippet is not installed in {name}."),(true,"up_to_date")=>format!("Atlas snippet in {name} is up to date."),(true,"outdated")=>format!("Atlas snippet in {name} is outdated. Run `luuna-atlas onboard` to update."),(true,"legacy")=>format!("Atlas snippet in {name} uses legacy format (no markers). Run `luuna-atlas onboard` to migrate."),(false,"created")=>format!("Atlas onboarding snippet written to {name}."),(false,"appended")=>format!("Atlas onboarding snippet appended to {name}."),(false,"updated")=>format!("Atlas onboarding snippet updated in {name}."),(false,"migrated")=>format!("Atlas onboarding snippet migrated to marker format in {name}."),_=>format!("Atlas snippet in {name} is already up to date. No changes made.")};
        let color = if (check && action != "up_to_date") || (!check && action == "up_to_date") {
            "yellow"
        } else {
            "green"
        };
        a.human(a.styled(color, &text));
        if found.len() > 1 {
            a.human(a.styled(
                "yellow",
                &format!(
                    "Warning: luuna-atlas snippet also found in: {}",
                    found[1..].join(", ")
                ),
            ));
        }
    }
    Ok(())
}
