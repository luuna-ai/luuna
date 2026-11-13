//! Pinning retains the exact record lines used by the selection and refusal check.
use super::{AuthoringError as E, YamlValue};
use crate::{
    gofmt,
    gojson::{self, GoValue},
    prompts, store, Beacon, BeaconError, Prompt,
};
use std::{
    collections::HashMap,
    fs,
    io::{self, Read},
};
const PROMPT_FIELDS: &[&str] = &[
    "id",
    "name",
    "description",
    "version",
    "sections",
    "extends",
    "mixins",
    "tags",
    "schema",
    "emitAs",
    "emitDir",
    "pinned",
    "frontmatter",
    "atlas",
    "extends_atlas",
    "status",
    "createdAt",
    "updatedAt",
];
const SECTION_FIELDS: &[&str] = &["name", "body", "required"];
const ATLAS_FIELDS: &[&str] = &["prime", "budget", "on_empty"];
const PRIME_FIELDS: &[&str] = &["domains", "files"];
#[derive(Clone, Copy)]
enum Shape {
    Unknown,
    Prompt,
    Section,
    Atlas,
    Prime,
    Free,
    ArraySection,
    ArrayFree,
    Scalar(bool),
}
fn shape(field: &str, parent: Shape) -> Shape {
    match (parent, field) {
        (Shape::Prompt, "sections") => Shape::ArraySection,
        (Shape::Prompt, "frontmatter") => Shape::Free,
        (Shape::Prompt, "atlas" | "mulch") => Shape::Atlas,
        (Shape::Atlas, "prime") => Shape::Prime,
        (Shape::Prompt, "mixins" | "tags") | (Shape::Prime, "domains" | "files") => {
            Shape::ArrayFree
        }
        (Shape::Prompt, "pinned" | "extends_atlas" | "extends_mulch")
        | (Shape::Section, "required") => Shape::Scalar(false),
        _ => Shape::Scalar(true),
    }
}
fn joined(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.into()
    } else {
        format!("{path}.{key}")
    }
}
impl Beacon {
    pub fn pin(&self, reference: &str) -> Result<Prompt, BeaconError> {
        self.set_pin(reference, true)
    }
    pub fn unpin(&self, reference: &str) -> Result<Prompt, BeaconError> {
        self.set_pin(reference, false)
    }
    fn set_pin(&self, reference: &str, set: bool) -> Result<Prompt, BeaconError> {
        self.migrate_legacy()?;
        if reference.is_empty() {
            return Err(BeaconError::PromptNameRequired);
        }
        let (name, version) = reference
            .split_once('@')
            .map_or((reference, None), |(n, v)| (n, Some(v)));
        if set && version.is_none() {
            return Err(E::ExpectedNameAtVersion.into());
        }
        let path = self.prompts_path()?;
        let bytes = match fs::File::open(&path) {
            Ok(mut file) => {
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)
                    .map_err(|e| BeaconError::os("read", &path, e))?;
                bytes
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(BeaconError::os("open", &path, e)),
        };
        let mut records = Vec::new();
        let mut lines = Vec::new();
        for (index, line) in bytes.split(|b| *b == b'\n').enumerate() {
            if std::str::from_utf8(line).is_ok_and(|text| text.trim().is_empty()) {
                continue;
            }
            records.push(Prompt::from_go_json(line).map_err(|message| {
                BeaconError::InvalidJsonl {
                    line: index + 1,
                    path: path.clone(),
                    message,
                }
            })?);
            lines.push(line)
        }
        let current = prompts::current(records.clone());
        let mut prompt = current[prompts::find(&current, name)?].clone();
        let pinned = if set {
            let raw = version.unwrap();
            let wanted =
                gofmt::sscan_int(raw).ok_or_else(|| BeaconError::InvalidVersion(raw.into()))?;
            if !records
                .iter()
                .any(|p| p.id == prompt.id && p.version == wanted)
            {
                return Err(E::VersionNotFound(wanted).into());
            }
            Some(wanted)
        } else {
            None
        };
        for (record, line) in records.iter().zip(lines) {
            if record == &prompt {
                let raw = gojson::parse(line).map_err(BeaconError::Yaml)?;
                let checked = check(&raw, &prompt);
                drain(raw);
                checked?
            }
        }
        prompt.pinned = pinned;
        prompt.version += 1;
        prompt.updated_at = self.now_text();
        store::append_jsonl(&path, &prompt)?;
        Ok(prompt)
    }
}
fn check(raw: &GoValue, prompt: &Prompt) -> Result<(), BeaconError> {
    let mut work = vec![(raw, Shape::Prompt, String::new())];
    while let Some((value, context, path)) = work.pop() {
        if matches!(context, Shape::Unknown) {
            return Err(E::PinRecordKeyUnknown(prompt.name.clone(), path).into());
        }
        match value {
            GoValue::Object(entries) => {
                let fields = match context {
                    Shape::Prompt => PROMPT_FIELDS,
                    Shape::Section => SECTION_FIELDS,
                    Shape::Atlas => ATLAS_FIELDS,
                    Shape::Prime => PRIME_FIELDS,
                    _ => &[],
                };
                let mut seen: HashMap<String, Vec<&GoValue>> = HashMap::new();
                let mut children = Vec::new();
                for (index, (key, value)) in entries.iter().enumerate() {
                    let canonical = if fields.is_empty() {
                        key.as_str()
                    } else if let Some(field) = gojson::field(key, fields) {
                        field
                    } else if matches!(context, Shape::Prompt)
                        && ((key == "mulch" && legacy_used(entries, "atlas"))
                            || (key == "extends_mulch" && legacy_used(entries, "extends_atlas")))
                    {
                        if entries[index + 1..].iter().any(|(later, _)| later == key) {
                            let last = &entries
                                .iter()
                                .rev()
                                .find(|(later, _)| later == key)
                                .unwrap()
                                .1;
                            if !matches!(value, GoValue::Null) && !equal(value, last)? {
                                return Err(E::PinRecordKeyRepeated(
                                    prompt.name.clone(),
                                    joined(&path, key),
                                )
                                .into());
                            }
                            continue;
                        }
                        key.as_str()
                    } else {
                        children.push((value, Shape::Unknown, joined(&path, key)));
                        continue;
                    };
                    let next = if matches!(context, Shape::Free | Shape::ArrayFree) {
                        match value {
                            GoValue::Array(_) => Shape::ArrayFree,
                            GoValue::Object(_) => Shape::Free,
                            _ => Shape::Scalar(false),
                        }
                    } else {
                        shape(canonical, context)
                    };
                    let prior = seen.entry(canonical.into()).or_default();
                    for &old in prior.iter() {
                        repeat(
                            old,
                            value,
                            if matches!(context, Shape::Free) {
                                Shape::Scalar(false)
                            } else {
                                next
                            },
                            &joined(&path, key),
                            &prompt.name,
                        )?
                    }
                    // Bound objects merge into the previous object. Keep all contributors
                    // so a disjoint intermediate object cannot conceal an overwritten key.
                    if !matches!(value, GoValue::Null) {
                        if matches!(
                            next,
                            Shape::Free | Shape::Atlas | Shape::Prime | Shape::ArraySection
                        ) {
                            if !prior.iter().any(|old| equal(old, value).unwrap_or(false)) {
                                prior.push(value)
                            }
                        } else {
                            prior.clear();
                            prior.push(value)
                        }
                    } else if !matches!(next, Shape::Scalar(true)) {
                        prior.clear();
                        prior.push(value)
                    }
                    children.push((value, next, joined(&path, key)));
                }
                work.extend(children.into_iter().rev());
            }
            GoValue::Array(values) => {
                let child = if matches!(context, Shape::ArraySection) {
                    Shape::Section
                } else {
                    Shape::Free
                };
                for (index, v) in values.iter().enumerate().rev() {
                    work.push((v, child, joined(&path, &index.to_string())))
                }
            }
            _ => {}
        }
    }
    Ok(())
}
fn legacy_used(entries: &[(String, GoValue)], name: &str) -> bool {
    entries
        .iter()
        .rev()
        .find(|(key, _)| gojson::field(key, PROMPT_FIELDS) == Some(name))
        .is_none_or(|(_, v)| matches!(v, GoValue::Null))
}
fn repeat(
    old: &GoValue,
    new: &GoValue,
    context: Shape,
    path: &str,
    name: &str,
) -> Result<(), BeaconError> {
    let mut work = vec![(old, new, context, path.to_owned())];
    while let Some((old, new, context, path)) = work.pop() {
        if matches!(old, GoValue::Null) {
            continue;
        }
        if matches!(new, GoValue::Null) && matches!(context, Shape::Scalar(true)) {
            continue;
        }
        match (old, new, context) {
            (
                GoValue::Object(old),
                GoValue::Object(new),
                Shape::Prompt | Shape::Section | Shape::Atlas | Shape::Prime | Shape::Free,
            ) => {
                // Maps at a bound object field merge; nested free-map values replace.
                let fields = match context {
                    Shape::Section => SECTION_FIELDS,
                    Shape::Atlas => ATLAS_FIELDS,
                    Shape::Prime => PRIME_FIELDS,
                    Shape::Prompt => PROMPT_FIELDS,
                    _ => &[],
                };
                for (key, value) in old {
                    let canonical = gojson::field(key, fields).unwrap_or(key);
                    if let Some((_, new)) = new
                        .iter()
                        .rev()
                        .find(|(k, _)| gojson::field(k, fields).unwrap_or(k) == canonical)
                    {
                        let next = if matches!(context, Shape::Free) {
                            Shape::Scalar(false)
                        } else {
                            shape(canonical, context)
                        };
                        work.push((value, new, next, joined(&path, key)))
                    }
                }
            }
            (GoValue::Array(old), GoValue::Array(new), Shape::ArraySection | Shape::ArrayFree) => {
                for (index, value) in old.iter().enumerate() {
                    if let Some(new) = new.get(index) {
                        work.push((
                            value,
                            new,
                            if matches!(context, Shape::ArraySection) {
                                Shape::Section
                            } else {
                                Shape::Scalar(true)
                            },
                            joined(&path, &index.to_string()),
                        ))
                    } else if !matches!(value, GoValue::Null) {
                        return Err(E::PinRecordKeyRepeated(
                            name.into(),
                            joined(&path, &index.to_string()),
                        )
                        .into());
                    }
                }
            }
            _ => {
                if !equal(old, new)? {
                    return Err(E::PinRecordKeyRepeated(name.into(), path).into());
                }
            }
        }
    }
    Ok(())
}
fn equal(a: &GoValue, b: &GoValue) -> Result<bool, BeaconError> {
    let a = gojson::to_any(a).map_err(BeaconError::Yaml)?;
    let b = gojson::to_any(b).map_err(BeaconError::Yaml)?;
    Ok(YamlValue::from_json(&a) == YamlValue::from_json(&b))
}
fn drain(value: GoValue) {
    let mut work = vec![value];
    while let Some(v) = work.pop() {
        match v {
            GoValue::Array(v) => work.extend(v),
            GoValue::Object(v) => work.extend(v.into_iter().map(|(_, v)| v)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(bytes: &[u8]) -> (tempfile::TempDir, Beacon) {
        let dir = tempfile::tempdir().unwrap();
        let b = Beacon::open(dir.path()).unwrap();
        b.init().unwrap();
        fs::write(b.store_dir().join("prompts.jsonl"), bytes).unwrap();
        (dir, b)
    }
    const RECORD: &str =
        "{\"id\":\"id\",\"name\":\"p\",\"version\":1,\"sections\":[],\"status\":\"active\"}\n";
    #[test]
    fn private_reader_matches_foundation_unicode_blank_lines() {
        let source = format!("\u{2003}\n{RECORD}");
        let (_d, b) = fixture(source.as_bytes());
        let foundation: Vec<Prompt> = store::read_jsonl(&b.prompts_path().unwrap()).unwrap();
        let pinned = b.pin("p@1").unwrap();
        assert_eq!(pinned.id, foundation[0].id);
        assert_eq!(pinned.version, 2);
        assert!(fs::read_to_string(b.prompts_path().unwrap())
            .unwrap()
            .starts_with(&source))
    }
    #[test]
    fn private_reader_matches_foundation_invalid_json_line() {
        let (_d, b) = fixture(b"\n\ninvalid\n");
        let expected = store::read_jsonl::<Prompt>(&b.prompts_path().unwrap())
            .unwrap_err()
            .to_string();
        assert_eq!(b.pin("p@1").unwrap_err().to_string(), expected)
    }
    #[test]
    fn private_reader_matches_foundation_read_error() {
        let (_d, b) = fixture(b"");
        let path = b.prompts_path().unwrap();
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let expected = store::read_jsonl::<Prompt>(&path).unwrap_err().to_string();
        assert_eq!(b.pin("p@1").unwrap_err().to_string(), expected)
    }
    #[test]
    fn missing_version_precedes_unknown_key_refusal() {
        let source = RECORD.replace("\"sections\":[]", "\"unknown\":5,\"sections\":[]");
        let (_d, b) = fixture(source.as_bytes());
        assert!(matches!(
            b.pin("p@9"),
            Err(BeaconError::Authoring(E::VersionNotFound(9)))
        ));
        assert_eq!(
            fs::read(b.prompts_path().unwrap()).unwrap(),
            source.as_bytes()
        )
    }
    #[test]
    fn three_object_contributors_cannot_hide_an_overwritten_field() {
        let source=RECORD.replace("\"sections\":[]","\"atlas\":{\"on_empty\":\"old\"},\"atlas\":{\"budget\":1},\"atlas\":{\"on_empty\":\"new\"},\"sections\":[]");
        let (_d, b) = fixture(source.as_bytes());
        assert!(
            matches!(b.pin("p@1"),Err(BeaconError::Authoring(E::PinRecordKeyRepeated(_,ref path)))if path=="atlas.on_empty")
        );
        assert_eq!(
            fs::read(b.prompts_path().unwrap()).unwrap(),
            source.as_bytes()
        )
    }
    #[test]
    fn tree_cycle_is_refused_without_mutation() {
        let source = RECORD.replace("\"sections\":[]", "\"extends\":\"p\",\"sections\":[]");
        let (_d, b) = fixture(source.as_bytes());
        assert!(matches!(b.tree("p"),Err(BeaconError::CircularInheritance(ref name))if name=="p"));
        assert_eq!(
            fs::read(b.prompts_path().unwrap()).unwrap(),
            source.as_bytes()
        )
    }
    #[test]
    fn diff_failed_second_scan_reuses_first_version() {
        let (_d, b) = fixture(RECORD.as_bytes());
        assert!(b.diff("p", "1", "bad").unwrap().is_empty())
    }
    #[test]
    fn stats_counts_repeated_tags_and_only_current_versions() {
        let second = RECORD.replace("\"version\":1", "\"version\":2,\"tags\":[\"x\",\"x\"]");
        let (_d, b) = fixture(format!("{RECORD}{second}").as_bytes());
        let stats = b.stats().unwrap();
        assert_eq!(stats.total, 1);
        assert_eq!(stats.by_tag.get("x"), Some(&2));
        assert_eq!(stats.by_status.get("active"), Some(&1))
    }
}
