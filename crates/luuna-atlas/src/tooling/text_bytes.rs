//! Reversible internal text projection for surgical edits to arbitrary stored bytes.
//! ASCII marker positions and Unicode whitespace retain their meaning. A reserved
//! scalar is escaped even when originally valid, so user text cannot collide with it.
const ESC: char = '\u{f0000}';
pub(super) fn encode(mut bytes: &[u8]) -> String {
    let mut out = String::new();
    while !bytes.is_empty() {
        match std::str::from_utf8(bytes) {
            Ok(s) => {
                for c in s.chars() {
                    out.push(c);
                    if c == ESC {
                        out.push(ESC)
                    }
                }
                break;
            }
            Err(e) => {
                let (valid, rest) = bytes.split_at(e.valid_up_to());
                for c in std::str::from_utf8(valid).unwrap().chars() {
                    out.push(c);
                    if c == ESC {
                        out.push(ESC)
                    }
                }
                let n = e.error_len().unwrap_or(rest.len());
                for &b in &rest[..n] {
                    out.push(ESC);
                    out.push(char::from_u32(0xf0100 + u32::from(b)).unwrap());
                }
                bytes = &rest[n..];
            }
        }
    }
    out
}
pub(super) fn decode(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    let mut buf = [0u8; 4];
    while let Some(c) = chars.next() {
        if c == ESC {
            match chars.next() {
                Some(ESC) => out.extend_from_slice(ESC.encode_utf8(&mut buf).as_bytes()),
                Some(next) if (0xf0100..=0xf01ff).contains(&(next as u32)) => {
                    out.push((next as u32 - 0xf0100) as u8)
                }
                Some(next) => {
                    out.extend_from_slice(ESC.encode_utf8(&mut buf).as_bytes());
                    out.extend_from_slice(next.encode_utf8(&mut buf).as_bytes())
                }
                None => out.extend_from_slice(ESC.encode_utf8(&mut buf).as_bytes()),
            }
        } else {
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    out
}
pub(super) fn remove_git_section(content: &str) -> String {
    let raw = decode(content);
    let find = |word: &[u8]| raw.windows(word.len()).position(|s| s == word);
    let Some(start) = find(b"# luuna-atlas:start") else {
        return content.into();
    };
    let cut = find(b"# luuna-atlas:end")
        .map_or(b"# luuna-atlas:end".len() - 1, |n| {
            n + b"# luuna-atlas:end".len()
        })
        .min(raw.len());
    let mut out = raw[..start].to_vec();
    out.extend_from_slice(&raw[cut..]);
    encode(&out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn byte_projection_is_reversible_and_cannot_collide() {
        let mut bytes = Vec::new();
        for n in 0..=255 {
            bytes.push(n);
        }
        bytes.extend_from_slice("α\u{f0000}\u{f01ff}\n".as_bytes());
        assert_eq!(decode(&encode(&bytes)), bytes);
        assert_eq!(decode(&encode(b"prefix\xff\x80\n")), b"prefix\xff\x80\n");
    }
}
