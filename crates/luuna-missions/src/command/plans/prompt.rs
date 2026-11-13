//! Structured planning prompts and optional Atlas prior art.
const INSTRUCTIONS: &str = r###"Fill every section. Required fields are marked. Use prior_art entries to ground decisions. Reply with JSON shaped { "template": "<name>", "name": "<short label>", "sections": { "<section-name>": <value>, ... } } — drop the plan_request wrapper, and sections in your reply is an object keyed by name (not the array of section metadata above). The top-level `name` field is an optional short human-readable label (e.g. "Schema-driven config editor"); if you omit it, luuna-missions plan submit derives one from the parent seed title. Each step is shaped { title?, type?, priority?, blocks?: number[], labels?: string[], plan_template?, existing_seed? }. In each step, `blocks` lists 1-based step indices that this step blocks (step 1 is the first step, step N is the last); e.g. step 1 with `blocks: [2]` means step 1 must finish before step 2 starts. Leave empty if nothing depends on it. Optional `labels` is an array of non-empty strings applied to the spawned (or adopted) child seed; values are normalized (lowercased, trimmed, deduped) and merged additively on adoption — they never clobber existing labels."###;

use super::{atlas, common::*};
use crate::{
    command::{Context, Failure},
    config_tree::{self, SectionKind},
    error::Error,
};
pub(super) fn run(c: &mut Context<'_>, args: &[String]) -> Result<(), Failure> {
    let o = parse(args, &["template", "domain"], &[])?;
    let seed_id = need(&o, "prompt <seed-id> [options]")?;
    let store = c.store()?;
    let issues = store.read_tickets()?;
    let seed = issues
        .iter()
        .find(|t| t.id == seed_id)
        .ok_or_else(|| Failure::message(format!("Seed not found: {seed_id}")))?;
    let templates = config_tree::load_plan_templates(store.dir())?;
    let mut template_name = o.value("template").to_owned();
    if template_name.is_empty() {
        template_name = atlas::inherited(seed, &store)?;
        if template_name.is_empty() {
            template_name = config_tree::default_template_for_type(&seed.ticket_type).into();
        }
    }
    let template = templates.get(&template_name).ok_or_else(|| {
        Failure::message(format!(
            "Unknown template: {template_name}. Available: {}",
            template_names(&templates).join(", ")
        ))
    })?;
    let domain = atlas::infer(c, seed, o.value("domain"));
    let mut sections = vec![];
    for name in section_names(template) {
        let spec = &template.sections[&name];
        let mut fields = vec![
            ("name", quoted(&name)),
            (
                "required",
                if spec.required {
                    b"true".to_vec()
                } else {
                    b"false".to_vec()
                },
            ),
            ("kind", kind_json(&spec.kind)),
            ("prompt", quoted(&spec.prompt)),
            (
                "prior_art",
                array_json(atlas::prior(c, &domain, &name, &spec.atlas_source)),
            ),
        ];
        if let Some(n) = spec.min_length {
            fields.push(("min_length", n.to_string().into_bytes()));
        }
        if let Some(n) = spec.min {
            fields.push(("min", n.to_string().into_bytes()));
        }
        if let Some(item) = &spec.item {
            fields.push(("item", item_json(item)));
        }
        sections.push(object_json(fields));
    }
    let min_steps = template
        .sections
        .get("steps")
        .and_then(|s| s.min)
        .unwrap_or(0);
    let min_acceptance = template
        .sections
        .get("acceptance")
        .and_then(|s| s.min)
        .unwrap_or(0);
    if o.has("json") {
        let validation = object_json([
            ("all_required_present", b"true".to_vec()),
            ("min_steps", min_steps.to_string().into_bytes()),
            ("min_acceptance", min_acceptance.to_string().into_bytes()),
        ]);
        let request = object_json([
            ("seed", quoted(seed_id)),
            ("template", quoted(&template_name)),
            ("instructions", quoted(INSTRUCTIONS)),
            ("sections", array_json(sections)),
            ("validation", validation),
        ]);
        let raw = object_json([
            ("success", b"true".to_vec()),
            ("command", quoted("plan prompt")),
            ("plan_request", request),
        ]);
        output(c, &raw, "cli.planRequest")?;
    } else {
        c.stdout.extend_from_slice(format!("Plan prompt for {seed_id}\nTemplate: {template_name}\nSeed title: {}\n\n{INSTRUCTIONS}\n\n",seed.title).as_bytes());
        for name in section_names(template) {
            let s = &template.sections[&name];
            let tag = if s.required { "required" } else { "optional" };
            let kind = match s.kind {
                SectionKind::Text => "text",
                SectionKind::List => "list",
                SectionKind::Steps => "steps",
                SectionKind::Object(_) => "object",
            };
            c.stdout.extend_from_slice(
                format!("  {name} ({kind}) {tag}\n    {}\n", s.prompt).as_bytes(),
            );
            if let Some(n) = s.min_length {
                c.stdout
                    .extend_from_slice(format!("    min_length: {n}\n").as_bytes());
            }
            if let Some(n) = s.min {
                c.stdout
                    .extend_from_slice(format!("    min entries: {n}\n").as_bytes());
            }
        }
        c.stdout.extend_from_slice(format!("\nPipe --json into a file the LLM fills, then run:\n  luuna-missions plan submit {seed_id} --plan <file>\n").as_bytes());
        if c.stdout.len() > 128 * 1024 * 1024 {
            return Err(Error::unsupported(
                "plan output beyond 128 MiB",
                "none (bounded native plan output)",
            )
            .into());
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_tree::tree::test_support::{invoke, project};
    use std::fs;
    #[test]
    fn prompt_has_declared_sections_and_does_not_mutate_the_store() {
        let t = project("project: demo\nversion: \"1\"\n");
        let path = t.path().join(".luuna/missions/issues.jsonl");
        let before = b"{\"id\":\"demo-1\",\"title\":\"Example seed\",\"type\":\"feature\"}\n";
        fs::write(&path, before).unwrap();
        let out = invoke(t.path(), &["plan", "prompt", "demo-1", "--json"]);
        assert_eq!(out.exit_code, 0);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let p = &v["plan_request"];
        assert_eq!(p["instructions"], INSTRUCTIONS);
        assert_eq!(
            p["sections"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "context",
                "approach",
                "alternatives",
                "steps",
                "risks",
                "acceptance"
            ]
        );
        assert_eq!(
            p["validation"],
            serde_json::json!({"all_required_present":true,"min_steps":2,"min_acceptance":1})
        );
        assert_eq!(fs::read(path).unwrap(), before);
    }
    #[test]
    fn prompt_text_uses_the_seed_type_default_and_exact_next_command() {
        let t = project("project: demo\nversion: \"1\"\n");
        fs::write(
            t.path().join(".luuna/missions/issues.jsonl"),
            b"{\"id\":\"demo-1\",\"title\":\"Example bug\",\"type\":\"bug\"}\n",
        )
        .unwrap();
        let out = invoke(t.path(), &["plan", "prompt", "demo-1"]);
        assert_eq!(out.exit_code, 0);
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(
            text.starts_with("Plan prompt for demo-1\nTemplate: bug\nSeed title: Example bug\n\n")
        );
        assert!(text.ends_with("\nPipe --json into a file the LLM fills, then run:\n  luuna-missions plan submit demo-1 --plan <file>\n"));
        assert!(out.stderr.is_empty());
    }
}
