//! Embedded format assets and the session-end reminder. Owner: s5-atlas-lib-2.
//!
//! A port of the Go tool's `format_assets.go`: the 32 generated prose blocks the prime,
//! session-close, setup and onboard output is built from.

/// The 32 asset keys, in `format_assets.go` order.
pub const KEYS: [&str; 32] = [
    "prime_compact",
    "prime_empty_compact",
    "prime_markdown",
    "prime_empty_markdown",
    "prime_plain",
    "prime_empty_plain",
    "prime_xml",
    "prime_empty_xml",
    "close_conditional_compact",
    "close_conditional_markdown",
    "close_conditional_plain",
    "close_conditional_xml",
    "close_conditional_embedded",
    "close_minimal_compact",
    "close_minimal_markdown",
    "close_minimal_plain",
    "close_minimal_xml",
    "close_minimal_embedded",
    "close_directive_compact",
    "close_directive_markdown",
    "close_directive_plain",
    "close_directive_xml",
    "close_directive_embedded",
    "close_none_compact",
    "close_none_markdown",
    "close_none_plain",
    "close_none_xml",
    "close_none_embedded",
    "cursor",
    "codex",
    "git_hook",
    "onboard",
];

/// The asset `key` byte for byte as `formatAssets[key]`, or `""` for any other key (a Go
/// map's zero value).
pub fn get(key: &str) -> &'static str {
    // stub until t2
    let _ = key;
    ""
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_key_is_empty() {
        assert_eq!(get(""), "");
        assert_eq!(get("no_such_asset"), "");
        assert_eq!(get("PRIME_COMPACT"), "");
        assert_eq!(get("close_conditional_json"), "");
    }

    #[test]
    fn thirty_two_keys_in_go_order() {
        assert_eq!(KEYS.len(), 32);
        assert_eq!(KEYS[0], "prime_compact");
        assert_eq!(KEYS[8], "close_conditional_compact");
        assert_eq!(KEYS[31], "onboard");
        for (i, key) in KEYS.iter().enumerate() {
            assert!(!KEYS[..i].contains(key), "duplicate key {key}");
            if key.starts_with("close_none_") {
                assert_eq!(get(key), "", "{key}");
            } else {
                assert!(!get(key).is_empty(), "{key} is empty");
            }
        }
        assert!(get("prime_compact").starts_with("# Project Expertise (via Atlas)\n"));
        assert_eq!(get("prime_plain"), "__ATLAS_SECTIONS__");
    }

    /// Reads the Go string literal at the start of `s` (after its opening quote) and
    /// returns its bytes and the rest of `s` after the closing quote.
    fn unquote_go(s: &str) -> (Vec<u8>, &str) {
        let bytes = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        let hex = |digits: &[u8]| -> u32 {
            u32::from_str_radix(std::str::from_utf8(digits).expect("hex"), 16).expect("hex")
        };
        loop {
            match bytes[i] {
                b'"' => return (out, &s[i + 1..]),
                b'\\' => {
                    let c = bytes[i + 1];
                    i += 2;
                    let simple = match c {
                        b'a' => Some(7),
                        b'b' => Some(8),
                        b'f' => Some(12),
                        b'n' => Some(b'\n'),
                        b'r' => Some(b'\r'),
                        b't' => Some(b'\t'),
                        b'v' => Some(11),
                        b'\\' => Some(b'\\'),
                        b'"' => Some(b'"'),
                        b'\'' => Some(b'\''),
                        _ => None,
                    };
                    if let Some(b) = simple {
                        out.push(b);
                        continue;
                    }
                    match c {
                        b'x' => {
                            out.push(hex(&bytes[i..i + 2]) as u8);
                            i += 2;
                        }
                        b'u' | b'U' => {
                            let n = if c == b'u' { 4 } else { 8 };
                            let ch = char::from_u32(hex(&bytes[i..i + n])).expect("char");
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                            i += n;
                        }
                        b'0'..=b'7' => {
                            let digits = std::str::from_utf8(&bytes[i - 1..i + 2]).expect("oct");
                            out.push(u8::from_str_radix(digits, 8).expect("octal"));
                            i += 2;
                        }
                        other => panic!("unknown escape \\{}", other as char),
                    }
                }
                b => {
                    out.push(b);
                    i += 1;
                }
            }
        }
    }

    #[test]
    #[ignore = "needs LUUNA_ATLAS_REFERENCE"]
    fn matches_go_source() {
        let reference = std::env::var_os("LUUNA_ATLAS_REFERENCE")
            .expect("LUUNA_ATLAS_REFERENCE names the Go reference clone");
        let path = std::path::Path::new(&reference)
            .join("internal")
            .join("atlas")
            .join("format_assets.go");
        let source = std::fs::read_to_string(&path).expect("read format_assets.go");
        let mut seen = Vec::new();
        for line in source.lines() {
            let Some(rest) = line.trim_start().strip_prefix('"') else {
                continue;
            };
            let (key, rest) = unquote_go(rest);
            let key = String::from_utf8(key).expect("key is UTF-8");
            let rest = rest
                .trim_start()
                .strip_prefix(':')
                .expect("colon")
                .trim_start();
            let value = rest.strip_prefix('"').expect("a double-quoted literal");
            let (value, tail) = unquote_go(value);
            assert_eq!(tail.trim(), ",", "one literal per line for {key}");
            assert_eq!(get(&key).as_bytes(), &value[..], "asset {key}");
            seen.push(key);
        }
        assert_eq!(seen, KEYS.to_vec());
    }
}
