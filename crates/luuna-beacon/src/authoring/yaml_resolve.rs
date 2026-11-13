//! Scalar resolution follows yaml.v3 resolve.go, including its legacy octals.
use super::yaml_events::{parser::Tag, scanner::TScalarStyle};
use super::yaml_value::Node;
use crate::BeaconError;
use regex::Regex;
use std::sync::{Arc, OnceLock};

fn string(s: &str) -> Node {
    Node::String(Arc::from(s.as_bytes()))
}
fn kind(n: &Node) -> &'static str {
    match n {
        Node::Null => "null",
        Node::Bool(_) => "bool",
        Node::Int(_) | Node::Uint(_) => "int",
        Node::Float(_) => "float",
        Node::Time(_) => "timestamp",
        _ => "str",
    }
}
pub(super) fn resolve(
    s: &str,
    style: TScalarStyle,
    tag: Option<&Tag>,
) -> Result<Node, BeaconError> {
    let explicit = tag.map(|t| {
        if t.handle == "tag:yaml.org,2002:" {
            t.suffix.as_str()
        } else {
            "custom"
        }
    });
    if explicit == Some("binary") {
        return decode_binary(s).map(Node::String).ok_or_else(|| {
            BeaconError::Yaml("!!binary value contains invalid base64 data".into())
        });
    }
    if explicit == Some("str")
        || explicit == Some("custom")
        || (explicit.is_none() && style != TScalarStyle::Plain)
    {
        return Ok(string(s));
    }
    let mut n = automatic(s);
    if explicit == Some("float") {
        if let Node::Int(v) = n {
            n = Node::Float(v as f64)
        }
    }
    if let Some(want) = explicit {
        if kind(&n) != want {
            return Err(BeaconError::Yaml(format!(
                "cannot decode !!{} `{s}` as a !!{want}",
                kind(&n)
            )));
        }
    }
    Ok(n)
}
pub(super) fn automatic(s: &str) -> Node {
    match s {
        "" | "~" | "null" | "Null" | "NULL" => return Node::Null,
        "true" | "True" | "TRUE" => return Node::Bool(true),
        "false" | "False" | "FALSE" => return Node::Bool(false),
        ".nan" | ".NaN" | ".NAN" => return Node::Float(f64::NAN),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => {
            return Node::Float(f64::INFINITY)
        }
        "-.inf" | "-.Inf" | "-.INF" => return Node::Float(f64::NEG_INFINITY),
        _ => {}
    }
    if let Some(time) = timestamp(s) {
        return Node::Time(time);
    }
    if !s.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '+' | '-' | '.')) {
        return string(s);
    }
    let clean = s.replace('_', "");
    let (negative, digits) = if let Some(v) = clean.strip_prefix('-') {
        (true, v)
    } else {
        (false, clean.strip_prefix('+').unwrap_or(&clean))
    };
    let (radix, digits) = if let Some(v) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        (16, v)
    } else if let Some(v) = digits
        .strip_prefix("0b")
        .or_else(|| digits.strip_prefix("0B"))
    {
        (2, v)
    } else if let Some(v) = digits
        .strip_prefix("0o")
        .or_else(|| digits.strip_prefix("0O"))
    {
        (8, v)
    } else if digits.len() > 1 && digits.starts_with('0') {
        (8, &digits[1..])
    } else {
        (10, digits)
    };
    if let Ok(v) = u64::from_str_radix(digits, radix) {
        if negative {
            if v <= 1u64 << 63 {
                return Node::Int((v as i64).wrapping_neg());
            }
        } else if v <= i64::MAX as u64 {
            return Node::Int(v as i64);
        } else if !clean.starts_with('+') {
            return Node::Uint(v);
        }
    }
    static FLOAT: OnceLock<Regex> = OnceLock::new();
    if FLOAT
        .get_or_init(|| {
            Regex::new(r"^[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?$").unwrap()
        })
        .is_match(&clean)
    {
        if let Ok(v) = clean.parse::<f64>() {
            if v.is_finite() {
                return Node::Float(v);
            }
        }
    }
    string(s)
}
fn timestamp(s: &str) -> Option<String> {
    use chrono::{DateTime, NaiveDate, NaiveDateTime, SecondsFormat};
    if s.as_bytes().get(4) != Some(&b'-') {
        return None;
    }
    // Go accepts excess fractional digits and truncates them to nanoseconds.
    let mut text = s.replace('t', "T");
    if let Some(dot) = text.find('.') {
        let digits = text[dot + 1..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        if digits > 9 {
            text.replace_range(dot + 10..dot + 1 + digits, "");
        }
    }
    static SHORT: OnceLock<Regex> = OnceLock::new();
    if let Some(c)=SHORT.get_or_init(||Regex::new(r"^(\d{4})-(\d{1,2})-(\d{1,2})T(\d{1,2}):(\d{1,2}):(\d{1,2})(\.[0-9]+)?(Z|[+-][0-9]{2}:[0-9]{2})$").unwrap()).captures(&text){
        let number=|i|c.get(i).unwrap().as_str().parse::<u32>().unwrap();
        text=format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{}{}",number(1),number(2),number(3),number(4),number(5),number(6),c.get(7).map_or("",|c|c.as_str()),c.get(8).unwrap().as_str());
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(&text) {
        return Some(nano(dt.to_rfc3339_opts(SecondsFormat::AutoSi, true)));
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(&text, "%Y-%m-%d %H:%M:%S%.f") {
        return Some(nano(
            dt.and_utc().to_rfc3339_opts(SecondsFormat::AutoSi, true),
        ));
    }
    NaiveDate::parse_from_str(&text, "%Y-%m-%d").ok().map(|d| {
        d.and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .to_rfc3339_opts(SecondsFormat::AutoSi, true)
    })
}
fn decode_binary(s: &str) -> Option<Arc<[u8]>> {
    let chars: Vec<u8> = s.bytes().filter(|b| !matches!(b, b'\n' | b'\r')).collect();
    if !chars.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::new();
    for (i, chunk) in chars.chunks(4).enumerate() {
        let mut v = 0u32;
        let mut pads = 0;
        for (j, &b) in chunk.iter().enumerate() {
            let n = match b {
                b'A'..=b'Z' => b - b'A',
                b'a'..=b'z' => b - b'a' + 26,
                b'0'..=b'9' => b - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' if j >= 2 && i + 1 == chars.len() / 4 => {
                    pads += 1;
                    0
                }
                _ => return None,
            };
            if pads > 0 && b != b'=' {
                return None;
            }
            v = (v << 6) | u32::from(n)
        }
        out.push((v >> 16) as u8);
        if pads < 2 {
            out.push((v >> 8) as u8)
        }
        if pads < 1 {
            out.push(v as u8)
        }
    }
    Some(Arc::from(out))
}

fn nano(mut text: String) -> String {
    if let Some(dot) = text.find('.') {
        let count = text[dot + 1..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        let end = dot + 1 + count;
        let kept = text[dot + 1..end].trim_end_matches('0').len();
        text.replace_range(if kept == 0 { dot } else { dot + 1 + kept }..end, "");
    }
    text
}
