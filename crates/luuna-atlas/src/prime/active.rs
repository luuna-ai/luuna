//! Active-work signals with handwritten ASCII RE2 matchers and explicit children.
use crate::{
    actions::record::text,
    evidence, git,
    json::{self, Object},
    options::Context,
    records,
};
use std::{
    collections::BTreeMap,
    path::Path,
    process::{Command, Stdio},
};
#[derive(Clone, Default)]
pub struct ActiveWorkResult {
    pub missions: String,
    pub gh: String,
    pub linear: String,
    pub bead: String,
    pub warnings: Vec<String>,
}
#[derive(Clone, Default)]
pub struct ActiveContext {
    pub changed_files: Vec<String>,
    pub trackers: BTreeMap<String, String>,
}
pub fn has_signal(ctx: &ActiveContext) -> bool {
    !ctx.changed_files.is_empty() || ctx.trackers.values().any(|v| !v.is_empty())
}
pub fn filter_by_active_context(rows: &[Object], ctx: &ActiveContext) -> Vec<Object> {
    rows.iter()
        .filter(|r| {
            let ev = records::obj(r.get("evidence"));
            let hit = ["missions", "gh", "linear", "bead"]
                .into_iter()
                .any(|tracker| {
                    let value = if tracker == "missions" {
                        evidence::evidence_missions(r.get("evidence"))
                    } else {
                        text(ev.get(tracker))
                    };
                    ctx.trackers
                        .get(tracker)
                        .is_some_and(|active| !active.is_empty() && *active == value)
                });
            hit || !git::has_file_anchors(r) || git::matches_file_anchors(r, &ctx.changed_files)
        })
        .cloned()
        .collect()
}
fn word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
fn boundary(bytes: &[u8], at: usize) -> bool {
    (at > 0 && word(bytes[at - 1])) != (at < bytes.len() && word(bytes[at]))
}
fn numeric_end(bytes: &[u8], mut at: usize) -> Option<usize> {
    let first = at;
    while bytes.get(at).is_some_and(u8::is_ascii_digit) {
        at += 1;
    }
    if at > first && boundary(bytes, at) {
        Some(at)
    } else {
        None
    }
}
/// Captures the exact matches of the four Go branch tracker patterns.
pub fn tracker_matches(branch: &str, tracker: &str) -> Vec<String> {
    let bytes = branch.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let found = match tracker {
            "missions" if boundary(bytes, at) && bytes[at].is_ascii_lowercase() => {
                let mut end = at + 1;
                while bytes
                    .get(end)
                    .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                {
                    end += 1;
                }
                if bytes.get(end) != Some(&b'-') {
                    None
                } else {
                    let first = end + 1;
                    end = first;
                    while end - first < 8
                        && bytes
                            .get(end)
                            .is_some_and(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
                    {
                        end += 1;
                    }
                    if end - first >= 4 && boundary(bytes, end) {
                        Some((at, end))
                    } else {
                        None
                    }
                }
            }
            "linear" if boundary(bytes, at) && bytes[at].is_ascii_uppercase() => {
                let mut end = at;
                while bytes.get(end).is_some_and(u8::is_ascii_uppercase) {
                    end += 1;
                }
                if end - at < 2 {
                    None
                } else {
                    while bytes
                        .get(end)
                        .is_some_and(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
                    {
                        end += 1;
                    }
                    if bytes.get(end) == Some(&b'-') {
                        numeric_end(bytes, end + 1).map(|end| (at, end))
                    } else {
                        None
                    }
                }
            }
            "bead" if boundary(bytes, at) => {
                let tail = &branch[at..];
                let prefix = if tail
                    .get(..5)
                    .is_some_and(|s| s.eq_ignore_ascii_case("bead-"))
                {
                    5
                } else if tail.get(..3).is_some_and(|s| s.eq_ignore_ascii_case("bd-")) {
                    3
                } else {
                    0
                };
                if prefix == 0 {
                    None
                } else {
                    numeric_end(bytes, at + prefix).map(|end| (at, end))
                }
            }
            "gh" => {
                let marker = |start: usize| {
                    let tail = bytes.get(start..)?;
                    let first = if tail.starts_with(b"gh-") {
                        start + 3
                    } else if tail.starts_with(b"#") {
                        start + 1
                    } else {
                        return None;
                    };
                    numeric_end(bytes, first).map(|end| (first, end))
                };
                if at == 0 {
                    marker(at).or_else(|| {
                        if !bytes[at].is_ascii_alphanumeric() {
                            marker(at + branch[at..].chars().next().unwrap().len_utf8())
                        } else {
                            None
                        }
                    })
                } else if !bytes[at].is_ascii_alphanumeric() {
                    marker(at + branch[at..].chars().next().unwrap().len_utf8())
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some((start, end)) = found {
            out.push(branch[start..end].to_owned());
            at = end;
        } else {
            at += branch[at..].chars().next().unwrap().len_utf8();
        }
    }
    let mut out = records::unique(&out);
    out.sort();
    out
}
fn in_progress(path: &Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for line in bytes.split(|b| *b == b'\n') {
        let view = json::Value::from_go_bytes(line.to_vec());
        let line = view.as_str().unwrap().trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Ok(value) = json::parse(line) {
            let r = records::obj(Some(&value));
            let id = text(r.get("id"));
            if text(r.get("status")) == "in_progress" && !id.is_empty() {
                ids.push(id);
            }
        }
    }
    let mut ids = records::unique(&ids);
    ids.sort();
    ids
}
pub fn resolve_active_work(
    ctx: &Context,
    overrides: &BTreeMap<String, String>,
) -> ActiveWorkResult {
    let mut branch = git::git(&ctx.cwd, &ctx.env, &["rev-parse", "--abbrev-ref", "HEAD"])
        .unwrap_or_default()
        .trim()
        .to_owned();
    if branch == "HEAD" {
        branch.clear();
    }
    let mut hits: BTreeMap<&str, (Vec<String>, &str)> = BTreeMap::new();
    let missions = in_progress(&evidence::resolve_missions_dir(&ctx.cwd).join("issues.jsonl"));
    hits.insert(
        "missions",
        if missions.is_empty() {
            (tracker_matches(&branch, "missions"), "branch")
        } else {
            (missions, "in_progress")
        },
    );
    let mut gh = Vec::new();
    if !branch.is_empty() {
        if let Some(program) = ctx.env.look_path("gh") {
            let result = Command::new(program)
                .args(["pr", "view", "--json", "number", "--jq", ".number"])
                .current_dir(&ctx.cwd)
                .env_clear()
                .envs(ctx.env.vars().iter().map(|(k, v)| (k, v)))
                .env("PWD", &ctx.cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output();
            if let Some(result) = result.ok().filter(|r| r.status.success()) {
                let value = String::from_utf8_lossy(&result.stdout).trim().to_owned();
                if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
                    gh.push(format!("#{value}"));
                }
            }
        }
    }
    hits.insert(
        "gh",
        if gh.is_empty() {
            (
                tracker_matches(&branch, "gh")
                    .into_iter()
                    .map(|id| format!("#{id}"))
                    .collect(),
                "branch",
            )
        } else {
            (gh, "gh-cli")
        },
    );
    hits.insert("linear", (tracker_matches(&branch, "linear"), "branch"));
    let bead = in_progress(&ctx.cwd.join(".beads/issues.jsonl"));
    hits.insert(
        "bead",
        if bead.is_empty() {
            let mut ids = tracker_matches(&branch, "bead")
                .into_iter()
                .map(|s| s.to_lowercase())
                .collect::<Vec<_>>();
            ids.sort();
            (ids, "branch")
        } else {
            (bead, "in_progress")
        },
    );
    let mut values = BTreeMap::new();
    let mut warnings = Vec::new();
    for tracker in ["missions", "gh", "linear", "bead"] {
        if let Some(value) = overrides.get(tracker).filter(|s| !s.is_empty()) {
            values.insert(tracker, value.clone());
            continue;
        }
        let (matches, source) = &hits[tracker];
        if matches.len() == 1 {
            values.insert(tracker, matches[0].clone());
        } else if matches.len() > 1 {
            warnings.push(format!("active-work: multiple {tracker} candidates [{}] from {source}; pass --evidence-{tracker} <id> to disambiguate",matches.join(", ")));
        }
    }
    ActiveWorkResult {
        missions: values.remove("missions").unwrap_or_default(),
        gh: values.remove("gh").unwrap_or_default(),
        linear: values.remove("linear").unwrap_or_default(),
        bead: values.remove("bead").unwrap_or_default(),
        warnings,
    }
}
