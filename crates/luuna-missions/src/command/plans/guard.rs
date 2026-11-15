//! Refuse demonstrated Go loss before any planned store write.
use super::{raw, record::Plan};
use crate::{
    error::{Error, Result},
    json,
    store::{trim_space, DataFile, Record, Store},
};
use std::{
    collections::{HashMap, HashSet},
    fs, io,
};
fn refusal(text: String) -> Error {
    Error::Command { message: text }
}
pub(super) fn plan_rewrite(store: &Store) -> Result<()> {
    let path = store.path(DataFile::Plans);
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => vec![],
        Err(e) => return Err(Error::io("read", &path, e)),
    };
    let mut ids = HashMap::new();
    for (i, line) in bytes.split(|b| *b == b'\n').enumerate() {
        let line = trim_space(line);
        if line.is_empty() {
            continue;
        }
        let n = i + 1;
        let Some(plan) = Plan::from_line(line) else {
            return Err(refusal(format!("Refusing to rewrite {}: line {n} is not a plan record the Go tool can read, and rewriting the file would drop it. Nothing was written.",path.display())));
        };
        let mut keys = HashSet::new();
        for key in raw::keys(line) {
            let key = json::unquote(key);
            if !keys.insert(key.clone()) {
                return Err(refusal(format!("Refusing to rewrite {}: line {n} repeats the key \"{key}\", and rewriting the file would drop its earlier value. Nothing was written.",path.display())));
            }
        }
        if let Some(first) = ids.insert(plan.id.clone(), n) {
            return Err(refusal(format!("Refusing to rewrite {}: line {n} repeats the id \"{}\" of line {first}, and rewriting the file would drop the earlier record. Nothing was written.",path.display(),plan.id)));
        }
    }
    Ok(())
}
/// `ignored` contains only paths the command explicitly replaces. Paths are
/// components, so a custom section named `steps.0` is never confused with steps.
pub(super) fn section_change(store: &Store, after: &Plan, ignored: &[Vec<String>]) -> Result<()> {
    if !after.sections_changed()? {
        return Ok(());
    }
    let Some(raw) = after.raw_sections() else {
        return Ok(());
    };
    let path = store.path(DataFile::Plans);
    if raw != b"null" && raw.first() != Some(&b'{') {
        return Err(refusal(format!("Refusing to rewrite {}: plan \"{}\" has sections that are not an object, and editing a section would replace that value. Nothing was written.",path.display(),after.id)));
    }
    if let Some(problem) = raw::problem(raw, ignored) {
        return Err(refusal(format!("Refusing to rewrite {}: plan \"{}\" sections {problem}, and the Go tool would change it without a message. Nothing was written.",path.display(),after.id)));
    }
    Ok(())
}
pub(super) fn step_object(store: &Store, plan: &Plan, index: usize) -> Result<()> {
    if let Some(crate::json::Value::Array(steps)) = plan.section("steps") {
        if steps
            .get(index)
            .is_some_and(|s| !matches!(s, crate::json::Value::Object(_)))
        {
            return Err(refusal(format!("Refusing to rewrite {}: plan \"{}\" steps item {index} is not an object, and editing its fields would replace that value. Nothing was written.",store.path(DataFile::Plans).display(),plan.id)));
        }
    }
    Ok(())
}

pub(super) fn array_changes(store: &Store, after: &Plan) -> Result<()> {
    for key in ["children", "adoptedChildren"] {
        if !after.field_changed(key)? {
            continue;
        }
        let Some(source) = after.raw_field(key) else {
            continue;
        };
        if source == b"null" {
            continue;
        }
        let problem = match raw::array(source) {
            None => Some("is not a list".into()),
            Some(items) => items.iter().enumerate().find_map(|(i, item)| {
                if item.first() != Some(&b'"') {
                    Some(format!("item {i} is not a string"))
                } else {
                    raw::string_problem(item).map(|why| format!("item {i} holds {why}"))
                }
            }),
        };
        if let Some(problem) = problem {
            return Err(refusal(format!("Refusing to rewrite {}: plan \"{}\" {key} {problem}, and the Go tool would replace it without a message. Nothing was written.",store.path(DataFile::Plans).display(),after.id)));
        }
    }
    Ok(())
}

