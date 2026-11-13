//! `%#v` descriptions in yaml.v3's invalid-map-key error.
use super::{
    yaml_json,
    yaml_value::{Node, YamlValue},
};
pub(super) fn value(value: &YamlValue, root: usize) -> String {
    enum Step {
        Node(usize),
        Text(String),
    }
    let mut out = String::new();
    let mut work = vec![Step::Node(root)];
    while let Some(step) = work.pop() {
        match step {
            Step::Text(s) => out.push_str(&s),
            Step::Node(id) => match &value.nodes[id] {
                Node::Null => out.push_str("interface {}(nil)"),
                Node::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
                Node::Int(n) => out.push_str(&n.to_string()),
                Node::Uint(n) => out.push_str(&format!("0x{n:x}")),
                Node::Float(n) => {
                    let text = if n.is_nan() {
                        "NaN".into()
                    } else if *n == f64::INFINITY {
                        "+Inf".into()
                    } else if *n == f64::NEG_INFINITY {
                        "-Inf".into()
                    } else {
                        super::yaml_encode::float(*n)
                    };
                    out.push_str(&text)
                }
                Node::String(s) => out.push_str(&crate::gofmt::quote(&yaml_json::lossy(s))),
                Node::Time(s) => {
                    use chrono::{Datelike, Timelike};
                    let d = chrono::DateTime::parse_from_rfc3339(s).expect("resolved timestamp");
                    let month = [
                        "January",
                        "February",
                        "March",
                        "April",
                        "May",
                        "June",
                        "July",
                        "August",
                        "September",
                        "October",
                        "November",
                        "December",
                    ][d.month0() as usize];
                    out.push_str(&format!(
                        "time.Date({}, time.{month}, {}, {}, {}, {}, {}, {})",
                        d.year(),
                        d.day(),
                        d.hour(),
                        d.minute(),
                        d.second(),
                        d.nanosecond(),
                        if d.offset().local_minus_utc() == 0 {
                            "time.UTC"
                        } else {
                            "time.Location(\"\")"
                        }
                    ));
                }
                Node::Seq(ids) => {
                    out.push_str("[]interface {}{");
                    work.push(Step::Text("}".into()));
                    for (i, &id) in ids.iter().enumerate().rev() {
                        work.push(Step::Node(id));
                        if i > 0 {
                            work.push(Step::Text(", ".into()))
                        }
                    }
                }
                Node::Map(fields) => {
                    let string = fields
                        .iter()
                        .all(|(key, _)| matches!(value.nodes[*key], Node::String(_)));
                    out.push_str(if string {
                        "map[string]interface {}{"
                    } else {
                        "map[interface {}]interface {}{"
                    });
                    let mut fields = fields.clone();
                    fields.sort_by(
                        |(a, _), (b, _)| match (&value.nodes[*a], &value.nodes[*b]) {
                            (Node::String(a), Node::String(b)) => a.cmp(b),
                            _ => super::yaml_sort::compare(&value.nodes[*a], &value.nodes[*b]),
                        },
                    );
                    work.push(Step::Text("}".into()));
                    for (i, (key, v)) in fields.into_iter().enumerate().rev() {
                        work.push(Step::Node(v));
                        work.push(Step::Text(":".into()));
                        work.push(Step::Node(key));
                        if i > 0 {
                            work.push(Step::Text(", ".into()))
                        }
                    }
                }
            },
        }
    }
    out
}
