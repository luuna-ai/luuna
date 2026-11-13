//! Optional best-effort Atlas prior art, using exactly the invocation environment.
use super::common::*;
use crate::{
    command::{output::Envelope, Context},
    error::Result,
    json::{self, Value as V},
    model::Ticket,
    store::Store,
};
use std::{
    collections::HashSet,
    path::PathBuf,
    process::{Command, Stdio},
};
fn executable(c: &Context<'_>, name: &str) -> Option<PathBuf> {
    let path = c.env("PATH")?;
    if path.is_empty() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for dir in std::env::split_paths(path) {
            let p = dir.join(name);
            let resolved = if p.is_absolute() {
                p.clone()
            } else {
                c.cwd.join(&p)
            };
            if std::fs::metadata(&resolved)
                .is_ok_and(|m| !m.is_dir() && m.permissions().mode() & 0o111 != 0)
            {
                return p.is_absolute().then_some(p);
            }
        }
        None
    }
    #[cfg(not(unix))]
    {
        Some(PathBuf::from(name))
    }
}
fn capture(c: &Context<'_>, name: &str, args: &[&str]) -> Option<Vec<u8>> {
    let binary = executable(c, name)?;
    let mut cmd = Command::new(binary);
    cmd.args(args)
        .current_dir(&c.cwd)
        .env_clear()
        .envs(&c.invocation.env)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    cmd.env("PWD", &c.cwd);
    let out = cmd.output().ok()?;
    out.status.success().then_some(out.stdout)
}

fn fold(name: &str) -> String {
    name.chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c == 'ſ' { 's' } else { c })
        .collect()
}
fn last_field<'a>(raw: &'a [u8], name: &str) -> Option<&'a [u8]> {
    let mut value = None;
    for (key, v) in super::raw::pairs(raw) {
        if fold(&json::unquote(key)) == name {
            value = Some(v)
        }
    }
    value
}
// Go's typed structs fold field names and apply alias occurrences in source
// order. Record maps remain case-sensitive, including their unknown metadata.
fn normalize(raw: &[u8], status: bool) -> Option<V> {
    if !json::is_valid(raw) {
        return None;
    }
    let raw = crate::store::trim_space(raw);
    if raw == b"null" {
        return Some(V::Null);
    }
    if raw.first() != Some(&b'{') {
        return None;
    }
    let mut result = std::collections::BTreeMap::new();
    let Some(domains) = last_field(raw, "domains") else {
        return Some(V::Object(result));
    };
    if domains == b"null" {
        result.insert("domains".into(), V::Null);
        return Some(V::Object(result));
    }
    let mut entries = vec![];
    for item in super::raw::array(domains)? {
        if item == b"null" {
            entries.push(V::Null);
            continue;
        }
        if item.first() != Some(&b'{') {
            return None;
        }
        let key = if status { "domain" } else { "records" };
        let mut entry = std::collections::BTreeMap::new();
        if let Some(value) = last_field(item, key) {
            entry.insert(key.into(), V::decode(value).ok()?);
        }
        entries.push(V::Object(entry));
    }
    result.insert("domains".into(), V::Array(entries));
    Some(V::Object(result))
}
fn payload(c: &Context<'_>, args: &[&str]) -> Option<V> {
    let out = capture(c, "luuna-atlas", args)?;
    normalize(&out, args.get(1) == Some(&"status"))
}
fn field<'a>(v: &'a V, name: &str) -> Option<&'a V> {
    object(v)?.get(name)
}