/// Fresh draft replacement is an own change for standard identity/state fields,
/// never permission to discard unrelated metadata, body text or old child links.
pub(super) fn replace_draft(store: &Store, old: &Plan, explicit_blueprint: bool) -> Result<()> {
    let refuse = |why: &str| {
        refusal(format!("Refusing to replace draft \"{}\" in {}: {why}, and the Go tool would discard it without a message. Nothing was written.",old.id,store.path(DataFile::Plans).display()))
    };
    let own = [
        "id",
        "seed",
        "template",
        "name",
        "status",
        "revision",
        "sections",
        "children",
        "createdAt",
        "updatedAt",
    ];
    for key in old.original_keys() {
        if !own.contains(&key.as_str()) {
            return Err(refuse(&format!("it holds the unrelated key \"{key}\"")));
        }
    }
    if let Some(raw) = old.raw_field("children") {
        if raw != b"null" && raw::array(raw).is_none_or(|a| !a.is_empty()) {
            return Err(refuse("it holds existing child-link data"));
        }
    }
    if !explicit_blueprint {
        if let Some(raw) = old.raw_sections().filter(|r| *r != b"null") {
            if raw.first() != Some(&b'{') {
                return Err(refuse("it holds a non-object sections value"));
            }
            let mut seen = HashSet::new();
            for (key, value) in raw::pairs(raw) {
                let key = json::unquote(key);
                if !seen.insert(key.clone())
                    || key != "steps"
                    || value != b"null" && raw::array(value).is_none_or(|a| !a.is_empty())
                {
                    return Err(refuse("it holds existing blueprint body data"));
                }
            }
        }
    }
    Ok(())
}
/// Only backref mutations call this: explicit full-description updates remain
/// the command's own change. The caller also runs lib-2's whole-file/array guard.
pub(super) fn ticket_backrefs(
    store: &Store,
    before: &[crate::model::Ticket],
    after: &[crate::model::Ticket],
) -> Result<()> {
    let path = store.path(DataFile::Issues);
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(Error::io("read", &path, e)),
    };
    let old: HashMap<_, _> = before.iter().map(|t| (t.id.as_str(), t)).collect();
    let new: HashMap<_, _> = after.iter().map(|t| (t.id.as_str(), t)).collect();
    for (i, line) in bytes.split(|b| *b == b'\n').enumerate() {
        let line = trim_space(line);
        let Some(ticket) = crate::model::Ticket::from_json(line) else {
            continue;
        };
        let (Some(old), Some(new)) = (old.get(ticket.id.as_str()), new.get(ticket.id.as_str()))
        else {
            continue;
        };
        if old.description == new.description {
            continue;
        }
        let members = json::parse_object(line).expect("a decoded ticket is an object");
        let Some(raw) = members.get("description").filter(|r| *r != b"null") else {
            continue;
        };
        let problem = if raw.first() != Some(&b'"') {
            Some("is not a string".into())
        } else {
            raw::string_problem(raw).map(|why| format!("holds {why}"))
        };
        if let Some(problem) = problem {
            return Err(refusal(format!("Refusing to rewrite {}: line {} issue \"{}\" description {problem}, and updating its plan backref would drop or change user text. Nothing was written.",path.display(),i+1,ticket.id)));
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn store(raw: &[u8]) -> (tempfile::TempDir, Store) {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join(".luuna/missions");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("config.yaml"), b"project: demo\nversion: \"1\"\n").unwrap();
        fs::write(d.join("plans.jsonl"), raw).unwrap();
        let s = Store::open(d).unwrap();
        (t, s)
    }
    fn change(plan: &mut Plan) {
        plan.sections
            .get_or_insert_with(Default::default)
            .insert("approach".into(), json::Value::String("new".into()));
    }
    #[test]
    fn refuses_skipped_plan_lines_without_writing() {
        for bytes in [b"not json\n".as_slice(), b"[1]\n"] {
            let (_t, s) = store(bytes);
            assert!(plan_rewrite(&s)
                .unwrap_err()
                .to_string()
                .contains("line 1 is not a plan record"));
            assert_eq!(fs::read(s.path(DataFile::Plans)).unwrap(), bytes);
        }
    }
    #[test]
    fn refuses_repeated_ids_and_top_level_keys_without_writing() {
        for (bytes, message) in [
            (
                b"{\"id\":\"a\"}\n{\"id\":\"a\"}\n".as_slice(),
                "repeats the id \"a\"",
            ),
            (br#"{"id":"a","id":"b"}"#, "repeats the key \"id\""),
        ] {
            let (_t, s) = store(bytes);
            let mode = fs::metadata(s.path(DataFile::Plans)).unwrap().permissions();
            assert!(plan_rewrite(&s).unwrap_err().to_string().contains(message));
            assert_eq!(fs::read(s.path(DataFile::Plans)).unwrap(), bytes);
            assert_eq!(
                fs::metadata(s.path(DataFile::Plans)).unwrap().permissions(),
                mode
            );
        }
    }
    #[test]
    fn name_changes_preserve_lossy_raw_sections() {
        let bytes = br#"{"id":"a","sections":{"n":9007199254740993,"x":"\ud800","n":2}}"#;
        let (_t, s) = store(bytes);
        let mut p = Plan::from_line(bytes).unwrap();
        p.name = "new".into();
        assert!(section_change(&s, &p, &[]).is_ok());
        assert!(p
            .marshal()
            .unwrap()
            .windows(b"\\ud800".len())
            .any(|x| x == b"\\ud800"));
    }
    #[test]
    fn rejects_retained_numeric_precision_loss() {
        let bytes = br#"{"id":"a","sections":{"n":9007199254740993,"approach":"old"}}"#;
        let (_t, s) = store(bytes);
        let mut p = Plan::from_line(bytes).unwrap();
        change(&mut p);
        let err = section_change(&s, &p, &[]).unwrap_err().to_string();
        assert!(err.contains("9007199254740993"));
        assert!(err.contains("9007199254740992"));
        assert_eq!(fs::read(s.path(DataFile::Plans)).unwrap(), bytes);
    }
    #[test]
    fn permits_value_preserving_number_formatting() {
        let bytes = br#"{"id":"a","sections":{"n":1.0,"approach":"old"}}"#;
        let (_t, s) = store(bytes);
        let mut p = Plan::from_line(bytes).unwrap();
        change(&mut p);
        section_change(&s, &p, &[]).unwrap();
    }
    #[test]
    fn detects_nested_repeated_keys_and_invalid_encodings() {
        for bytes in [
            br#"{"id":"a","sections":{"nested":{"a":1,"a":2}}}"#.as_slice(),
            br#"{"id":"a","sections":{"x":"\ud800"}}"#,
            b"{\"id\":\"a\",\"sections\":{\"x\":\"\xff\"}}",
        ] {
            let (_t, s) = store(bytes);
            let mut p = Plan::from_line(bytes).unwrap();
            change(&mut p);
            assert!(section_change(&s, &p, &[])
                .unwrap_err()
                .to_string()
                .ends_with("Nothing was written."));
            assert_eq!(fs::read(s.path(DataFile::Plans)).unwrap(), bytes);
        }
    }
    #[test]
    fn explicit_replacement_paths_are_component_aware() {
        let bytes = br#"{"id":"a","sections":{"steps.0":9007199254740993,"approach":"old"}}"#;
        let (_t, s) = store(bytes);
        let mut p = Plan::from_line(bytes).unwrap();
        change(&mut p);
        assert!(section_change(&s, &p, &[vec!["steps".into(), "0".into()]]).is_err());
        assert!(section_change(&s, &p, &[vec!["steps.0".into()]]).is_ok());
    }
    #[test]
    fn refuses_non_object_sections_and_scalar_steps() {
        for bytes in [
            br#"{"id":"a","sections":"keep me"}"#.as_slice(),
            br#"{"id":"a","sections":{"steps":[5]}}"#,
        ] {
            let (_t, s) = store(bytes);
            let mut p = Plan::from_line(bytes).unwrap();
            if p.sections.is_none() {
                change(&mut p);
                assert!(section_change(&s, &p, &[]).is_err());
            } else {
                assert!(step_object(&s, &p, 0).is_err());
            }
            assert_eq!(fs::read(s.path(DataFile::Plans)).unwrap(), bytes);
        }
    }
    #[test]
    fn changed_plan_arrays_refuse_typed_value_loss() {
        for key in ["children", "adoptedChildren"] {
            for value in [r#""keep me""#, r#"{"keep":true}"#, "[5]", r#"["\ud800"]"#] {
                let bytes = format!("{{\"id\":\"a\",\"{key}\":{value}}}");
                let (_t, s) = store(bytes.as_bytes());
                let mut p = Plan::from_line(bytes.as_bytes()).unwrap();
                let mode = fs::metadata(s.path(DataFile::Plans)).unwrap().permissions();
                array_changes(&s, &p).unwrap();
                if key == "children" {
                    p.children.get_or_insert_with(Vec::new).push("new".into());
                } else {
                    p.adopted_children
                        .get_or_insert_with(Vec::new)
                        .push("new".into());
                }
                assert!(array_changes(&s, &p)
                    .unwrap_err()
                    .to_string()
                    .ends_with("Nothing was written."));
                assert_eq!(fs::read(s.path(DataFile::Plans)).unwrap(), bytes.as_bytes());
                assert_eq!(
                    fs::metadata(s.path(DataFile::Plans)).unwrap().permissions(),
                    mode
                );
            }
        }
    }
    #[test]
    fn null_and_valid_arrays_allow_explicit_link_changes() {
        for value in ["null", "[]", r#"["existing"]"#] {
            let bytes = format!("{{\"children\":{value},\"adoptedChildren\":{value}}}");
            let (_t, s) = store(bytes.as_bytes());
            let mut p = Plan::from_line(bytes.as_bytes()).unwrap();
            p.children.get_or_insert_with(Vec::new).push("new".into());
            p.adopted_children
                .get_or_insert_with(Vec::new)
                .push("new".into());
            array_changes(&s, &p).unwrap();
        }
    }
    #[test]
    fn fresh_draft_replacement_refuses_unrelated_metadata_and_old_links() {
        for suffix in [
            r#","future":{"keep":"user"}"#,
            r#","outcomeNote":"keep note""#,
        ] {
            let bytes = format!(
                "{{\"id\":\"pl-old\",\"sections\":{{\"steps\":[]}},\"children\":[]{suffix}}}"
            );
            let (_t, s) = store(bytes.as_bytes());
            let p = Plan::from_line(bytes.as_bytes()).unwrap();
            for explicit in [false, true] {
                assert!(replace_draft(&s, &p, explicit)
                    .unwrap_err()
                    .to_string()
                    .ends_with("Nothing was written."));
                assert_eq!(fs::read(s.path(DataFile::Plans)).unwrap(), bytes.as_bytes());
            }
        }
        let bytes = br#"{"id":"pl-old","children":["child"]}"#;
        let (_t, s) = store(bytes);
        let p = Plan::from_line(bytes).unwrap();
        assert!(replace_draft(&s, &p, true).is_err());
    }
    #[test]
    fn explicit_blueprint_replacement_and_empty_draft_controls_follow_go() {
        for raw in [
            br#"{"sections":{}}"#.as_slice(),
            br#"{"sections":{"steps":[]},"children":[]}"#,
            br#"{"sections":null,"children":null}"#,
        ] {
            let (_t, s) = store(raw);
            let p = Plan::from_line(raw).unwrap();
            replace_draft(&s, &p, false).unwrap();
            replace_draft(&s, &p, true).unwrap();
        }
        let raw = br#"{"sections":{"context":"old body","steps":[]},"children":[]}"#;
        let (_t, s) = store(raw);
        let p = Plan::from_line(raw).unwrap();
        assert!(replace_draft(&s, &p, false).is_err());
        replace_draft(&s, &p, true).unwrap();
    }
    #[test]
    fn backref_changes_refuse_unrelated_description_loss() {
        for desc in [
            br#"{"keep":"user"}"#.as_slice(),
            b"\"manual\xffbody\"",
            br#""manual\ud800body""#,
        ] {
            let raw = [
                b"{\"id\":\"child\",\"description\":".as_slice(),
                desc,
                b"}\n",
            ]
            .concat();
            let (_t, s) = store(b"");
            let path = s.path(DataFile::Issues);
            fs::write(&path, &raw).unwrap();
            let before = s.read_tickets().unwrap();
            let mut after = before.clone();
            after[0].description = format!("block\n\n{}", before[0].description);
            let mode = fs::metadata(&path).unwrap().permissions();
            assert!(ticket_backrefs(&s, &before, &after)
                .unwrap_err()
                .to_string()
                .contains("user text. Nothing was written."));
            assert_eq!(fs::read(&path).unwrap(), raw);
            assert_eq!(fs::metadata(&path).unwrap().permissions(), mode);
            ticket_backrefs(&s, &before, &before).unwrap();
        }
    }
    #[test]
    fn backref_empty_and_normal_manual_text_controls_follow_go() {
        for desc in [b"null".as_slice(), br#""manual body""#, br#""""#] {
            let raw = [
                b"{\"id\":\"child\",\"description\":".as_slice(),
                desc,
                b"}\n",
            ]
            .concat();
            let (_t, s) = store(b"");
            fs::write(s.path(DataFile::Issues), raw).unwrap();
            let before = s.read_tickets().unwrap();
            let mut after = before.clone();
            after[0].description = format!("block\n\n{}", before[0].description);
            ticket_backrefs(&s, &before, &after).unwrap();
        }
    }
}
