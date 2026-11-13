//! Shared native plan helpers; wire objects retain Go struct field order.
use super::record::Plan;
use crate::{
    command::{
        options::{self, Options},
        output::Envelope,
        Failure,
    },
    config_tree::{PlanTemplate, SectionItem, SectionKind, SectionSpec},
    error::{Error, Result},
    json::{self, Value as V},
    model::Ticket,
    store::{read_records, DataFile, Store},
};
use std::collections::{BTreeMap, HashSet};
pub(super) fn parse(
    args: &[String],
    values: &[&str],
    switches: &[&str],
) -> std::result::Result<Options, Failure> {
    let mut switches = switches.to_vec();
    switches.push("json");
    Ok(options::parse(args, values, &switches)?)
}
pub(super) fn need<'a>(o: &'a Options, usage: &str) -> std::result::Result<&'a str, Failure> {
    o.positionals
        .first()
        .map(String::as_str)
        .ok_or_else(|| Failure::message(format!("Usage: luuna-missions plan {usage}")))
}
pub(super) fn load(store: &Store) -> Result<Vec<Plan>> {
    read_records(&store.path(DataFile::Plans))
}
pub(super) fn missing(id: &str) -> Error {
    Error::Command {
        message: format!(
            "Plan not found: {id}. Run 'luuna-missions plan list' to see available plans."
        ),
    }
}
pub(super) fn resolve(store: &Store, arg: &str) -> Result<String> {
    if arg.starts_with("pl-") {
        return Ok(arg.into());
    }
    for t in store.read_tickets()? {
        if t.id == arg {
            if t.plan_id.is_empty() {
                return Err(Error::Command{message:format!("Seed {arg} has no plan. Submit one with 'luuna-missions plan submit {arg} --plan <file>'.")});
            }
            return Ok(t.plan_id);
        }
    }
    Err(missing(arg))
}
pub(super) fn plan_index(plans: &[Plan], id: &str) -> Option<usize> {
    plans.iter().position(|p| p.id == id)
}
pub(super) fn ticket_index(issues: &[Ticket], id: &str) -> Option<usize> {
    issues.iter().position(|p| p.id == id)
}
pub(super) fn object(v: &V) -> Option<&BTreeMap<String, V>> {
    if let V::Object(m) = v {
        Some(m)
    } else {
        None
    }
}
pub(super) fn array(v: &V) -> Option<&[V]> {
    if let V::Array(a) = v {
        Some(a)
    } else {
        None
    }
}
pub(super) fn text(v: Option<&V>) -> &str {
    if let Some(V::String(s)) = v {
        s
    } else {
        ""
    }
}
pub(super) fn number(v: &V) -> Option<f64> {
    if let V::Number(n) = v {
        Some(*n)
    } else {
        None
    }
}
pub(super) fn go_int(n: f64) -> i64 {
    if !(-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&n) {
        i64::MIN
    } else {
        n as i64
    }
}
pub(super) fn encoded(v: &V, html: bool) -> Result<Vec<u8>> {
    let mut out = vec![];
    v.encode(&mut out, html).map_err(|e| Error::Encode {
        message: e.to_string(),
    })?;
    Ok(out)
}
pub(super) fn compact(o: Envelope) -> Vec<u8> {
    json::compact(&o.render()).expect("a constructed envelope is JSON")
}
pub(super) fn array_json(items: impl IntoIterator<Item = Vec<u8>>) -> Vec<u8> {
    let mut out = vec![b'['];
    for (i, raw) in items.into_iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        out.extend(raw);
    }
    out.push(b']');
    out
}
pub(super) fn section_names(t: &PlanTemplate) -> Vec<String> {
    let mut out = vec![];
    let mut seen = HashSet::new();
    for n in &t.section_order {
        if t.sections.contains_key(n) && seen.insert(n.clone()) {
            out.push(n.clone());
        }
    }
    out.extend(t.sections.keys().filter(|n| !seen.contains(*n)).cloned());
    out
}
pub(super) fn template_names(ts: &BTreeMap<String, PlanTemplate>) -> Vec<String> {
    let mut names = vec![];
    for n in ["feature", "bug", "refactor"] {
        if ts.contains_key(n) {
            names.push(n.into());
        }
    }
    names.extend(
        ts.keys()
            .filter(|n| !matches!(n.as_str(), "feature" | "bug" | "refactor"))
            .cloned(),
    );
    names
}

