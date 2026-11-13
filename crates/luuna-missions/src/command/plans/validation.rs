//! Go's partial-state validation contract, with iterative nested specification checks.
use super::common::*;
use crate::{
    command::output::Envelope,
    config_tree::{PlanTemplate, SectionItem, SectionKind, SectionSpec},
    json::Value as V,
};
use std::collections::BTreeMap;
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Entry {
    pub path: String,
    pub code: String,
    pub fix: String,
}
fn entry(path: &str, code: &str, fix: String) -> Entry {
    Entry {
        path: path.into(),
        code: code.into(),
        fix,
    }
}
fn required(path: &str) -> Entry {
    entry(
        path,
        "required",
        format!(
            "add a '{}' field",
            path.rsplit('.').next().unwrap_or_default()
        ),
    )
}
fn type_error(path: &str, kind: &str) -> Entry {
    entry(
        path,
        "type",
        format!(
            "'{}' must be a {kind}",
            if path.is_empty() { "value" } else { path }
        ),
    )
}
fn min_length(path: &str, n: i64) -> Entry {
    entry(
        path,
        "minLength",
        format!("expand '{path}' to at least {n} characters"),
    )
}
fn min_items(path: &str, n: i64, got: usize) -> Entry {
    let need = (n - got as i64).max(1);
    entry(
        path,
        "min",
        format!(
            "add at least {need} more {}",
            if need == 1 { "entry" } else { "entries" }
        ),
    )
}
pub(super) fn diff_json(errors: &[Entry], current: &V) -> crate::Result<Vec<u8>> {
    Ok(object_json([
        (
            "errors",
            array_json(errors.iter().map(|e| {
                compact(
                    Envelope::object()
                        .string("path", &e.path)
                        .string("code", &e.code)
                        .string("fix", &e.fix),
                )
            })),
        ),
        ("current", encoded(current, false)?),
    ]))
}
pub(super) fn validate(template: &PlanTemplate, submitted: &V) -> Vec<Entry> {
    let mut errors = vec![];
    let Some(data) = object(submitted) else {
        return vec![type_error("", "object")];
    };
    for k in ["template", "sections"] {
        if !data.contains_key(k) {
            errors.push(required(k));
        }
    }
    if let Some(name) = data.get("template") {
        match name {
            V::String(s) if s == &template.name => (),
            V::String(_) => errors.push(entry(
                "template",
                "const",
                format!("'template': const (allowedValue={})", template.name),
            )),
            _ => errors.push(type_error("template", "string")),
        }
    }
    if let Some(raw) = data.get("sections") {
        let Some(sections) = object(raw) else {
            errors.push(type_error("sections", "object"));
            return errors;
        };
        let names = section_names(template);
        for name in &names {
            if template.sections[name].required && !sections.contains_key(name) {
                errors.push(required(&format!("sections.{name}")));
            }
        }
        for name in &names {
            if let Some(value) = sections.get(name) {
                section(
                    &template.sections[name],
                    value,
                    format!("sections.{name}"),
                    &mut errors,
                );
            }
        }
        for name in &names {
            if !matches!(template.sections[name].kind, SectionKind::Steps) {
                continue;
            }
            let Some(steps) = sections.get(name).and_then(array) else {
                continue;
            };
            for (index, value) in steps.iter().enumerate() {
                let Some(step) = object(value) else { continue };
                let base = format!("sections.{name}.{index}");
                if text(step.get("title")).is_empty() && text(step.get("existing_seed")).is_empty()
                {
                    errors.push(entry(&base,"missing-title",format!("step {} must declare either 'title' (fresh spawn) or 'existing_seed' (adoption)",index+1)));
                }
                if let Some(blocks) = step.get("blocks").and_then(array) {
                    for raw in blocks {
                        let Some(n) = number(raw).filter(|n| n.trunc() == *n) else {
                            continue;
                        };
                        let path = format!("{base}.blocks");
                        if go_int(n) == index as i64 + 1 {
                            errors.push(entry(
                                &path,
                                "self-reference",
                                format!(
                                    "step {} cannot block itself; remove {} from blocks",
                                    index + 1,
                                    go_int(n)
                                ),
                            ));
                        } else if n < 1. || n > steps.len() as f64 {
                            errors.push(entry(&path,"out-of-range",format!("step index {} is out of range (step indices are 1-based; valid range 1..{})",go_int(n),steps.len())));
                        }
                    }
                }
            }
        }
    }
    errors
}
enum Task<'a> {
    Spec(&'a SectionSpec, &'a V, String),
    Object(&'a BTreeMap<String, SectionSpec>, &'a V, String),
    Item(Option<&'a SectionItem>, &'a V, String),
    Step(&'a V, String),
}
fn section<'a>(spec: &'a SectionSpec, value: &'a V, path: String, errors: &mut Vec<Entry>) {
    let mut tasks = vec![Task::Spec(spec, value, path)];
    while let Some(task) = tasks.pop() {
        match task {
            Task::Spec(s, v, p) => match &s.kind {
                SectionKind::Text => {
                    if let V::String(text) = v {
                        let min = s.min_length.unwrap_or(1);
                        if (text.encode_utf16().count() as i64) < min {
                            errors.push(min_length(&p, min));
                        }
                    } else {
                        errors.push(type_error(&p, "string"));
                    }
                }
                SectionKind::List | SectionKind::Steps => {
                    let Some(items) = array(v) else {
                        errors.push(type_error(&p, "array"));
                        continue;
                    };
                    if let Some(min) = s.min {
                        if (items.len() as i64) < min {
                            errors.push(min_items(&p, min, items.len()));
                        }
                    }
                    for (i, item) in items.iter().enumerate().rev() {
                        let path = format!("{p}.{i}");
                        tasks.push(if matches!(s.kind, SectionKind::Steps) {
                            Task::Step(item, path)
                        } else {
                            Task::Item(s.item.as_ref(), item, path)
                        });
                    }
                }
                SectionKind::Object(m) => tasks.push(Task::Object(m, v, p)),
            },
            Task::Object(fields, v, p) => {
                let Some(o) = object(v) else {
                    errors.push(type_error(&p, "object"));
                    continue;
                };
                for (name, s) in fields {
                    if s.required && !o.contains_key(name) {
                        errors.push(required(&format!("{p}.{name}")));
                    }
                }
                for (name, s) in fields.iter().rev() {
                    if let Some(value) = o.get(name) {
                        tasks.push(Task::Spec(s, value, format!("{p}.{name}")));
                    }
                }
            }
            Task::Item(item, v, p) => match item {
                None | Some(SectionItem::Text) => {
                    if let V::String(text) = v {
                        if text.is_empty() {
                            errors.push(min_length(&p, 1));
                        }
                    } else {
                        errors.push(type_error(&p, "string"));
                    }
                }
                Some(SectionItem::Object(m)) => tasks.push(Task::Object(m, v, p)),
            },
            Task::Step(v, p) => step_fields(v, &p, errors),
        }
    }
}
fn step_fields(value: &V, path: &str, errors: &mut Vec<Entry>) {
    let Some(step) = object(value) else {
        errors.push(type_error(path, "object"));
        return;
    };
    for name in [
        "title",
        "type",
        "priority",
        "blocks",
        "plan_template",
        "existing_seed",
        "labels",
    ] {
        let Some(value) = step.get(name) else {
            continue;
        };
        let p = format!("{path}.{name}");
        match name {
            "title" | "plan_template" | "existing_seed" => {
                if let V::String(t) = value {
                    if name != "plan_template" && t.is_empty() {
                        errors.push(min_length(&p, 1));
                    }
                } else {
                    errors.push(type_error(&p, "string"));
                }
            }
            "type" => {
                if let V::String(t) = value {
                    if !["task", "bug", "feature", "epic"].contains(&t.as_str()) {
                        errors.push(entry(
                            &p,
                            "enum",
                            format!("'{p}': enum (allowedValues=task,bug,feature,epic)"),
                        ));
                    }
                } else {
                    errors.push(type_error(&p, "string"));
                }
            }
            "priority" => match number(value) {
                None => errors.push(type_error(&p, "integer")),
                Some(n) if n.trunc() != n => errors.push(type_error(&p, "integer")),
                Some(n) if n < 0. => errors.push(entry(
                    &p,
                    "minimum",
                    format!("'{p}': minimum (comparison=>=, limit=0)"),
                )),
                Some(n) if n > 4. => errors.push(entry(
                    &p,
                    "maximum",
                    format!("'{p}': maximum (comparison=<=, limit=4)"),
                )),
                _ => (),
            },
            "blocks" | "labels" => {
                let Some(items) = array(value) else {
                    errors.push(type_error(&p, "array"));
                    continue;
                };
                for (i, item) in items.iter().enumerate() {
                    let p = format!("{p}.{i}");
                    if name == "blocks" {
                        if number(item).is_none_or(|n| n.trunc() != n) {
                            errors.push(type_error(&p, "integer"));
                        }
                    } else {
                        match item {
                            V::String(s) if s.is_empty() => errors.push(min_length(&p, 1)),
                            V::String(s) if crate::store::trim_space(s.as_bytes()).is_empty() => {
                                errors.push(entry(
                                    &p,
                                    "pattern",
                                    format!("'{p}': pattern (pattern=\\S)"),
                                ))
                            }
                            V::String(_) => (),
                            _ => errors.push(type_error(&p, "string")),
                        }
                    }
                }
            }
            _ => (),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_tree::built_in_templates;
    fn value(s: &str) -> V {
        V::decode(s.as_bytes()).unwrap()
    }
    fn template() -> PlanTemplate {
        built_in_templates().remove("feature").unwrap()
    }
    #[test]
    fn non_object_and_missing_fields_have_exact_errors() {
        assert_eq!(
            validate(&template(), &V::Null),
            [Entry {
                path: "".into(),
                code: "type".into(),
                fix: "'value' must be a object".into()
            }]
        );
        let errors = validate(&template(), &value("{}"));
        assert_eq!(errors, [required("template"), required("sections")]);
    }
    #[test]
    fn required_sections_precede_field_errors_in_declared_order() {
        let e = validate(
            &template(),
            &value(r#"{"template":"feature","sections":{"context":3}}"#),
        );
        assert_eq!(
            e.iter().map(|x| x.path.as_str()).collect::<Vec<_>>(),
            [
                "sections.approach",
                "sections.steps",
                "sections.acceptance",
                "sections.context"
            ]
        );
        assert_eq!(e[3].code, "type");
    }
    #[test]
    fn text_minimum_counts_utf16_units() {
        let mut t = template();
        t.sections.retain(|name, _| name == "context");
        t.sections.get_mut("context").unwrap().min_length = Some(2);
        assert!(validate(
            &t,
            &value(r#"{"template":"feature","sections":{"context":"😀"}}"#)
        )
        .is_empty());
        assert_eq!(
            validate(
                &t,
                &value(r#"{"template":"feature","sections":{"context":"a"}}"#)
            )[0],
            min_length("sections.context", 2)
        );
    }
    #[test]
    fn step_field_errors_precede_step_graph_errors() {
        let mut t = template();
        t.sections.retain(|n, _| n == "steps");
        let e = validate(
            &t,
            &value(
                r#"{"template":"feature","sections":{"steps":[{"title":"","type":"bad","priority":5,"blocks":[1,3],"labels":[" ",3]}]}}"#,
            ),
        );
        assert_eq!(
            e.iter().map(|x| x.code.as_str()).collect::<Vec<_>>(),
            [
                "min",
                "minLength",
                "enum",
                "maximum",
                "pattern",
                "type",
                "missing-title",
                "self-reference",
                "out-of-range"
            ]
        );
    }
    #[test]
    fn numeric_block_indices_require_integer_type() {
        let mut t = template();
        t.sections.retain(|n, _| n == "steps");
        let e = validate(
            &t,
            &value(
                r#"{"template":"feature","sections":{"steps":[{"title":"a","blocks":[1.5,"2"]},{"existing_seed":"old"}]}}"#,
            ),
        );
        assert_eq!(e.len(), 2);
        assert!(e.iter().all(|e| e.code == "type"));
        assert_eq!(e[0].path, "sections.steps.0.blocks.0");
    }
    #[test]
    fn custom_object_required_fields_are_sorted_before_values() {
        let mut t = template();
        let mut text = t.sections["approach"].clone();
        text.required = true;
        let nested = SectionKind::Object(BTreeMap::from([
            ("z".into(), text.clone()),
            ("a".into(), text.clone()),
        ]));
        text.kind = nested;
        t.sections = BTreeMap::from([("custom".into(), text)]);
        t.section_order = vec!["custom".into()];
        let e = validate(
            &t,
            &value(r#"{"template":"feature","sections":{"custom":{"z":4}}}"#),
        );
        assert_eq!(
            e.iter().map(|x| x.path.as_str()).collect::<Vec<_>>(),
            ["sections.custom.a", "sections.custom.z"]
        );
    }
    #[test]
    fn optional_list_text_allows_whitespace_but_labels_do_not() {
        let mut t = template();
        t.sections.retain(|n, _| n == "acceptance");
        assert!(validate(
            &t,
            &value(r#"{"template":"feature","sections":{"acceptance":[" "]}}"#)
        )
        .is_empty());
        assert_eq!(
            validate(
                &t,
                &value(r#"{"template":"feature","sections":{"acceptance":[""]}}"#)
            )[0],
            min_length("sections.acceptance.0", 1)
        );
    }
    #[test]
    fn unknown_keys_are_retained_without_validation_errors() {
        let t = PlanTemplate {
            name: "custom".into(),
            description: String::new(),
            sections: BTreeMap::new(),
            section_order: vec![],
        };
        assert!(validate(
            &t,
            &value(r#"{"template":"custom","sections":{"unknown":{"anything":[1,null]}}}"#)
        )
        .is_empty());
    }
    #[test]
    fn non_object_steps_do_not_duplicate_missing_title_errors() {
        let mut t = template();
        t.sections.retain(|n, _| n == "steps");
        let e = validate(
            &t,
            &value(r#"{"template":"feature","sections":{"steps":[1,null]}}"#),
        );
        assert_eq!(
            e,
            [
                type_error("sections.steps.0", "object"),
                type_error("sections.steps.1", "object")
            ]
        );
    }
    #[test]
    fn diff_has_go_struct_order_and_current_map_order() {
        let current = value(r#"{"z":1,"a":"<"}"#);
        let bytes = diff_json(&[required("template")], &current).unwrap();
        assert_eq!(bytes,br#"{"errors":[{"path":"template","code":"required","fix":"add a 'template' field"}],"current":{"a":"<","z":1}}"#);
    }
}
