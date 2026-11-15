//! Plan template contracts and Go-compatible configuration loading.
use super::{legacy, tree::command, validate, yaml, TreeValue as V};
use crate::{
    config,
    error::{Error, Result},
};
use std::{collections::BTreeMap, fs, path::Path};
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlanTemplate {
    pub name: String,
    pub description: String,
    pub sections: BTreeMap<String, SectionSpec>,
    pub section_order: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SectionSpec {
    pub required: bool,
    pub kind: SectionKind,
    pub prompt: String,
    pub min_length: Option<i64>,
    pub min: Option<i64>,
    pub item: Option<SectionItem>,
    pub atlas_source: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SectionKind {
    Text,
    List,
    Steps,
    Object(BTreeMap<String, SectionSpec>),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SectionItem {
    Text,
    Object(BTreeMap<String, SectionSpec>),
}
fn spec(required: bool, kind: SectionKind, prompt: &str) -> SectionSpec {
    SectionSpec {
        required,
        kind,
        prompt: prompt.into(),
        min_length: None,
        min: None,
        item: None,
        atlas_source: String::new(),
    }
}
fn template(name: &str, description: &str, sections: Vec<(&str, SectionSpec)>) -> PlanTemplate {
    PlanTemplate {
        name: name.into(),
        description: description.into(),
        section_order: sections.iter().map(|(k, _)| (*k).into()).collect(),
        sections: sections.into_iter().map(|(k, v)| (k.into(), v)).collect(),
    }
}
pub(crate) fn built_in_templates() -> BTreeMap<String, PlanTemplate> {
    use SectionKind::{List, Steps, Text};
    let mut context = spec(
        true,
        Text,
        "Why does this work need to happen? What problem or opportunity drives it?",
    );
    context.min_length = Some(50);
    let mut steps = spec(
        true,
        Steps,
        "Decompose into ordered, independent implementation steps. Each becomes a child seed.",
    );
    steps.min = Some(2);
    let mut alternatives = spec(
        false,
        List,
        "What other approaches were considered and rejected?",
    );
    alternatives.item = Some(SectionItem::Object(BTreeMap::from([
        ("name".into(), spec(true, Text, "")),
        ("rejected_because".into(), spec(true, Text, "")),
    ])));
    let mut risks=spec(false,List,"What could go wrong? Known failure modes from prior work are pre-filled when luuna-atlas is available.");
    risks.item = Some(SectionItem::Text);
    risks.atlas_source = "failure".into();
    let mut acceptance = spec(
        true,
        List,
        "Concrete, verifiable conditions for plan completion.",
    );
    acceptance.item = Some(SectionItem::Text);
    acceptance.min = Some(1);
    let feature = template(
        "feature",
        "New capability or significant change. Default for type: feature.",
        vec![
            ("context", context),
            (
                "approach",
                spec(
                    true,
                    Text,
                    "What's the chosen approach, and why this over alternatives?",
                ),
            ),
            ("alternatives", alternatives),
            ("steps", steps),
            ("risks", risks),
            ("acceptance", acceptance),
        ],
    );
    let mut reproduction = spec(
        true,
        Text,
        "Concrete steps to reproduce. Inputs, environment, observed vs. expected.",
    );
    reproduction.min_length = Some(50);
    let mut cause = spec(
        true,
        Text,
        "What's actually broken? Trace the defect to its source, not just the symptom.",
    );
    cause.min_length = Some(50);
    let mut steps = spec(true, Steps, "Ordered fix steps. Each becomes a child seed.");
    steps.min = Some(1);
    let mut acceptance = spec(
        true,
        List,
        "Verifiable conditions: regression test, behavior, etc.",
    );
    acceptance.item = Some(SectionItem::Text);
    acceptance.min = Some(1);
    let bug = template(
        "bug",
        "Defect fix. Adds reproduction and root_cause sections. Default for type: bug.",
        vec![
            (
                "context",
                spec(
                    true,
                    Text,
                    "Why does fixing this matter? Who is affected and how?",
                ),
            ),
            ("reproduction", reproduction),
            ("root_cause", cause),
            (
                "approach",
                spec(
                    true,
                    Text,
                    "Chosen fix and the rationale for it over alternatives.",
                ),
            ),
            ("steps", steps),
            ("acceptance", acceptance),
        ],
    );
    let mut invariant=spec(true,Text,"The contract that MUST remain equal across the refactor. Be specific — this is what acceptance tests verify.");
    invariant.min_length = Some(50);
    let mut steps = spec(
        true,
        Steps,
        "Ordered restructuring steps. Each becomes a child seed.",
    );
    steps.min = Some(1);
    let mut acceptance = spec(true, List, "How we'll verify the invariant is preserved.");
    acceptance.item = Some(SectionItem::Text);
    acceptance.min = Some(1);
    let refactor=template("refactor","Internal restructuring. Adds behavior_invariant (must stay equal). Opt-in via --template refactor.",vec![("context",spec(true,Text,"Why this refactor? What pain does it relieve?")),("behavior_invariant",invariant),("approach",spec(true,Text,"Chosen restructuring strategy.")),("steps",steps),("acceptance",acceptance)]);
    [feature, bug, refactor]
        .into_iter()
        .map(|t| (t.name.clone(), t))
        .collect()
}
pub(crate) fn default_template_for_type(kind: &str) -> &'static str {
    if kind == "bug" {
        "bug"
    } else {
        "feature"
    }
}
pub(crate) fn max_plan_depth(config: &config::Config) -> i64 {
    config
        .max_plan_depth
        .unwrap_or(config::DEFAULT_MAX_PLAN_DEPTH)
}
fn describe(v: Option<&V>) -> &str {
    match v {
        None | Some(V::Null) => "null",
        Some(V::Str(_)) => "string",
        Some(V::Bool(_)) => "boolean",
        Some(V::Int(_) | V::Float(_)) => "number",
        Some(V::Seq(_)) => "array",
        Some(V::Map(_)) => "object",
        Some(V::Uint(_)) => "uint64",
        Some(V::Other(t)) => t,
    }
}
fn nested(raw: &[(String, V)], path: &str) -> Result<BTreeMap<String, SectionSpec>> {
    let mut raw = raw.iter().collect::<Vec<_>>();
    raw.sort_by_key(|(k, _)| k);
    raw.into_iter()
        .map(|(name, v)| Ok((name.clone(), parse_spec(v, &format!("{path}.{name}"))?)))
        .collect()
}
fn kind(raw: Option<&V>, path: &str) -> Result<SectionKind> {
    if let Some(V::Str(s)) = raw {
        match s.as_str() {
            "text" => return Ok(SectionKind::Text),
            "list" => return Ok(SectionKind::List),
            "steps" => return Ok(SectionKind::Steps),
            _ => {
                return Err(command(format!(
                    "{path}: unknown kind '{s}' (expected text|list|steps|object)"
                )))
            }
        }
    }
    if let Some(V::Map(m)) = raw {
        return Ok(SectionKind::Object(nested(m, path)?));
    }
    Err(command(format!(
        "{path}: unknown kind '{}' (expected text|list|steps|object)",
        describe(raw)
    )))
}
fn item(raw: &V, path: &str) -> Result<SectionItem> {
    match raw {
        V::Str(s) if s == "text" => Ok(SectionItem::Text),
        V::Map(m) => Ok(SectionItem::Object(nested(m, path)?)),
        v => Err(command(format!(
            "{path}: must be 'text' or an object spec (got: {})",
            describe(Some(v))
        ))),
    }
}
fn parse_spec(value: &V, path: &str) -> Result<SectionSpec> {
    if !matches!(value, V::Map(_)) {
        return Err(command(format!("{path}: must be a mapping")));
    }
    let required = match value.get("required") {
        Some(V::Bool(v)) => *v,
        v => {
            return Err(command(format!(
                "{path}.required: must be a boolean (got: {})",
                describe(v)
            )))
        }
    };
    let prompt = match value.get("prompt").and_then(V::text) {
        Some(v) => v.to_owned(),
        None => {
            return Err(command(format!(
                "{path}.prompt: must be a string (got: {})",
                describe(value.get("prompt"))
            )))
        }
    };
    Ok(SectionSpec {
        required,
        prompt,
        kind: kind(value.get("kind"), &format!("{path}.kind"))?,
        min_length: value.get("min_length").and_then(validate::whole),
        min: value.get("min").and_then(validate::whole),
        item: value
            .get("item")
            .map(|v| item(v, &format!("{path}.item")))
            .transpose()?,
        atlas_source: value
            .get("atlas_source")
            .and_then(V::text)
            .unwrap_or_default()
            .into(),
    })
}
pub(crate) fn load_plan_templates(dir: &Path) -> Result<BTreeMap<String, PlanTemplate>> {
    let mut result = built_in_templates();
    let path = dir.join("config.yaml");
    let bytes = match fs::read(&path) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        Err(e) => return Err(Error::io("read", &path, e)),
    };
    let tree = legacy::upgrade_legacy_config_keys(
        yaml::decode_document(&bytes)
            .map_err(|e| yaml::in_file(&path, e))?
            .unwrap_or(V::Map(Vec::new())),
    );
    let orders = yaml::key_orders(&bytes)?;
    let Some(V::Map(user)) = tree.get("plan_templates") else {
        return Ok(result);
    };
    let mut user = user.iter().collect::<Vec<_>>();
    user.sort_by_key(|(k, _)| k);
    for (name, raw) in user {
        if !matches!(raw, V::Map(_)) {
            return Err(command(format!("plan_templates.{name} must be a mapping")));
        }
        let Some(V::Map(sections)) = raw.get("sections") else {
            return Err(command(format!(
                "plan_templates.{name}.sections must be a mapping"
            )));
        };
        let path = format!("plan_templates.{name}.sections");
        let sections = nested(sections, &path)?;
        let value = PlanTemplate {
            name: name.clone(),
            description: raw
                .get("description")
                .and_then(V::text)
                .unwrap_or_default()
                .into(),
            section_order: orders
                .get(&format!("/plan_templates/{name}/sections"))
                .cloned()
                .unwrap_or_default(),
            sections,
        };
        result.insert(name.clone(), value);
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn load(text: &str) -> Result<BTreeMap<String, PlanTemplate>> {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("config.yaml"), text).unwrap();
        load_plan_templates(t.path())
    }
    #[test]
    fn built_in_template_order() {
        let all = built_in_templates();
        assert_eq!(all.len(), 3);
        assert_eq!(
            all["feature"].section_order,
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
            all["bug"].section_order,
            [
                "context",
                "reproduction",
                "root_cause",
                "approach",
                "steps",
                "acceptance"
            ]
        );
        assert_eq!(
            all["refactor"].section_order,
            [
                "context",
                "behavior_invariant",
                "approach",
                "steps",
                "acceptance"
            ]
        );
        assert_eq!(all["feature"].sections["context"].min_length, Some(50));
        assert_eq!(all["feature"].sections["steps"].min, Some(2));
        assert_eq!(default_template_for_type("bug"), "bug");
        assert_eq!(default_template_for_type("task"), "feature");
        assert_eq!(max_plan_depth(&config::Config::default()), 3);
        let t = tempfile::tempdir().unwrap();
        fs::write(
            t.path().join("config.yaml"),
            "project: demo\nversion: \"1\"\n",
        )
        .unwrap();
        assert_eq!(load_plan_templates(t.path()).unwrap(), all);
    }
    #[test]
    fn custom_template_keeps_declaration_order() {
        let all=load("project: demo\nversion: \"1\"\nplan_templates:\n  feature:\n    description: custom\n    sections:\n      zeta: {required: true, kind: text, prompt: Z}\n      alpha: {required: false, kind: list, prompt: A, item: text, min: 2.0}\n").unwrap();
        assert_eq!(all["feature"].section_order, ["zeta", "alpha"]);
        assert_eq!(all["feature"].description, "custom");
        assert_eq!(all["feature"].sections["alpha"].min, Some(2));
        assert_eq!(
            all["feature"].sections["alpha"].item,
            Some(SectionItem::Text)
        );
    }
    #[test]
    fn each_template_error_has_go_s_text() {
        for (raw,error) in [("plan_templates: {x: 5}","plan_templates.x must be a mapping"),("plan_templates: {x: {}}","plan_templates.x.sections must be a mapping"),("plan_templates: {x: {sections: {a: 5}}}","plan_templates.x.sections.a: must be a mapping"),("plan_templates: {x: {sections: {a: {}}}}","plan_templates.x.sections.a.required: must be a boolean (got: null)"),("plan_templates: {x: {sections: {a: {required: true}}}}","plan_templates.x.sections.a.prompt: must be a string (got: null)"),("plan_templates: {x: {sections: {a: {required: true, prompt: x, kind: object}}}}","plan_templates.x.sections.a.kind: unknown kind 'object' (expected text|list|steps|object)"),("plan_templates: {x: {sections: {a: {required: true, prompt: x, kind: text, item: 5}}}}","plan_templates.x.sections.a.item: must be 'text' or an object spec (got: number)")]{assert_eq!(load(raw).unwrap_err().to_string(),error);}
    }
    #[test]
    fn several_bad_entries_report_the_smallest_name() {
        assert_eq!(
            load("plan_templates: {z: 5, a: 6}")
                .unwrap_err()
                .to_string(),
            "plan_templates.a must be a mapping"
        );
        assert_eq!(
            load("plan_templates: {x: {sections: {z: 5, a: 6}}}")
                .unwrap_err()
                .to_string(),
            "plan_templates.x.sections.a: must be a mapping"
        );
    }
    #[test]
    fn the_alias_guard_applies_to_plan_templates() {
        const BOMB:&str="plan_templates:\n  a: &a [x,x,x,x,x,x,x,x,x]\n  b: &b [*a,*a,*a,*a,*a,*a,*a,*a,*a]\n  c: &c [*b,*b,*b,*b,*b,*b,*b,*b,*b]\n  d: &d [*c,*c,*c,*c,*c,*c,*c,*c,*c]\n  e: &e [*d,*d,*d,*d,*d,*d,*d,*d,*d]\n  f: &f [*e,*e,*e,*e,*e,*e,*e,*e,*e]\n  g: &g [*f,*f,*f,*f,*f,*f,*f,*f,*f]\n  h: &h [*g,*g,*g,*g,*g,*g,*g,*g,*g]\n  i: [*h,*h,*h,*h,*h,*h,*h,*h,*h]\n";
        assert_eq!(
            load(BOMB).unwrap_err().to_string(),
            "yaml: document contains excessive aliasing"
        );
    }
    #[test]
    fn a_missing_config_gives_the_built_ins() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(load_plan_templates(t.path()).unwrap(), built_in_templates());
    }
}
