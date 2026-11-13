//! Native plan templates, listing, showing and validation.
use super::{common::*, record::Plan, tree::Tree, validation};
use crate::{
    command::{output::Envelope, read::sprint, Context, Failure},
    config_tree::{self, PlanTemplate, SectionKind},
    error::Error,
    json::{self, Value as V},
};
use std::collections::BTreeMap;
pub(super) fn templates(c: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let o = parse(args, &[], &[])?;
    let store = c.store()?;
    let ts = config_tree::load_plan_templates(store.dir())?;
    if o.has("json") {
        let entries = array_json(ts.iter().map(|(name, t)| {
            compact(
                Envelope::object()
                    .string("name", name)
                    .string("description", &t.description),
            )
        }));
        c.json(
            Envelope::new(true, "plan templates")
                .raw("templates", &entries)
                .integer("count", ts.len() as i64),
        );
    } else {
        c.stdout.extend_from_slice(b"Available templates:\n");
        for (name, t) in ts {
            c.stdout.extend_from_slice(
                format!(
                    "  {name}{}\n",
                    if t.description.is_empty() {
                        String::new()
                    } else {
                        format!("  {}", t.description)
                    }
                )
                .as_bytes(),
            );
        }
    }
    Ok(())
}
pub(super) fn list(c: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let o = parse(args, &["seed", "status", "outcome", "template"], &[])?;
    let status = o.value("status");
    let outcome = o.value("outcome");
    if !status.is_empty() && !["draft", "approved", "active", "done"].contains(&status) {
        return Err(Failure::message(format!(
            "Invalid --status value: {status}. Valid: draft|approved|active|done"
        )));
    }
    if !outcome.is_empty() && !["success", "partial", "failure"].contains(&outcome) {
        return Err(Failure::message(format!(
            "Invalid --outcome value: {outcome}. Valid: success|partial|failure"
        )));
    }
    let store = c.store()?;
    let plans = load(&store)?;
    let mut selected: Vec<_> = plans
        .iter()
        .filter(|p| {
            [
                ("seed", p.seed.as_str()),
                ("status", p.status.as_str()),
                ("outcome", p.outcome.as_str()),
                ("template", p.template.as_str()),
            ]
            .into_iter()
            .all(|(key, value)| o.value(key).is_empty() || o.value(key) == value)
        })
        .collect();
    selected.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    if o.has("json") {
        let raw = selected
            .iter()
            .map(|p| p.marshal())
            .collect::<crate::Result<Vec<_>>>()?;
        let raw = object_json([
            ("success", b"true".to_vec()),
            ("command", quoted("plan list")),
            ("plans", array_json(raw)),
            ("count", selected.len().to_string().into_bytes()),
        ]);
        output(c, &raw, "model.Plan")?;
    } else if selected.is_empty() {
        c.stdout.extend_from_slice(b"No plans match.\n");
    } else {
        for p in selected {
            let name = if p.name.is_empty() {
                "(unnamed)"
            } else {
                &p.name
            };
            let mut name = if name.encode_utf16().count() > 40 {
                if name.chars().count() < 39 {
                    return Err(Error::unsupported("plan list truncation of a name over 40 UTF-16 units with fewer than 39 runes","none (the reference Go tool panics for this input)").into());
                }
                name.chars().take(39).collect::<String>() + "…"
            } else {
                name.into()
            };
            let pad = 40usize.saturating_sub(name.encode_utf16().count());
            name.extend(std::iter::repeat_n(' ', pad));
            c.stdout.extend_from_slice(
                format!(
                    "{}  {}  rev {}  {name}  {}  seed={}  children={}{}  {}\n",
                    p.id,
                    p.status,
                    p.revision,
                    p.template,
                    p.seed,
                    p.children().len(),
                    if p.outcome.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", p.outcome)
                    },
                    p.created_at
                )
                .as_bytes(),
            );
        }
    }
    Ok(())
}
pub(super) fn show(c: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let o = parse(args, &[], &[])?;
    let id = need(&o, "show <id> [options]")?;
    let store = c.store()?;
    let id = resolve(&store, id)?;
    let settings = crate::config::read_config(store.dir())?;
    let max_depth = config_tree::max_plan_depth(&settings);
    let plans = load(&store)?;
    let plan = &plans[plan_index(&plans, &id).ok_or_else(|| missing(&id))?];
    let issues = store.read_tickets()?;
    let templates = config_tree::load_plan_templates(store.dir())?;
    let tree = Tree::build(plan, &plans, &issues, max_depth)?;
    if o.has("json") {
        let raw = object_json([
            ("success", b"true".to_vec()),
            ("command", quoted("plan show")),
            ("plan", tree.plan_json().to_vec()),
            ("children", tree.children_json(0)),
            ("children_plans", tree.nested_json()?),
        ]);
        output(c, &raw, "model.Plan")?;
        return Ok(());
    }
    c.stdout.extend_from_slice(
        format!("{}  {}  rev {}\n", plan.id, plan.status, plan.revision).as_bytes(),
    );
    if !plan.name.is_empty() {
        c.stdout
            .extend_from_slice(format!("Name:     {}\n", plan.name).as_bytes());
    }
    c.stdout.extend_from_slice(
        format!(
            "Seed:     {}\nTemplate: {}\nCreated:  {}\nUpdated:  {}\n",
            plan.seed, plan.template, plan.created_at, plan.updated_at
        )
        .as_bytes(),
    );
    if !plan.outcome.is_empty() {
        c.stdout.extend_from_slice(
            format!(
                "Outcome:  {}{}\n",
                plan.outcome,
                if plan.outcome_note.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", plan.outcome_note)
                }
            )
            .as_bytes(),
        );
    }
    if plan.reviewed_by.is_empty() && ["approved", "active"].contains(&plan.status.as_str()) {
        c.stdout
            .extend_from_slice(b"Review suggested (no reviewer recorded yet)\n");
    }
    if !plan.reviewed_by.is_empty() {
        c.stdout
            .extend_from_slice(format!("Reviewed: {}\n", plan.reviewed_by).as_bytes());
    }
    c.stdout.extend_from_slice(b"\nSections:\n");
    render_sections(c, plan, templates.get(&plan.template))?;
    c.stdout
        .extend_from_slice(format!("\nChildren ({}):\n", tree.child_count(0)).as_bytes());
    if tree.child_count(0) == 0 {
        c.stdout.extend_from_slice(b"  (none)\n");
    } else {
        tree.children_text(0, "  ", &mut c.stdout)?;
    }
    tree.nested_text(&mut c.stdout)?;
    if c.stdout.len() > 128 * 1024 * 1024 {
        return Err(Error::unsupported(
            "plan output beyond 128 MiB",
            "none (bounded native plan output)",
        )
        .into());
    }
    Ok(())
}
fn render_sections(
    c: &mut Context<'_>,
    p: &Plan,
    template: Option<&PlanTemplate>,
) -> Result<(), Failure> {
    let mut names = vec![];
    let base = [
        "context",
        "approach",
        "alternatives",
        "steps",
        "risks",
        "acceptance",
    ];
    for name in base {
        if p.section(name).is_some() {
            names.push(name.to_owned());
        }
    }
    names.extend(
        p.section_keys()
            .into_iter()
            .filter(|n| !base.contains(&n.as_str())),
    );
    for name in names {
        let value = p.section(&name).unwrap();
        c.stdout.extend_from_slice(format!("  {name}\n").as_bytes());
        match value {
            V::Null => c.stdout.extend_from_slice(b"    (empty)\n"),
            V::String(text) => {
                for line in text.split('\n') {
                    c.stdout
                        .extend_from_slice(format!("    {line}\n").as_bytes());
                }
            }
            V::Array(items) => {
                if items.is_empty() {
                    c.stdout.extend_from_slice(b"    (none)\n");
                }
                for (i, v) in items.iter().enumerate() {
                    let steps = template
                        .and_then(|t| t.sections.get(&name))
                        .is_some_and(|s| matches!(s.kind, SectionKind::Steps));
                    render_entry(c, i + 1, v, steps)?;
                }
            }
            _ => {
                c.stdout.extend_from_slice(b"    ");
                c.stdout.extend(encoded(value, true)?);
                c.stdout.push(b'\n');
            }
        }
    }
    Ok(())
}
fn render_entry(c: &mut Context<'_>, number: usize, value: &V, steps: bool) -> Result<(), Failure> {
    let marker = format!("    {number}.");
    if let V::String(s) = value {
        c.stdout
            .extend_from_slice(format!("{marker} {s}\n").as_bytes());
        return Ok(());
    }
    let Some(data) = object(value).filter(|_| steps) else {
        c.stdout.extend_from_slice(
            format!(
                "{marker} {}\n",
                String::from_utf8(encoded(value, true)?).unwrap()
            )
            .as_bytes(),
        );
        return Ok(());
    };
    let title = text(data.get("title"));
    let title = if !title.is_empty() {
        title.to_owned()
    } else if let Some(V::String(adopt)) = data.get("existing_seed") {
        format!("(adopt {adopt})")
    } else {
        String::from_utf8(encoded(value, true)?).unwrap()
    };
    c.stdout
        .extend_from_slice(format!("{marker} {title}\n").as_bytes());
    let indent = " ".repeat(marker.len() + 1);
    if let Some(blocks) = data.get("blocks").and_then(array).filter(|a| !a.is_empty()) {
        c.stdout.extend_from_slice(
            format!(
                "{indent}blocks: {}\n",
                blocks.iter().map(sprint).collect::<Vec<_>>().join(", ")
            )
            .as_bytes(),
        );
    }
    for field in [
        "requires_plan",
        "plan_template",
        "type",
        "priority",
        "labels",
        "existing_seed",
    ] {
        let Some(value) = data.get(field) else {
            continue;
        };
        if field == "existing_seed" && matches!(data.get("title"), None | Some(V::Null)) {
            continue;
        }
        if field == "requires_plan" && value != &V::Bool(true) {
            continue;
        }
        if matches!(value,V::String(s)if s.is_empty()) {
            continue;
        }
        let rendered = if field == "labels" {
            array(value)
                .map(|a| a.iter().map(sprint).collect::<Vec<_>>().join(", "))
                .unwrap_or_else(|| sprint(value))
        } else {
            sprint(value)
        };
        if field == "labels" && rendered.is_empty() {
            continue;
        }
        c.stdout
            .extend_from_slice(format!("{indent}{field}: {rendered}\n").as_bytes());
    }
    Ok(())
}
pub(super) fn validate(c: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let o = parse(args, &[], &[])?;
    let id = need(&o, "validate <id> [options]")?;
    let store = c.store()?;
    let id = resolve(&store, id)?;
    let plans = load(&store)?;
    let p = &plans[plan_index(&plans, &id).ok_or_else(|| missing(&id))?];
    let templates = config_tree::load_plan_templates(store.dir())?;
    let template = templates.get(&p.template).ok_or_else(|| {
        Failure::message(format!(
            "Plan {id} references unknown template '{}'. Available: {}.",
            p.template,
            template_names(&templates).join(", ")
        ))
    })?;
    let current = V::Object(BTreeMap::from([
        ("template".into(), V::String(p.template.clone())),
        (
            "sections".into(),
            p.sections
                .as_ref()
                .map(|m| V::Object(m.clone()))
                .unwrap_or(V::Null),
        ),
    ]));
    let errors = validation::validate(template, &current);
    if errors.is_empty() {
        if o.has("json") {
            c.json(
                Envelope::new(true, "plan validate")
                    .boolean("valid", true)
                    .string("plan_id", &id),
            );
        } else {
            c.success(format!("plan {id} valid").as_bytes());
        }
    } else {
        let diff = validation::diff_json(&errors, &current)?;
        if json::exceeds_max_depth(&diff) || !super::raw::pretty_fits(&diff, 128 * 1024 * 1024) {
            return Err(Error::unsupported(
                "plan validation diff exceeds native output bounds",
                "none (bounded native plan output)",
            )
            .into());
        }
        c.stderr
            .extend(json::to_output(&diff).map_err(|e| Failure::message(e.to_string()))?);
        c.exit_code = 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        command::{run, Invocation},
        config_tree::tree::test_support::{invoke, project},
    };
    use std::fs;
    #[test]
    fn listing_is_newest_first_with_stable_ties_and_filters() {
        let t = project("project: demo\nversion: \"1\"\n");
        fs::write(t.path().join(".luuna/missions/plans.jsonl"),b"{\"id\":\"pl-a\",\"status\":\"draft\",\"createdAt\":\"z\"}\n{\"id\":\"pl-b\",\"status\":\"draft\",\"createdAt\":\"z\"}\n{\"id\":\"pl-c\",\"status\":\"approved\",\"createdAt\":\"a\"}\n").unwrap();
        let out = invoke(t.path(), &["plan", "list", "--status", "draft", "--json"]);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["count"], 2);
        assert_eq!(v["plans"][0]["id"], "pl-a");
        assert_eq!(v["plans"][1]["id"], "pl-b");
    }
    #[test]
    fn go_unicode_list_panic_shape_is_an_explicit_refusal() {
        let t = project("project: demo\nversion: \"1\"\n");
        let path = t.path().join(".luuna/missions/plans.jsonl");
        let bytes = format!("{{\"id\":\"pl-a\",\"name\":\"{}\"}}\n", "😀".repeat(21));
        fs::write(&path, &bytes).unwrap();
        let err = run(&Invocation::new(t.path(), ["plan", "list"])).unwrap_err();
        assert!(matches!(err, Error::Unsupported { .. }));
        let json = invoke(t.path(), &["plan", "list", "--json"]);
        assert_eq!(json.exit_code, 0);
        let v: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
        assert_eq!(v["plans"][0]["name"], "😀".repeat(21));
        assert_eq!(fs::read(path).unwrap(), bytes.as_bytes());
    }
    #[test]
    fn invalid_validation_writes_a_diff_to_stderr_even_under_json() {
        let t = project("project: demo\nversion: \"1\"\n");
        let path = t.path().join(".luuna/missions/plans.jsonl");
        let before = b"{\"id\":\"pl-a\",\"template\":\"feature\",\"sections\":{}}\n";
        fs::write(&path, before).unwrap();
        let out = invoke(t.path(), &["plan", "validate", "pl-a", "--json"]);
        assert_eq!(out.exit_code, 1);
        assert!(out.stdout.is_empty());
        let v: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(v["errors"][0]["path"], "sections.context");
        assert_eq!(
            v["current"],
            serde_json::json!({"sections":{},"template":"feature"})
        );
        assert_eq!(fs::read(path).unwrap(), before);
    }
}
