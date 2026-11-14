//! Stored settings preflight; the duplicate-key scanner mirrors the existing record guard.
use crate::{
    actions::record::message,
    json::{self, Object},
    records, Result,
};
use std::{collections::HashSet, fs, path::Path};
pub(super) fn check(path: &Path, next: &Object) -> Result<()> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(message(format!(
                "Refusing to rewrite unreadable {}: {e}",
                path.display()
            )))
        }
    };
    if std::str::from_utf8(&bytes).is_err() {
        return Err(message(format!("Refusing to rewrite {}: stored JSON contains invalid UTF-8 bytes; nothing was written.",path.display())));
    }
    if let Some(key) = first_repeated_key(&bytes) {
        return Err(message(format!("Refusing to rewrite {}: repeated JSON key {} would lose a stored value; nothing was written.",path.display(),records::go_quote(&key))));
    }
    records::check_retained_numbers(path, [(next, next)])
}
enum ScanFrame {
    Array,
    /// The keys seen so far, decoded, and whether the next string is a key.
    Object(HashSet<String>, bool),
}

/// GDL3: the first key, in text order, that an object of `line` holds twice, compared
/// after decoding as the parser decodes it (escapes, invalid UTF-8 and lone surrogates
/// as U+FFFD).
///
/// `line` has already been parsed successfully, so it is well-formed JSON. One pass over
/// the bytes with an explicit stack of open containers: no recursion per nesting level,
/// and each byte is looked at a bounded number of times (each key is decoded once, from
/// its own slice), so the scan is linear in the line's length.
fn first_repeated_key(line: &[u8]) -> Option<String> {
    let mut stack: Vec<ScanFrame> = Vec::new();
    let mut i = 0;
    while i < line.len() {
        match line[i] {
            b'{' => stack.push(ScanFrame::Object(HashSet::new(), true)),
            b'[' => stack.push(ScanFrame::Array),
            b'}' | b']' => {
                stack.pop();
            }
            b',' => {
                if let Some(ScanFrame::Object(_, expect_key)) = stack.last_mut() {
                    *expect_key = true;
                }
            }
            b'"' => {
                let start = i;
                i += 1;
                while i < line.len() {
                    match line[i] {
                        b'\\' => i += 2,
                        b'"' => break,
                        _ => i += 1,
                    }
                }
                let end = (i + 1).min(line.len());
                if let Some(ScanFrame::Object(keys, expect_key)) = stack.last_mut() {
                    if *expect_key {
                        *expect_key = false;
                        // The parser's own decoding of the key, so equal keys are the
                        // keys Go's decoder merges.
                        if let Ok(json::Value::String(key)) =
                            json::parse_bytes(&line[start..end]).as_mut()
                        {
                            // `Value` implements Drop, so the text is taken, not moved.
                            let key = std::mem::take(key);
                            if keys.contains(&key) {
                                return Some(key);
                            }
                            keys.insert(key);
                        }
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}
