//! Go `fmt` behaviour the Go tool relies on: `%q` quoting of names in error
//! messages, `fmt.Sscanf("%d")` for the `name@N` and `--limit` forms, and
//! `%x` of an `int64` for record ids.

use regex::Regex;
use std::fmt::Write as _;
use std::sync::OnceLock;

/// `fmt.Sprintf("%q", text)`, which is `strconv.Quote`: a double-quoted Go
/// string literal. Printable characters (Unicode categories L, M, N, P, S and
/// the ASCII space) are kept; `"` and `\` are backslashed; `\a \b \f \n \r \t
/// \v` use their short escapes; other control characters below U+0020 and
/// U+007F become `\xNN`; any other character Go does not print becomes
/// `\uNNNN` or `\UNNNNNNNN` (lowercase hex).
pub(crate) fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' | '\\' => {
                out.push('\\');
                out.push(character);
            }
            _ if is_print(character) => out.push(character),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{b}' => out.push_str("\\v"),
            _ => {
                let code = u32::from(character);
                if code < 0x20 || code == 0x7f {
                    let _ = write!(out, "\\x{code:02x}");
                } else if code < 0x10000 {
                    let _ = write!(out, "\\u{code:04x}");
                } else {
                    let _ = write!(out, "\\U{code:08x}");
                }
            }
        }
    }
    out.push('"');
    out
}

/// Go's `strconv.IsPrint`: letters, marks, numbers, punctuation, symbols and
/// the ASCII space.
fn is_print(character: char) -> bool {
    if character.is_ascii() {
        return (' '..='~').contains(&character);
    }
    static PRINTABLE: OnceLock<Regex> = OnceLock::new();
    let printable = PRINTABLE.get_or_init(|| {
        Regex::new(r"^[\p{L}\p{M}\p{N}\p{P}\p{S}]$").expect("the printable class compiles")
    });
    let mut buffer = [0u8; 4];
    printable.is_match(character.encode_utf8(&mut buffer))
}

/// `fmt.Sscanf(input, "%d", &n)` into a Go `int` (64-bit): `None` where Go
/// returns an error.
///
/// Go skips leading white space (but fails on a newline, which `Sscanf` does
/// not treat as space; a carriage return directly before it is skipped
/// first), accepts one optional `+` or `-`, then requires at least one ASCII
/// digit and takes the longest run of them. Whatever follows the digits is
/// ignored. A value outside the `int64` range is an error.
pub(crate) fn sscan_int(input: &str) -> Option<i64> {
    let mut rest = input;
    // Go calls SkipSpace twice (doScanf, then scanInt); once is equivalent.
    loop {
        let mut chars = rest.chars();
        match chars.next() {
            None => return None, // unexpected EOF
            Some('\r') if chars.as_str().starts_with('\n') => rest = chars.as_str(),
            Some('\n') => return None, // unexpected newline
            Some(character) if is_scan_space(character) => rest = chars.as_str(),
            Some(_) => break,
        }
    }
    let mut token = String::new();
    if let Some(sign) = rest.chars().next().filter(|c| *c == '+' || *c == '-') {
        token.push(sign);
        rest = &rest[1..];
    }
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None; // expected integer
    }
    token.push_str(&rest[..digits]);
    // strconv.ParseInt(token, 10, 64): accepts the sign, fails on overflow.
    token.parse::<i64>().ok()
}

/// The white space of Go's `fmt` scanner (`fmt/scan.go`, `space`).
fn is_scan_space(character: char) -> bool {
    matches!(
        u32::from(character),
        0x0009..=0x000d
            | 0x0020
            | 0x0085
            | 0x00a0
            | 0x1680
            | 0x2000..=0x200a
            | 0x2028..=0x2029
            | 0x202f
            | 0x205f
            | 0x3000
    )
}

/// `fmt.Sprintf("%x", value)` for a Go `int64`: lowercase hex, with a leading
/// `-` for a negative value.
pub(crate) fn hex_i64(value: i64) -> String {
    if value < 0 {
        format!("-{:x}", value.unsigned_abs())
    } else {
        format!("{value:x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_like_go_percent_q() {
        let cases: &[(&str, &str)] = &[
            ("", r#""""#),
            ("plain name", r#""plain name""#),
            ("say \"hi\"", r#""say \"hi\"""#),
            ("back\\slash", r#""back\\slash""#),
            ("<b>&amp;</b>", r#""<b>&amp;</b>""#),
            ("a\u{7}\u{8}\u{c}\n\r\t\u{b}z", r#""a\a\b\f\n\r\t\vz""#),
            ("\u{0}\u{1}\u{1f}\u{7f}", r#""\x00\x01\x1f\x7f""#),
            (
                "caf\u{e9} \u{65e5}\u{672c} \u{1f600}",
                "\"caf\u{e9} \u{65e5}\u{672c} \u{1f600}\"",
            ),
            // Combining mark (M) and currency symbol (S) are printable.
            ("e\u{301} \u{20ac}", "\"e\u{301} \u{20ac}\""),
            // Spaces other than U+0020, separators and format characters are not.
            ("a\u{a0}b", r#""a\u00a0b""#),
            ("\u{2028}\u{2029}", r#""\u2028\u2029""#),
            ("soft\u{ad}hyphen", r#""soft\u00adhyphen""#),
            ("\u{200b}\u{feff}", r#""\u200b\ufeff""#),
            ("\u{85}", r#""\u0085""#),
            ("\u{e000}", r#""\ue000""#),
            ("\u{e0001}", r#""\U000e0001""#),
            ("\u{10ffff}", r#""\U0010ffff""#),
        ];
        for (input, expected) in cases {
            assert_eq!(quote(input), *expected, "quote({input:?})");
        }
    }

    #[test]
    fn scans_integers_like_go_sscanf() {
        let cases: &[(&str, Option<i64>)] = &[
            ("1", Some(1)),
            ("42", Some(42)),
            ("007", Some(7)),
            ("+5", Some(5)),
            ("-3", Some(-3)),
            ("-0", Some(0)),
            ("  12", Some(12)),
            ("\t\u{b}\u{c}\r9", Some(9)),
            ("\u{a0}\u{3000}4", Some(4)),
            ("5abc", Some(5)),
            ("5 6", Some(5)),
            ("12.9", Some(12)),
            ("1e3", Some(1)),
            ("0x10", Some(0)),
            ("1_000", Some(1)),
            ("9223372036854775807", Some(i64::MAX)),
            ("-9223372036854775808", Some(i64::MIN)),
            ("9223372036854775808", None),
            ("", None),
            ("   ", None),
            ("abc", None),
            ("+", None),
            ("-", None),
            ("+-1", None),
            ("--1", None),
            ("\n1", None),
            ("\r\n1", None),
            (" \n1", None),
            ("\u{660}", None),
        ];
        for (input, expected) in cases {
            assert_eq!(sscan_int(input), *expected, "sscan_int({input:?})");
        }
    }

    #[test]
    fn formats_int64_hex_like_go() {
        assert_eq!(hex_i64(0), "0");
        assert_eq!(hex_i64(255), "ff");
        assert_eq!(hex_i64(0x18a3_f00d_beef_1234), "18a3f00dbeef1234");
        assert_eq!(hex_i64(-1), "-1");
        assert_eq!(hex_i64(i64::MIN), "-8000000000000000");
    }
}
