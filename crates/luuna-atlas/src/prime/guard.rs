//! The reference's intentionally simple shell/file guard matchers.
use crate::{actions::record::text, json::Object, records};
use std::path::Path;
pub const GUARD_REASON:&str="luuna-atlas: do not edit .luuna/atlas/expertise/*.jsonl by hand. Use `luuna-atlas record <domain> --type <t> --description ...` to add, `luuna-atlas edit <id>` to change, `luuna-atlas delete <id>` to remove (they lock, validate, and dedup the store).";
pub fn file_tool_field(tool: &str) -> Option<&'static str> {
    match tool {
        "Write" | "Edit" | "MultiEdit" => Some("file_path"),
        "NotebookEdit" => Some("notebook_path"),
        _ => None,
    }
}
fn separator(b: u8) -> bool {
    b == b'/' || b == b'\\'
}
fn expertise_end(bytes: &[u8], start: usize) -> Option<usize> {
    let tail = bytes.get(start..)?;
    if !tail.starts_with(b".luuna") {
        return None;
    }
    let mut at = start + 6;
    let first = at;
    while bytes.get(at).is_some_and(|b| separator(*b)) {
        at += 1;
    }
    if at == first || !bytes.get(at..)?.starts_with(b"atlas") {
        return None;
    }
    at += 5;
    let first = at;
    while bytes.get(at).is_some_and(|b| separator(*b)) {
        at += 1;
    }
    if at == first || !bytes.get(at..)?.starts_with(b"expertise") {
        return None;
    }
    Some(at + 9)
}
pub fn expertise_mention(input: &str) -> bool {
    input
        .match_indices(".luuna")
        .any(|(at, _)| expertise_end(input.as_bytes(), at).is_some())
}
pub fn is_expertise_file(path: &str, cwd: &Path) -> bool {
    let raw = Path::new(path);
    let path = if raw.is_absolute() {
        raw.to_owned()
    } else {
        cwd.join(raw)
    };
    let path = crate::commands::clean_path(&path);
    let path = path.to_string_lossy();
    let bytes = path.as_bytes();
    path.match_indices(".luuna").any(|(at, _)| {
        if at > 0 && !separator(bytes[at - 1]) {
            return false;
        }
        let Some(mut end) = expertise_end(bytes, at) else {
            return false;
        };
        let first = end;
        while bytes.get(end).is_some_and(|b| separator(*b)) {
            end += 1;
        }
        if end == first {
            return false;
        }
        let tail = &path[end..];
        tail.ends_with(".jsonl") && tail.len() > 6 && !tail.bytes().any(separator)
    })
}
fn unquote(mut value: &str) -> &str {
    if value.starts_with(['\'', '"']) {
        value = &value[1..];
    }
    if value.ends_with(['\'', '"']) {
        value = &value[..value.len() - 1];
    }
    value
}
fn assign(value: &str) -> bool {
    let b = value.as_bytes();
    if b.first()
        .is_none_or(|b| !b.is_ascii_alphabetic() && *b != b'_')
    {
        return false;
    }
    let mut at = 1;
    while b
        .get(at)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
    {
        at += 1;
    }
    b.get(at) == Some(&b'=')
}
fn ascii_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n' | 0x0c)
}
fn redirect_target(segment: &str, start: usize) -> Option<(usize, &str)> {
    let b = segment.as_bytes();
    let mut at = start;
    if b.get(at) == Some(&b'&') {
        at += 1;
    }
    if b.get(at) != Some(&b'>') {
        at = start;
        if b.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        } else {
            return None;
        }
        if b.get(at) != Some(&b'>') {
            return None;
        }
    }
    at += 1;
    if b.get(at) == Some(&b'>') {
        at += 1;
    }
    while b.get(at).is_some_and(|b| ascii_space(*b)) {
        at += 1;
    }
    let first = at;
    let quote = *b.get(at)?;
    if quote == b'"' || quote == b'\'' {
        if let Some(end) = b[at + 1..].iter().position(|b| *b == quote) {
            return Some((at + end + 2, &segment[first..at + end + 2]));
        }
    }
    while b
        .get(at)
        .is_some_and(|b| !ascii_space(*b) && !b";&|<>".contains(b))
    {
        at += 1;
    }
    if at == first {
        None
    } else {
        Some((at, &segment[first..at]))
    }
}
fn redirects(segment: &str) -> Vec<&str> {
    let b = segment.as_bytes();
    let mut at = 0;
    let mut out = Vec::new();
    while at < b.len() {
        let found = if at == 0 {
            redirect_target(segment, at).or_else(|| {
                if !b[at].is_ascii_digit() && b[at] != b'<' {
                    redirect_target(
                        segment,
                        at + segment[at..].chars().next().unwrap().len_utf8(),
                    )
                } else {
                    None
                }
            })
        } else if !b[at].is_ascii_digit() && b[at] != b'<' {
            redirect_target(
                segment,
                at + segment[at..].chars().next().unwrap().len_utf8(),
            )
        } else {
            None
        };
        if let Some((end, target)) = found {
            out.push(target);
            at = end;
        } else {
            at += segment[at..].chars().next().unwrap().len_utf8();
        }
    }
    out
}
fn inplace(arg: &str) -> bool {
    let b = arg.as_bytes();
    if b.first() != Some(&b'-') {
        return false;
    }
    let mut at = 1;
    while let Some(byte) = b.get(at).filter(|b| b.is_ascii_alphabetic()) {
        if *byte == b'i' {
            return true;
        }
        at += 1;
    }
    false
}
pub fn shell_segment_writes(segment: &str) -> bool {
    let words = segment.split_whitespace().map(unquote).collect::<Vec<_>>();
    let mut skip = 0;
    while words.get(skip).is_some_and(|word| {
        assign(word) || ["sudo", "env", "command", "nohup", "time", "xargs"].contains(word)
    }) {
        skip += 1;
    }
    let words = &words[skip..];
    let name = words
        .first()
        .copied()
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let args = words.get(1..).unwrap_or_default();
    if ["luuna-atlas", "ml", "mulch"].contains(&name) {
        return false;
    }
    if redirects(segment)
        .into_iter()
        .any(|target| expertise_mention(unquote(target)))
    {
        return true;
    }
    let paths = args
        .iter()
        .filter(|arg| !arg.starts_with('-'))
        .copied()
        .collect::<Vec<_>>();
    let any = paths.iter().any(|arg| expertise_mention(arg));
    if [
        "tee", "rm", "mv", "truncate", "touch", "unlink", "shred", "sponge",
    ]
    .contains(&name)
    {
        return any;
    }
    if ["cp", "ln", "install", "rsync"].contains(&name) {
        return paths.last().is_some_and(|arg| expertise_mention(arg));
    }
    if ["sed", "perl"].contains(&name) {
        return any
            && args
                .iter()
                .any(|arg| inplace(arg) || arg.starts_with("--in-place"));
    }
    if name == "dd" {
        return args
            .iter()
            .any(|arg| arg.starts_with("of=") && expertise_mention(arg));
    }
    false
}
pub fn bash_writes_expertise(command: &str) -> bool {
    if !expertise_mention(command) {
        return false;
    }
    let b = command.as_bytes();
    let mut first = 0;
    let mut at = 0;
    while at < b.len() {
        let size = match b[at] {
            b'\n' | b';' => 1,
            b'&' if b.get(at + 1) == Some(&b'&') => 2,
            b'|' if b.get(at + 1) == Some(&b'|') => 2,
            b'|' => 1,
            _ => 0,
        };
        if size > 0 {
            if shell_segment_writes(&command[first..at]) {
                return true;
            }
            at += size;
            first = at;
        } else {
            at += 1;
        }
    }
    shell_segment_writes(&command[first..])
}
pub fn guard_decision(input: &Object, cwd: &Path) -> &'static str {
    let tool = text(input.get("tool_name"));
    let args = records::obj(input.get("tool_input"));
    if tool == "Bash" {
        return if args
            .get("command")
            .and_then(crate::json::Value::as_str)
            .is_some_and(bash_writes_expertise)
        {
            GUARD_REASON
        } else {
            ""
        };
    }
    if let Some(key) = file_tool_field(&tool) {
        if args
            .get(key)
            .and_then(crate::json::Value::as_str)
            .is_some_and(|path| !path.is_empty() && is_expertise_file(path, cwd))
        {
            return GUARD_REASON;
        }
    }
    ""
}
