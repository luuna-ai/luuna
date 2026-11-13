//! Go's `base64.StdEncoding`: the padded standard alphabet, and a decoder that
//! skips carriage returns and line feeds and reports `CorruptInputError`
//! offsets the way `encoding/base64` does (`decodeQuantum`, non-strict).

use super::ArtifactsError;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `base64.StdEncoding.EncodeToString`.
pub(crate) fn encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(char::from(ALPHABET[(value >> 18) as usize & 63]));
        out.push(char::from(ALPHABET[(value >> 12) as usize & 63]));
        out.push(if chunk.len() > 1 {
            char::from(ALPHABET[(value >> 6) as usize & 63])
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            char::from(ALPHABET[value as usize & 63])
        } else {
            '='
        });
    }
    out
}

fn sextet(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn corrupt(offset: usize) -> ArtifactsError {
    ArtifactsError::Base64 { offset }
}

fn is_newline(byte: u8) -> bool {
    byte == b'\n' || byte == b'\r'
}

/// `base64.StdEncoding.DecodeString`. The output holds at most three bytes
/// for every four input bytes, so it is bounded by the input.
pub(crate) fn decode(source: &[u8]) -> Result<Vec<u8>, ArtifactsError> {
    let mut out = Vec::with_capacity(source.len() / 4 * 3 + 3);
    let mut index = 0;
    while index < source.len() {
        // One quantum (Go's decodeQuantum).
        let mut quad = [0u8; 4];
        let mut length = 4;
        let mut error: Option<ArtifactsError> = None;
        let mut slot = 0;
        while slot < 4 {
            if index == source.len() {
                if slot == 0 {
                    // Only line breaks were left: a clean end.
                    return Ok(out);
                }
                // One sextet, or padding required (it always is).
                return Err(corrupt(index - slot));
            }
            let byte = source[index];
            index += 1;
            if let Some(value) = sextet(byte) {
                quad[slot] = value;
                slot += 1;
                continue;
            }
            if is_newline(byte) {
                continue;
            }
            if byte != b'=' {
                return Err(corrupt(index - 1));
            }
            // Padding.
            match slot {
                0 | 1 => return Err(corrupt(index - 1)),
                2 => {
                    while index < source.len() && is_newline(source[index]) {
                        index += 1;
                    }
                    if index == source.len() {
                        return Err(corrupt(source.len()));
                    }
                    if source[index] != b'=' {
                        return Err(corrupt(index - 1));
                    }
                    index += 1;
                }
                _ => {}
            }
            while index < source.len() && is_newline(source[index]) {
                index += 1;
            }
            if index < source.len() {
                error = Some(corrupt(index));
            }
            length = slot;
            break;
        }
        let value = (u32::from(quad[0]) << 18)
            | (u32::from(quad[1]) << 12)
            | (u32::from(quad[2]) << 6)
            | u32::from(quad[3]);
        let bytes = [(value >> 16) as u8, (value >> 8) as u8, value as u8];
        // length is 2, 3 or 4 here; it yields length - 1 bytes.
        out.extend_from_slice(&bytes[..length - 1]);
        if let Some(error) = error {
            return Err(error);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offset(input: &str) -> usize {
        match decode(input.as_bytes()) {
            Err(ArtifactsError::Base64 { offset }) => offset,
            other => panic!("{input:?}: expected an error, got {other:?}"),
        }
    }

    #[test]
    fn encodes_like_std_encoding() {
        let cases: &[(&[u8], &str)] = &[
            (b"", ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"fooba", "Zm9vYmE="),
            (b"foobar", "Zm9vYmFy"),
            (&[0xff, 0xfe, 0xfd, 0xfc, 0xfb], "//79/Ps="),
            (&[0xfb, 0xff], "+/8="),
        ];
        for (input, expected) in cases {
            assert_eq!(encode(input), *expected);
            assert_eq!(decode(expected.as_bytes()).unwrap(), *input);
        }
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(decode(encode(&all).as_bytes()).unwrap(), all);
    }

    #[test]
    fn skips_cr_and_lf() {
        assert_eq!(decode(b"aGVs\r\nbG8=").unwrap(), b"hello");
        assert_eq!(decode(b"\nZ\rm\n9v\n").unwrap(), b"foo");
        assert_eq!(decode(b"Zg=\n=\r\n").unwrap(), b"f");
        assert_eq!(decode(b"\r\n").unwrap(), b"");
        assert_eq!(decode(b"").unwrap(), b"");
    }

    #[test]
    fn reports_go_offsets() {
        assert_eq!(offset("AAAA!AAA"), 4);
        assert_eq!(offset("!AAA"), 0);
        assert_eq!(offset("A"), 0);
        assert_eq!(offset("AAA"), 0);
        assert_eq!(offset("AAAAA"), 4);
        assert_eq!(offset("="), 0);
        assert_eq!(offset("A="), 1);
        assert_eq!(offset("AA="), 3);
        assert_eq!(offset("AA=x"), 2);
        assert_eq!(offset("AA==A"), 4);
        assert_eq!(offset("AAA=A"), 4);
        assert_eq!(offset("AAA= "), 4);
        assert_eq!(offset("AAAA=AAA"), 4);
        assert_eq!(
            ArtifactsError::Base64 { offset: 4 }.to_string(),
            "illegal base64 data at input byte 4"
        );
    }
}
