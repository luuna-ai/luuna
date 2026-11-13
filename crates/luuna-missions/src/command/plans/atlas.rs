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
    #[derive(Default)]
    struct Domain {
        name: String,
        records: Vec<Option<std::collections::BTreeMap<String, V>>>,
        len: usize,
        nil: bool,
    }
    let mut storage: Vec<Domain> = vec![];
    let mut len = 0;
    let mut nil = true;
    for (key, value) in super::raw::pairs(raw) {
        if fold(&json::unquote(key)) != "domains" {
            continue;
        }
        if value == b"null" {
            storage.clear();
            len = 0;
            nil = true;
            continue;
        }
        let items = super::raw::array(value)?;
        len = items.len();
        nil = false;
        storage.resize_with(storage.len().max(len), Domain::default);
        for (i, item) in items.into_iter().enumerate() {
            if item == b"null" {
                continue;
            }
            if item.first() != Some(&b'{') {
                return None;
            }
            let domain = &mut storage[i];
            for (key, value) in super::raw::pairs(item) {
                let key = fold(&json::unquote(key));
                if status && key == "domain" {
                    match V::decode(value).ok()? {
                        V::Null => (),
                        V::String(ref s) => domain.name = s.clone(),
                        _ => return None,
                    }
                }
                if !status && key == "records" {
                    if value == b"null" {
                        domain.records.clear();
                        domain.len = 0;
                        domain.nil = true;
                        continue;
                    }
                    let records = super::raw::array(value)?;
                    domain.len = records.len();
                    domain.nil = false;
                    domain
                        .records
                        .resize_with(domain.records.len().max(domain.len), || None);
                    for (j, record) in records.into_iter().enumerate() {
                        if crate::json_error::unmarshal_error(&String::from_utf8_lossy(record))
                            .is_some()
                        {
                            return None;
                        }
                        match V::decode(record).ok()? {
                            V::Null => domain.records[j] = None,
                            V::Object(ref m) => {
                                for value in m.values() {
                                    encoded(value, true).ok()?;
                                }
                                domain.records[j].get_or_insert_default().extend(m.clone())
                            }
                            _ => return None,
                        }
                    }
                }
            }
        }
    }
    let entries = storage
        .into_iter()
        .take(len)
        .map(|d| {
            let value = if status {
                V::String(d.name)
            } else if d.nil {
                V::Null
            } else {
                V::Array(
                    d.records
                        .into_iter()
                        .take(d.len)
                        .map(|m| m.map(V::Object).unwrap_or(V::Null))
                        .collect(),
                )
            };
            V::Object(std::collections::BTreeMap::from([(
                if status { "domain" } else { "records" }.into(),
                value,
            )]))
        })
        .collect();
    Some(V::Object(std::collections::BTreeMap::from([(
        "domains".into(),
        if nil { V::Null } else { V::Array(entries) },
    )])))
}
fn recorded_id(raw: &[u8]) -> Option<String> {
    if !json::is_valid(raw) {
        return None;
    }
    let raw = crate::store::trim_space(raw);
    if raw == b"null" {
        return Some(String::new());
    }
    if raw.first() != Some(&b'{') {
        return None;
    }
    let mut id = String::new();
    for (key, record) in super::raw::pairs(raw) {
        if fold(&json::unquote(key)) != "record" {
            continue;
        }
        if record == b"null" {
            continue;
        }
        if record.first() != Some(&b'{') {
            return None;
        }
        for (key, value) in super::raw::pairs(record) {
            if fold(&json::unquote(key)) == "id" {
                match V::decode(value).ok()? {
                    V::Null => (),
                    V::String(ref s) => id = s.clone(),
                    _ => return None,
                }
            }
        }
    }
    Some(id)
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
/// Submit's optional decision write uses the project root, unlike prompt queries.
pub(super) fn record(
    c: &mut Context<'_>,
    store: &Store,
    seed: &Ticket,
    plan: &super::record::Plan,
    approach: Option<&V>,
    forced: &str,
) -> String {
    let Some(binary) = executable(c, "luuna-atlas") else {
        c.stderr.extend_from_slice(
            "⚠ --record-decision: luuna-atlas not found on PATH; skipping\n".as_bytes(),
        );
        return String::new();
    };
    let root = crate::config::project_root(store.dir());
    let run = |args: &[&str]| {
        let mut cmd = Command::new(&binary);
        cmd.args(args)
            .current_dir(&root)
            .env_clear()
            .envs(&c.invocation.env)
            .stdin(Stdio::null());
        #[cfg(unix)]
        cmd.env("PWD", &root);
        cmd.output()
    };
    let mut domain = forced.to_owned();
    if domain.is_empty() {
        if let Ok(out) = run(&["--json", "status"]) {
            if out.status.success() {
                if let Some(p) = normalize(&out.stdout, true) {
                    if let Some(set) = status_domains(&p) {
                        domain = submitted_domain(seed, &set);
                    }
                }
            }
        }
    }
    if domain.is_empty() {
        c.stderr.extend_from_slice(
            "⚠ --record-decision: no luuna-atlas domain inferred (skipping)\n".as_bytes(),
        );
        return String::new();
    }
    let out = run(&[
        "record",
        &domain,
        "--type",
        "decision",
        "--title",
        &seed.title,
        "--rationale",
        text(approach),
        "--evidence-missions",
        &plan.id,
        "--json",
    ]);
    match out {
        Ok(out) if out.status.success() => recorded_id(&out.stdout).unwrap_or_default(),
        other => {
            let detail = match other {
                Ok(out) => String::from_utf8_lossy(&out.stderr)
                    .trim()
                    .split('\n')
                    .next()
                    .filter(|s| !s.is_empty())
                    .map(|s| format!(": {s}"))
                    .unwrap_or_default(),
                Err(_) => String::new(),
            };
            c.stderr.extend_from_slice(
                format!("⚠ --record-decision: luuna-atlas record failed{detail}\n").as_bytes(),
            );
            String::new()
        }
    }
}
fn submitted_domain(seed: &Ticket, set: &HashSet<String>) -> String {
    for label in seed.labels.as_deref().unwrap_or_default() {
        if set.contains(label) {
            return label.clone();
        }
    }
    for token in seed.description.split_whitespace() {
        let token = token.trim_end_matches(['.', ',', ';', ':', ')', ']']);
        if !token.contains('/') || !token.rsplit('/').next().unwrap_or_default().contains('.') {
            continue;
        }
        for part in token.split('/') {
            if set.contains(part) {
                return part.into();
            }
        }
    }
    String::new()
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
    #[test]
    fn repeated_record_fields_merge_nulls_and_preserve_prior_type_errors() {
        for raw in [
            br#"{"record":{"id":"first"},"Record":{}}"#.as_slice(),
            br#"{"record":{"id":"first","ID":null}}"#,
            br#"{"record":{"id":"first"},"Record":null}"#,
        ] {
            assert_eq!(recorded_id(raw), Some("first".into()));
        }
        for raw in [
            br#"{"record":{"id":3,"ID":"valid"}}"#.as_slice(),
            br#"{"record":3,"Record":{"id":"valid"}}"#,
        ] {
            assert!(recorded_id(raw).is_none());
        }
    }
    #[test]
    fn status_aliases_reuse_slice_structs_as_go_does() {
        for raw in [
            br#"{"domains":[{"domain":"first","Domain":null}]}"#.as_slice(),
            br#"{"domains":[{"domain":"first"}],"Domains":[{}]}"#,
            br#"{"domains":[{"domain":"first"}],"Domains":[null]}"#,
        ] {
            assert!(status_domains(&normalize(raw, true).unwrap())
                .unwrap()
                .contains("first"));
        }
        assert!(normalize(br#"{"domains":3,"Domains":[{"domain":"valid"}]}"#, true).is_none());
        assert!(normalize(br#"{"domains":[{"domain":3,"Domain":"valid"}]}"#, true).is_none());
    }
    #[test]
    fn submit_domain_inference_uses_labels_then_token_segments() {
        let set = HashSet::from(["core".into(), "ui".into()]);
        let mut seed = Ticket::default();
        seed.labels = Some(vec!["ui".into()]);
        seed.description = "core/file.rs".into();
        assert_eq!(submitted_domain(&seed, &set), "ui");
        seed.labels = None;
        seed.description = "Look at [core/file.rs), and ui/view.ts".into();
        assert_eq!(submitted_domain(&seed, &set), "ui");
        seed.description = "core/file.rs),".into();
        assert_eq!(submitted_domain(&seed, &set), "core");
    }
    #[test]
    #[cfg(unix)]
    fn recording_runs_at_project_root_with_invocation_environment_and_keeps_local_success() {
        use crate::{
            command::{run, Invocation},
            config_tree::tree::test_support::project,
        };
        use std::{fs, os::unix::fs::PermissionsExt};
        let t = project("project: demo\nversion: \"1\"\n");
        let dir = t.path().join(".luuna/missions");
        fs::write(
            dir.join("issues.jsonl"),
            b"{\"id\":\"demo-1\",\"title\":\"Parent\",\"status\":\"open\"}\n",
        )
        .unwrap();
        let bin = t.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let script = bin.join("luuna-atlas");
        fs::write(&script,b"#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$SENTINEL\" \"$@\" > atlas-call\nprintf '%s' '{\"Record\":{\"ID\":\"decision-id\"}}'\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let nested = t.path().join("nested");
        fs::create_dir(&nested).unwrap();
        let raw=br#"{"template":"feature","sections":{"context":"A sufficiently long explanation of this work and its intended motivation.","approach":"Use native behavior","steps":[{"title":"A"},{"title":"B"}],"acceptance":["Done"]}}"#;
        let inv = Invocation::new(
            &nested,
            [
                "plan",
                "submit",
                "demo-1",
                "--plan",
                "-",
                "--record-decision",
                "--domain",
                "core",
            ],
        )
        .with_stdin(raw.to_vec())
        .with_env(std::collections::BTreeMap::from([
            ("PATH".into(), bin.to_str().unwrap().into()),
            ("SENTINEL".into(), "supplied".into()),
        ]));
        let out = run(&inv).unwrap();
        assert_eq!(out.exit_code, 0, "{:?}", out);
        assert!(String::from_utf8(out.stdout)
            .unwrap()
            .contains("recorded luuna-atlas decision decision-id"));
        let call = fs::read_to_string(t.path().join("atlas-call")).unwrap();
        assert!(
            call.starts_with(&format!("{}\nsupplied\nrecord\ncore\n", t.path().display())),
            "{call}"
        );
        assert!(!nested.join("atlas-call").exists());
        fs::write(
            &script,
            b"#!/bin/sh\nprintf 'first line\\nsecond line\\n' >&2\nexit 1\n",
        )
        .unwrap();
        let mut inv = inv;
        inv.args.push("--overwrite".into());
        let out = run(&inv).unwrap();
        assert_eq!(out.exit_code, 0);
        assert!(String::from_utf8(out.stderr)
            .unwrap()
            .contains("record failed: first line\n"));
    }
    #[test]
    fn repeated_query_maps_merge_and_report_earlier_overflow() {
        let p=normalize(br#"{"domains":[{"records":[{"id":"a","keep":true}]}],"Domains":[{"Records":[{"title":"b"}]}]}"#,false).unwrap();
        let records = query_records(&p).unwrap();
        assert_eq!(text(field(records[0], "id")), "a");
        assert_eq!(text(field(records[0], "title")), "b");
        assert!(normalize(br#"{"domains":[{"records":[{"n":1e400,"n":1}]}]}"#, false).is_none());
    }
}
