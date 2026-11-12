//! Stable Roots identifiers and collision handling.

use rand::RngCore;
use std::collections::BTreeSet;

pub const SHORT_ATTEMPTS: usize = 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdPrefix {
    Idea,
    Sprout,
    Proposal,
    Question,
    Edge,
    Session,
    Heading,
    Flow,
}

impl IdPrefix {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idea => "r",
            Self::Sprout => "s",
            Self::Proposal => "p",
            Self::Question => "q",
            Self::Edge => "e",
            Self::Session => "ss",
            Self::Heading => "h",
            Self::Flow => "fl",
        }
    }
}

#[must_use]
pub fn random_hex(length: usize) -> String {
    let mut bytes = vec![0_u8; length.div_ceil(2)];
    rand::rng().fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
        .chars()
        .take(length)
        .collect()
}

pub fn generate_hex<F>(taken: &BTreeSet<String>, mut random: F) -> String
where
    F: FnMut(usize) -> String,
{
    for length in [4, 6] {
        for _ in 0..SHORT_ATTEMPTS {
            let hex = random(length);
            if !taken.contains(&hex) {
                return hex;
            }
        }
    }
    loop {
        let hex = random(8);
        if !taken.contains(&hex) {
            return hex;
        }
    }
}

#[must_use]
pub fn make_id(prefix: IdPrefix, hex: &str) -> String {
    format!("{}-{hex}", prefix.as_str())
}

#[must_use]
pub fn generate_id(prefix: IdPrefix, taken_hex: &BTreeSet<String>) -> String {
    make_id(prefix, &generate_hex(taken_hex, random_hex))
}

#[must_use]
pub fn hex_of(id: &str) -> &str {
    id.split_once('-').map_or(id, |(_, hex)| hex)
}

#[must_use]
pub fn prefix_of(id: &str) -> Option<&str> {
    let (prefix, hex) = id.split_once('-')?;
    if matches!(prefix, "r" | "s" | "p" | "q" | "e" | "ss" | "h" | "fl")
        && (4..=8).contains(&hex.len())
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Some(prefix)
    } else {
        None
    }
}

#[must_use]
pub fn is_id(id: &str, expected: Option<IdPrefix>) -> bool {
    let Some(prefix) = prefix_of(id) else {
        return false;
    };
    expected.is_none_or(|kind| kind.as_str() == prefix)
}

#[must_use]
pub fn hex_set<'a>(ids: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    ids.into_iter().map(|id| hex_of(id).to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_short_ids_before_extending() {
        let taken = ["a1b2".to_owned()].into_iter().collect();
        let mut calls = 0;
        let id = generate_hex(&taken, |_| {
            calls += 1;
            if calls <= SHORT_ATTEMPTS {
                "a1b2".to_owned()
            } else {
                "abcdef".to_owned()
            }
        });
        assert_eq!(id, "abcdef");
    }

    #[test]
    fn validates_known_id_shapes() {
        assert!(is_id("ss-a1b2", Some(IdPrefix::Session)));
        assert!(!is_id("r-A1B2", None));
        assert_eq!(hex_of("r-a1b2"), "a1b2");
    }
}
