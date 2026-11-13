//! Ordered adopt/reorder/release mutations and their preflight boundary.
use super::{
    backref::{self, Backref},
    common::*,
    guard,
};
use crate::{
    command::{output::Envelope, shared::go_lower, workflow::lossy, Context, Failure},
    json::Value as V,
    model,
    store::{self, DataFile},
};
use std::collections::{HashMap, HashSet};
fn duplicate_check(ids: &[String]) -> Result<(), Failure> {
    let mut seen = HashSet::new();
    let mut dup = vec![];
    for id in ids {
        if !seen.insert(id) && !dup.contains(&id) {
            dup.push(id)
        }
    }
    if dup.is_empty() {
        Ok(())
    } else {
        Err(Failure::message(format!(
            "Duplicate seed id{} in args: {}.",
            if dup.len() > 1 { "s" } else { "" },
            dup.iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }
}
fn positive(value: &str, name: &str) -> Result<i64, Failure> {
    let trimmed = value.trim();
    match trimmed.parse::<i64>() {
        Ok(n) if n > 0 && n.to_string() == trimmed => Ok(n),
        _ => Err(Failure::message(format!(
            "--{name} must be a positive integer (got: {value})."
        ))),
    }
}
fn add(list: &mut Option<Vec<String>>, id: &str) {
    let list = list.get_or_insert_default();
    if !list.iter().any(|s| s == id) {
        list.push(id.into());
    }
}
fn remove(list: &mut Option<Vec<String>>, id: &str) {
    let mut values = list.take().unwrap_or_default();
    values.retain(|v| v != id);
    *list = (!values.is_empty()).then_some(values);
}
pub(super) fn reorder(c: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let o = parse(args, &[], &[])?;
    if o.positionals.len() < 2 {
        return Err(Failure::message("At least one seed id is required."));
    }
    let store = c.store()?;
    let id = resolve(&store, &o.positionals[0])?;
    let ids = &o.positionals[1..];
    duplicate_check(ids)?;
    let revision = {
        let _plans = store.lock(DataFile::Plans)?;
        let mut plans = load(&store)?;
        let i = plan_index(&plans, &id).ok_or_else(|| missing(&id))?;
        let p = &mut plans[i];
        let current: HashSet<_> = p.children().iter().collect();
        let provided: HashSet<_> = ids.iter().collect();
        let missing: Vec<_> = p
            .children()
            .iter()
            .filter(|id| !provided.contains(id))
            .cloned()
            .collect();
        let extra: Vec<_> = ids
            .iter()
            .filter(|id| !current.contains(id))
            .cloned()
            .collect();
        if !extra.is_empty() {
            return Err(Failure::message(format!(
                "{} {} not {} of plan {id}. Adopt first with 'luuna-missions plan adopt'.",
                extra.join(", "),
                if extra.len() > 1 { "are" } else { "is" },
                if extra.len() > 1 {
                    "children"
                } else {
                    "a child"
                }
            )));
        }
        if !missing.is_empty() {
            return Err(Failure::message(format!("reorder must list every child exactly once; missing: {}. Use 'luuna-missions plan release' to drop a child.",missing.join(", "))));
        }
        p.children = Some(ids.to_vec());
        p.revision = p.revision.wrapping_add(1);
        p.updated_at = model::now_iso();
        guard::plan_rewrite(&store)?;
        guard::array_changes(&store, p)?;
        let revision = p.revision;
        store::write_records(&store.path(DataFile::Plans), &plans)?;
        revision
    };
    if o.has("json") {
        c.json(
            Envelope::new(true, "plan reorder")
                .string("plan_id", &id)
                .raw("children", &super::record::string_array(Some(ids)))
                .integer("revision", revision),
        );
    } else {
        c.success(format!("plan {id} children reordered (revision {revision})").as_bytes());
        c.success(format!("order: {}", ids.join(", ")).as_bytes());
    }
    Ok(())
}
pub(super) fn run(c: &mut Context<'_>, args: &[String], release: bool) -> Result<(), Failure> {
    let o = parse(
        args,
        if release {
            &[]
        } else {
            &["step", "at", "before", "after"]
        },
        &[],
    )?;
    if o.positionals.len() < 2 {
        return Err(Failure::message("At least one seed id is required."));
    }
    let store = c.store()?;
    let id = resolve(&store, &o.positionals[0])?;
    let ids = &o.positionals[1..];
    duplicate_check(ids)?;
    let step = if o.has("step") {
        Some(positive(o.value("step"), "step")? - 1)
    } else {
        None
    };
    let flags: Vec<_> = ["at", "before", "after"]
        .into_iter()
        .filter(|n| o.has(n))
        .map(|n| format!("--{n}"))
        .collect();
    if flags.len() > 1 {
        return Err(Failure::message(format!(
            "--at, --before, and --after are mutually exclusive (got: {}).",
            flags.join(", ")
        )));
    }
    let at = if o.has("at") {
        positive(o.value("at"), "at")?
    } else {
        0
    };
    for n in ["before", "after"] {
        if o.has(n) && o.value(n).trim().is_empty() {
            return Err(Failure::message(format!("--{n} requires a seed id.")));
        }
    }
    let revision = {
        let _plans = store.lock(DataFile::Plans)?;
        let _issues = store.lock(DataFile::Issues)?;
        let before = store.read_tickets()?;
        let mut issues = before.clone();
        let mut plans = load(&store)?;
        let pi = plan_index(&plans, &id).ok_or_else(|| missing(&id))?;
        let p = &mut plans[pi];
        let steps = p.section("steps").and_then(array).unwrap_or_default();
        if let Some(step) = step {
            if step as usize >= steps.len() {
                return Err(Failure::message(format!(
                    "--step {} is out of range (plan {id} has {} step{}).",
                    step + 1,
                    steps.len(),
                    if steps.len() == 1 { "" } else { "s" }
                )));
            }
        }
        let by_id: HashMap<_, _> = issues
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id.clone(), i))
            .collect();
        let parent_index = *by_id.get(&p.seed).ok_or_else(|| {
            Failure::message(format!(
                "Plan {id} references parent seed {} which no longer exists.",
                p.seed
            ))
        })?;
        let mut parent = issues[parent_index].clone();
        for child in ids {
            if child == &p.seed {
                return Err(Failure::message(if release {
                    format!("cannot release the parent seed {child} from its own plan {id}.")
                } else {
                    format!("cannot adopt the parent seed {child} into its own plan {id}.")
                }));
            }
            let index = *by_id
                .get(child)
                .ok_or_else(|| Failure::message(format!("seed {child} not found.")))?;
            let seed = &issues[index];
            if release {
                if seed.plan_id != id {
                    return Err(Failure::message(if seed.plan_id.is_empty() {
                        format!("seed {child} is not attached to plan {id}.")
                    } else {
                        format!(
                            "seed {child} is attached to plan {}, not {id}.",
                            seed.plan_id
                        )
                    }));
                }
            } else {
                if seed.status == "closed" {
                    return Err(Failure::message(format!(
                        "seed {child} is closed; only open or in-progress missions can be adopted."
                    )));
                }
                if !seed.plan_id.is_empty() {
                    return Err(Failure::message(format!(
                        "seed {child} is already attached to plan {}; release it first.",
                        seed.plan_id
                    )));
                }
            }
        }
        let mut insertion = p.children().len();
        if at > 0 {
            insertion = at as usize - 1;
            if insertion > p.children().len() {
                return Err(Failure::message(format!(
                    "--at {at} is out of range (plan {id} has {} child{}; valid range 1..{}).",
                    p.children().len(),
                    if p.children().len() == 1 { "" } else { "ren" },
                    p.children().len() + 1
                )));
            }
        }
        for n in ["before", "after"] {
            if o.has(n) {
                let anchor = o.value(n);
                insertion = p
                    .children()
                    .iter()
                    .position(|id| id == anchor)
                    .ok_or_else(|| {
                        Failure::message(format!("--{n} {anchor} is not a child of plan {id}."))
                    })?
                    + usize::from(n == "after");
            }
        }
        let step_labels: Vec<_> = step
            .and_then(|i| steps.get(i as usize))
            .and_then(object)
            .and_then(|m| m.get("labels"))
            .and_then(array)
            .unwrap_or_default()
            .iter()
            .filter_map(|v| {
                if let V::String(s) = v {
                    Some(go_lower(s.trim()))
                } else {
                    None
                }
            })
            .filter(|s| !s.is_empty())
            .collect();
        let now = model::now_iso();
        let backref = Backref {
            plan_id: &id,
            parent_id: &parent.id,
            parent_title: &parent.title,
            template: &p.template,
            approach: p.section("approach"),
        };
        for child in ids {
            let seed = &mut issues[by_id[child]];
            if release {
                seed.plan_id.clear();
                seed.plan_step_index = None;
                seed.description = backref::strip(&seed.description);
                remove(&mut seed.blocks, &parent.id);
                remove(&mut parent.blocked_by, child);
            } else {
                seed.plan_id = id.clone();
                seed.plan_step_index = step;
                seed.description = backref.apply(&seed.description, step);
                add(&mut seed.blocks, &parent.id);
                for label in &step_labels {
                    add(&mut seed.labels, label)
                }
                add(&mut parent.blocked_by, child);
            }
            seed.updated_at = now.clone();
        }
        parent.updated_at = now.clone();
        issues[parent_index] = parent;
        if release {
            let mut children = Some(p.children().to_vec());
            let mut adopted = Some(p.adopted().to_vec());
            for child in ids {
                remove(&mut children, child);
                remove(&mut adopted, child)
            }
            p.children = Some(children.unwrap_or_default());
            p.adopted_children = adopted;
        } else {
            let mut children = p.children().to_vec();
            children.splice(insertion..insertion, ids.iter().cloned());
            p.children = Some(children);
            let mut adopted = Some(p.adopted().to_vec());
            for child in ids {
                add(&mut adopted, child)
            }
            p.adopted_children = adopted;
        }
        p.revision = p.revision.wrapping_add(1);
        p.updated_at = now;
        guard::plan_rewrite(&store)?;
        guard::array_changes(&store, p)?;
        guard::ticket_backrefs(&store, &before, &issues)?;
        lossy::refuse_lossy_ticket_changes(&store, &before, &issues)?;
        let revision = p.revision;
        store.write_tickets(&issues)?;
        store::write_records(&store.path(DataFile::Plans), &plans)?;
        revision
    };
    let command = if release {
        "plan release"
    } else {
        "plan adopt"
    };
    if o.has("json") {
        c.json(
            Envelope::new(true, command)
                .string("plan_id", &id)
                .raw(
                    if release { "released" } else { "adopted" },
                    &super::record::string_array(Some(ids)),
                )
                .integer("revision", revision),
        );
    } else {
        for child in ids {
            c.success(
                format!(
                    "{child} {} plan {id}",
                    if release {
                        "released from"
                    } else {
                        "adopted into"
                    }
                )
                .as_bytes(),
            );
        }
        c.success(format!("plan {id} revision bumped to {revision}").as_bytes());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_tree::tree::test_support::{invoke, project};
    use std::{fs, path::Path};
    const PLAN:&[u8]=br#"{"id":"pl-a","seed":"demo-1","template":"feature","status":"approved","revision":1,"sections":{"steps":[{"labels":[" X ","x",7]}],"approach":"Use the established contract","n":9007199254740993},"children":["old"],"adoptedChildren":["old"],"future":true}"#;
    const ISSUES:&[u8]=b"{\"id\":\"demo-1\",\"title\":\"Parent\",\"status\":\"open\",\"blockedBy\":[\"old\",\"outside\"]}\n{\"id\":\"old\",\"title\":\"Old\",\"status\":\"open\",\"plan_id\":\"pl-a\",\"blocks\":[\"demo-1\",\"other\"]}\n{\"id\":\"new\",\"title\":\"New\",\"status\":\"open\",\"description\":\"Manual text\"}\n";
    fn setup() -> tempfile::TempDir {
        let t = project("project: demo\nversion: \"1\"\n");
        fs::write(t.path().join(".luuna/missions/issues.jsonl"), ISSUES).unwrap();
        fs::write(t.path().join(".luuna/missions/plans.jsonl"), PLAN).unwrap();
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
    fn state(root: &Path) -> (super::super::record::Plan, Vec<crate::model::Ticket>) {
        let s = store::Store::open(root.join(".luuna/missions")).unwrap();
        (load(&s).unwrap().remove(0), s.read_tickets().unwrap())
    }
    #[test]
    fn adopt_inserts_at_requested_position_sets_step_and_keeps_manual_text() {
        let t = setup();
        let out = invoke(
            t.path(),
            &[
                "plan", "adopt", "pl-a", "new", "--before", "old", "--step", "1", "--json",
            ],
        );
        assert_eq!(out.exit_code, 0);
        let (p, issues) = state(t.path());
        assert_eq!(p.children(), &["new", "old"]);
        assert_eq!(p.adopted(), &["old", "new"]);
        assert_eq!(p.revision, 2);
        assert_eq!(issues[2].plan_step_index, Some(0));
        assert_eq!(issues[2].labels.as_ref().unwrap(), &["x"]);
        assert!(issues[2].description.ends_with("Manual text"));
        assert_eq!(issues[2].blocks.as_ref().unwrap(), &["demo-1"]);
        assert_eq!(
            issues[0].blocked_by.as_ref().unwrap(),
            &["old", "outside", "new"]
        );
        let raw = String::from_utf8(files(t.path())[1].0.clone()).unwrap();
        assert!(raw.contains("9007199254740993"));
    }
    #[test]
    fn position_and_duplicate_errors_are_read_only() {
        for (args, error) in [
            (vec!["new", "--at", "3"], "valid range 1..2"),
            (vec!["new", "--before", "missing"], "is not a child"),
            (vec!["new", "--after", " "], "requires a seed id"),
            (vec!["new", "--at", "01"], "must be a positive integer"),
            (
                vec!["new", "--at", "1", "--before", "old"],
                "mutually exclusive",
            ),
            (vec!["new", "new"], "Duplicate seed id"),
            (vec!["new", "--step", "2"], "has 1 step"),
        ] {
            let t = setup();
            let before = files(t.path());
            let mut cmd = vec!["plan", "adopt", "pl-a"];
            cmd.extend(args);
            let out = invoke(t.path(), &cmd);
            assert_eq!(out.exit_code, 1);
            assert!(String::from_utf8(out.stderr).unwrap().contains(error));
            assert_eq!(files(t.path()), before);
        }
    }
    #[test]
    fn release_removes_only_parent_edge_and_generated_backref() {
        let t = setup();
        assert_eq!(
            invoke(t.path(), &["plan", "adopt", "pl-a", "new", "--step", "1"]).exit_code,
            0
        );
        let out = invoke(t.path(), &["plan", "release", "pl-a", "new", "--json"]);
        assert_eq!(out.exit_code, 0);
        let (p, issues) = state(t.path());
        assert_eq!(p.children(), &["old"]);
        assert_eq!(p.adopted(), &["old"]);
        assert_eq!(issues[2].description, "Manual text");
        assert!(issues[2].plan_id.is_empty());
        assert_eq!(issues[2].plan_step_index, None);
        assert_eq!(issues[2].blocks, None);
        assert_eq!(issues[0].blocked_by.as_ref().unwrap(), &["old", "outside"]);
        assert_eq!(issues[2].labels.as_ref().unwrap(), &["x"]);
        assert_eq!(
            invoke(t.path(), &["plan", "release", "pl-a", "old"]).exit_code,
            0
        );
        let (p, issues) = state(t.path());
        assert_eq!(p.children, Some(vec![]));
        assert_eq!(issues[1].blocks.as_ref().unwrap(), &["other"]);
    }
    #[test]
    fn reorder_changes_only_plan_order_not_issue_indices_or_links() {
        let t = setup();
        assert_eq!(
            invoke(t.path(), &["plan", "adopt", "pl-a", "new"]).exit_code,
            0
        );
        let before = files(t.path());
        let out = invoke(
            t.path(),
            &["plan", "reorder", "pl-a", "new", "old", "--json"],
        );
        assert_eq!(out.exit_code, 0);
        let (p, _) = state(t.path());
        assert_eq!(p.children(), &["new", "old"]);
        assert_eq!(files(t.path())[0], before[0]);
        assert_eq!(p.revision, 3);
    }
    #[test]
    fn reorder_rejects_extra_before_missing_and_requires_every_child() {
        for (ids, message) in [
            (vec!["absent"], "is not a child"),
            (vec!["old", "other"], "is not a child"),
            (vec!["old", "old"], "Duplicate seed id"),
        ] {
            let t = setup();
            let before = files(t.path());
            let mut args = vec!["plan", "reorder", "pl-a"];
            args.extend(ids);
            let out = invoke(t.path(), &args);
            assert_eq!(out.exit_code, 1);
            assert!(String::from_utf8(out.stderr).unwrap().contains(message));
            assert_eq!(files(t.path()), before);
        }
        let t = setup();
        invoke(t.path(), &["plan", "adopt", "pl-a", "new"]);
        let before = files(t.path());
        let out = invoke(t.path(), &["plan", "reorder", "pl-a", "new"]);
        assert_eq!(out.exit_code, 1);
        assert!(String::from_utf8(out.stderr)
            .unwrap()
            .contains("missing: old"));
        assert_eq!(files(t.path()), before);
    }
    #[test]
    fn rewrite_refusals_preserve_both_files_and_modes() {
        for (file, raw) in [
            ("issues.jsonl", [ISSUES, b"unreadable\n"].concat()),
            ("plans.jsonl", [PLAN, b"\nunreadable\n"].concat()),
            (
                "plans.jsonl",
                String::from_utf8(PLAN.to_vec())
                    .unwrap()
                    .replace("[\"old\"]", "[\"old\",17]")
                    .into_bytes(),
            ),
        ] {
            let t = setup();
            fs::write(t.path().join(".luuna/missions").join(file), raw).unwrap();
            let before = files(t.path());
            let out = invoke(t.path(), &["plan", "adopt", "pl-a", "new", "--json"]);
            assert_eq!(out.exit_code, 1);
            assert!(String::from_utf8(out.stdout)
                .unwrap()
                .contains("Nothing was written"));
            assert_eq!(files(t.path()), before);
            assert!(fs::read_dir(t.path().join(".luuna/missions"))
                .unwrap()
                .all(|e| !e.unwrap().file_name().to_string_lossy().contains(".lock")));
        }
    }
    #[test]
    fn release_preflight_protects_retained_manual_encoding() {
        let t = setup();
        let data=String::from_utf8(ISSUES.to_vec()).unwrap().replace("\"plan_id\":\"pl-a\"","\"description\":\"<!-- missions:plan-backref:start -->generated<!-- missions:plan-backref:end -->\\ud800\",\"plan_id\":\"pl-a\"");
        fs::write(t.path().join(".luuna/missions/issues.jsonl"), data).unwrap();
        let before = files(t.path());
        let out = invoke(t.path(), &["plan", "release", "pl-a", "old"]);
        assert_eq!(out.exit_code, 1);
        assert_eq!(files(t.path()), before);
    }
}
