//! Plan every Go fix first; refuse silent loss before the first side effect.
use super::{
    doctor::{self, Check, Line, Object},
    lossy, stale_lock,
    template_record::{array as raw_array, string_end},
};
use crate::{
    command::Failure,
    config,
    error::Error,
    json::{self, Value as V},
    model,
    store::{DataFile, Store},
};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};
fn dedup(lines: &[Line]) -> (Vec<Object>, usize) {
    let mut result = Vec::new();
    let mut positions = HashMap::new();
    let mut parsed = 0;
    for line in lines {
        if line.error.is_some() {
            continue;
        }
        parsed += 1;
        let Some(object) = doctor::object(line.parsed.as_ref()) else {
            continue;
        };
        let Some(id) = doctor::text(object.get("id")) else {
            continue;
        };
        if let Some(&i) = positions.get(id) {
            result[i] = object.clone();
        } else {
            positions.insert(id, result.len());
            result.push(object.clone());
        }
    }
    (result, parsed)
}
fn objects_bytes(objects: &[Object]) -> Result<Vec<u8>, Failure> {
    let mut out = Vec::new();
    for object in objects {
        json::encode_map(&mut out, object, true).map_err(|e| Error::Encode {
            message: e.to_string(),
        })?;
        out.push(b'\n');
    }
    Ok(out)
}
fn string(value: Option<&V>) -> &str {
    doctor::text(value).unwrap_or_default()
}
fn array_owned(value: Option<&V>) -> Vec<V> {
    doctor::array(value).unwrap_or_default().to_vec()
}
fn mutate(objects: &mut [Object], name: &str, loss_hints: &mut Vec<(String, String)>) -> bool {
    let mut changed = false;
    match name {
        "referential-integrity" => {
            let ids = objects
                .iter()
                .filter_map(|o| doctor::text(o.get("id")))
                .map(str::to_owned)
                .collect::<HashSet<_>>();
            for o in objects {
                for field in ["blockedBy", "blocks"] {
                    if let Some(V::Array(values)) = o.get(field) {
                        let kept = values
                            .iter()
                            .filter_map(|v| doctor::text(Some(v)))
                            .filter(|s| ids.contains(*s))
                            .map(|s| V::String(s.into()))
                            .collect::<Vec<_>>();
                        if kept.len() != values.len() {
                            changed = true;
                        }
                        if kept.is_empty() {
                            o.remove(field);
                        } else {
                            o.insert(field.into(), V::Array(kept));
                        }
                    }
                }
            }
        }
        "bidirectional-consistency" => {
            let by_id = objects
                .iter()
                .enumerate()
                .filter_map(|(i, o)| doctor::text(o.get("id")).map(|id| (id.to_owned(), i)))
                .collect::<HashMap<_, _>>();
            for i in 0..objects.len() {
                let id = string(objects[i].get("id")).to_owned();
                for (field, back) in [("blockedBy", "blocks"), ("blocks", "blockedBy")] {
                    let refs = array_owned(objects[i].get(field));
                    for value in refs {
                        let reference = string(Some(&value));
                        if let Some(&j) = by_id.get(reference) {
                            let mut list = array_owned(objects[j].get(back));
                            if !list.iter().any(|v| doctor::text(Some(v)) == Some(&id)) {
                                if objects[j]
                                    .get(back)
                                    .is_some_and(|v| !matches!(v, V::Array(_) | V::Null))
                                {
                                    loss_hints.push((
                                        string(objects[j].get("id")).to_owned(),
                                        back.into(),
                                    ));
                                }
                                list.push(V::String(id.clone()));
                                objects[j].insert(back.into(), V::Array(list));
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
        "label-schema" => {
            for o in objects {
                if let Some(value) = o.get("labels") {
                    if let V::Array(values) = value {
                        let kept = values
                            .iter()
                            .filter_map(|v| doctor::text(Some(v)))
                            .filter(|s| !s.trim().is_empty())
                            .map(|s| V::String(s.into()))
                            .collect::<Vec<_>>();
                        if kept.len() != values.len() {
                            if kept.is_empty() {
                                o.remove("labels");
                            } else {
                                o.insert("labels".into(), V::Array(kept));
                            }
                            changed = true;
                        }
                    } else {
                        o.remove("labels");
                        changed = true;
                    }
                }
            }
        }
        "extensions-schema" => {
            for o in objects {
                if o.get("extensions")
                    .is_some_and(|v| !matches!(v, V::Object(_)))
                {
                    o.remove("extensions");
                    changed = true;
                }
            }
        }
        "closed-fields-consistency" => {
            for o in objects {
                if string(o.get("status")) != "closed" {
                    if o.remove("closedAt").is_some() {
                        changed = true;
                    }
                    if o.remove("closeReason").is_some() {
                        changed = true;
                    }
                } else if !doctor::truthy(o.get("closedAt")) {
                    if let Some(v) = o.get("updatedAt") {
                        o.insert("closedAt".into(), v.clone());
                    } else {
                        o.remove("closedAt");
                    }
                    changed = true;
                }
            }
        }
        _ => {}
    }
    changed
}
fn label(name: &str) -> &str {
    match name {
        "referential-integrity" => "Removed dangling dependency references",
        "bidirectional-consistency" => "Added missing bidirectional dependency links",
        "label-schema" => "Cleaned up invalid label entries",
        "extensions-schema" => "Dropped malformed extensions fields",
        "closed-fields-consistency" => "Repaired status/close-metadata mismatches",
        _ => "",
    }
}
// Operations retain Go's order, including no-op lock acquisitions. A stale
// lock is inspected when its Go fix runs, after any earlier lock admission.
enum Operation {
    File {
        kind: DataFile,
        before: Option<Vec<u8>>,
        after: Option<Vec<u8>>,
        fixed: Option<String>,
    },
    Locks,
    Attributes {
        path: PathBuf,
        before: Option<Vec<u8>>,
        after: Vec<u8>,
        fixed: String,
    },
}
fn value_end(raw: &[u8], start: usize) -> usize {
    if raw[start] == b'"' {
        return string_end(raw, start);
    }
    if matches!(raw[start], b'{' | b'[') {
        let (mut i, mut depth) = (start, 0usize);
        while i < raw.len() {
            match raw[i] {
                b'"' => {
                    i = string_end(raw, i);
                    continue;
                }
                b'{' | b'[' => depth += 1,
                b'}' | b']' => {
                    depth -= 1;
                    if depth == 0 {
                        return i + 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        return raw.len();
    }
    let mut i = start;
    while i < raw.len() && !matches!(raw[i], b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t') {
        i += 1;
    }
    i
}
fn ws(raw: &[u8], mut i: usize) -> usize {
    while i < raw.len() && matches!(raw[i], b' ' | b'\n' | b'\r' | b'\t') {
        i += 1;
    }
    i
}
fn pairs(raw: &[u8]) -> Vec<(&[u8], &[u8])> {
    let mut out = Vec::new();
    let mut i = ws(raw, 1);
    while i < raw.len() && raw[i] != b'}' {
        let end = string_end(raw, i);
        let key = &raw[i..end];
        i = ws(raw, end) + 1;
        i = ws(raw, i);
        let end = value_end(raw, i);
        out.push((key, &raw[i..end]));
        i = ws(raw, end);
        if raw.get(i) == Some(&b',') {
            i = ws(raw, i + 1);
        } else {
            break;
        }
    }
    out
}
fn decimal(text: &str) -> Option<(bool, String, i64)> {
    let (negative, text) = text.strip_prefix('-').map_or((false, text), |s| (true, s));
    let (base, exponent) = match text.find(['e', 'E']) {
        Some(i) => (&text[..i], text[i + 1..].parse::<i64>().ok()?),
        None => (text, 0),
    };
    let fraction = base.find('.').map_or(0, |i| base.len() - i - 1);
    let mut digits = base.replace('.', "").trim_start_matches('0').to_owned();
    if digits.is_empty() {
        return Some((false, "0".into(), 0));
    }
    let mut exponent = exponent.checked_sub(fraction as i64)?;
    while digits.ends_with('0') {
        digits.pop();
        exponent = exponent.checked_add(1)?;
    }
    Some((negative, digits, exponent))
}
/// Category order on a line: number, repeated key, then invalid string.
fn raw_problem(raw: &[u8]) -> Option<String> {
    let (mut number, mut repeated, mut invalid) = (None, None, None);
    let mut stack = vec![(raw, String::new())];
    while let Some((value, path)) = stack.pop() {
        match value.first() {
            Some(b'{') => {
                let entries = pairs(value);
                let mut keys = HashSet::new();
                let mut pending = Vec::new();
                for (key, value) in entries {
                    let decoded = json::unquote(key);
                    let child = if path.is_empty() {
                        decoded.clone()
                    } else {
                        format!("{path}.{decoded}")
                    };
                    if !keys.insert(decoded.clone()) && repeated.is_none() {
                        repeated = Some(format!("repeats the key \"{decoded}\""));
                    }
                    if invalid.is_none() {
                        if let Some(why) = lossy::string_problem(key) {
                            invalid = Some(format!(
                                "holds {why} at {child}, which the Go tool would write as U+FFFD"
                            ));
                        }
                    }
                    pending.push((value, child));
                }
                stack.extend(pending.into_iter().rev());
            }
            Some(b'[') => {
                if let Some(items) = raw_array(value) {
                    for (i, v) in items.into_iter().enumerate().rev() {
                        stack.push((v, format!("{path}.{i}")));
                    }
                }
            }
            Some(b'"') => {
                if invalid.is_none() {
                    if let Some(why) = lossy::string_problem(value) {
                        invalid = Some(format!(
                            "holds {why} at {path}, which the Go tool would write as U+FFFD"
                        ));
                    }
                }
            }
            Some(b'-' | b'0'..=b'9') => {
                if number.is_none() {
                    if let Ok(V::Number(n)) = V::decode(value) {
                        if let Ok(encoded) = json::format_float(n) {
                            let source = std::str::from_utf8(value).unwrap();
                            if decimal(source) != decimal(&encoded) {
                                number=Some(format!("holds the number {source}, which the Go tool would write as {encoded}"));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    number.or(repeated).or(invalid)
}
fn guard(path: &Path, bytes: &[u8], loss_hints: &[(String, String)]) -> Result<(), Failure> {
    let rows = doctor::lines(bytes);
    let mut last = HashMap::new();
    for line in &rows {
        if let Some(id) = doctor::text(doctor::field(line.parsed.as_ref(), "id")) {
            last.insert(id.to_owned(), line.number);
        }
    }
    for line in rows {
        if line.error.is_some() {
            continue;
        }
        if let Some(id) = doctor::text(doctor::field(line.parsed.as_ref(), "id")) {
            if last[id] != line.number {
                continue;
            }
        }
        let why = if doctor::object(line.parsed.as_ref()).is_none() {
            Some("is not a JSON object".into())
        } else if doctor::text(doctor::field(line.parsed.as_ref(), "id")).is_none() {
            Some("has no string id".into())
        } else {
            raw_problem(&line.raw)
        };
        if let Some(why) = why {
            return Err(Failure::message(format!("Refusing to fix {}: line {} {why}, and the Go tool would drop or change it without a message. Nothing was written.",path.display(),line.number)));
        }
        if let Some(id) = doctor::text(doctor::field(line.parsed.as_ref(), "id")) {
            if let Some((_, field)) = loss_hints.iter().find(|(target, _)| target == id) {
                return Err(Failure::message(format!("Refusing to fix {}: line {} holds a \"{field}\" value on issue \"{id}\" that is not an array, and fixing a bidirectional link would replace that value. Nothing was written.",path.display(),line.number)));
            }
        }
    }
    Ok(())
}
fn atomic(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
    let metadata = match fs::metadata(path) {
        Ok(m) => Some(m),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(Error::io("stat", path, e).into()),
    };
    let mut random = [0; 8];
    let (mut file, tmp) = loop {
        model::random_bytes(&mut random);
        let tmp = path.with_file_name(format!(
            "{}.tmp.{}",
            path.file_name().unwrap().to_string_lossy(),
            model::hex(&random)
        ));
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        match opts.open(&tmp) {
            Ok(f) => break (f, tmp),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(Error::io("create", tmp, e).into()),
        }
    };
    let result = (|| {
        file.write_all(bytes)
            .map_err(|e| Error::io("write", &tmp, e))?;
        if let Some(m) = metadata {
            file.set_permissions(m.permissions())
                .map_err(|e| Error::io("chmod", &tmp, e))?;
        }
        drop(file);
        fs::rename(&tmp, path).map_err(|e| Error::io("rename", path, e))
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result.map_err(Failure::from)
}
fn plan(store: &Store, checks: &[Check]) -> Result<Vec<Operation>, Failure> {
    let kinds = [DataFile::Issues, DataFile::Templates];
    let originals = kinds
        .iter()
        .map(|kind| doctor::read_optional(&store.path(*kind)))
        .collect::<Result<Vec<_>, _>>()?;
    let mut work = originals.clone();
    let mut ops = Vec::new();
    let mut object_rewrites = [false, false];
    let mut loss_hints = Vec::new();
    for check in checks {
        if check.status == "pass" || !check.fixable {
            continue;
        }
        match check.name {
            "jsonl-integrity" => {
                for i in 0..2 {
                    let rows = doctor::lines(work[i].as_deref().unwrap_or_default());
                    let valid = rows
                        .iter()
                        .filter(|r| r.error.is_none())
                        .collect::<Vec<_>>();
                    let (after, fixed) = if valid.len() == rows.len() {
                        (None, None)
                    } else {
                        let mut bytes = Vec::new();
                        for row in &valid {
                            bytes.extend_from_slice(&row.raw);
                            bytes.push(b'\n');
                        }
                        (
                            Some(bytes),
                            Some(format!(
                                "Removed {} malformed line(s) from {}",
                                rows.len() - valid.len(),
                                kinds[i].file_name()
                            )),
                        )
                    };
                    ops.push(Operation::File {
                        kind: kinds[i],
                        before: work[i].clone(),
                        after: after.clone(),
                        fixed,
                    });
                    if let Some(bytes) = after {
                        work[i] = Some(bytes);
                    }
                }
            }
            "duplicate-ids" => {
                for i in 0..2 {
                    let rows = doctor::lines(work[i].as_deref().unwrap_or_default());
                    let (objects, parsed) = dedup(&rows);
                    let (after, fixed) = if objects.len() >= parsed {
                        (None, None)
                    } else {
                        object_rewrites[i] = true;
                        (
                            Some(objects_bytes(&objects)?),
                            Some(format!("Deduplicated {}", kinds[i].file_name())),
                        )
                    };
                    ops.push(Operation::File {
                        kind: kinds[i],
                        before: work[i].clone(),
                        after: after.clone(),
                        fixed,
                    });
                    if let Some(bytes) = after {
                        work[i] = Some(bytes);
                    }
                }
            }
            "referential-integrity"
            | "bidirectional-consistency"
            | "label-schema"
            | "extensions-schema"
            | "closed-fields-consistency" => {
                let rows = doctor::lines(work[0].as_deref().unwrap_or_default());
                let (mut objects, _) = dedup(&rows);
                let (after, fixed) = if mutate(&mut objects, check.name, &mut loss_hints) {
                    object_rewrites[0] = true;
                    (
                        Some(objects_bytes(&objects)?),
                        Some(label(check.name).into()),
                    )
                } else {
                    (None, None)
                };
                ops.push(Operation::File {
                    kind: DataFile::Issues,
                    before: work[0].clone(),
                    after: after.clone(),
                    fixed,
                });
                if let Some(bytes) = after {
                    work[0] = Some(bytes);
                }
            }
            "stale-locks" => ops.push(Operation::Locks),
            "gitattributes" => {
                let path = config::project_root(store.dir()).join(".gitattributes");
                let before = doctor::read_optional(&path)?;
                let mut after = before.clone().unwrap_or_default();
                let missing = doctor::MERGE_LINES
                    .iter()
                    .filter(|line| !after.windows(line.len()).any(|s| s == line.as_bytes()))
                    .copied()
                    .collect::<Vec<_>>();
                if !missing.is_empty() {
                    if !after.is_empty() && !after.ends_with(b"\n") {
                        after.push(b'\n');
                    }
                    for line in missing {
                        after.extend_from_slice(line.as_bytes());
                        after.push(b'\n');
                    }
                    let fixed = if before.is_none() {
                        "Created .gitattributes with merge=union entries"
                    } else {
                        "Added missing merge=union entries to .gitattributes"
                    }
                    .into();
                    ops.push(Operation::Attributes {
                        path,
                        before,
                        after,
                        fixed,
                    });
                }
            }
            _ => {}
        }
    }
    for i in 0..2 {
        if object_rewrites[i] {
            guard(
                &store.path(kinds[i]),
                originals[i].as_deref().unwrap_or_default(),
                if i == 0 { &loss_hints } else { &[] },
            )?;
        }
    }
    Ok(ops)
}
pub(super) fn apply(store: &Store, checks: &[Check]) -> Result<Vec<String>, Failure> {
    let ops = plan(store, checks)?;
    let mut fixed = Vec::new();
    for op in ops {
        match op {
            Operation::File {
                kind,
                before,
                after,
                fixed: message,
            } => {
                let _lock = store.lock(kind)?;
                let path = store.path(kind);
                if doctor::read_optional(&path)? != before {
                    return Err(Failure::message(
                        "Store changed while planning doctor fixes; retry the command",
                    ));
                }
                if let Some(bytes) = after {
                    atomic(&path, &bytes)?;
                }
                if let Some(message) = message {
                    fixed.push(message);
                }
            }
            Operation::Locks => {
                for kind in [DataFile::Issues, DataFile::Templates] {
                    let path = store.dir().join(format!("{}.lock", kind.file_name()));
                    if let Ok(meta) = fs::metadata(&path) {
                        if doctor::age(&meta).is_some_and(|s| s > 30.0)
                            && stale_lock::reclaim_stale_lock(&path, &meta)
                        {
                            fixed.push(format!("Removed stale {}.lock", kind.file_name()));
                        }
                    }
                }
            }
            Operation::Attributes {
                path,
                before,
                after,
                fixed: message,
            } => {
                if doctor::read_optional(&path)? != before {
                    return Err(Failure::message(
                        "Gitattributes changed while planning doctor fixes; retry the command",
                    ));
                }
                let mut opts = fs::OpenOptions::new();
                opts.write(true).create(true).truncate(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    opts.mode(0o644);
                }
                let mut file = opts.open(&path).map_err(|e| Error::io("write", &path, e))?;
                file.write_all(&after)
                    .map_err(|e| Error::io("write", &path, e))?;
                fixed.push(message);
            }
        }
    }
    Ok(fixed)
}

#[cfg(test)]
mod tests {
    use super::super::doctor::tests::{fixture, record, run};
    use super::*;
    use crate::config_tree::tree::test_support::invoke;
    fn preserve(issues: &[u8], templates: &[u8], name: &str, line: usize, why: &str) {
        for mode in [0o644, 0o600] {
            let t = fixture();
            let dir = t.path().join(".luuna/missions");
            let ip = dir.join("issues.jsonl");
            let tp = dir.join("templates.jsonl");
            fs::write(&ip, issues).unwrap();
            fs::write(&tp, templates).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                for file in [&ip, &tp] {
                    fs::set_permissions(file, fs::Permissions::from_mode(mode)).unwrap();
                }
            }
            #[cfg(not(unix))]
            let _ = mode;
            let im = fs::metadata(&ip).unwrap().permissions();
            let tm = fs::metadata(&tp).unwrap().permissions();
            let attrs = t.path().join(".gitattributes");
            fs::write(&attrs, b"keep attributes").unwrap();
            let lock = dir.join("issues.jsonl.lock");
            fs::write(&lock, b"keep stale lock").unwrap();
            let old = std::time::SystemTime::now() - std::time::Duration::from_secs(40);
            fs::File::options()
                .write(true)
                .open(&lock)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(old))
                .unwrap();
            let lock_time = fs::metadata(&lock).unwrap().modified().unwrap();
            let out = invoke(t.path(), &["doctor", "--fix"]);
            assert_eq!(out.exit_code, 1);
            assert!(out.stdout.is_empty());
            let file = dir.join(name);
            assert_eq!(out.stderr,format!("Error: Refusing to fix {}: line {line} {why}, and the Go tool would drop or change it without a message. Nothing was written.\n",file.display()).as_bytes());
            assert_eq!(fs::read(&ip).unwrap(), issues);
            assert_eq!(fs::read(&tp).unwrap(), templates);
            assert_eq!(fs::metadata(&ip).unwrap().permissions(), im);
            assert_eq!(fs::metadata(&tp).unwrap().permissions(), tm);
            assert_eq!(fs::read(attrs).unwrap(), b"keep attributes");
            assert_eq!(fs::read(&lock).unwrap(), b"keep stale lock");
            assert_eq!(fs::metadata(lock).unwrap().modified().unwrap(), lock_time);
            assert_eq!(fs::read_dir(dir).unwrap().count(), 4);
        }
    }
    fn dangling(extra: &[u8]) -> Vec<u8> {
        record(
            "t",
            &[br#","blockedBy":["missing"]"#.as_slice(), extra].concat(),
        )
    }
    #[test]
    fn fix_refuses_a_line_that_is_not_an_object_and_changes_nothing() {
        preserve(
            &[dangling(b""), b"[1]\n".to_vec()].concat(),
            b"",
            "issues.jsonl",
            2,
            "is not a JSON object",
        );
        let t = fixture();
        let path = t.path().join(".luuna/missions/issues.jsonl");
        fs::write(&path, b"[1]\n").unwrap();
        let (code, payload) = run(t.path(), true);
        assert_eq!(code, 1);
        assert_eq!(payload["success"], false);
        assert_eq!(fs::read(path).unwrap(), b"[1]\n");
    }
    #[test]
    fn fix_refuses_a_record_without_an_id_and_changes_nothing() {
        preserve(
            &[dangling(b""), b"{\"title\":\"x\"}\n".to_vec()].concat(),
            b"",
            "issues.jsonl",
            2,
            "has no string id",
        );
    }
    #[test]
    fn fix_refuses_a_number_float64_would_change_and_changes_nothing() {
        preserve(
            &dangling(br#","n":9007199254740993"#),
            b"",
            "issues.jsonl",
            1,
            "holds the number 9007199254740993, which the Go tool would write as 9007199254740992",
        );
    }
    #[test]
    fn fix_refuses_a_repeated_key_and_changes_nothing() {
        preserve(
            &dangling(br#","title":"second""#),
            b"",
            "issues.jsonl",
            1,
            "repeats the key \"title\"",
        );
    }
    #[test]
    fn fix_refuses_all_four_together_naming_the_first() {
        preserve(
            &[
                dangling(br#","n":9007199254740993,"title":"second""#),
                b"[1]\n{\"title\":\"x\"}\n".to_vec(),
            ]
            .concat(),
            b"",
            "issues.jsonl",
            1,
            "holds the number 9007199254740993, which the Go tool would write as 9007199254740992",
        );
    }
    #[test]
    fn fix_refuses_invalid_utf8_or_an_unpaired_surrogate_and_changes_nothing() {
        let a=b"{\"id\":\"t\",\"title\":\"x\xff\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"c\",\"updatedAt\":\"u\",\"blockedBy\":[\"missing\"]}\n";
        preserve(
            a,
            b"",
            "issues.jsonl",
            1,
            "holds invalid UTF-8 at title, which the Go tool would write as U+FFFD",
        );
        let a=b"{\"id\":\"t\",\"title\":\"x\\ud800\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"c\",\"updatedAt\":\"u\",\"blockedBy\":[\"missing\"]}\n";
        preserve(
            a,
            b"",
            "issues.jsonl",
            1,
            "holds the unpaired escape \\ud800 at title, which the Go tool would write as U+FFFD",
        );
        preserve(
            &dangling(b",\"note\":\"x\xff\""),
            b"",
            "issues.jsonl",
            1,
            "holds invalid UTF-8 at note, which the Go tool would write as U+FFFD",
        );
        preserve(b"",b"{\"id\":\"tpl-1\",\"name\":\"old\",\"n\":9007199254740993,\"steps\":[]}\n{\"id\":\"tpl-1\",\"name\":\"u\\ud800\",\"steps\":[]}\n","templates.jsonl",2,"holds the unpaired escape \\ud800 at name, which the Go tool would write as U+FFFD");
    }
    #[test]
    fn fix_deduplicates_templates_as_go_does() {
        let t = fixture();
        let path = t.path().join(".luuna/missions/templates.jsonl");
        fs::write(&path,b"{\"id\":\"tpl-1\",\"name\":\"old\",\"n\":9007199254740993,\"steps\":[]}\n{\"id\":\"tpl-1\",\"name\":\"u\",\"steps\":[]}\n").unwrap();
        let (code, payload) = run(t.path(), true);
        assert_eq!(code, 0);
        assert_eq!(
            payload["fixed"],
            serde_json::json!(["Deduplicated templates.jsonl"])
        );
        assert_eq!(
            fs::read(path).unwrap(),
            b"{\"id\":\"tpl-1\",\"name\":\"u\",\"steps\":[]}\n"
        );
    }
    #[test]
    fn fix_leaves_the_parent_of_the_temporary_project_unchanged() {
        let p = tempfile::tempdir().unwrap();
        let sentinel = p.path().join("sentinel");
        let attrs = p.path().join(".gitattributes");
        fs::write(&sentinel, b"parent").unwrap();
        fs::write(&attrs, b"parent attributes").unwrap();
        let sm = fs::metadata(&sentinel).unwrap().permissions();
        let am = fs::metadata(&attrs).unwrap().permissions();
        let root = p.path().join("project");
        crate::config_tree::tree::test_support::store(&root, "project: demo\nversion: \"1\"\n");
        fs::write(root.join(".luuna/missions/issues.jsonl"), dangling(b"")).unwrap();
        fs::write(root.join(".gitattributes"), b"project").unwrap();
        assert_eq!(run(&root, true).0, 0);
        assert!(fs::read_to_string(root.join(".gitattributes"))
            .unwrap()
            .contains("merge=union"));
        assert_eq!(fs::read(sentinel).unwrap(), b"parent");
        assert_eq!(fs::read(&attrs).unwrap(), b"parent attributes");
        assert_eq!(
            fs::metadata(p.path().join("sentinel"))
                .unwrap()
                .permissions(),
            sm
        );
        assert_eq!(fs::metadata(attrs).unwrap().permissions(), am);
        assert_eq!(fs::read_dir(p.path()).unwrap().count(), 3);
        assert!(!p.path().join(".luuna").exists());
    }
    #[test]
    fn fix_refuses_a_non_array_reciprocal_field_and_changes_nothing() {
        for (field, target, first, second, line) in [
            (
                "blocks",
                "b",
                br#","blockedBy":["b"]"#.as_slice(),
                br#","blocks":"keep me""#.as_slice(),
                2,
            ),
            (
                "blockedBy",
                "a",
                br#","blockedBy":"keep me""#.as_slice(),
                br#","blocks":["a"]"#.as_slice(),
                1,
            ),
        ] {
            for mode in [0o644, 0o600] {
                let t = fixture();
                let path = t.path().join(".luuna/missions/issues.jsonl");
                let bytes = [record("a", first), record("b", second)].concat();
                fs::write(&path, &bytes).unwrap();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
                }
                #[cfg(not(unix))]
                let _ = mode;
                let permissions = fs::metadata(&path).unwrap().permissions();
                let out = invoke(t.path(), &["doctor", "--fix"]);
                assert_eq!(out.exit_code, 1);
                assert_eq!(out.stderr,format!("Error: Refusing to fix {}: line {line} holds a \"{field}\" value on issue \"{target}\" that is not an array, and fixing a bidirectional link would replace that value. Nothing was written.\n",path.display()).replace("\\\"","\"").as_bytes());
                assert_eq!(fs::read(&path).unwrap(), bytes);
                assert_eq!(fs::metadata(&path).unwrap().permissions(), permissions);
            }
            for old in [b"null".as_slice(), b"[]"] {
                let t = fixture();
                let (a, b) = if field == "blocks" {
                    (
                        record("a", br#","blockedBy":["b"]"#),
                        record("b", &[b",\"blocks\":", old].concat()),
                    )
                } else {
                    (
                        record("a", &[b",\"blockedBy\":", old].concat()),
                        record("b", br#","blocks":["a"]"#),
                    )
                };
                fs::write(
                    t.path().join(".luuna/missions/issues.jsonl"),
                    [a, b].concat(),
                )
                .unwrap();
                assert_eq!(run(t.path(), true).0, 0);
            }
        }
    }
}