pub(super) fn object_json(fields: impl IntoIterator<Item = (&'static str, Vec<u8>)>) -> Vec<u8> {
    let mut out = vec![b'{'];
    for (i, (key, raw)) in fields.into_iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        json::quote(&mut out, key, false);
        out.push(b':');
        out.extend(raw);
    }
    out.push(b'}');
    out
}
pub(super) fn quoted(text: &str) -> Vec<u8> {
    let mut out = vec![];
    json::quote(&mut out, text, false);
    out
}
pub(super) fn output(
    c: &mut crate::command::Context<'_>,
    raw: &[u8],
    ty: &str,
) -> std::result::Result<(), Failure> {
    if json::exceeds_max_depth(raw) {
        return Err(Error::Encode {
            message: json::marshaler_too_deep(ty),
        }
        .into());
    }
    if !super::raw::pretty_fits(raw, 128 * 1024 * 1024) {
        return Err(Error::unsupported(
            "plan output beyond 128 MiB",
            "none (bounded native plan output)",
        )
        .into());
    }
    c.stdout
        .extend(json::to_output(raw).map_err(|e| Error::Encode {
            message: e.to_string(),
        })?);
    Ok(())
}
fn quote(s: &str) -> Vec<u8> {
    let mut out = vec![];
    json::quote(&mut out, s, false);
    out
}
enum Part<'a> {
    Spec(&'a SectionSpec),
    Kind(&'a SectionKind),
    Item(&'a SectionItem),
    Map(&'a BTreeMap<String, SectionSpec>),
    Bytes(Vec<u8>),
}
/// Iterative, including arbitrary nested custom section specifications.
pub(super) fn kind_json(kind: &SectionKind) -> Vec<u8> {
    spec_part(Part::Kind(kind))
}
pub(super) fn item_json(item: &SectionItem) -> Vec<u8> {
    spec_part(Part::Item(item))
}
fn spec_part(root: Part<'_>) -> Vec<u8> {
    let mut out = vec![];
    let mut stack = vec![root];
    while let Some(part) = stack.pop() {
        match part {
            Part::Bytes(b) => out.extend(b),
            Part::Kind(k) => match k {
                SectionKind::Text => out.extend(b"\"text\""),
                SectionKind::List => out.extend(b"\"list\""),
                SectionKind::Steps => out.extend(b"\"steps\""),
                SectionKind::Object(m) => stack.push(Part::Map(m)),
            },
            Part::Item(i) => match i {
                SectionItem::Text => out.extend(b"\"text\""),
                SectionItem::Object(m) => stack.push(Part::Map(m)),
            },
            Part::Map(m) => {
                out.push(b'{');
                stack.push(Part::Bytes(vec![b'}']));
                for (i, (key, spec)) in m.iter().enumerate().rev() {
                    stack.push(Part::Spec(spec));
                    let mut prefix = if i == 0 { vec![] } else { vec![b','] };
                    prefix.extend(quote(key));
                    prefix.push(b':');
                    stack.push(Part::Bytes(prefix));
                }
            }
            Part::Spec(s) => {
                let mut fields = vec![
                    (
                        "required",
                        Part::Bytes(if s.required {
                            b"true".to_vec()
                        } else {
                            b"false".to_vec()
                        }),
                    ),
                    ("kind", Part::Kind(&s.kind)),
                    ("prompt", Part::Bytes(quote(&s.prompt))),
                ];
                if let Some(n) = s.min_length {
                    fields.push(("min_length", Part::Bytes(n.to_string().into_bytes())));
                }
                if let Some(n) = s.min {
                    fields.push(("min", Part::Bytes(n.to_string().into_bytes())));
                }
                if let Some(item) = &s.item {
                    fields.push(("item", Part::Item(item)));
                }
                if !s.atlas_source.is_empty() {
                    fields.push(("atlas_source", Part::Bytes(quote(&s.atlas_source))));
                }
                out.push(b'{');
                stack.push(Part::Bytes(vec![b'}']));
                for (i, (key, part)) in fields.into_iter().enumerate().rev() {
                    stack.push(part);
                    let mut prefix = if i == 0 { vec![] } else { vec![b','] };
                    prefix.extend(quote(key));
                    prefix.push(b':');
                    stack.push(Part::Bytes(prefix));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn builtin_and_custom_template_names_match_declared_order() {
        let mut ts = crate::config_tree::built_in_templates();
        let mut extra = ts["feature"].clone();
        extra.name = "aaa".into();
        ts.insert("aaa".into(), extra);
        assert_eq!(template_names(&ts), ["feature", "bug", "refactor", "aaa"]);
    }
    #[test]
    fn nested_kind_spec_preserves_go_struct_order_and_html() {
        let mut spec =
            crate::config_tree::built_in_templates()["feature"].sections["approach"].clone();
        spec.prompt = "<prompt>".into();
        let kind = SectionKind::Object(BTreeMap::from([("nested".into(), spec)]));
        assert_eq!(
            kind_json(&kind),
            br#"{"nested":{"required":true,"kind":"text","prompt":"<prompt>"}}"#
        );
    }
    #[test]
    fn section_order_deduplicates_and_appends_missing_keys() {
        let mut t = crate::config_tree::built_in_templates()
            .remove("feature")
            .unwrap();
        t.section_order = vec!["steps".into(), "steps".into(), "missing".into()];
        assert_eq!(
            section_names(&t),
            [
                "steps",
                "acceptance",
                "alternatives",
                "approach",
                "context",
                "risks"
            ]
        );
    }
}
