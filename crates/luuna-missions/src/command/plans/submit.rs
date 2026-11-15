//! Submission preserves Go child matching, edge accumulation and overwrite semantics.
use super::{backref::Backref, common::*, guard, record::Plan, validation};
use crate::{
    command::{output::Envelope, shared::go_lower, workflow::lossy, Context, Failure},
    config, config_tree,
    error::Error,
    json::{self, Value as V},
    json_error,
    model::{self, Ticket},
    store::{self, DataFile},
};
use std::collections::{BTreeMap, HashSet};
#[derive(Default, Debug)]
struct Step {
    title: String,
    kind: String,
    priority: Option<i64>,
    blocks: Vec<i64>,
    template: String,
    existing: String,
    labels: Vec<String>,
}
fn steps(sections: &BTreeMap<String, V>) -> Result<Vec<Step>, Failure> {
    let values = sections
        .get("steps")
        .and_then(array)
        .ok_or_else(|| Failure::message("Validated plan has no steps array"))?;
    let mut out = vec![];
    for v in values {
        let m = object(v)
            .ok_or_else(|| Failure::message("Validated plan contains a non-object step"))?;
        let mut s = Step {
            title: text(m.get("title")).into(),
            kind: text(m.get("type")).into(),
            template: text(m.get("plan_template")).into(),
            existing: text(m.get("existing_seed")).into(),
            priority: m.get("priority").and_then(number).map(go_int),
            ..Step::default()
        };
        if let Some(a) = m.get("blocks").and_then(array) {
            for v in a {
                s.blocks.push(number(v).map(go_int).ok_or_else(||Error::unsupported("custom plan schema permits a nonnumeric step dependency that Go cannot process","none (invalid submitted step shape)"))?);
            }
        }
        if let Some(a) = m.get("labels").and_then(array) {
            for v in a {
                let V::String(v) = v else {
                    return Err(Error::unsupported(
                        "custom plan schema permits a nonstring step label that Go cannot process",
                        "none (invalid submitted step shape)",
                    )
                    .into());
                };
                s.labels.push(v.clone());
            }
        }
        out.push(s);
    }
    Ok(out)
}
fn add(list: &mut Option<Vec<String>>, id: &str) {
    let v = list.get_or_insert_default();
    if !v.iter().any(|s| s == id) {
        v.push(id.into());
    }
}
fn labels(existing: Option<&[String]>, values: &[String]) -> Option<Vec<String>> {
    let mut out = existing.unwrap_or_default().to_vec();
    for s in values {
        let s = go_lower(s.trim());
        if !s.is_empty() && !out.contains(&s) {
            out.push(s)
        }
    }
    (!out.is_empty()).then_some(out)
}
fn adoptions(
    c: &mut Context<'_>,
    steps: &[Step],
    parent: &str,
    issues: &[Ticket],
    allowed: &str,
) -> Result<BTreeMap<usize, usize>, Failure> {
    let mut out = BTreeMap::new();
    let mut seen = HashSet::new();
    for (i, s) in steps.iter().enumerate() {
        let id = &s.existing;
        if id.is_empty() {
            continue;
        }
        let label = if s.title.is_empty() {
            format!("step {} (adopt {id})", i + 1)
        } else {
            format!("step {} ({})", i + 1, s.title)
        };
        let fail = |s: String| Failure::message(format!("{label}: {s}"));
        if !s.template.is_empty() {
            return Err(fail("existing_seed and plan_template are mutually exclusive — adoption replaces spawning, so a sub-plan template cannot apply.".into()));
        }
        if id == parent {
            return Err(fail(format!(
                "cannot adopt the parent seed {parent} into its own plan."
            )));
        }
        if !seen.insert(id.clone()) {
            return Err(fail(format!(
                "existing_seed {id} is already adopted by an earlier step in this plan."
            )));
        }
        let index = ticket_index(issues, id)
            .ok_or_else(|| fail(format!("existing_seed {id} not found.")))?;
        let seed = &issues[index];
        if seed.status == "closed" {
            return Err(fail(format!(
                "existing_seed {id} is closed; only open or in-progress missions can be adopted."
            )));
        }
        if !seed.plan_id.is_empty() && seed.plan_id != allowed {
            return Err(fail(format!(
                "existing_seed {id} is already attached to plan {}.",
                seed.plan_id
            )));
        }
        if !s.title.is_empty() && s.title != seed.title {
            c.stderr.extend_from_slice(format!("⚠ step {}: existing_seed {id} title \"{}\" differs from step.title \"{}\"; seed title is preserved.\n",i+1,seed.title,s.title).as_bytes());
        }
        out.insert(i, index);
    }
    Ok(out)
}
struct Submitted {
    plan: Plan,
    seed: Ticket,
    obsolete: Vec<Ticket>,
    draft: Option<usize>,
    existing: Option<usize>,
}
#[allow(clippy::too_many_arguments)]
fn build(
    c: &mut Context<'_>,
    issues: &mut Vec<Ticket>,
    plans: &[Plan],
    seed_index: usize,
    existing: Option<usize>,
    steps: &[Step],
    project: &str,
    template: &str,
    sections: &BTreeMap<String, V>,
    order: Vec<String>,
    name: &str,
    now: &str,
) -> Result<Submitted, Failure> {
    let mut seed = issues[seed_index].clone();
    let old = existing.map(|i| &plans[i]);
    let adopted = adoptions(
        c,
        steps,
        &seed.id,
        issues,
        old.map(|p| p.id.as_str()).unwrap_or_default(),
    )?;
    let mut issue_ids: HashSet<_> = issues.iter().map(|t| t.id.clone()).collect();
    let plan_ids = plans.iter().map(|p| p.id.clone()).collect();
    let id = old
        .map(|p| p.id.clone())
        .unwrap_or_else(|| model::generate_id("pl", &plan_ids));
    let old_children: Vec<_> = old
        .into_iter()
        .flat_map(|p| p.children())
        .filter_map(|id| ticket_index(issues, id).map(|i| issues[i].clone()))
        .collect();
    let old_set: HashSet<_> = old_children.iter().map(|t| t.id.clone()).collect();
    let mut used = HashSet::new();
    let mut external = HashSet::new();
    let mut children = vec![];
    for (i, s) in steps.iter().enumerate() {
        let mut child = String::new();
        if let Some(index) = adopted.get(&i) {
            child = issues[*index].id.clone();
            if old_set.contains(&child) {
                used.insert(child.clone());
            } else {
                external.insert(child.clone());
            }
        } else if let Some(t) = old_children
            .iter()
            .find(|t| !used.contains(&t.id) && t.title == s.title)
        {
            child = t.id.clone();
            used.insert(child.clone());
        }
        if child.is_empty() {
            child = model::generate_id(project, &issue_ids);
            issue_ids.insert(child.clone());
        }
        children.push(child);
    }
    let backref = Backref {
        plan_id: &id,
        parent_id: &seed.id,
        parent_title: &seed.title,
        template,
        approach: sections.get("approach"),
    };
    let mut spawned = vec![];
    for (i, s) in steps.iter().enumerate() {
        let child = &children[i];
        if used.contains(child) || external.contains(child) {
            let index = ticket_index(issues, child).expect("selected existing child");
            let t = &mut issues[index];
            if !used.contains(child) {
                t.plan_id = id.clone();
                t.plan_step_index = Some(i as i64);
            }
            t.description = backref.apply(&t.description, Some(i as i64));
            t.labels = labels(t.labels.as_deref(), &s.labels);
            t.updated_at = now.into();
        } else {
            let mut t = Ticket::default();
            t.id = child.clone();
            t.title = s.title.clone();
            t.status = "open".into();
            t.ticket_type = if s.kind.is_empty() {
                "task".into()
            } else {
                s.kind.clone()
            };
            t.priority = s.priority.unwrap_or(2);
            t.plan_step_index = Some(i as i64);
            t.description = backref.apply("", Some(i as i64));
            t.labels = labels(None, &s.labels);
            t.created_at = now.into();
            t.updated_at = now.into();
            if s.template.is_empty() {
                t.plan_id = id.clone()
            } else {
                t.requires_plan = true
            }
            if existing.is_some() {
                t.blocks = Some(vec![seed.id.clone()]);
            }
            spawned.push(t);
        }
    }
    // Append after existing records, exactly as Go does. Edge changes retain old links.
    let spawned_start = issues.len();
    issues.extend(spawned);
    for (i, s) in steps.iter().enumerate() {
        for target in &s.blocks {
            if *target < 1 || *target as usize > children.len() {
                return Err(Error::unsupported("custom plan schema permits an out-of-range step dependency that Go cannot process","none (invalid submitted step shape)").into());
            }
            let target = *target as usize - 1;
            edge(
                issues,
                &children[i],
                true,
                &children[target],
                now,
                spawned_start,
            );
            edge(
                issues,
                &children[target],
                false,
                &children[i],
                now,
                spawned_start,
            );
        }
    }
    for child in &children {
        if existing.is_none() || external.contains(child) {
            edge(issues, child, true, &seed.id, now, spawned_start);
        }
    }
    let obsolete = old_children
        .into_iter()
        .filter(|t| !used.contains(&t.id))
        .collect();
    if existing.is_some() {
        let final_set: HashSet<_> = children.iter().collect();
        let mut blocked = seed.blocked_by.take().unwrap_or_default();
        blocked.retain(|id| !old_set.contains(id) && !final_set.contains(id));
        blocked.extend(children.clone());
        seed.blocked_by = Some(blocked);
    } else {
        for child in &children {
            add(&mut seed.blocked_by, child)
        }
    }
    seed.plan_id = id.clone();
    seed.updated_at = now.into();
    issues[seed_index] = seed.clone();
    let mut plan = old.cloned().unwrap_or_default();
    plan.id = id;
    plan.seed = seed.id.clone();
    plan.template = template.into();
    plan.sections = Some(sections.clone());
    plan.section_order = order;
    plan.children = Some(children.clone());
    plan.updated_at = now.into();
    if let Some(old) = old {
        plan.revision = old.revision.wrapping_add(1);
        if !name.is_empty() {
            plan.name = name.into()
        } else if plan.name.is_empty() {
            plan.name = seed.title.trim().into()
        }
        let mut adopted = None;
        for child in old.adopted() {
            if children.contains(child) {
                add(&mut adopted, child)
            }
        }
        for child in &children {
            if external.contains(child) {
                add(&mut adopted, child)
            }
        }
        plan.adopted_children = adopted;
    } else {
        plan.name = if name.is_empty() {
            seed.title.trim().into()
        } else {
            name.into()
        };
        plan.status = "approved".into();
        plan.revision = 1;
        plan.created_at = now.into();
        plan.adopted_children = (!adopted.is_empty())
            .then(|| adopted.values().map(|i| issues[*i].id.clone()).collect());
    }
    let draft = if existing.is_none() {
        plans
            .iter()
            .position(|p| p.seed == seed.id && p.status == "draft")
    } else {
        None
    };
    Ok(Submitted {
        plan,
        seed,
        obsolete,
        draft,
        existing,
    })
}
fn edge(
    issues: &mut [Ticket],
    id: &str,
    blocks: bool,
    value: &str,
    now: &str,
    spawned_start: usize,
) {
    if let Some(i) = ticket_index(issues, id) {
        let t = &mut issues[i];
        let list = if blocks {
            &mut t.blocks
        } else {
            &mut t.blocked_by
        };
        let before = list.clone();
        add(list, value);
        if i < spawned_start && *list != before {
            t.updated_at = now.into();
        }
    }
}
pub(super) fn run(c: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let o = parse(
        args,
        &["plan", "domain", "name"],
        &["overwrite", "record-decision"],
    )?;
    let seed_id = need(&o, "submit [options] <seed-id>")?;
    let file = o.value("plan");
    if file.is_empty() {
        return Err(Failure::message(
            "required option '--plan <file>' not specified",
        ));
    }
    let store = c.store()?;
    let raw = if file == "-" {
        c.stdin().to_vec()
    } else {
        let path = c.cwd.join(file);
        std::fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Failure::message(format!("Plan file not found: {file}"))
            } else {
                Error::io("read", path, e).into()
            }
        })?
    };
    if let Some(e) = json_error::unmarshal_error(&String::from_utf8_lossy(&raw)) {
        return Err(Failure::message(format!("Invalid JSON in plan file: {e}")));
    }
    let data =
        V::decode(&raw).map_err(|e| Failure::message(format!("Invalid JSON in plan file: {e}")))?;
    let template = object(&data)
        .and_then(|m| m.get("template"))
        .and_then(|v| {
            if let V::String(s) = v {
                Some(s.as_str())
            } else {
                None
            }
        })
        .unwrap_or("feature");
    let templates = config_tree::load_plan_templates(store.dir())?;
    let names = templates.keys().cloned().collect::<Vec<_>>().join(", ");
    let schema = templates.get(template).ok_or_else(|| {
        Failure::message(format!(
            "Unknown template in plan: {template}. Available: {names}"
        ))
    })?;
    let errors = validation::validate(schema, &data);
    if !errors.is_empty() {
        let diff = validation::diff_json(&errors, &data)?;
        if !super::raw::pretty_fits(&diff, 128 * 1024 * 1024) || json::exceeds_max_depth(&diff) {
            return Err(Error::unsupported(
                "plan validation diff exceeds native output bounds",
                "none (bounded native plan output)",
            )
            .into());
        }
        c.stderr
            .extend(json::to_output(&diff).map_err(|e| Failure::message(e.to_string()))?);
        c.exit_code = 1;
        return Ok(());
    }
    let map = object(&data).ok_or_else(|| Failure::message("Validated plan is not an object"))?;
    let sections = map
        .get("sections")
        .and_then(object)
        .ok_or_else(|| Failure::message("Validated plan has no sections object"))?;
    let steps = steps(sections)?;
    for (i, s) in steps.iter().enumerate() {
        if !s.template.is_empty() && !templates.contains_key(&s.template) {
            let label = if s.title.is_empty() {
                "untitled"
            } else {
                &s.title
            };
            c.stderr.extend_from_slice(format!("step {} ({label}): plan_template '{}' is not defined. Available: {names}. Add it under plan_templates: in .luuna/missions/config.yaml.\n",i+1,s.template).as_bytes());
            c.exit_code = 1;
            return Ok(());
        }
    }
    let config = config::read_config(store.dir())?;
    let name = if o.value("name").trim().is_empty() {
        text(map.get("name")).trim()
    } else {
        o.value("name").trim()
    };
    let order = super::raw::pairs(&raw)
        .into_iter()
        .filter(|(k, _)| json::unquote(k) == "sections")
        .next_back()
        .map(|(_, v)| {
            super::raw::pairs(v)
                .into_iter()
                .map(|(k, _)| json::unquote(k))
                .collect()
        })
        .unwrap_or_default();
    let result = {
        let _plans = store.lock(DataFile::Plans)?;
        let _issues = store.lock(DataFile::Issues)?;
        let before = store.read_tickets()?;
        let mut issues = before.clone();
        let mut plans = load(&store)?;
        let seed_index = ticket_index(&issues, seed_id)
            .ok_or_else(|| Failure::message(format!("Seed not found: {seed_id}")))?;
        let existing = plans
            .iter()
            .position(|p| p.seed == seed_id && p.status != "draft");
        if let Some(i) = existing {
            let p = &plans[i];
            if !o.has("overwrite") {
                c.stderr.extend_from_slice(format!("✗ plan {} already exists for {seed_id} (status: {}, revision: {})\n  Use --overwrite to replace it.\n",p.id,p.status,p.revision).as_bytes());
                c.exit_code = 1;
                return Ok(());
            }
        }
        let result = build(
            c,
            &mut issues,
            &plans,
            seed_index,
            existing,
            &steps,
            &config.project,
            template,
            sections,
            order,
            name,
            &model::now_iso(),
        )?;
        if result.existing.is_some() || result.draft.is_some() {
            guard::plan_rewrite(&store)?;
        }
        if let Some(i) = result.draft {
            guard::replace_draft(&store, &plans[i], true)?;
        }
        if result.existing.is_some() {
            guard::array_changes(&store, &result.plan)?;
        }
        guard::ticket_backrefs(&store, &before, &issues)?;
        lossy::refuse_lossy_ticket_changes(&store, &before, &issues)?;
        store.write_tickets(&issues)?;
        if let Some(i) = result.existing.or(result.draft) {
            plans[i] = result.plan.clone();
            store::write_records(&store.path(DataFile::Plans), &plans)?;
        } else {
            store::append_record(&store.path(DataFile::Plans), &result.plan)?;
        }
        result
    };
    // Optional Atlas recording is performed after both local writes.
    let recorded = if o.has("record-decision") {
        super::atlas::record(
            c,
            &store,
            &result.seed,
            &result.plan,
            sections.get("approach"),
            o.value("domain"),
        )
    } else {
        String::new()
    };
    for issue in &result.obsolete {
        c.stderr.extend_from_slice(
            format!(
                "luuna-missions close {} --reason \"obsoleted by plan {} revision {}\"\n",
                issue.id, result.plan.id, result.plan.revision
            )
            .as_bytes(),
        );
    }
    let p = &result.plan;
    let children = p.children();
    if o.has("json") {
        c.json(
            Envelope::new(true, "plan submit")
                .string("plan_id", &p.id)
                .raw("children", &super::record::string_array(Some(children)))
                .string("parent_seed", seed_id)
                .integer("revision", p.revision)
                .raw(
                    "obsolete",
                    &array_json(result.obsolete.iter().map(|t| quoted(&t.id))),
                )
                .boolean("overwritten", p.revision > 1),
        );
        return Ok(());
    }
    if p.revision > 1 {
        c.success(
            format!(
                "plan {} overwritten (revision {}, status: approved)",
                p.id, p.revision
            )
            .as_bytes(),
        )
    } else {
        c.success(format!("plan {} created (status: approved)", p.id).as_bytes())
    }
    c.success(
        format!(
            "{} {}: {}",
            children.len(),
            if children.len() == 1 {
                "child seed"
            } else {
                "child seeds"
            },
            children.join(", ")
        )
        .as_bytes(),
    );
    if !result.obsolete.is_empty() {
        c.success(
            format!(
                "{} {} flagged (see stderr for close suggestions)",
                result.obsolete.len(),
                if result.obsolete.len() == 1 {
                    "obsolete child seed"
                } else {
                    "obsolete child seeds"
                }
            )
            .as_bytes(),
        )
    }
    c.success(format!("{seed_id} now blocked by {} children", children.len()).as_bytes());
    if !recorded.is_empty() {
        c.success(format!("recorded luuna-atlas decision {recorded}").as_bytes())
    }
    c.stderr.extend_from_slice(format!("\nNext:\n  luuna-missions plan show {}          # review the plan as a unit\n  luuna-missions ready                      # pick up the first child step\n",p.id).as_bytes());
    if p.reviewed_by.is_empty() && matches!(p.status.as_str(), "approved" | "active") {
        c.stderr.extend_from_slice(
            format!(
                "  luuna-missions plan review {} --by <name>   # record approval (optional)\n",
                p.id
            )
            .as_bytes(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        command::{run as dispatch, Invocation},
        config_tree::tree::test_support::{invoke, project},
    };
    use std::{fs, path::Path};
    const SEED: &[u8] =
        b"{\"id\":\"demo-1\",\"title\":\"Parent\",\"status\":\"open\",\"type\":\"feature\"}\n";
    fn setup() -> tempfile::TempDir {
        let t = project("project: demo\nversion: \"1\"\n");
        fs::write(t.path().join(".luuna/missions/issues.jsonl"), SEED).unwrap();
        fs::write(t.path().join(".luuna/missions/plans.jsonl"), b"").unwrap();
        t
    }
    fn input(steps: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"template":"feature","sections":{"context":"A sufficiently long explanation of the intended work and its motivation.","approach":"Keep the established contracts.","steps":steps,"acceptance":["Verified"]}})).unwrap()
    }
    fn submit(root: &Path, input: Vec<u8>, extra: &[&str]) -> crate::command::Outcome {
        let mut args = vec!["plan", "submit", "demo-1", "--plan", "-", "--json"];
        args.extend(extra);
        dispatch(&Invocation::new(root, args).with_stdin(input)).unwrap()
    }
    fn files(root: &Path) -> Vec<(Vec<u8>, fs::Permissions)> {
        ["issues.jsonl", "plans.jsonl"]
            .into_iter()
            .map(|f| {
                let p = root.join(".luuna/missions").join(f);
                (
                    fs::read(&p).unwrap(),
                    fs::metadata(p).unwrap().permissions(),
                )
            })
            .collect()
    }
    fn tickets(root: &Path) -> Vec<Ticket> {
        let s = store::Store::open(root.join(".luuna/missions")).unwrap();
        s.read_tickets().unwrap()
    }
    #[test]
    fn submit_stdin_creates_ordered_children_and_reciprocal_edges() {
        let t = setup();
        let out = submit(
            t.path(),
            input(
                serde_json::json!([{"title":"First","blocks":[2],"labels":[" A ","a","İ"]},{"title":"Second","plan_template":"bug"}]),
            ),
            &[],
        );
        assert_eq!(out.exit_code, 0, "{:?}", out);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let ids: Vec<_> = v["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        let issues = tickets(t.path());
        assert_eq!(issues.len(), 3);
        assert_eq!(issues[0].blocked_by.as_ref().unwrap(), &ids);
        assert_eq!(
            issues[1].blocks.as_ref().unwrap(),
            &[ids[1].clone(), "demo-1".into()]
        );
        assert_eq!(issues[2].blocked_by.as_ref().unwrap(), &[ids[0].clone()]);
        assert_eq!(issues[1].labels.as_ref().unwrap(), &["a", "i"]);
        assert!(issues[2].requires_plan);
        assert!(issues[2].plan_id.is_empty());
        assert_eq!(issues[2].plan_step_index, Some(1));
    }
    #[test]
    fn overwrite_preserves_go_matched_child_fields_and_stale_edges() {
        let t = setup();
        assert_eq!(submit(t.path(),input(serde_json::json!([{"title":"A","priority":4,"type":"bug","blocks":[2]},{"title":"B"}])),&[]).exit_code,0);
        let before = tickets(t.path());
        let out = submit(
            t.path(),
            input(
                serde_json::json!([{"title":"B","priority":0},{"title":"A","priority":1,"type":"feature"}]),
            ),
            &["--overwrite"],
        );
        assert_eq!(out.exit_code, 0);
        let after = tickets(t.path());
        assert_eq!(after[1].priority, 4);
        assert_eq!(after[1].ticket_type, "bug");
        assert_eq!(after[1].plan_step_index, Some(0));
        assert_eq!(after[2].plan_step_index, Some(1));
        assert_eq!(after[1].blocks, before[1].blocks);
        assert_eq!(after[2].blocked_by, before[2].blocked_by);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["children"][0], before[2].id);
        assert_eq!(v["revision"], 2);
    }
    #[test]
    fn adoption_keeps_manual_title_text_labels_and_records_order() {
        let t = setup();
        let path = t.path().join(".luuna/missions/issues.jsonl");
        let mut bytes = SEED.to_vec();
        bytes.extend_from_slice(b"{\"id\":\"existing\",\"title\":\"Manual\",\"status\":\"in_progress\",\"labels\":[\"UPPER\"],\"description\":\"Keep manual text\"}\n");
        fs::write(path, bytes).unwrap();
        let out = submit(
            t.path(),
            input(
                serde_json::json!([{"title":"Other","existing_seed":"existing","labels":["upper"]},{"title":"New"}]),
            ),
            &[],
        );
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.stderr,"⚠ step 1: existing_seed existing title \"Manual\" differs from step.title \"Other\"; seed title is preserved.\n".as_bytes());
        let ts = tickets(t.path());
        assert_eq!(ts[1].title, "Manual");
        assert_eq!(ts[1].labels.as_ref().unwrap(), &["UPPER", "upper"]);
        assert!(ts[1].description.ends_with("Keep manual text"));
        let s = store::Store::open(t.path().join(".luuna/missions")).unwrap();
        assert_eq!(load(&s).unwrap()[0].adopted(), &["existing"]);
    }
    #[test]
    fn adoption_failures_leave_both_files_unchanged() {
        for (steps, expected) in [
            (
                serde_json::json!([{"existing_seed":"demo-1"},{"title":"B"}]),
                "cannot adopt the parent",
            ),
            (
                serde_json::json!([{"existing_seed":"absent"},{"title":"B"}]),
                "not found",
            ),
            (
                serde_json::json!([{"existing_seed":"ext","plan_template":"bug"},{"title":"B"}]),
                "mutually exclusive",
            ),
            (
                serde_json::json!([{"existing_seed":"ext"},{"existing_seed":"ext"}]),
                "already adopted",
            ),
        ] {
            let t = setup();
            let mut data = SEED.to_vec();
            data.extend_from_slice(
                b"{\"id\":\"ext\",\"title\":\"External\",\"status\":\"open\"}\n",
            );
            fs::write(t.path().join(".luuna/missions/issues.jsonl"), data).unwrap();
            let before = files(t.path());
            let out = submit(t.path(), input(steps), &[]);
            assert_eq!(out.exit_code, 1);
            assert!(String::from_utf8(out.stdout).unwrap().contains(expected));
            assert_eq!(files(t.path()), before);
        }
    }
    #[test]
    fn draft_replacement_accepts_explicit_body_but_protects_unrelated_data() {
        for (suffix, ok) in [
            ("", true),
            (r#", "future":{"keep":true}"#, false),
            (r#", "outcomeNote":"keep""#, false),
            (r#", "children":["keep"]"#, false),
        ] {
            let t = setup();
            fs::write(t.path().join(".luuna/missions/plans.jsonl"),format!(r#"{{"id":"pl-draft","seed":"demo-1","status":"draft","sections":{{"manual":"replaced by explicit blueprint"}}{suffix}}}"#)).unwrap();
            let before = files(t.path());
            let out = submit(
                t.path(),
                input(serde_json::json!([{"title":"A"},{"title":"B"}])),
                &[],
            );
            assert_eq!(out.exit_code, if ok { 0 } else { 1 });
            if !ok {
                assert_eq!(files(t.path()), before);
            }
        }
    }
    #[test]
    fn all_preflight_losses_refuse_before_either_write() {
        for (file,content) in [("issues.jsonl",[SEED,b"unreadable\n"].concat()),("plans.jsonl",b"{\"id\":\"pl-a\",\"seed\":\"demo-1\",\"status\":\"approved\",\"children\":[]}\nunreadable\n".to_vec()),("plans.jsonl",b"{\"id\":\"pl-a\",\"seed\":\"demo-1\",\"status\":\"approved\",\"children\":[17]}\n".to_vec())]{let t=setup();fs::write(t.path().join(".luuna/missions").join(file),content).unwrap();let before=files(t.path());let out=submit(t.path(),input(serde_json::json!([{"title":"A"},{"title":"B"}])),&["--overwrite"]);assert_eq!(out.exit_code,1,"{:?}",out);assert!(String::from_utf8(out.stdout).unwrap().contains("Nothing was written"));assert_eq!(files(t.path()),before);}
    }
    #[test]
    fn backref_invalid_description_refuses_without_normalizing_it() {
        for raw in [
            br#"{"keep":"object"}"#.as_slice(),
            br#""\ud800""#,
            b"\"bad\xff\"",
        ] {
            let t = setup();
            let mut data = SEED.to_vec();
            data.extend_from_slice(
                b"{\"id\":\"ext\",\"title\":\"External\",\"status\":\"open\",\"description\":",
            );
            data.extend_from_slice(raw);
            data.extend_from_slice(b"}\n");
            fs::write(t.path().join(".luuna/missions/issues.jsonl"), data).unwrap();
            let before = files(t.path());
            let out = submit(
                t.path(),
                input(serde_json::json!([{"existing_seed":"ext"},{"title":"B"}])),
                &[],
            );
            assert_eq!(out.exit_code, 1);
            assert_eq!(files(t.path()), before);
        }
    }
    #[test]
    fn validation_is_stderr_even_with_json_and_unknown_template_is_direct() {
        let t = setup();
        let before = files(t.path());
        let out = submit(
            t.path(),
            br#"{"template":"feature","sections":{}}"#.to_vec(),
            &[],
        );
        assert_eq!(out.exit_code, 1);
        assert!(out.stdout.is_empty());
        assert!(!out.stderr.is_empty());
        let out = submit(
            t.path(),
            input(serde_json::json!([{"title":"A","plan_template":"missing"},{"title":"B"}])),
            &[],
        );
        assert_eq!(out.exit_code, 1);
        assert!(out.stdout.is_empty());
        assert!(String::from_utf8(out.stderr)
            .unwrap()
            .contains("is not defined"));
        assert_eq!(files(t.path()), before);
    }
    #[test]
    fn raw_submission_order_and_number_normalization_are_explicit_input_changes() {
        let t = setup();
        let raw = input(serde_json::json!([{"title":"A"},{"title":"B"}]));
        let raw = String::from_utf8(raw)
            .unwrap()
            .replace("\"sections\":{", "\"sections\":{\"z\":9007199254740993,");
        let out = submit(t.path(), raw.into_bytes(), &[]);
        assert_eq!(out.exit_code, 0);
        let s = store::Store::open(t.path().join(".luuna/missions")).unwrap();
        let p = load(&s).unwrap().remove(0);
        assert_eq!(p.section_keys()[0], "z");
        assert_eq!(p.section("z"), Some(&V::Number(9007199254740992.0)));
    }
    #[test]
    fn parse_errors_and_no_overwrite_are_read_only() {
        let t = setup();
        let before = files(t.path());
        for raw in [b"{".to_vec(), b"1e400".to_vec()] {
            let out = submit(t.path(), raw, &[]);
            assert_eq!(out.exit_code, 1);
            assert!(String::from_utf8(out.stdout)
                .unwrap()
                .contains("Invalid JSON in plan file"));
            assert_eq!(files(t.path()), before);
        }
        let out = invoke(
            t.path(),
            &[
                "plan",
                "submit",
                "demo-1",
                "--plan",
                "missing.json",
                "--json",
            ],
        );
        assert_eq!(out.exit_code, 1);
        assert_eq!(files(t.path()), before);
        let raw = input(serde_json::json!([{"title":"A"},{"title":"B"}]));
        assert_eq!(submit(t.path(), raw.clone(), &[]).exit_code, 0);
        let before = files(t.path());
        let out = submit(t.path(), raw, &[]);
        assert_eq!(out.exit_code, 1);
        assert!(out.stdout.is_empty());
        assert!(String::from_utf8(out.stderr)
            .unwrap()
            .contains("Use --overwrite"));
        assert_eq!(files(t.path()), before);
    }
    #[test]
    fn custom_schema_panic_shapes_are_explicitly_refused_without_writes() {
        for step in [
            serde_json::json!({"title":"A","blocks":["x"]}),
            serde_json::json!({"title":"A","labels":[3]}),
            serde_json::json!({"title":"A","blocks":[20]}),
        ] {
            let t = setup();
            fs::write(t.path().join(".luuna/missions/config.yaml"),"project: demo\nversion: \"1\"\nplan_templates:\n  custom:\n    sections:\n      steps:\n        required: true\n        kind: list\n        item: {}\n        prompt: test\n").unwrap();
            let before = files(t.path());
            let raw = serde_json::to_vec(
                &serde_json::json!({"template":"custom","sections":{"steps":[step,{"title":"B"}]}}),
            )
            .unwrap();
            let err = dispatch(
                &Invocation::new(t.path(), ["plan", "submit", "demo-1", "--plan", "-"])
                    .with_stdin(raw),
            )
            .unwrap_err();
            assert!(matches!(err, Error::Unsupported { .. }));
            assert_eq!(files(t.path()), before);
        }
    }
    #[test]
    fn overwrite_keeps_old_metadata_and_reports_obsolete_children() {
        let t = setup();
        assert_eq!(
            submit(
                t.path(),
                input(serde_json::json!([{"title":"A"},{"title":"B"}])),
                &[]
            )
            .exit_code,
            0
        );
        let before = tickets(t.path());
        let path = t.path().join(".luuna/missions/plans.jsonl");
        let raw=String::from_utf8(fs::read(&path).unwrap()).unwrap().replace("\"status\":\"approved\"","\"status\":\"completed\",\"outcomeNote\":\"Manual\",\"reviewedBy\":\"Reviewer\",\"future\":9007199254740993");
        fs::write(&path, raw).unwrap();
        let out = submit(
            t.path(),
            input(serde_json::json!([{"title":"B"},{"title":"C"}])),
            &["--overwrite"],
        );
        assert_eq!(out.exit_code, 0);
        assert!(String::from_utf8(out.stderr)
            .unwrap()
            .contains(&format!("close {} --reason", before[1].id)));
        let raw = String::from_utf8(fs::read(path).unwrap()).unwrap();
        for keep in ["completed", "Manual", "Reviewer", "9007199254740993"] {
            assert!(raw.contains(keep));
        }
        assert_eq!(tickets(t.path())[1].id, before[1].id);
    }
    #[test]
    fn optional_missing_atlas_is_best_effort_after_local_success() {
        let t = setup();
        let raw = input(serde_json::json!([{"title":"A"},{"title":"B"}]));
        let out = submit(t.path(), raw, &["--record-decision"]);
        assert_eq!(out.exit_code, 0);
        assert_eq!(
            out.stderr,
            "⚠ --record-decision: luuna-atlas not found on PATH; skipping\n".as_bytes()
        );
        assert_eq!(tickets(t.path()).len(), 3);
    }
}
