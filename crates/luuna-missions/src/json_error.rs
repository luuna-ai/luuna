//! The error text Go's `json.Unmarshal` gives for a document it rejects
//! (`encoding/json/scanner.go`, `decode.go`), which `update --extensions`
//! prints as part of its own message.

/// The deepest nesting the scanner accepts (`maxNestingDepth`).
const MAX_DEPTH: usize = 10_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Parse {
    ObjectKey,
    ObjectValue,
    ArrayValue,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    BeginValueOrEmpty,
    BeginValue,
    BeginStringOrEmpty,
    BeginString,
    EndValue,
    EndTop,
    InString,
    InStringEsc,
    /// Hex digits still expected in a `\u` escape.
    InStringEscU(u8),
    Neg,
    Zero,
    Digits,
    Dot,
    DotDigits,
    Exp,
    ExpSign,
    ExpDigits,
    /// A literal: its text and how many bytes matched.
    Literal(&'static [u8], usize),
}

/// Go's `quoteChar`: the byte as a rune, quoted with single quotes.
fn quote_char(byte: u8) -> String {
    match byte {
        b'\'' => "'\\''".to_owned(),
        b'"' => "'\"'".to_owned(),
        b'\\' => "'\\\\'".to_owned(),
        0x07 => "'\\a'".to_owned(),
        0x08 => "'\\b'".to_owned(),
        0x0c => "'\\f'".to_owned(),
        b'\n' => "'\\n'".to_owned(),
        b'\r' => "'\\r'".to_owned(),
        b'\t' => "'\\t'".to_owned(),
        0x0b => "'\\v'".to_owned(),
        0x00..=0x1f | 0x7f => format!("'\\x{byte:02x}'"),
        0x80..=0xa0 | 0xad => format!("'\\u{byte:04x}'"),
        _ => format!("'{}'", char::from(byte)),
    }
}

/// Go's message for the first syntax error in `src`, or `None` when `src` is
/// one valid JSON value.
pub(crate) fn syntax_error(src: &[u8]) -> Option<String> {
    let invalid =
        |byte: u8, context: &str| Some(format!("invalid character {} {context}", quote_char(byte)));
    let mut stack: Vec<Parse> = Vec::new();
    let mut state = State::BeginValue;
    for &byte in src {
        let space = matches!(byte, b' ' | b'\t' | b'\r' | b'\n');
        // A state that ends a value hands the byte to `EndValue`.
        let mut current = state;
        loop {
            match current {
                State::BeginValueOrEmpty => {
                    if space {
                        break;
                    }
                    if byte == b']' {
                        current = State::EndValue;
                        continue;
                    }
                    current = State::BeginValue;
                    continue;
                }
                State::BeginValue => {
                    if space {
                        break;
                    }
                    state = match byte {
                        b'{' => {
                            stack.push(Parse::ObjectKey);
                            State::BeginStringOrEmpty
                        }
                        b'[' => {
                            stack.push(Parse::ArrayValue);
                            State::BeginValueOrEmpty
                        }
                        b'"' => State::InString,
                        b'-' => State::Neg,
                        b'0' => State::Zero,
                        b'1'..=b'9' => State::Digits,
                        b't' => State::Literal(b"true", 1),
                        b'f' => State::Literal(b"false", 1),
                        b'n' => State::Literal(b"null", 1),
                        _ => return invalid(byte, "looking for beginning of value"),
                    };
                    if stack.len() > MAX_DEPTH {
                        return Some("exceeded max depth".to_owned());
                    }
                    break;
                }
                State::BeginStringOrEmpty => {
                    if space {
                        break;
                    }
                    if byte == b'}' {
                        if let Some(top) = stack.last_mut() {
                            *top = Parse::ObjectValue;
                        }
                        current = State::EndValue;
                        continue;
                    }
                    current = State::BeginString;
                    continue;
                }
                State::BeginString => {
                    if space {
                        break;
                    }
                    if byte == b'"' {
                        state = State::InString;
                        break;
                    }
                    return invalid(byte, "looking for beginning of object key string");
                }
                State::EndValue => {
                    let Some(top) = stack.last().copied() else {
                        current = State::EndTop;
                        continue;
                    };
                    if space {
                        state = State::EndValue;
                        break;
                    }
                    match (top, byte) {
                        (Parse::ObjectKey, b':') => {
                            *stack.last_mut().expect("non-empty") = Parse::ObjectValue;
                            state = State::BeginValue;
                        }
                        (Parse::ObjectKey, _) => return invalid(byte, "after object key"),
                        (Parse::ObjectValue, b',') => {
                            *stack.last_mut().expect("non-empty") = Parse::ObjectKey;
                            state = State::BeginString;
                        }
                        (Parse::ObjectValue, b'}') => {
                            stack.pop();
                            state = State::EndValue;
                        }
                        (Parse::ObjectValue, _) => {
                            return invalid(byte, "after object key:value pair")
                        }
                        (Parse::ArrayValue, b',') => state = State::BeginValue,
                        (Parse::ArrayValue, b']') => {
                            stack.pop();
                            state = State::EndValue;
                        }
                        (Parse::ArrayValue, _) => return invalid(byte, "after array element"),
                    }
                    break;
                }
                State::EndTop => {
                    if !space {
                        return invalid(byte, "after top-level value");
                    }
                    state = State::EndTop;
                    break;
                }
                State::InString => {
                    match byte {
                        b'"' => state = State::EndValue,
                        b'\\' => state = State::InStringEsc,
                        0..=0x1f => return invalid(byte, "in string literal"),
                        _ => {}
                    }
                    break;
                }
                State::InStringEsc => {
                    state = match byte {
                        b'b' | b'f' | b'n' | b'r' | b't' | b'\\' | b'/' | b'"' => State::InString,
                        b'u' => State::InStringEscU(4),
                        _ => return invalid(byte, "in string escape code"),
                    };
                    break;
                }
                State::InStringEscU(left) => {
                    if !byte.is_ascii_hexdigit() {
                        return invalid(byte, "in \\u hexadecimal character escape");
                    }
                    state = if left == 1 {
                        State::InString
                    } else {
                        State::InStringEscU(left - 1)
                    };
                    break;
                }
                State::Neg => {
                    state = match byte {
                        b'0' => State::Zero,
                        b'1'..=b'9' => State::Digits,
                        _ => return invalid(byte, "in numeric literal"),
                    };
                    break;
                }
                State::Digits if byte.is_ascii_digit() => {
                    state = State::Digits;
                    break;
                }
                State::Digits | State::Zero => match byte {
                    b'.' => {
                        state = State::Dot;
                        break;
                    }
                    b'e' | b'E' => {
                        state = State::Exp;
                        break;
                    }
                    _ => {
                        current = State::EndValue;
                        continue;
                    }
                },
                State::Dot => {
                    if !byte.is_ascii_digit() {
                        return invalid(byte, "after decimal point in numeric literal");
                    }
                    state = State::DotDigits;
                    break;
                }
                State::DotDigits => match byte {
                    b'0'..=b'9' => break,
                    b'e' | b'E' => {
                        state = State::Exp;
                        break;
                    }
                    _ => {
                        current = State::EndValue;
                        continue;
                    }
                },
                State::Exp if byte == b'+' || byte == b'-' => {
                    state = State::ExpSign;
                    break;
                }
                State::Exp | State::ExpSign => {
                    if !byte.is_ascii_digit() {
                        return invalid(byte, "in exponent of numeric literal");
                    }
                    state = State::ExpDigits;
                    break;
                }
                State::ExpDigits => {
                    if byte.is_ascii_digit() {
                        break;
                    }
                    current = State::EndValue;
                    continue;
                }
                State::Literal(text, matched) => {
                    if byte != text[matched] {
                        let name = String::from_utf8_lossy(text).into_owned();
                        let expected = char::from(text[matched]);
                        return invalid(
                            byte,
                            &format!("in literal {name} (expecting '{expected}')"),
                        );
                    }
                    state = if matched + 1 == text.len() {
                        State::EndValue
                    } else {
                        State::Literal(text, matched + 1)
                    };
                    break;
                }
            }
        }
    }
    // The end of the input: a number or a finished value may end the
    // document; anything else is cut short.
    let finished = match state {
        State::EndValue | State::EndTop => stack.is_empty(),
        State::Zero | State::Digits | State::DotDigits | State::ExpDigits => stack.is_empty(),
        _ => false,
    };
    (!finished).then(|| "unexpected end of JSON input".to_owned())
}

/// Go's message for the first number in the valid document `src` that
/// `float64` cannot hold: decoding into `any` goes on and reports it at the
/// end (`json: cannot unmarshal number 1e999 into Go value of type float64`).
pub(crate) fn out_of_range_number(src: &[u8]) -> Option<String> {
    let mut index = 0;
    while index < src.len() {
        match src[index] {
            b'"' => {
                index += 1;
                while index < src.len() && src[index] != b'"' {
                    index += if src[index] == b'\\' { 2 } else { 1 };
                }
                index += 1;
            }
            b'-' | b'0'..=b'9' => {
                let start = index;
                while index < src.len()
                    && matches!(src[index], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                {
                    index += 1;
                }
                let text = std::str::from_utf8(&src[start..index]).unwrap_or("0");
                if text.parse::<f64>().is_ok_and(f64::is_infinite) {
                    return Some(format!(
                        "json: cannot unmarshal number {text} into Go value of type float64"
                    ));
                }
            }
            _ => index += 1,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(text: &str) -> Option<String> {
        syntax_error(text.as_bytes())
    }

    #[test]
    fn valid_documents_have_no_error() {
        for text in [
            "{}",
            " [1, 2.5e-3, \"a\\u00e9\", true, false, null, {\"a\": []}] ",
            "0",
            "-0.5",
            "\"x\"",
            "1E+9",
        ] {
            assert_eq!(error(text), None, "{text}");
        }
    }

    #[test]
    fn messages_follow_go() {
        for (text, message) in [
            ("", "unexpected end of JSON input"),
            ("   ", "unexpected end of JSON input"),
            ("{", "unexpected end of JSON input"),
            ("{\"a\":", "unexpected end of JSON input"),
            ("[1,", "unexpected end of JSON input"),
            ("\"abc", "unexpected end of JSON input"),
            ("tru", "unexpected end of JSON input"),
            ("1.", "unexpected end of JSON input"),
            ("-", "unexpected end of JSON input"),
            ("x", "invalid character 'x' looking for beginning of value"),
            (
                "{a:1}",
                "invalid character 'a' looking for beginning of object key string",
            ),
            ("{\"a\" 1}", "invalid character '1' after object key"),
            (
                "{\"a\":1 \"b\"}",
                "invalid character '\"' after object key:value pair",
            ),
            ("[1 2]", "invalid character '2' after array element"),
            ("{} x", "invalid character 'x' after top-level value"),
            (
                "{\"a\":1,}",
                "invalid character '}' looking for beginning of object key string",
            ),
            (
                "[1,]",
                "invalid character ']' looking for beginning of value",
            ),
            (
                "tx",
                "invalid character 'x' in literal true (expecting 'r')",
            ),
            (
                "nul1",
                "invalid character '1' in literal null (expecting 'l')",
            ),
            ("-x", "invalid character 'x' in numeric literal"),
            (
                "1.x",
                "invalid character 'x' after decimal point in numeric literal",
            ),
            (
                "1ex",
                "invalid character 'x' in exponent of numeric literal",
            ),
            (
                "1e+x",
                "invalid character 'x' in exponent of numeric literal",
            ),
            ("\"\\q\"", "invalid character 'q' in string escape code"),
            (
                "\"\\u12x4\"",
                "invalid character 'x' in \\u hexadecimal character escape",
            ),
            ("\"a\nb\"", "invalid character '\\n' in string literal"),
            (
                "'a'",
                "invalid character '\\'' looking for beginning of value",
            ),
            (
                "\u{e9}",
                "invalid character 'Ã' looking for beginning of value",
            ),
            ("01", "invalid character '1' after top-level value"),
        ] {
            assert_eq!(error(text).as_deref(), Some(message), "{text:?}");
        }
    }

    #[test]
    fn nesting_is_limited() {
        let deep = "[".repeat(MAX_DEPTH) + &"]".repeat(MAX_DEPTH);
        assert_eq!(error(&deep), None);
        let too_deep = "[".repeat(MAX_DEPTH + 1);
        assert_eq!(error(&too_deep).as_deref(), Some("exceeded max depth"));
    }

    #[test]
    fn numbers_out_of_range_are_reported_in_order() {
        assert_eq!(out_of_range_number(b"{\"a\":[1,2e3]}"), None);
        assert_eq!(
            out_of_range_number(b"{\"a\":\"1e999\",\"b\":[1e999,-1e999]}").as_deref(),
            Some("json: cannot unmarshal number 1e999 into Go value of type float64")
        );
    }
}