fn domains(p: &V) -> Option<&[V]> {
    match p {
        V::Null => Some(&[]),
        V::Object(m) => match m.get("domains") {
            None | Some(V::Null) => Some(&[]),
            Some(V::Array(a)) => Some(a),
            _ => None,
        },
        _ => None,
    }
}
fn status_domains(p: &V) -> Option<HashSet<String>> {
    let mut out = HashSet::new();
    for d in domains(p)? {
        match d {
            V::Null => {
                out.insert(String::new());
            }
            V::Object(m) => match m.get("domain") {
                None | Some(V::Null) => {
                    out.insert(String::new());
                }
                Some(V::String(s)) => {
                    out.insert(s.clone());
                }
                _ => return None,
            },
            _ => return None,
        }
    }
    Some(out)
}
fn finite(v: &V) -> bool {
    let mut stack = vec![v];
    while let Some(v) = stack.pop() {
        match v {
            V::Number(n) if !n.is_finite() => return false,
            V::Object(m) => stack.extend(m.values()),
            V::Array(a) => stack.extend(a),
            _ => (),
        }
    }
    true
}
fn query_records(p: &V) -> Option<Vec<&V>> {
    let mut out = vec![];
    for d in domains(p)? {
        let records = match d {
            V::Null => continue,
            V::Object(m) => match m.get("records") {
                None | Some(V::Null) => continue,
                Some(V::Array(a)) => a,
                _ => return None,
            },
            _ => return None,
        };
        for r in records {
            if !matches!(r, V::Null | V::Object(_)) || !finite(r) {
                return None;
            }
            out.push(r);
        }
    }
    Some(out)
}
pub(super) fn infer(c: &Context<'_>, seed: &Ticket, explicit: &str) -> String {
    if !explicit.is_empty() {
        return explicit.into();
    }
    let Some(p) = payload(c, &["--json", "status"]) else {
        return String::new();
    };
    let Some(domains) = status_domains(&p) else {
        return String::new();
    };
    for label in seed.labels.iter().flatten() {
        if domains.contains(label) {
            return label.clone();
        }
    }
    let mut refs = path_references(&seed.description);
    if refs.is_empty() {
        return String::new();
    }
    if let Some(out) = capture(c, "git", &["diff", "--name-only", "HEAD"]) {
        let changed: HashSet<_> = String::from_utf8_lossy(&out)
            .lines()
            .map(|s| s.trim().to_owned())
            .collect();
        let intersection: Vec<_> = refs
            .iter()
            .filter(|r| changed.contains(*r))
            .cloned()
            .collect();
        if !intersection.is_empty() {
            refs = intersection;
        }
    }
    for r in refs {
        for segment in r.split('/') {
            if domains.contains(segment) {
                return segment.into();
            }
        }
    }
    String::new()
}
/// The reference RE2 expression consists only of ASCII path segments. Find its
/// greedy matches without a new regex dependency or interpreting shell text.
pub(super) fn path_references(description: &str) -> Vec<String> {
    fn segment(b: u8) -> bool {
        b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-')
    }
    let bytes = description.as_bytes();
    let (mut i, mut out, mut seen) = (0, vec![], HashSet::new());
    while i < bytes.len() {
        if !segment(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        let (mut j, mut slash, mut end, mut segment_start) = (i, false, None, i);
        while j < bytes.len() {
            if segment(bytes[j]) {
                if bytes[j] == b'.' && slash && j > segment_start {
                    let mut k = j + 1;
                    while k < bytes.len() && bytes[k].is_ascii_alphanumeric() {
                        k += 1
                    }
                    if k > j + 1 {
                        end = Some(k);
                    }
                }
                j += 1;
            } else if bytes[j] == b'/'
                && j > start
                && segment(bytes[j - 1])
                && bytes.get(j + 1).is_some_and(|b| segment(*b))
            {
                slash = true;
                j += 1;
                segment_start = j;
            } else {
                break;
            }
        }
        if let Some(end) = end {
            let value = description[start..end]
                .trim_end_matches(['.', ',', ';', ':', ')', ']'])
                .to_owned();
            if seen.insert(value.clone()) {
                out.push(value)
            }
            i = end;
        } else {
            i = j.max(i + 1);
        }
    }
    out
}
pub(super) fn prior(c: &Context<'_>, domain: &str, name: &str, hint: &str) -> Vec<Vec<u8>> {
    if domain.is_empty() {
        return vec![];
    }
    let kinds = if !hint.is_empty() {
        vec![hint]
    } else {
        match name {
            "approach" => vec!["pattern", "decision"],
            "risks" => vec!["failure"],
            "acceptance" => vec!["guide"],
            _ => vec![],
        }
    };
    let mut entries = vec![];
    for kind in kinds {
        let Some(p) = payload(c, &["--json", "query", domain, "--type", kind]) else {
            continue;
        };
        let Some(records) = query_records(&p) else {
            continue;
        };
        for (index, record) in records.into_iter().take(5).enumerate() {
            let id = text(field(record, "id"));
            if id.is_empty() {
                continue;
            }
            let ty = text(field(record, "type"));
            let ty = if ty.is_empty() { kind } else { ty };
            let name = text(field(record, "name"));
            let desc = text(field(record, "description"));
            let mut summary = if name.is_empty() {
                desc.into()
            } else if desc.is_empty() {
                name.into()
            } else {
                format!("{name}: {desc}")
            };
            if summary.chars().count() > 240 {
                summary = summary
                    .chars()
                    .take(239)
                    .collect::<String>()
                    .trim_end_matches([' ', '\n', '\t'])
                    .to_owned()
                    + "…";
            }
            entries.push(compact(
                Envelope::object()
                    .string("id", id)
                    .string("type", ty)
                    .string("summary", &summary)
                    .raw(
                        "relevance",
                        json::format_float((5 - index) as f64 / 5.)
                            .unwrap()
                            .as_bytes(),
                    ),
            ));
        }
    }
    entries.truncate(5);
    entries
}
pub(super) fn inherited(seed: &Ticket, store: &Store) -> Result<String> {
    let Some(index) = seed.plan_step_index else {
        return Ok(String::new());
    };
    let plans = load(store)?;
    let parent = if !seed.plan_id.is_empty() {
        plans.iter().find(|p| p.id == seed.plan_id)
    } else {
        None
    }
    .or_else(|| plans.iter().find(|p| p.children().contains(&seed.id)));
    let Some(parent) = parent else {
        return Ok(String::new());
    };
    let Some(steps) = parent.section("steps").and_then(array) else {
        return Ok(String::new());
    };
    if index < 0 {
        return Ok(String::new());
    }
    let Some(step) = steps.get(index as usize).and_then(object) else {
        return Ok(String::new());
    };
    Ok(text(step.get("plan_template")).into())
}
/// Go's submit writer uses this process path too; later native Atlas wiring
/// replaces the transport while retaining the command-level result contract.
pub(super) fn invoke(c: &Context<'_>, args: &[&str]) -> Option<Vec<u8>> {
    capture(c, "luuna-atlas", args)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ascii_path_matches_are_greedy_deduplicated_and_ordered() {
        assert_eq!(
            path_references(
                "see src/a.rs, lib/b.test.ts and src/a.rs; a//pkg/c.go then foo/bar.rs/noext"
            ),
            ["src/a.rs", "lib/b.test.ts", "pkg/c.go", "foo/bar.rs"]
        );
    }
    #[test]
    fn inherited_template_uses_plan_id_then_first_parent() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join(".luuna/missions");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.yaml"), b"project: demo\nversion: \"1\"\n").unwrap();
        std::fs::write(dir.join("plans.jsonl"),b"{\"id\":\"pl-a\",\"children\":[\"child\"],\"sections\":{\"steps\":[{\"plan_template\":\"bug\"}]}}\n").unwrap();
        let store = Store::open(dir).unwrap();
        let mut seed = Ticket::default();
        seed.id = "child".into();
        seed.plan_step_index = Some(0);
        assert_eq!(inherited(&seed, &store).unwrap(), "bug");
        seed.plan_step_index = Some(-1);
        assert_eq!(inherited(&seed, &store).unwrap(), "");
    }
    #[test]
    fn paths_need_a_nonempty_filename_before_the_extension() {
        assert_eq!(
            path_references("src/.rs src/.hidden.rs x/a.rs/next.ext"),
            ["src/.hidden.rs", "x/a.rs/next.ext"]
        );
    }
    #[test]
    fn typed_status_errors_discard_the_whole_payload() {
        let p = V::decode(br#"{"domains":[{"domain":"good"},{"domain":3}]}"#).unwrap();
        assert!(status_domains(&p).is_none());
        let p = V::decode(br#"{"domains":[null,{"domain":"good","ignored":1e400}]}"#).unwrap();
        assert!(status_domains(&p).unwrap().contains("good"));
    }
    #[test]
    fn typed_query_errors_discard_all_records_and_nonfinite_generic_values() {
        for raw in [
            br#"{"domains":[{"records":[{"id":"good"},3]}]}"#.as_slice(),
            br#"{"domains":[{"records":[{"id":"good","n":1e400}]}]}"#,
        ] {
            let p = V::decode(raw).unwrap();
            assert!(query_records(&p).is_none());
        }
        let p = V::decode(br#"{"domains":[{"records":[null,{"id":"good"}],"ignored":1e400}]}"#)
            .unwrap();
        assert_eq!(query_records(&p).unwrap().len(), 2);
    }
    #[test]
    fn struct_aliases_follow_go_case_folding_and_last_source_occurrence() {
        for raw in [br#"{"Domains":[{"Domain":"parity"}]}"#.as_slice(),br#"{"domains":[{"domain":"wrong"}],"Domains":[{"domain":"wrong"}],"domains":[{"domain":"parity"}]}"#,"{\"domainſ\":[{\"domain\":\"parity\"}]}".as_bytes()]{let p=normalize(raw,true).unwrap();let names=status_domains(&p).unwrap();assert!(names.contains("parity"));assert!(!names.contains("wrong"));}
    }
    #[test]
    fn query_struct_aliases_leave_record_maps_case_sensitive() {
        let p = normalize(
            br#"{"Domains":[{"Records":[{"ID":"not-id","id":"p"}]}]}"#,
            false,
        )
        .unwrap();
        let records = query_records(&p).unwrap();
        assert_eq!(text(field(records[0], "id")), "p");
        assert_eq!(text(field(records[0], "ID")), "not-id");
    }
}
