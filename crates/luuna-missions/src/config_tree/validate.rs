//! Full config schema validation and Go's message order.
use super::{tree::command, TreeValue as V};
use crate::error::Result;
fn get<'a>(m: &'a [(String, V)], key: &str) -> Option<&'a V> {
    m.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}
pub(crate) fn whole(v: &V) -> Option<i64> {
    match v {
        V::Int(n) => Some(*n),
        V::Float(n)
            if n.is_finite()
                && *n >= i64::MIN as f64
                && *n < 9_223_372_036_854_775_808.0
                && n.fract() == 0.0 =>
        {
            Some(*n as i64)
        }
        _ => None,
    }
}
fn required(m: &[(String, V)], path: &str, keys: &[&str], errors: &mut Vec<String>) {
    for key in keys {
        if get(m, key).is_none() {
            let field = if path.is_empty() {
                key.to_string()
            } else {
                format!("{path}.{key}")
            };
            errors.push(format!("  {field}: add a '{key}' field"));
        }
    }
}
fn type_error(errors: &mut Vec<String>, path: &str, expected: &str) {
    errors.push(format!("  {path}: '{path}' must be a {expected}"));
}
fn rule(errors: &mut Vec<String>, path: &str, name: &str, detail: &str) {
    let where_ = if path.is_empty() { "(root)" } else { path };
    let prefix = if path.is_empty() {
        String::new()
    } else {
        format!("'{path}': ")
    };
    errors.push(format!("  {where_}: {prefix}{name} ({detail})"));
}
fn sorted(m: &[(String, V)]) -> Vec<(&str, &V)> {
    let mut v = m.iter().map(|(k, v)| (k.as_str(), v)).collect::<Vec<_>>();
    v.sort_by_key(|(k, _)| *k);
    v
}
fn shape(v: &V, path: &str, literals: &[&str], errors: &mut Vec<String>) {
    match v {
        V::Str(s) if literals.contains(&s.as_str()) => {}
        V::Map(m) => {
            for (k, v) in sorted(m) {
                spec(v, &format!("{path}.{k}"), errors)
            }
        }
        _ => rule(errors, path, "oneOf", "passingSchemas=null"),
    }
}
fn spec(v: &V, path: &str, errors: &mut Vec<String>) {
    let V::Map(m) = v else {
        type_error(errors, path, "object");
        return;
    };
    required(m, path, &["required", "kind", "prompt"], errors);
    for (key, v) in sorted(m) {
        let field = format!("{path}.{key}");
        match key {
            "required" => {
                if !matches!(v, V::Bool(_)) {
                    type_error(errors, &field, "boolean");
                }
            }
            "prompt" | "atlas_source" => {
                if !matches!(v, V::Str(_)) {
                    type_error(errors, &field, "string");
                }
            }
            "min" | "min_length" => match whole(v) {
                None => type_error(errors, &field, "integer"),
                Some(n) if n < 0 => rule(errors, &field, "minimum", "comparison=>=, limit=0"),
                _ => {}
            },
            "kind" => shape(v, &field, &["text", "list", "steps"], errors),
            "item" => shape(v, &field, &["text"], errors),
            _ => rule(
                errors,
                path,
                "additionalProperties",
                &format!("additionalProperty={key}"),
            ),
        }
    }
}
fn template(v: &V, path: &str, errors: &mut Vec<String>) {
    let V::Map(m) = v else {
        type_error(errors, path, "object");
        return;
    };
    required(m, path, &["sections"], errors);
    for (key, v) in sorted(m) {
        let field = format!("{path}.{key}");
        match key {
            "description" => {
                if !matches!(v, V::Str(_)) {
                    type_error(errors, &field, "string");
                }
            }
            "sections" => {
                if let V::Map(sections) = v {
                    for (key, v) in sorted(sections) {
                        spec(v, &format!("{field}.{key}"), errors);
                    }
                } else {
                    type_error(errors, &field, "object");
                }
            }
            _ => rule(
                errors,
                path,
                "additionalProperties",
                &format!("additionalProperty={key}"),
            ),
        }
    }
}
pub(crate) fn validate_tree(v: &V) -> Result<()> {
    let V::Map(m) = v else {
        return Err(command("config root must be a mapping"));
    };
    let mut errors = Vec::new();
    required(m, "", &["project", "version"], &mut errors);
    for (key, v) in sorted(m) {
        match key {
            "project" => match v {
                V::Str(s) if s.is_empty() => {
                    errors.push("  project: expand 'project' to at least 1 characters".into())
                }
                V::Str(_) => {}
                _ => type_error(&mut errors, key, "string"),
            },
            "version" => {
                if !matches!(v, V::Str(_)) {
                    type_error(&mut errors, key, "string");
                }
            }
            "max_plan_depth" => match whole(v) {
                None => type_error(&mut errors, key, "integer"),
                Some(n) if n < 1 => rule(&mut errors, key, "minimum", "comparison=>=, limit=1"),
                _ => {}
            },
            "plan_templates" => {
                if let V::Map(m) = v {
                    for (name, v) in sorted(m) {
                        template(v, &format!("plan_templates.{name}"), &mut errors);
                    }
                } else {
                    type_error(&mut errors, key, "object");
                }
            }
            _ => rule(
                &mut errors,
                "",
                "additionalProperties",
                &format!("additionalProperty={key}"),
            ),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(command(format!(
            "Config validation failed:\n{}",
            errors.join("\n")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::super::yaml;
    use super::*;
    #[test]
    fn rejects_nested_partial_and_root_extras() {
        let tree=yaml::decode_document(b"project: demo\nversion: \"1\"\nextra: x\nplan_templates:\n  x:\n    sections:\n      a:\n        extra: x\n").unwrap().unwrap();
        assert_eq!(validate_tree(&tree).unwrap_err().to_string(),"Config validation failed:\n  (root): additionalProperties (additionalProperty=extra)\n  plan_templates.x.sections.a.required: add a 'required' field\n  plan_templates.x.sections.a.kind: add a 'kind' field\n  plan_templates.x.sections.a.prompt: add a 'prompt' field\n  plan_templates.x.sections.a: 'plan_templates.x.sections.a': additionalProperties (additionalProperty=extra)");
        assert_eq!(whole(&V::Float(9_223_372_036_854_775_808.0)), None);
    }
}
