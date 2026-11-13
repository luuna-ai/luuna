//! Natural ordering follows yaml.v3 sorter.go.
use super::{unicode_data, yaml_value::Node};
fn member(c: char, ranges: &[(u32, u32)]) -> bool {
    let c = c as u32;
    let i = ranges.partition_point(|(start, _)| *start <= c);
    i > 0 && c <= ranges[i - 1].1
}
fn letter(c: char) -> bool {
    member(c, unicode_data::LETTER)
}
fn digit(c: char) -> bool {
    member(c, unicode_data::DIGIT)
}
pub(super) fn compare(a: &Node, b: &Node) -> std::cmp::Ordering {
    fn rank(n: &Node) -> u8 {
        match n {
            Node::Null => 0,
            Node::Bool(_) => 1,
            Node::Int(_) => 2,
            Node::Uint(_) => 11,
            Node::Float(_) => 14,
            Node::String(_) => 24,
            Node::Seq(_) => 23,
            Node::Map(_) => 21,
            Node::Time(_) => 25,
        }
    }
    fn number(n: &Node) -> Option<f64> {
        match n {
            Node::Bool(v) => Some(if *v { 1.0 } else { 0.0 }),
            Node::Int(v) => Some(*v as f64),
            Node::Uint(v) => Some(*v as f64),
            Node::Float(v) => Some(*v),
            _ => None,
        }
    }
    if let (Some(a), Some(b)) = (number(a), number(b)) {
        if a != b {
            return a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal);
        }
    }
    if let (Node::String(a), Node::String(b)) = (a, b) {
        return natural(&super::yaml_json::lossy(a), &super::yaml_json::lossy(b));
    }
    match (a, b) {
        (Node::Int(a), Node::Int(b)) => a.cmp(b),
        (Node::Uint(a), Node::Uint(b)) => a.cmp(b),
        _ => rank(a).cmp(&rank(b)),
    }
}
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut digits = false;
    for i in 0..a.len().min(b.len()) {
        if a[i] == b[i] {
            digits = digit(a[i]);
            continue;
        }
        let al = letter(a[i]);
        let bl = letter(b[i]);
        if al && bl {
            return a[i].cmp(&b[i]);
        }
        if al || bl {
            return if digits { bl.cmp(&al) } else { al.cmp(&bl) };
        }
        let mut an = 0i64;
        let mut bn = 0i64;
        if a[i] == '0' || b[i] == '0' {
            for j in (0..i).rev() {
                if !digit(a[j]) {
                    break;
                }
                if a[j] != '0' {
                    an = 1;
                    bn = 1;
                    break;
                }
            }
        }
        let mut ai = i;
        let mut bi = i;
        while ai < a.len() && digit(a[ai]) {
            an = an
                .wrapping_mul(10)
                .wrapping_add(i64::from(a[ai] as u32) - 48);
            ai += 1
        }
        while bi < b.len() && digit(b[bi]) {
            bn = bn
                .wrapping_mul(10)
                .wrapping_add(i64::from(b[bi] as u32) - 48);
            bi += 1
        }
        if an != bn {
            return an.cmp(&bn);
        }
        if ai != bi {
            return ai.cmp(&bi);
        }
        return a[i].cmp(&b[i]);
    }
    let n = a.len().cmp(&b.len());
    if n == Ordering::Equal {
        Ordering::Equal
    } else {
        n
    }
}
