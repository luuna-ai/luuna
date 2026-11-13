//! Plan creation and informational mutations, with whole-operation A8 preflight.
use super::{common::*, guard, record::Plan};
use crate::{
    command::{output::Envelope, workflow::lossy, Context, Failure},
    config_tree,
    json::Value as V,
    model,
    store::{self, DataFile},
};
use std::collections::{BTreeMap, HashSet};
pub(super) fn create(c: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let o = parse(args, &["name", "template"], &[])?;
    let seed_id = need(&o, "create <seed-id> [options]")?;
    let store = c.store()?;
    let templates = config_tree::load_plan_templates(store.dir())?;
    let (created, template) = {
        let _plans = store.lock(DataFile::Plans)?;
        let _issues = store.lock(DataFile::Issues)?;
        let before = store.read_tickets()?;
        let mut issues = before.clone();
        let mut plans = load(&store)?;
        let seed_index = ticket_index(&issues, seed_id)
            .ok_or_else(|| Failure::message(format!("Seed not found: {seed_id}")))?;
        let seed = &issues[seed_index];
        let template = if o.value("template").is_empty() {
            config_tree::default_template_for_type(&seed.ticket_type)
        } else {
            o.value("template")
        }
        .to_owned();
        if !templates.contains_key(&template) {
            return Err(Failure::message(format!(
                "Unknown template: {template}. Available: {}",
                template_names(&templates).join(", ")
            )));
        }
        for p in &plans {
            if p.seed == seed_id && p.status != "draft" {
                c.stderr.extend_from_slice(format!("✗ plan {} already exists for {seed_id} (status: {}, revision: {})\n  Adopt missions into it with 'luuna-missions plan adopt {} <seed-ids...>'.\n",p.id,p.status,p.revision,p.id).as_bytes());
                c.exit_code = 1;
                return Ok(());
            }
        }
        let ids: HashSet<_> = plans.iter().map(|p| p.id.clone()).collect();
        let id = model::generate_id("pl", &ids);
        let now = model::now_iso();
        let mut name = o.value("name").trim().to_owned();
        if name.is_empty() {
            name = seed.title.trim().into();
        }
        let mut next = Plan::default();
        next.id = id.clone();
        next.seed = seed_id.into();
        next.template = template.clone();
        next.name = name;
        next.status = "approved".into();
        next.revision = 1;
        next.sections = Some(BTreeMap::from([("steps".into(), V::Array(vec![]))]));
        next.children = Some(vec![]);
        next.created_at = now.clone();
        next.updated_at = now.clone();
        issues[seed_index].plan_id = id.clone();
        issues[seed_index].updated_at = now;
        let draft = plans
            .iter()
            .position(|p| p.seed == seed_id && p.status == "draft");
        if let Some(i) = draft {
            guard::plan_rewrite(&store)?;
            guard::replace_draft(&store, &plans[i], false)?;
        }
        lossy::refuse_lossy_ticket_changes(&store, &before, &issues)?;
        store.write_tickets(&issues)?;
        if let Some(i) = draft {
            plans[i] = next;
            store::write_records(&store.path(DataFile::Plans), &plans)?;
        } else {
            store::append_record(&store.path(DataFile::Plans), &next)?;
        }
        (id, template)
    };
    if o.has("json") {
        c.json(
            Envelope::new(true, "plan create")
                .string("plan_id", &created)
                .string("parent_seed", seed_id)
                .string("template", &template)
                .raw("children", b"[]"),
        );
    } else {
        c.success(format!("plan {created} created (status: approved, adopt-only)").as_bytes());
        c.stderr.extend_from_slice(format!("\nNext:\n  luuna-missions plan adopt {created} <seed-ids...>   # populate children in order\n  luuna-missions plan reorder {created} <seed-ids...> # set the exact children order\n").as_bytes());
    }
    Ok(())
}
pub(super) fn outcome(c: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let o = parse(args, &["result", "note"], &[])?;
    let id = need(&o, "outcome <id> --result <value> [options]")?;
    let result = o.value("result");
    if result.is_empty() {
        return Err(Failure::message(
            "required option '--result <value>' not specified",
        ));
    }
    if !["success", "partial", "failure"].contains(&result) {
        return Err(Failure::message(format!(
            "Invalid --result value: {result}. Valid: success|partial|failure"
        )));
    }
    let store = c.store()?;
    let id = resolve(&store, id)?;
    let (note, open) = {
        let _plans = store.lock(DataFile::Plans)?;
        let _issues = store.lock(DataFile::Issues)?;
        let mut plans = load(&store)?;
        let i = plan_index(&plans, &id).ok_or_else(|| missing(&id))?;
        let issues = store.read_tickets()?;
        let open = plans[i]
            .children()
            .iter()
            .filter(|id| issues.iter().any(|t| t.id == **id && t.status != "closed"))
            .count();
        plans[i].outcome = result.into();
        plans[i].updated_at = model::now_iso();
        if o.has("note") {
            plans[i].outcome_note = o.value("note").into();
        }
        guard::plan_rewrite(&store)?;
        store::write_records(&store.path(DataFile::Plans), &plans)?;
        (plans[i].outcome_note.clone(), open)
    };
    if open > 0 {
        c.stderr.extend_from_slice(
            format!(
                "⚠ plan {id} has {open} open {}\n",
                if open == 1 { "child" } else { "children" }
            )
            .as_bytes(),
        );
    }
    if o.has("json") {
        let mut out = Envelope::new(true, "plan outcome")
            .string("plan_id", &id)
            .string("outcome", result);
        if o.has("note") || !note.is_empty() {
            out = out.string("outcomeNote", &note);
        }
        c.json(out.integer("open_children", open as i64));
    } else {
        c.success(
            format!(
                "plan {id} outcome recorded: {result}{}",
                if note.is_empty() {
                    String::new()
                } else {
                    format!(" — {note}")
                }
            )
            .as_bytes(),
        );
    }
    Ok(())
}
pub(super) fn review(c: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let o = parse(args, &["by"], &[])?;
    let id = need(&o, "review <id> --by <name> [options]")?;
    let by = o.value("by");
    if by.is_empty() {
        return Err(Failure::message(
            "required option '--by <name>' not specified",
        ));
    }
    let store = c.store()?;
    let id = resolve(&store, id)?;
    {
        let _plans = store.lock(DataFile::Plans)?;
        let mut plans = load(&store)?;
        let i = plan_index(&plans, &id).ok_or_else(|| missing(&id))?;
        plans[i].reviewed_by = by.into();
        plans[i].updated_at = model::now_iso();
        guard::plan_rewrite(&store)?;
        store::write_records(&store.path(DataFile::Plans), &plans)?;
    }
    if o.has("json") {
        c.json(
            Envelope::new(true, "plan review")
                .string("plan_id", &id)
                .string("reviewedBy", by),
        );
    } else {
        c.success(format!("plan {id} reviewed by {by}").as_bytes());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::config_tree::tree::test_support::{invoke, project};
    use std::{fs, path::Path};
    const SEED:&[u8]=b"{\"id\":\"demo-1\",\"title\":\"Example seed\",\"type\":\"feature\",\"status\":\"open\"}\n";
    fn setup(plans: &[u8]) -> tempfile::TempDir {
        let t = project("project: demo\nversion: \"1\"\n");
        fs::write(t.path().join(".luuna/missions/issues.jsonl"), SEED).unwrap();
        fs::write(t.path().join(".luuna/missions/plans.jsonl"), plans).unwrap();
        t
    }
    fn files(root: &Path) -> Vec<(Vec<u8>, fs::Permissions)> {
        ["issues.jsonl", "plans.jsonl"]
            .into_iter()
            .map(|n| {
                let p = root.join(".luuna/missions").join(n);
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
    fn create_append_preserves_unreadable_old_plan_lines() {
        let t = setup(b"unreadable\n");
        let out = invoke(t.path(), &["plan", "create", "demo-1", "--json"]);
        assert_eq!(out.exit_code, 0);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(v["plan_id"].as_str().unwrap().starts_with("pl-"));
        assert!(fs::read(t.path().join(".luuna/missions/plans.jsonl"))
            .unwrap()
            .starts_with(b"unreadable\n"));
        no_locks(t.path());
    }
    #[test]
    fn create_refuses_protected_drafts_before_both_writes() {
        for raw in [br#"{"id":"pl-old","seed":"demo-1","status":"draft","future":{"keep":"user"}}"#.as_slice(),br#"{"id":"pl-old","seed":"demo-1","status":"draft","sections":{"context":"manual body"}}"#,br#"{"id":"pl-old","seed":"demo-1","status":"draft","children":["child"]}"#]{let t=setup(raw);let before=files(t.path());let out=invoke(t.path(),&["plan","create","demo-1","--json"]);assert_eq!(out.exit_code,1);assert!(String::from_utf8(out.stdout).unwrap().contains("Nothing was written."));assert_eq!(files(t.path()),before);no_locks(t.path());}
    }
    #[test]
    fn create_reuses_an_empty_draft_and_refuses_lossy_issues() {
        let t=setup(br#"{"id":"pl-old","seed":"demo-1","status":"draft","sections":{"steps":[]},"children":[]}"#);
        let out = invoke(t.path(), &["plan", "create", "demo-1", "--json"]);
        assert_eq!(out.exit_code, 0);
        assert!(!String::from_utf8(
            fs::read(t.path().join(".luuna/missions/plans.jsonl")).unwrap()
        )
        .unwrap()
        .contains("pl-old"));
        let t = setup(b"");
        let path = t.path().join(".luuna/missions/issues.jsonl");
        let mut raw = SEED.to_vec();
        raw.extend_from_slice(b"unreadable\n");
        fs::write(path, raw).unwrap();
        let before = files(t.path());
        assert_eq!(invoke(t.path(), &["plan", "create", "demo-1"]).exit_code, 1);
        assert_eq!(files(t.path()), before);
        no_locks(t.path());
    }
    #[test]
    fn review_and_outcome_refuse_lossy_plan_rewrites() {
        for args in [
            vec!["plan", "review", "pl-a", "--by", "Example"],
            vec!["plan", "outcome", "pl-a", "--result", "partial"],
        ] {
            let t = setup(b"{\"id\":\"pl-a\",\"children\":[]}\nunreadable\n");
            let before = files(t.path());
            let out = invoke(t.path(), &args);
            assert_eq!(out.exit_code, 1);
            assert!(String::from_utf8(out.stderr)
                .unwrap()
                .contains("line 2 is not a plan record"));
            assert_eq!(files(t.path()), before);
            no_locks(t.path());
        }
    }
    #[test]
    fn informational_mutations_preserve_raw_sections_and_unmodified_issue_lines() {
        let t = setup(
            br#"{"id":"pl-a","sections":{"n":9007199254740993},"children":[],"future":true}"#,
        );
        let issues = t.path().join(".luuna/missions/issues.jsonl");
        fs::write(&issues, b"unreadable\n").unwrap();
        assert_eq!(
            invoke(t.path(), &["plan", "review", "pl-a", "--by", "Example"]).exit_code,
            0
        );
        assert_eq!(
            invoke(
                t.path(),
                &["plan", "outcome", "pl-a", "--result", "partial"]
            )
            .exit_code,
            0
        );
        let raw = fs::read(t.path().join(".luuna/missions/plans.jsonl")).unwrap();
        assert!(String::from_utf8(raw).unwrap().contains("9007199254740993"));
        assert_eq!(fs::read(issues).unwrap(), b"unreadable\n");
        no_locks(t.path());
    }
    #[test]
    fn outcome_counts_only_existing_open_children_and_keeps_explicit_empty_note() {
        let t = setup(br#"{"id":"pl-a","children":["open","closed","missing"]}"#);
        fs::write(
            t.path().join(".luuna/missions/issues.jsonl"),
            b"{\"id\":\"open\",\"status\":\"open\"}\n{\"id\":\"closed\",\"status\":\"closed\"}\n",
        )
        .unwrap();
        let out = invoke(
            t.path(),
            &[
                "plan", "outcome", "pl-a", "--result", "partial", "--note", "", "--json",
            ],
        );
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.stderr, "⚠ plan pl-a has 1 open child\n".as_bytes());
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["outcomeNote"], "");
        assert_eq!(v["open_children"], 1);
    }
}
