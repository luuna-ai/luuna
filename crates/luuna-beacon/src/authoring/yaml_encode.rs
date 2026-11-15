//! Go-style block YAML emission with an explicit traversal stack.
use super::yaml_sort::compare;
use super::{
    yaml_resolve,
    yaml_value::{Node, YamlValue},
};
pub(super) fn float(n: f64) -> String {
    if n.is_nan() {
        return ".nan".into();
    }
    if n == f64::INFINITY {
        return ".inf".into();
    }
    if n == f64::NEG_INFINITY {
        return "-.inf".into();
    }
    if n == 0.0 {
        return if n.is_sign_negative() { "-0" } else { "0" }.into();
    }
    let e = format!("{n:e}");
    let (mantissa, exp) = e.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    if !(-4..6).contains(&exp) {
        format!(
            "{mantissa}e{}{abs:02}",
            if exp >= 0 { '+' } else { '-' },
            abs = exp.abs()
        )
    } else {
        n.to_string()
    }
}
fn scalar(n: &Node) -> String {
    match n {
        Node::Null => "null".into(),
        Node::Bool(b) => b.to_string(),
        Node::Int(n) => n.to_string(),
        Node::Uint(n) => n.to_string(),
        Node::Float(n) => float(*n),
        Node::Time(s) => s.clone(),
        Node::Seq(_) => "[]".into(),
        Node::Map(_) => "{}".into(),
        Node::String(bytes) => {
            let Ok(s) = std::str::from_utf8(bytes) else {
                return format!("!!binary {}", base64(bytes));
            };
            let typed = !matches!(yaml_resolve::automatic(s), Node::String(_));
            let old_bool = matches!(
                s,
                "y" | "Y"
                    | "yes"
                    | "Yes"
                    | "YES"
                    | "n"
                    | "N"
                    | "no"
                    | "No"
                    | "NO"
                    | "on"
                    | "On"
                    | "ON"
                    | "off"
                    | "Off"
                    | "OFF"
            );
            let base60 = s.split(':').count() > 1
                && s.chars().all(|c| c.is_ascii_digit() || ":.+-".contains(c));
            if s.chars().any(|c| c as u32 > 0xfffd)
                || s.starts_with('\u{feff}')
                || s.is_empty()
                || (s.contains(" \u{2028}") || s.contains(" \u{2029}"))
                || s.contains('\n') && !literal(bytes)
                || typed
                || old_bool
                || base60
                || s.chars().any(|c| c.is_control() && c != '\n')
            {
                return double_quote(s);
            }
            if s.contains(['\u{2028}', '\u{2029}'])
                || s == "..."
                || s.starts_with([
                    ' ', '\t', '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|',
                    '>', '\'', '"', '%', '@', '`',
                ])
                || s.ends_with([' ', '\t', ':'])
                || s.contains(": ")
                || s.contains(" #")
            {
                return format!("'{}'", s.replace('\'', "''"));
            }
            s.into()
        }
    }
}
fn base64(bytes: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        s.push(A[(n >> 18) as usize] as char);
        s.push(A[((n >> 12) & 63) as usize] as char);
        s.push(if chunk.len() > 1 {
            A[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        s.push(if chunk.len() > 2 {
            A[(n & 63) as usize] as char
        } else {
            '='
        })
    }
    s
}
fn nonempty(n: &Node) -> bool {
    matches!(n,Node::Seq(v) if !v.is_empty()) || matches!(n,Node::Map(v) if !v.is_empty())
}
pub(super) fn encode(value: &YamlValue) -> String {
    enum Step {
        Value(usize, usize, String),
    }
    let mut out = String::new();
    let mut work = vec![Step::Value(value.root, 0, String::new())];
    while let Some(step) = work.pop() {
        match step {
            Step::Value(id, indent, prefix) => {
                let n = &value.nodes[id];
                match n {
                    Node::Map(fields) if !fields.is_empty() => {
                        if !prefix.is_empty() && !compact(&prefix) {
                            out.push_str(&prefix);
                            out.push('\n')
                        };
                        let mut fields = fields.clone();
                        fields
                            .sort_by(|(a, _), (b, _)| compare(&value.nodes[*a], &value.nodes[*b]));
                        for (position, (k, v)) in fields.into_iter().enumerate().rev() {
                            let pad = if position == 0 && compact(&prefix) {
                                prefix.clone()
                            } else {
                                " ".repeat(indent)
                            };
                            let complex = match &value.nodes[k] {
                                Node::String(s) => {
                                    s.len() > 128
                                        || std::str::from_utf8(s).is_ok_and(|s| {
                                            s.contains([
                                                '\n', '\r', '\u{85}', '\u{2028}', '\u{2029}',
                                            ])
                                        })
                                }
                                Node::Seq(_) | Node::Map(_) => true,
                                _ => false,
                            };
                            if complex {
                                let head = format!("{}:", " ".repeat(indent));
                                if nonempty(&value.nodes[v]) {
                                    work.push(Step::Value(v, indent + 4, format!("{head}   ")))
                                } else {
                                    work.push(Step::Value(v, indent + 4, format!("{head} ")))
                                }
                                work.push(Step::Value(k, indent + 4, format!("{pad}? ")));
                                continue;
                            }
                            let key = at(&value.nodes[k], indent + 4);
                            let head = format!("{pad}{key}:");
                            if nonempty(&value.nodes[v]) {
                                work.push(Step::Value(v, indent + 4, head))
                            } else {
                                work.push(Step::Value(v, indent + 4, format!("{head} ")))
                            }
                        }
                    }
                    Node::Seq(children) if !children.is_empty() => {
                        if !prefix.is_empty() && !compact(&prefix) {
                            out.push_str(&prefix);
                            out.push('\n')
                        };
                        for (position, &child) in children.iter().enumerate().rev() {
                            let head = if position == 0 && compact(&prefix) {
                                format!("{prefix}- ")
                            } else {
                                format!("{}- ", " ".repeat(indent))
                            };
                            if nonempty(&value.nodes[child]) {
                                work.push(Step::Value(child, indent + 2, head));
                            } else {
                                work.push(Step::Value(child, indent + 2, head))
                            }
                        }
                    }
                    Node::String(s) if literal(s) => {
                        let s = std::str::from_utf8(s).unwrap();
                        out.push_str(&prefix);
                        out.push('|');
                        if s.starts_with([' ', '\n', '\u{2028}', '\u{2029}']) {
                            out.push('4')
                        }
                        if !s.ends_with('\n') {
                            out.push('-')
                        } else if s == "\n" || s.ends_with("\n\n") {
                            out.push('+')
                        }
                        out.push('\n');
                        for (index, line) in s.split_terminator('\n').enumerate() {
                            if index == 0 && line.is_empty() {
                                continue;
                            }
                            if !line.is_empty() {
                                out.push_str(&" ".repeat(indent.max(4)));
                                out.push_str(line)
                            }
                            out.push('\n')
                        }
                    }
                    _ => {
                        out.push_str(&prefix);
                        out.push_str(&at(n, indent.max(4)));
                        out.push('\n')
                    }
                }
            }
        }
    }
    out
}

fn double_quote(s: &str) -> String {
    let mut out = String::from("\"");
    let force = s.starts_with('\u{feff}');
    for c in s.chars() {
        match c {
            '\0' => out.push_str("\\0"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{b}' => out.push_str("\\v"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            '\u{1b}' => out.push_str("\\e"),
            '\u{85}' => out.push_str("\\N"),
            '\u{a0}' => out.push_str("\\_"),
            '\u{2028}' => out.push_str("\\L"),
            '\u{2029}' => out.push_str("\\P"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => {
                use std::fmt::Write;
                let _ = write!(out, "\\x{:02X}", c as u32);
            }
            c if force || c == '\u{feff}' || c as u32 > 0xfffd => {
                use std::fmt::Write;
                let code = c as u32;
                if code <= 0xff {
                    let _ = write!(out, "\\x{code:02X}");
                } else if code <= 0xffff {
                    let _ = write!(out, "\\u{code:04X}");
                } else {
                    let _ = write!(out, "\\U{code:08X}");
                }
            }
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn at(n: &Node, indent: usize) -> String {
    let text = scalar(n);
    if !text.contains(['\u{2028}', '\u{2029}']) {
        return text;
    }
    let mut out = String::new();
    let mut broke = false;
    let chars: Vec<_> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if matches!(c, '\u{2028}' | '\u{2029}') {
            out.push(c);
            broke = true
        } else {
            if broke && i + 1 < chars.len() {
                out.push_str(&" ".repeat(indent))
            }
            out.push(c);
            broke = false
        }
    }
    out
}
fn literal(bytes: &[u8]) -> bool {
    let Ok(s) = std::str::from_utf8(bytes) else {
        return false;
    };
    s.contains('\n')
        && !s.contains(" \n")
        && !s
            .chars()
            .any(|c| c as u32 > 0xfffd || c.is_control() && !matches!(c, '\n' | '\t'))
}

fn compact(prefix: &str) -> bool {
    prefix.ends_with("- ") || prefix.ends_with(":   ")
}
