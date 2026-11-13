//! In-place plan edits and child propagation, preserving Go's write/lock order.
use super::{
    backref::Backref,
    common::*,
    guard,
    record::{string_array, Plan},
};
use crate::{
    command::{output::Envelope, workflow::lossy, Context, Failure},
    config_tree::{self, SectionKind},
    error::Error,
    json::Value as V,
    model::{self, Ticket},
    store::{self, DataFile, Store},
};
use std::collections::HashSet;
struct Section {
    name: String,
    text: String,
}
struct Step {
    index: usize,
    title: Option<String>,
    priority: Option<i64>,
    ticket_type: Option<String>,
}
fn extract(args: &[String]) -> (Vec<String>, Vec<String>, bool) {
    let (mut rest, mut captured, mut provided, mut i) = (vec![], vec![], false, 0);
    while i < args.len() {
        let arg = &args[i];
        if arg != "--section" && !arg.starts_with("--section=") {
            rest.push(arg.clone());
            i += 1;
            continue;
        }
        provided = true;
        if let Some(value) = arg.strip_prefix("--section=") {
            captured.push(value.to_owned());
        }
        while i + 1 < args.len() && !args[i + 1].starts_with('-') {
            i += 1;
            captured.push(args[i].clone());
        }
        i += 1;
    }
    (rest, captured, provided)
}
fn section(raw: &[String], provided: bool) -> Result<Option<Section>, Failure> {
    if !provided {
        return Ok(None);
    }
    if raw.len() < 2 {
        return Err(Failure::message(
            "--section requires two arguments: --section <name> <text> (quote the text).",
        ));
    }
    if raw.len() > 2 {
        return Err(Failure::message("--section received more than two arguments. Quote the text: --section <name> \"<text>\"."));
    }
    if raw[0].trim().is_empty() {
        return Err(Failure::message(
            "--section name must be a non-empty string.",
        ));
    }
    Ok(Some(Section {
        name: raw[0].clone(),
        text: raw[1].clone(),
    }))
}
fn priority(raw: &str) -> Result<i64, Failure> {
    let trimmed = raw.trim();
    let value = if trimmed.starts_with(['p', 'P']) {
        &trimmed[1..]
    } else {
        trimmed
    };
    let value = value.trim_start_matches([' ', '\t', '\n', '\x0c', '\r']);
    let bytes = value.as_bytes();
    let mut end = usize::from(bytes.first().is_some_and(|b| matches!(b, b'+' | b'-')));
    let start = end;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1
    }
    let n = (end > start)
        .then(|| value[..end].parse::<i64>().ok())
        .flatten();
    n.filter(|n| (0..=4).contains(n))
        .ok_or_else(|| Failure::message(format!("--priority must be 0-4 or P0-P4 (got: {raw}).")))
}
fn step(o: &crate::command::options::Options) -> Result<Option<Step>, Failure> {
    let any = o.has("title") || o.has("priority") || o.has("type");
    if !o.has("step") {
        if any {
            return Err(Failure::message(
                "--title/--priority/--type require --step <i> (the step index to edit).",
            ));
        }
        return Ok(None);
    }
    let raw = o.value("step");
    let trimmed = raw.trim();
    let n = trimmed
        .parse::<i64>()
        .ok()
        .filter(|n| *n >= 1 && n.to_string() == trimmed)
        .ok_or_else(|| {
            Failure::message(format!("--step must be a positive integer (got: {raw})."))
        })?;
    if !any {
        return Err(Failure::message(
            "--step requires at least one of --title, --priority, --type.",
        ));
    }
    let mut patch = Step {
        index: n as usize - 1,
        title: None,
        priority: None,
        ticket_type: None,
    };
    if o.has("title") {
        let title = o.value("title").trim();
        if title.is_empty() {
            return Err(Failure::message("--title must be a non-empty string."));
        }
        patch.title = Some(title.into());
    }
    if o.has("priority") {
        patch.priority = Some(priority(o.value("priority"))?);
    }
    if o.has("type") {
        let ty = o.value("type");
        if !["task", "bug", "feature", "epic"].contains(&ty) {
            return Err(Failure::message(format!(
                "Invalid --type value: {ty}. Valid: task|bug|feature|epic"
            )));
        }
        patch.ticket_type = Some(ty.into());
    }
    Ok(Some(patch))
}
fn propagate(
    p: &Plan,
    before: &[Ticket],
    patch: Option<&Step>,
    approach_changed: bool,
    now: &str,
) -> (Vec<Ticket>, bool, Vec<String>) {
    let mut issues = before.to_vec();
    let mut dirty = false;
    let mut propagated = vec![];
    let child_ids: HashSet<_> = p.children().iter().map(String::as_str).collect();
    if approach_changed {
        if let Some(parent) = before.iter().find(|t| t.id == p.seed) {
            let backref = Backref {
                plan_id: &p.id,
                parent_id: &parent.id,
                parent_title: &parent.title,
                template: &p.template,
                approach: p.section("approach"),
            };
            for id in p.children() {
                if let Some(i) = ticket_index(&issues, id) {
                    issues[i].description =
                        backref.apply(&issues[i].description, issues[i].plan_step_index);
                    issues[i].updated_at = now.into();
                    dirty = true;
                }
            }
        }
    }
    if let Some(patch) = patch {
        for t in &mut issues {
            if !child_ids.contains(t.id.as_str()) || t.plan_step_index != Some(patch.index as i64) {
                continue;
            }
            t.updated_at = now.into();
            if let Some(title) = &patch.title {
                t.title = title.clone();
            }
            if let Some(priority) = patch.priority {
                t.priority = priority;
            }
            if let Some(ty) = &patch.ticket_type {
                t.ticket_type = ty.clone();
            }
            propagated.push(t.id.clone());
            dirty = true;
        }
    }
    (issues, dirty, propagated)
}
fn check_issues(store: &Store, before: &[Ticket], after: &[Ticket]) -> Result<(), Failure> {
    lossy::refuse_lossy_ticket_changes(store, before, after)?;
    guard::ticket_backrefs(store, before, after)?;
    Ok(())
}
fn late_refusal(id: &str, failure: Failure) -> Failure {
    if matches!(&failure, Failure::Library(Error::Unsupported { .. })) {
        return failure;
    }
    let message = match failure {
        Failure::Message(m) | Failure::UnknownOption(m) => m,
        Failure::Library(e) => e.to_string(),
    };
    let message = message.trim_end_matches("Nothing was written.").trim_end();
    Failure::message(format!("Plan {id} was saved, but child propagation was refused: {message} The issues file was not changed."))
}
pub(super) fn run(c: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let (without, raw_section, provided) = extract(args);
    let o = parse(
        &without,
        &["name", "step", "title", "priority", "type"],
        &[],
    )?;
    let id = need(&o, "edit <id> [options]")?;
    let store = c.store()?;
    let id = resolve(&store, id)?;
    let section = section(&raw_section, provided)?;
    let patch = step(&o)?;
    let mut edited = vec![];
    if o.has("name") {
        edited.push("name".to_owned());
    }
    if let Some(s) = &section {
        edited.push(format!("section:{}", s.name));
    }
    if let Some(s) = &patch {
        for (field, present) in [
            ("title", s.title.is_some()),
            ("priority", s.priority.is_some()),
            ("type", s.ticket_type.is_some()),
        ] {
            if present {
                edited.push(format!("step:{}:{field}", s.index + 1));
            }
        }
    }
    if edited.is_empty() {
        return Err(Failure::message("No fields to edit. Pass at least one of: --name <text>, --section <name> <text>, --step <i> --title/--priority/--type."));
    }
    let name = if o.has("name") {
        let name = o.value("name").trim();
        if name.is_empty() {
            return Err(Failure::message("--name must be a non-empty string."));
        }
        Some(name.to_owned())
    } else {
        None
    };
    let (revision, name, approach_changed, refreshed, propagated) = {
        let _plans = store.lock(DataFile::Plans)?;
        let mut plans = load(&store)?;
        let index = plan_index(&plans, &id).ok_or_else(|| missing(&id))?;
        let mut updated = plans[index].clone();
        let mut approach_changed = false;
        let mut ignored = vec![];
        if let Some(section) = &section {
            let templates = config_tree::load_plan_templates(store.dir())?;
            let template = templates.get(&updated.template).ok_or_else(|| {
                Failure::message(format!(
                    "Plan {id} references unknown template '{}'. Available: {}.",
                    updated.template,
                    template_names(&templates).join(", ")
                ))
            })?;
            let spec = template.sections.get(&section.name).ok_or_else(|| {
                Failure::message(format!(
                    "Unknown section '{}' for template '{}'. Known: {}.",
                    section.name,
                    updated.template,
                    section_names(template).join(", ")
                ))
            })?;
            if !matches!(spec.kind, SectionKind::Text) {
                let kind = match spec.kind {
                    SectionKind::List => "list",
                    SectionKind::Steps => "steps",
                    _ => "object",
                };
                return Err(Failure::message(format!("--section editing supports kind=text only (V1). Section '{}' is kind={kind}. Use 'luuna-missions plan submit --overwrite' for structural edits.",section.name)));
            }
            if spec.required && section.text.trim().is_empty() {
                return Err(Failure::message(format!(
                    "Section '{}' is required and cannot be empty.",
                    section.name
                )));
            }
            let min = spec.min_length.unwrap_or(0);
            let len = section.text.encode_utf16().count();
            if min > 0 && (len as i64) < min {
                return Err(Failure::message(format!(
                    "Section '{}' must be at least {min} characters (got {len}).",
                    section.name
                )));
            }
            approach_changed = section.name == "approach"
                && !matches!(updated.section(&section.name),Some(V::String(prior))if prior==&section.text);
            updated
                .sections
                .get_or_insert_with(Default::default)
                .insert(section.name.clone(), V::String(section.text.clone()));
            ignored.push(vec![section.name.clone()]);
        }
        if let Some(patch) = &patch {
            let steps=updated.section("steps").and_then(array).ok_or_else(||Failure::message(format!("Plan {id} has no steps section to edit. Use 'luuna-missions plan submit --overwrite' to add steps.")))?;
            if patch.index >= steps.len() {
                return Err(Failure::message(format!(
                    "--step {} is out of range (plan {id} has {} step{}).",
                    patch.index + 1,
                    steps.len(),
                    if steps.len() == 1 { "" } else { "s" }
                )));
            }
            guard::step_object(&store, &updated, patch.index)?;
            let mut next = object(&steps[patch.index]).cloned().unwrap_or_default();
            for (key, value) in [
                ("title", patch.title.as_ref().map(|s| V::String(s.clone()))),
                ("priority", patch.priority.map(|n| V::Number(n as f64))),
                (
                    "type",
                    patch.ticket_type.as_ref().map(|s| V::String(s.clone())),
                ),
            ] {
                if let Some(value) = value {
                    next.insert(key.into(), value);
                    ignored.push(vec!["steps".into(), patch.index.to_string(), key.into()]);
                }
            }
            let mut steps = steps.to_vec();
            steps[patch.index] = V::Object(next);
            updated
                .sections
                .get_or_insert_with(Default::default)
                .insert("steps".into(), V::Array(steps));
        }
        let now = model::now_iso();
        updated.revision = updated.revision.wrapping_add(1);
        updated.updated_at = now.clone();
        if let Some(name) = name {
            updated.name = name;
        }
        let may_propagate = approach_changed || patch.is_some();
        // Preflight ordinary A8 loss before Go's first write. I/O failures are raised
        // again at Go's original read stage; Unsupported is never discarded.
        if may_propagate {
            match store.read_tickets() {
                Ok(before) => {
                    let (after, dirty, _) =
                        propagate(&updated, &before, patch.as_ref(), approach_changed, &now);
                    if dirty {
                        check_issues(&store, &before, &after)?;
                    }
                }
                Err(e @ Error::Unsupported { .. }) => return Err(e.into()),
                Err(_) => (),
            }
        }
        guard::plan_rewrite(&store)?;
        guard::section_change(&store, &updated, &ignored)?;
        plans[index] = updated;
        store::write_records(&store.path(DataFile::Plans), &plans)?;
        let updated = &plans[index];
        let mut propagated = vec![];
        if may_propagate {
            let _issues = store.lock(DataFile::Issues)?;
            let before = store.read_tickets()?;
            let (after, dirty, ids) =
                propagate(updated, &before, patch.as_ref(), approach_changed, &now);
            if dirty {
                check_issues(&store, &before, &after).map_err(|e| late_refusal(&id, e))?;
                store.write_tickets(&after)?;
            }
            propagated = ids;
        }
        (
            updated.revision,
            updated.name.clone(),
            approach_changed,
            if approach_changed {
                updated.children().len()
            } else {
                0
            },
            propagated,
        )
    };
    if o.has("json") {
        let mut out = Envelope::new(true, "plan edit")
            .string("plan_id", &id)
            .integer("revision", revision)
            .raw("edited", &string_array(Some(&edited)));
        if !name.is_empty() {
            out = out.string("name", &name);
        }
        c.json(
            out.integer("backrefs_refreshed", refreshed as i64)
                .raw("propagated_children", &string_array(Some(&propagated))),
        );
    } else {
        c.success(
            format!(
                "plan {id} edited ({}); revision {revision}",
                edited.join(", ")
            )
            .as_bytes(),
        );
        if approach_changed {
            c.success(format!("refreshed backrefs on {refreshed} child seed(s)").as_bytes());
        }
        if !propagated.is_empty() {
            c.success(
                format!(
                    "propagated step metadata to {} child seed(s): {}",
                    propagated.len(),
                    propagated.join(", ")
                )
                .as_bytes(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        command::{run as command_run, Invocation},
        config_tree::tree::test_support::{invoke, project},
    };
    use std::{fs, path::Path};
    const PLAN:&[u8]=b"{\"id\":\"pl-a\",\"seed\":\"seed\",\"template\":\"feature\",\"status\":\"approved\",\"revision\":1,\"sections\":{\"approach\":\"old\",\"steps\":[{\"title\":\"A\"}]},\"children\":[\"child\"],\"future\":{\"n\":9007199254740993}}\n";
    fn setup(plan: &[u8], description: &[u8]) -> tempfile::TempDir {
        let t = project("project: demo\nversion: \"1\"\n");
        fs::write(t.path().join(".luuna/missions/plans.jsonl"), plan).unwrap();
        let bytes=[b"{\"id\":\"seed\",\"title\":\"Example parent\",\"status\":\"open\"}\n{\"id\":\"child\",\"title\":\"A\",\"status\":\"open\",\"plan_id\":\"pl-a\",\"plan_step_index\":0,\"description\":".as_slice(),description,b"}\n"].concat();
        fs::write(t.path().join(".luuna/missions/issues.jsonl"), bytes).unwrap();
        t
    }
    fn files(root: &Path) -> Vec<(Vec<u8>, fs::Permissions)> {
        ["issues.jsonl", "plans.jsonl"]
            .into_iter()
            .map(|name| {
                let p = root.join(".luuna/missions").join(name);
                (
                    fs::read(&p).unwrap(),
                    fs::metadata(p).unwrap().permissions(),
                )
            })
            .collect()
    }
    fn no_locks(root: &Path) {
        assert!(fs::read_dir(root.join(".luuna/missions"))
            .unwrap()
            .all(|e| {
                let n = e.unwrap().file_name().to_string_lossy().into_owned();
                !n.contains(".lock") && !n.contains(".tmp")
            }));
    }
    #[test]
    fn section_capture_keeps_go_two_argument_validation() {
        let args = [
            "pl-a",
            "--section",
            "approach",
            "unquoted",
            "words",
            "--json",
        ]
        .map(str::to_owned);
        let (rest, captured, provided) = extract(&args);
        assert_eq!(rest, ["pl-a", "--json"]);
        assert!(
            matches!(section(&captured,provided),Err(Failure::Message(m))if m=="--section received more than two arguments. Quote the text: --section <name> \"<text>\".")
        );
    }
    #[test]
    fn priority_copies_go_leading_integer_quirk() {
        for (raw, n) in [("P1.5", 1), ("p +2tail", 2), ("-0", 0), ("0x10", 0)] {
            assert_eq!(priority(raw).unwrap(), n);
        }
        assert!(priority("P\u{a0}2").is_err());
        assert!(priority("P9").is_err());
    }
    #[test]
    fn step_edits_propagate_metadata_and_keep_unrelated_raw_fields() {
        let t = setup(PLAN, br#""manual""#);
        let out = invoke(
            t.path(),
            &[
                "plan",
                "edit",
                "pl-a",
                "--step",
                "1",
                "--title",
                "Updated",
                "--priority",
                "P0",
                "--type",
                "bug",
                "--json",
            ],
        );
        assert_eq!(out.exit_code, 0);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(
            v["edited"],
            serde_json::json!(["step:1:title", "step:1:priority", "step:1:type"])
        );
        assert_eq!(v["propagated_children"], serde_json::json!(["child"]));
        let store = Store::open(t.path().join(".luuna/missions")).unwrap();
        let child = store
            .read_tickets()
            .unwrap()
            .into_iter()
            .find(|t| t.id == "child")
            .unwrap();
        assert_eq!(
            (
                child.title.as_str(),
                child.priority,
                child.ticket_type.as_str()
            ),
            ("Updated", 0, "bug")
        );
        assert!(
            String::from_utf8(fs::read(store.path(DataFile::Plans)).unwrap())
                .unwrap()
                .contains("9007199254740993")
        );
        no_locks(t.path());
    }
    #[test]
    fn approach_changes_refuse_description_loss_before_either_write() {
        for description in [
            br#"{"keep":"user"}"#.as_slice(),
            b"\"manual\xffbody\"",
            br#""manual\ud800body""#,
        ] {
            let t = setup(PLAN, description);
            let before = files(t.path());
            let out = invoke(
                t.path(),
                &[
                    "plan",
                    "edit",
                    "pl-a",
                    "--section",
                    "approach",
                    "new approach",
                    "--json",
                ],
            );
            assert_eq!(out.exit_code, 1);
            assert!(String::from_utf8(out.stdout)
                .unwrap()
                .contains("Nothing was written."));
            assert_eq!(files(t.path()), before);
            no_locks(t.path());
        }
    }
    #[test]
    fn section_edits_refuse_retained_value_loss_before_either_write() {
        for sections in [
            r#"{"approach":"old","n":9007199254740993}"#,
            r#"{"approach":"old","x":{"a":1,"a":2}}"#,
            r#"{"approach":"old","x":"\ud800"}"#,
        ] {
            let plan=format!("{{\"id\":\"pl-a\",\"seed\":\"seed\",\"template\":\"feature\",\"sections\":{sections},\"children\":[\"child\"]}}");
            let t = setup(plan.as_bytes(), br#""manual""#);
            let before = files(t.path());
            assert_eq!(
                invoke(
                    t.path(),
                    &[
                        "plan",
                        "edit",
                        "pl-a",
                        "--section",
                        "approach",
                        "new approach"
                    ]
                )
                .exit_code,
                1
            );
            assert_eq!(files(t.path()), before);
            no_locks(t.path());
        }
    }
    #[test]
    fn unchanged_sections_and_explicit_replacements_keep_go_boundaries() {
        let p=br#"{"id":"pl-a","template":"feature","sections":{"approach":"old","n":9007199254740993},"children":[]}"#;
        let t = setup(p, br#""manual""#);
        assert_eq!(
            invoke(
                t.path(),
                &["plan", "edit", "pl-a", "--section", "approach", "old"]
            )
            .exit_code,
            0
        );
        assert!(
            String::from_utf8(fs::read(t.path().join(".luuna/missions/plans.jsonl")).unwrap())
                .unwrap()
                .contains("9007199254740993")
        );
        let p=br#"{"id":"pl-a","template":"feature","sections":{"approach":{"n":9007199254740993}},"children":[]}"#;
        let t = setup(p, br#""manual""#);
        assert_eq!(
            invoke(
                t.path(),
                &["plan", "edit", "pl-a", "--section", "approach", "new"]
            )
            .exit_code,
            0
        );
    }
    #[test]
    fn changed_approach_reports_zero_refreshed_children_as_go_does() {
        let p=br#"{"id":"pl-a","template":"feature","revision":1,"sections":{"approach":"old"},"children":[]}"#;
        let t = setup(p, br#""manual""#);
        let out = invoke(
            t.path(),
            &["plan", "edit", "pl-a", "--section", "approach", "new"],
        );
        assert_eq!(out.stdout,"✓ plan pl-a edited (section:approach); revision 2\n✓ refreshed backrefs on 0 child seed(s)\n".as_bytes());
    }
    #[test]
    fn malformed_issue_rewrites_are_refused_before_plan_revision_changes() {
        let t = setup(PLAN, br#""manual""#);
        let path = t.path().join(".luuna/missions/issues.jsonl");
        let mut bytes = fs::read(&path).unwrap();
        bytes.extend_from_slice(b"unreadable\n");
        fs::write(path, bytes).unwrap();
        let before = files(t.path());
        assert_eq!(
            invoke(
                t.path(),
                &["plan", "edit", "pl-a", "--section", "approach", "new"]
            )
            .exit_code,
            1
        );
        assert_eq!(files(t.path()), before);
        no_locks(t.path());
    }
    #[test]
    fn name_only_changes_do_not_rewrite_or_refuse_unreadable_issues() {
        let t = setup(PLAN, br#""manual""#);
        let path = t.path().join(".luuna/missions/issues.jsonl");
        fs::write(&path, b"unreadable\n").unwrap();
        assert_eq!(
            invoke(t.path(), &["plan", "edit", "pl-a", "--name", "Renamed"]).exit_code,
            0
        );
        assert_eq!(fs::read(path).unwrap(), b"unreadable\n");
    }
    #[test]
    fn concurrent_issue_changes_are_rechecked_under_go_s_original_lock_order() {
        for unsafe_change in [false, true] {
            let t = setup(PLAN, br#""manual""#);
            let root = t.path().to_path_buf();
            let store = Store::open(root.join(".luuna/missions")).unwrap();
            let held = store.lock(DataFile::Issues).unwrap();
            let cwd = root.clone();
            let worker = std::thread::spawn(move || {
                command_run(&Invocation::new(
                    cwd,
                    [
                        "plan",
                        "edit",
                        "pl-a",
                        "--section",
                        "approach",
                        "new",
                        "--json",
                    ],
                ))
            });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            let saved = loop {
                let raw = fs::read(store.path(DataFile::Plans)).unwrap();
                let v: serde_json::Value =
                    serde_json::from_slice(crate::store::trim_space(&raw)).unwrap();
                if v["revision"] == 2 {
                    break true;
                }
                if std::time::Instant::now() >= deadline {
                    break false;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            };
            if !saved {
                drop(held);
                let _ = worker.join();
                panic!("plan write did not precede the issues-lock admission");
            }
            let mut changed = fs::read(store.path(DataFile::Issues)).unwrap();
            if unsafe_change {
                changed.extend_from_slice(b"unreadable\n");
            } else {
                changed = String::from_utf8(changed)
                    .unwrap()
                    .replace("manual", "new manual")
                    .into_bytes();
            }
            fs::write(store.path(DataFile::Issues), &changed).unwrap();
            drop(held);
            let out = worker.join().unwrap().unwrap();
            if unsafe_change {
                assert_eq!(out.exit_code, 1);
                assert!(String::from_utf8(out.stdout)
                    .unwrap()
                    .contains("was saved, but child propagation was refused"));
                assert_eq!(fs::read(store.path(DataFile::Issues)).unwrap(), changed);
            } else {
                assert_eq!(out.exit_code, 0);
                let child = store
                    .read_tickets()
                    .unwrap()
                    .into_iter()
                    .find(|t| t.id == "child")
                    .unwrap();
                assert!(child.description.ends_with("new manual"));
            }
            no_locks(&root);
        }
    }
}
