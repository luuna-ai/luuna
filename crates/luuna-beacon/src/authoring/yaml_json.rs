//! Iterative Go JSON encoding, retaining JSON-pointer and byte-offset errors.
use super::{
    yaml_value::{Node, YamlValue},
    AuthoringError as E,
};
use crate::{gojson, BeaconError};
pub(super) fn encode(value: &YamlValue) -> Result<String, BeaconError> {
    encode_in(value, "", "")
}
pub(super) fn encode_in(
    value: &YamlValue,
    prefix: &str,
    path: &str,
) -> Result<String, BeaconError> {
    enum Step {
        Node(usize, String),
        Text(String),
        Byte(char),
        InvalidKey(String),
    }
    let mut out = prefix.to_owned();
    let mut work = vec![Step::Node(value.root, path.into())];
    while let Some(step) = work.pop() {
        match step {
            Step::Text(s) => out.push_str(&s),
            Step::Byte(c) => out.push(c),
            Step::InvalidKey(path) => {
                let within = if path.is_empty() {
                    String::new()
                } else {
                    let mut quoted = String::new();
                    gojson::write_string(&mut quoted, &path);
                    format!(" within {quoted}")
                };
                return Err(E::JsonUnsupportedValue(format!(
                    "jsontext: object member name must be a string{within} after offset {}",
                    out.len() + 1
                ))
                .into());
            }
            Step::Node(id, path) => match &value.nodes[id] {
                Node::Null => out.push_str("null"),
                Node::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
                Node::Int(n) => out.push_str(&n.to_string()),
                Node::Uint(n) => out.push_str(&n.to_string()),
                Node::Float(n) => {
                    if !n.is_finite() {
                        return Err(E::JsonUnsupportedValue(
                            if n.is_nan() {
                                "NaN"
                            } else if n.is_sign_negative() {
                                "-Inf"
                            } else {
                                "+Inf"
                            }
                            .into(),
                        )
                        .into());
                    }
                    gojson::write_float(&mut out, *n)
                }
                Node::String(s) => gojson::write_string(&mut out, &lossy(s)),
                Node::Time(s) => gojson::write_string(&mut out, s),
                Node::Seq(children) => {
                    out.push('[');
                    work.push(Step::Byte(']'));
                    for (i, &n) in children.iter().enumerate().rev() {
                        work.push(Step::Node(n, format!("{path}/{i}")));
                        if i > 0 {
                            work.push(Step::Byte(','));
                        }
                    }
                }
                Node::Map(children) => {
                    let mut fields = Vec::new();
                    let mut invalid = false;
                    for &(k, v) in children {
                        let key = match &value.nodes[k] {
                            Node::String(s) => lossy(s),
                            Node::Int(n) => n.to_string(),
                            Node::Uint(n) => n.to_string(),
                            Node::Time(s) => s.clone(),
                            _ => {
                                invalid = true;
                                String::new()
                            }
                        };
                        fields.push((key, v))
                    }
                    out.push('{');
                    if invalid {
                        work.push(Step::InvalidKey(path));
                        continue;
                    }
                    fields.sort_by(|a, b| a.0.cmp(&b.0));
                    work.push(Step::Byte('}'));
                    for (i, (key, n)) in fields.into_iter().enumerate().rev() {
                        let escaped = key.replace('~', "~0").replace('/', "~1");
                        work.push(Step::Node(n, format!("{path}/{escaped}")));
                        work.push(Step::Byte(':'));
                        let mut q = String::new();
                        gojson::write_string(&mut q, &key);
                        work.push(Step::Text(q));
                        if i > 0 {
                            work.push(Step::Byte(','));
                        }
                    }
                }
            },
        }
    }
    Ok(out[prefix.len()..].to_owned())
}

/// Go consumes one byte for each malformed UTF-8 rune, including truncated tails.
pub(super) fn lossy(mut bytes: &[u8]) -> String {
    let mut out = String::new();
    while !bytes.is_empty() {
        match std::str::from_utf8(bytes) {
            Ok(s) => {
                out.push_str(s);
                break;
            }
            Err(e) => {
                let valid = e.valid_up_to();
                out.push_str(std::str::from_utf8(&bytes[..valid]).expect("valid prefix"));
                out.push('\u{fffd}');
                bytes = &bytes[valid + 1..]
            }
        }
    }
    out
}
