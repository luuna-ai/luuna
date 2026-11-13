//! Missions doctor checks in Go order, with iterative record and graph walks.
use super::{doctor_fix, tpl::strings};
use crate::{
    command::{options, output::Envelope, read::println, Context, Failure},
    config,
    error::Error,
    json::Value as V,
    json_error,
    model::Ticket,
    store::{trim_space, DataFile, Store},
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::Path,
    time::SystemTime,
};
pub(super) const MERGE_LINES: [&str; 3] = [
    ".luuna/missions/issues.jsonl merge=union",
    ".luuna/missions/templates.jsonl merge=union",
    ".luuna/missions/plans.jsonl merge=union",
];
pub(super) type Object = BTreeMap<String, V>;
#[derive(Clone)]
pub(super) struct Check {
    pub name: &'static str,
    pub status: &'static str,
    pub message: String,
    pub details: Vec<String>,
    pub fixable: bool,
}
pub(super) struct Line {
    pub number: usize,
    pub raw: Vec<u8>,
    pub parsed: Option<V>,
    pub error: Option<String>,
}
fn pass(name: &'static str, message: &str) -> Check {
    Check {
        name,
        status: "pass",
        message: message.into(),
        details: Vec::new(),
        fixable: false,
    }
}
fn problem(
    name: &'static str,
    status: &'static str,
    message: String,
    details: Vec<String>,
    fixable: bool,
) -> Check {
    Check {
        name,
        status,
        message,
        details,
        fixable,
    }
}
pub(super) fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, Failure> {
    match fs::read(path) {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::io("read", path, e).into()),
    }
}
pub(super) fn lines(bytes: &[u8]) -> Vec<Line> {
    let mut result = Vec::new();
    for (i, raw) in bytes.split(|b| *b == b'\n').enumerate() {
        let raw = trim_space(raw);
        if raw.is_empty() {
            continue;
        }
        let mut error = json_error::unmarshal_error(&String::from_utf8_lossy(raw));
        let parsed = if error.is_none() {
            match V::decode(raw) {
                Ok(v) => Some(v),
                Err(e) => {
                    error = Some(e.to_string());
                    None
                }
            }
        } else {
            None
        };
        result.push(Line {
            number: i + 1,
            raw: raw.to_vec(),
            parsed,
            error,
        });
    }
    result
}
pub(super) fn object(v: Option<&V>) -> Option<&Object> {
    if let Some(V::Object(m)) = v {
        Some(m)
    } else {
        None
    }
}
pub(super) fn field<'a>(v: Option<&'a V>, key: &str) -> Option<&'a V> {
    object(v).and_then(|o| o.get(key))
}
pub(super) fn text(v: Option<&V>) -> Option<&str> {
    if let Some(V::String(s)) = v {
        Some(s)
    } else {
        None
    }
}
pub(super) fn array(v: Option<&V>) -> Option<&[V]> {
    if let Some(V::Array(v)) = v {
        Some(v)
    } else {
        None
    }
}
pub(super) fn truthy(v: Option<&V>) -> bool {
    text(v).is_some_and(|s| !s.is_empty())
}
fn id(line: &Line) -> String {
    text(field(line.parsed.as_ref(), "id"))
        .map(str::to_owned)
        .unwrap_or_else(|| format!("line {}", line.number))
}
pub(super) fn encoded(value: &V) -> Vec<u8> {
    let mut out = Vec::new();
    if value.encode(&mut out, true).is_err() {
        out.clear();
    }
    out
}
fn json_text(value: &V) -> String {
    String::from_utf8(encoded(value)).unwrap()
}
fn number(value: f64) -> String {
    format!("{value:.6}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .into()
}
pub(super) fn age(meta: &fs::Metadata) -> Option<f64> {
    let modified = meta.modified().ok()?;
    Some(match SystemTime::now().duration_since(modified) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    })
}
fn config_check(dir: &Path) -> Result<Check, Failure> {
    if fs::metadata(dir).is_err() {
        return Ok(problem(
            "config",
            "fail",
            ".luuna/missions/ directory not found".into(),
            Vec::new(),
            false,
        ));
    }
    match config::read_config(dir) {
        Ok(v) if v.project.is_empty() => Ok(problem(
            "config",
            "fail",
            "config.yaml missing required 'project' field".into(),
            Vec::new(),
            false,
        )),
        Ok(_) => Ok(pass("config", "Config is valid")),
        Err(e @ Error::Unsupported { .. }) => Err(e.into()),
        Err(_) => Ok(problem(
            "config",
            "fail",
            "config.yaml is missing or unparseable".into(),
            Vec::new(),
            false,
        )),
    }
}
fn integrity(issues: &[Line], templates: &[Line]) -> Check {
    let mut details = Vec::new();
    for (name, lines) in [("issues.jsonl", issues), ("templates.jsonl", templates)] {
        for line in lines {
            if let Some(error) = &line.error {
                details.push(format!("{name} line {}: {error}", line.number));
            }
        }
    }
    if details.is_empty() {
        pass("jsonl-integrity", "All JSONL lines parse correctly")
    } else {
        problem(
            "jsonl-integrity",
            "fail",
            format!("{} malformed line(s) in JSONL files", details.len()),
            details,
            true,
        )
    }
}
fn schema(lines: &[Line]) -> Check {
    let mut details = Vec::new();
    for line in lines {
        if line.error.is_some() || matches!(line.parsed, None | Some(V::Null)) {
            continue;
        }
        let id = id(line);
        for key in ["id", "title", "createdAt", "updatedAt"] {
            if !truthy(field(line.parsed.as_ref(), key)) {
                details.push(format!("{id}: missing or invalid '{key}'"));
            }
        }
        if let Some(status) = text(field(line.parsed.as_ref(), "status")) {
            if !["open", "in_progress", "closed"].contains(&status) {
                details.push(format!("{id}: invalid status '{status}'"));
            }
        }
        if let Some(kind) = text(field(line.parsed.as_ref(), "type")) {
            if !["task", "bug", "feature", "epic"].contains(&kind) {
                details.push(format!("{id}: invalid type '{kind}'"));
            }
        }
        if let Some(V::Number(p)) = field(line.parsed.as_ref(), "priority") {
            if *p < 0.0 || *p > 4.0 {
                details.push(format!(
                    "{id}: invalid priority {} (must be 0-4)",
                    number(*p)
                ));
            }
        }
    }
    if details.is_empty() {
        pass("schema-validation", "All issues have valid schema")
    } else {
        problem(
            "schema-validation",
            "fail",
            format!("{} schema violation(s)", details.len()),
            details,
            false,
        )
    }
}
fn duplicates(issues: &[Line], templates: &[Line]) -> Check {
    let mut details = Vec::new();
    for (name, lines) in [("issues.jsonl", issues), ("templates.jsonl", templates)] {
        let mut counts = HashMap::new();
        let mut order = Vec::new();
        for line in lines {
            if line.error.is_some() {
                continue;
            }
            if let Some(id) = text(field(line.parsed.as_ref(), "id")) {
                let n = counts.entry(id).or_insert(0);
                if *n == 0 {
                    order.push(id);
                }
                *n += 1;
            }
        }
        for id in order {
            if counts[id] > 1 {
                details.push(format!("{id} appears {} times in {name}", counts[id]));
            }
        }
    }
    if details.is_empty() {
        pass("duplicate-ids", "No duplicate IDs")
    } else {
        problem(
            "duplicate-ids",
            "warn",
            format!("{} duplicate ID(s) found", details.len()),
            details,
            true,
        )
    }
}
fn references(issues: &[Ticket]) -> Check {
    let ids = issues.iter().map(|t| t.id.as_str()).collect::<HashSet<_>>();
    let mut details = Vec::new();
    for t in issues {
        for (field, refs) in [("blockedBy", &t.blocked_by), ("blocks", &t.blocks)] {
            for r in refs.as_deref().unwrap_or_default() {
                if !ids.contains(r.as_str()) {
                    details.push(format!("{}.{field} → {r} (not found)", t.id));
                }
            }
        }
    }
    if details.is_empty() {
        pass(
            "referential-integrity",
            "All dependency references are valid",
        )
    } else {
        problem(
            "referential-integrity",
            "warn",
            format!("{} dangling dependency reference(s)", details.len()),
            details,
            true,
        )
    }
}
fn bidirectional(issues: &[Ticket]) -> Check {
    let by_id = issues
        .iter()
        .map(|t| (t.id.as_str(), t))
        .collect::<HashMap<_, _>>();
    let mut details = Vec::new();
    for t in issues {
        for (name, refs, back) in [
            ("blockedBy", &t.blocked_by, "blocks"),
            ("blocks", &t.blocks, "blockedBy"),
        ] {
            for r in refs.as_deref().unwrap_or_default() {
                if let Some(other) = by_id.get(r.as_str()) {
                    let list = if back == "blocks" {
                        &other.blocks
                    } else {
                        &other.blocked_by
                    };
                    if !list.as_deref().unwrap_or_default().contains(&t.id) {
                        details.push(format!(
                            "{}.{name} has {r}, but {r}.{back} missing {}",
                            t.id, t.id
                        ));
                    }
                }
            }
        }
    }
    if details.is_empty() {
        pass(
            "bidirectional-consistency",
            "All dependency links are bidirectional",
        )
    } else {
        problem(
            "bidirectional-consistency",
            "warn",
            format!("{} bidirectional mismatch(es)", details.len()),
            details,
            true,
        )
    }
}
fn cycles(issues: &[Ticket]) -> Check {
    let graph = issues
        .iter()
        .map(|t| (t.id.as_str(), t.blocked_by.as_deref().unwrap_or_default()))
        .collect::<HashMap<_, _>>();
    let mut visited = HashSet::new();
    let mut active = HashSet::new();
    let mut path: Vec<&str> = Vec::new();
    let mut details = Vec::new();
    for t in issues {
        let mut stack = vec![(t.id.as_str(), false)];
        while let Some((id, exit)) = stack.pop() {
            if exit {
                active.remove(id);
                path.pop();
                continue;
            }
            if active.contains(id) {
                if let Some(i) = path.iter().position(|p| *p == id) {
                    let mut cycle = path[i..].to_vec();
                    cycle.push(id);
                    details.push(cycle.join(" → "));
                }
                continue;
            }
            if !visited.insert(id) {
                continue;
            }
            active.insert(id);
            path.push(id);
            stack.push((id, true));
            if let Some(refs) = graph.get(id) {
                for r in refs.iter().rev() {
                    stack.push((r.as_str(), false));
                }
            }
        }
    }
    if details.is_empty() {
        pass("circular-dependencies", "No circular dependencies")
    } else {
        problem(
            "circular-dependencies",
            "warn",
            format!("{} circular dependency chain(s) found", details.len()),
            details,
            false,
        )
    }
}
fn labels(lines: &[Line]) -> Check {
    let mut details = Vec::new();
    for line in lines {
        if line.error.is_some() {
            continue;
        }
        let Some(value) = field(line.parsed.as_ref(), "labels") else {
            continue;
        };
        let id = id(line);
        if let V::Array(items) = value {
            for item in items {
                match item {
                    V::String(label) if label.trim().is_empty() => {
                        details.push(format!("{id}: empty label string"))
                    }
                    V::String(_) => {}
                    v => details.push(format!("{id}: label is not a string: {}", json_text(v))),
                }
            }
        } else {
            details.push(format!("{id}: labels is not an array"));
        }
    }
    if details.is_empty() {
        pass("label-schema", "All label arrays are valid")
    } else {
        problem(
            "label-schema",
            "warn",
            format!("{} label schema issue(s)", details.len()),
            details,
            true,
        )
    }
}
fn extensions(lines: &[Line]) -> Check {
    let mut details = Vec::new();
    for line in lines {
        if line.error.is_some() {
            continue;
        }
        if let Some(value) = field(line.parsed.as_ref(), "extensions") {
            if !matches!(value, V::Object(_)) {
                details.push(format!(
                    "{}: extensions must be a plain object: {}",
                    id(line),
                    json_text(value)
                ));
            }
        }
    }
    if details.is_empty() {
        pass("extensions-schema", "All extensions fields are valid")
    } else {
        problem(
            "extensions-schema",
            "warn",
            format!("{} malformed extensions field(s)", details.len()),
            details,
            true,
        )
    }
}
fn closed(lines: &[Line]) -> Check {
    let (mut failures, mut warnings) = (Vec::new(), Vec::new());
    for line in lines {
        if line.error.is_some() {
            continue;
        }
        let value = line.parsed.as_ref();
        let id = id(line);
        let status = text(field(value, "status"));
        let at = truthy(field(value, "closedAt"));
        let reason = truthy(field(value, "closeReason"));
        if let Some(status) = status {
            if status != "closed" && (at || reason) {
                let mut fields = Vec::new();
                if at {
                    fields.push("closedAt");
                }
                if reason {
                    fields.push("closeReason");
                }
                failures.push(format!(
                    "{id}: status={status} but {} still set",
                    fields.join("+")
                ));
            } else if status == "closed" && !at {
                warnings.push(format!("{id}: status=closed but closedAt missing"));
            }
        }
    }
    if !failures.is_empty() {
        failures.extend(warnings);
        problem(
            "closed-fields-consistency",
            "fail",
            format!("{} status/close-metadata mismatch(es)", failures.len()),
            failures,
            true,
        )
    } else if !warnings.is_empty() {
        problem(
            "closed-fields-consistency",
            "warn",
            format!("{} closed issue(s) missing closedAt", warnings.len()),
            warnings,
            true,
        )
    } else {
        pass(
            "closed-fields-consistency",
            "All status/close-metadata pairs are consistent",
        )
    }
}
fn locks(dir: &Path) -> Check {
    let mut details = Vec::new();
    for name in ["issues.jsonl", "templates.jsonl"] {
        let path = dir.join(format!("{name}.lock"));
        match fs::metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => details.push(format!("{name}.lock exists but cannot stat")),
            Ok(meta) => {
                let seconds = age(&meta).unwrap_or_default();
                if seconds > 30.0 {
                    details.push(format!(
                        "{name}.lock is stale ({}s old)",
                        seconds.round() as i64
                    ));
                } else {
                    details.push(format!(
                        "{name}.lock exists ({}s old, may be active)",
                        seconds.round() as i64
                    ));
                }
            }
        }
    }
    if details.is_empty() {
        pass("stale-locks", "No stale lock files")
    } else {
        problem(
            "stale-locks",
            "warn",
            format!("{} lock file(s) found", details.len()),
            details,
            true,
        )
    }
}
fn attributes(dir: &Path) -> Check {
    let path = config::project_root(dir).join(".gitattributes");
    let mut details = Vec::new();
    match fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            details.push(".gitattributes file not found".into())
        }
        Ok(bytes) => {
            for line in MERGE_LINES {
                if !bytes.windows(line.len()).any(|s| s == line.as_bytes()) {
                    details.push(format!("Missing: {line}"));
                }
            }
        }
        Err(_) => {}
    }
    if details.is_empty() {
        pass("gitattributes", "Gitattributes configured correctly")
    } else {
        problem(
            "gitattributes",
            "warn",
            "Missing merge=union gitattributes entries".into(),
            details,
            true,
        )
    }
}
pub(super) fn collect(store: &Store) -> Result<Vec<Check>, Failure> {
    let dir = store.dir();
    let mut checks = vec![config_check(dir)?];
    if checks[0].status == "fail" {
        return Ok(checks);
    }
    let issues = lines(
        read_optional(&store.path(DataFile::Issues))?
            .as_deref()
            .unwrap_or_default(),
    );
    let templates = lines(
        read_optional(&store.path(DataFile::Templates))?
            .as_deref()
            .unwrap_or_default(),
    );
    checks.push(integrity(&issues, &templates));
    checks.push(schema(&issues));
    checks.push(duplicates(&issues, &templates));
    let tickets = store.read_tickets()?;
    checks.push(references(&tickets));
    checks.push(bidirectional(&tickets));
    checks.push(cycles(&tickets));
    checks.push(labels(&issues));
    checks.push(extensions(&issues));
    checks.push(closed(&issues));
    checks.push(locks(dir));
    checks.push(attributes(dir));
    Ok(checks)
}
pub(super) fn run(context: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let opts = options::parse(args, &[], &["fix", "json", "verbose"])?;
    let store = context.store()?;
    let mut checks = collect(&store)?;
    let fixed = if opts.has("fix") && checks.iter().any(|c| c.status != "pass" && c.fixable) {
        let fixed = doctor_fix::apply(&store, &checks)?;
        checks = collect(&store)?;
        fixed
    } else {
        Vec::new()
    };
    let pass = checks.iter().filter(|c| c.status == "pass").count();
    let warn = checks.iter().filter(|c| c.status == "warn").count();
    let fail = checks.iter().filter(|c| c.status == "fail").count();
    if opts.has("json") {
        let mut raw = vec![b'['];
        for (i, c) in checks.iter().enumerate() {
            if i > 0 {
                raw.push(b',');
            }
            let value = Envelope::object()
                .string("name", c.name)
                .string("status", c.status)
                .string("message", &c.message)
                .raw("details", &strings(&c.details))
                .boolean("fixable", c.fixable)
                .render();
            raw.extend(value);
        }
        raw.push(b']');
        let summary = Envelope::object()
            .integer("pass", pass as i64)
            .integer("warn", warn as i64)
            .integer("fail", fail as i64)
            .render();
        let mut response = Envelope::new(fail == 0, "doctor")
            .raw("checks", &raw)
            .raw("summary", &summary);
        if !fixed.is_empty() {
            response = response.raw("fixed", &strings(&fixed));
        }
        context.json(response);
    } else {
        context.stdout.extend_from_slice(b"\nMissions Doctor\n\n");
        for c in &checks {
            if c.status == "pass" && !opts.has("verbose") {
                continue;
            }
            let icon = match c.status {
                "warn" => "!",
                "fail" => "✗",
                _ => "✓",
            };
            println(context, &format!("  {icon} {}", c.message));
            for detail in &c.details {
                println(context, &format!("      {detail}"));
            }
        }
        println(
            context,
            &format!("\n{pass} passed, {warn} warning(s), {fail} failure(s)"),
        );
        if !fixed.is_empty() {
            println(context, "\nFixed:");
            for item in fixed {
                println(context, &format!("  ✓ {item}"));
            }
        }
    }
    if fail > 0 {
        context.exit_code = 1;
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::config_tree::tree::test_support::{invoke, project};
    pub fn fixture() -> tempfile::TempDir {
        let t = project("project: demo\nversion: \"1\"\n");
        let dir = t.path().join(".luuna/missions");
        fs::write(dir.join("issues.jsonl"), b"").unwrap();
        fs::write(dir.join("templates.jsonl"), b"").unwrap();
        fs::write(
            t.path().join(".gitattributes"),
            format!("{}\n", MERGE_LINES.join("\n")),
        )
        .unwrap();
        t
    }
    pub fn record(id: &str, extra: &[u8]) -> Vec<u8> {
        [format!(r#"{{"id":"{id}","title":"T","status":"open","type":"task","priority":2,"createdAt":"c","updatedAt":"u""#).as_bytes(),extra,b"}\n"].concat()
    }
    pub fn run(root: &Path, fix: bool) -> (i32, serde_json::Value) {
        let args = if fix {
            &["doctor", "--fix", "--json"][..]
        } else {
            &["doctor", "--json"][..]
        };
        let out = invoke(root, args);
        assert!(out.stderr.is_empty());
        (out.exit_code, serde_json::from_slice(&out.stdout).unwrap())
    }
    pub fn status<'a>(v: &'a serde_json::Value, name: &str) -> &'a str {
        v["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap()["status"]
            .as_str()
            .unwrap()
    }
    #[test]
    fn clean_project_and_malformed_fix() {
        let t = fixture();
        let (code, payload) = run(t.path(), false);
        assert_eq!(code, 0);
        assert_eq!(payload["checks"].as_array().unwrap().len(), 12);
        assert_eq!(
            payload["summary"],
            serde_json::json!({"pass":12,"warn":0,"fail":0})
        );
        assert_eq!(
            invoke(t.path(), &["doctor"]).stdout,
            b"\nMissions Doctor\n\n\n12 passed, 0 warning(s), 0 failure(s)\n"
        );
        let path = t.path().join(".luuna/missions/issues.jsonl");
        let valid = record("t", b"");
        fs::write(&path, [valid.as_slice(), b"NOT JSON\n"].concat()).unwrap();
        let (code, before) = run(t.path(), false);
        assert_eq!(code, 1);
        assert_eq!(status(&before, "jsonl-integrity"), "fail");
        let (code, after) = run(t.path(), true);
        assert_eq!(code, 0);
        assert_eq!(status(&after, "jsonl-integrity"), "pass");
        assert_eq!(fs::read(&path).unwrap(), valid);
        assert_eq!(
            after["fixed"],
            serde_json::json!(["Removed 1 malformed line(s) from issues.jsonl"])
        );
        fs::write(
            t.path().join(".luuna/missions/config.yaml"),
            "project: 2006-01-02\nversion: \"1\"\n",
        )
        .unwrap();
        let error = crate::command::run(&crate::command::Invocation::new(
            t.path(),
            ["doctor", "--json"],
        ))
        .unwrap_err();
        assert!(matches!(error, Error::Unsupported { .. }));
    }
    #[test]
    fn repairs_labels_extensions_and_close_metadata() {
        let t = fixture();
        let path = t.path().join(".luuna/missions/issues.jsonl");
        fs::write(&path,record("t",br#","labels":["manual","",1],"extensions":[],"closedAt":"old","closeReason":"old""#)).unwrap();
        let (code, before) = run(t.path(), false);
        assert_eq!(code, 1);
        assert_eq!(status(&before, "label-schema"), "warn");
        assert_eq!(status(&before, "extensions-schema"), "warn");
        let (code, after) = run(t.path(), true);
        assert_eq!(code, 0);
        assert_eq!(status(&after, "closed-fields-consistency"), "pass");
        let value: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(value["labels"], serde_json::json!(["manual"]));
        for key in ["extensions", "closedAt", "closeReason"] {
            assert!(value.get(key).is_none());
        }
        assert_eq!(
            after["fixed"],
            serde_json::json!([
                "Cleaned up invalid label entries",
                "Dropped malformed extensions fields",
                "Repaired status/close-metadata mismatches"
            ])
        );
    }
    #[test]
    fn dependency_fix_and_cycle_warning() {
        let t = fixture();
        let path = t.path().join(".luuna/missions/issues.jsonl");
        fs::write(
            &path,
            [
                record("a", br#","blockedBy":["b","missing"]"#),
                record("b", br#","blockedBy":["a"]"#),
            ]
            .concat(),
        )
        .unwrap();
        let (code, before) = run(t.path(), false);
        assert_eq!(code, 0);
        assert_eq!(status(&before, "referential-integrity"), "warn");
        assert_eq!(status(&before, "bidirectional-consistency"), "warn");
        assert_eq!(status(&before, "circular-dependencies"), "warn");
        let (code, after) = run(t.path(), true);
        assert_eq!(code, 0);
        assert_eq!(status(&after, "referential-integrity"), "pass");
        assert_eq!(status(&after, "bidirectional-consistency"), "pass");
        assert_eq!(status(&after, "circular-dependencies"), "warn");
        assert_eq!(
            after["fixed"],
            serde_json::json!([
                "Removed dangling dependency references",
                "Added missing bidirectional dependency links"
            ])
        );
    }
    #[test]
    fn removes_only_stale_locks() {
        use std::time::Duration;
        let t = fixture();
        let dir = t.path().join(".luuna/missions");
        let stale = dir.join("issues.jsonl.lock");
        let active = dir.join("templates.jsonl.lock");
        fs::write(&stale, b"").unwrap();
        fs::write(&active, b"active").unwrap();
        let old = SystemTime::now() - Duration::from_secs(40);
        fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(old))
            .unwrap();
        let (code, payload) = run(t.path(), true);
        assert_eq!(code, 0);
        assert_eq!(status(&payload, "stale-locks"), "warn");
        assert!(!stale.exists());
        assert_eq!(fs::read(active).unwrap(), b"active");
        assert_eq!(
            payload["fixed"],
            serde_json::json!(["Removed stale issues.jsonl.lock"])
        );
    }
}
