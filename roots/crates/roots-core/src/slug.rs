//! Kebab-case actor names and concise statement slugs.

use std::collections::BTreeSet;
use unicode_normalization::UnicodeNormalization;

pub const SLUG_MAX_WORDS: usize = 4;
pub const SLUG_MAX_LENGTH: usize = 40;

const STOPWORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "of", "to", "in", "on", "at", "for", "with", "by", "is",
    "are", "was", "were", "be", "been", "it", "its", "this", "that", "we", "i", "our", "should",
    "must", "can", "will", "so", "as", "from", "into",
];

fn words(text: &str) -> Vec<String> {
    let normalized = text
        .nfkd()
        .filter(|character| !unicode_normalization::char::is_combining_mark(*character))
        .collect::<String>()
        .to_lowercase();
    normalized
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn join_within(parts: &[String], max_length: usize) -> String {
    let mut output = String::new();
    for part in parts {
        let candidate = if output.is_empty() {
            part.clone()
        } else {
            format!("{output}-{part}")
        };
        if candidate.len() > max_length {
            break;
        }
        output = candidate;
    }
    if output.is_empty() {
        parts
            .first()
            .map_or_else(String::new, |part| part.chars().take(max_length).collect())
    } else {
        output
    }
}

#[must_use]
pub fn kebab(text: &str) -> String {
    words(text).join("-")
}

#[must_use]
pub fn slugify(text: &str) -> String {
    let all = words(text);
    let content = all
        .iter()
        .filter(|word| !STOPWORDS.contains(&word.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let chosen = if content.is_empty() { &all } else { &content };
    join_within(&chosen[..chosen.len().min(SLUG_MAX_WORDS)], SLUG_MAX_LENGTH)
}

#[must_use]
pub fn is_valid_slug(value: &str) -> bool {
    if value.is_empty() || value.len() > 64 || value.starts_with('-') || value.ends_with('-') {
        return false;
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return false;
    }
    if value.contains("--") {
        return false;
    }
    let mut pieces = value.split('-');
    let Some(prefix) = pieces.next() else {
        return false;
    };
    let Some(hex) = pieces.next() else {
        return true;
    };
    if !pieces.next().is_none() || !matches!(prefix, "r" | "s" | "p" | "q" | "e" | "ss") {
        return true;
    }
    !(4..=8).contains(&hex.len()) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[must_use]
pub fn unique_slug(base: &str, taken: &BTreeSet<String>) -> String {
    if !taken.contains(base) {
        return base.to_owned();
    }
    for number in 2.. {
        let candidate = format!("{base}-{number}");
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!("unbounded integer range is exhausted")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_a_compact_statement_slug() {
        assert_eq!(
            slugify("The project works offline without losing user work"),
            "project-works-offline-without"
        );
        assert_eq!(kebab("Jaymin West"), "jaymin-west");
    }

    #[test]
    fn validates_and_deduplicates_slugs() {
        assert!(is_valid_slug("offline-first"));
        assert!(!is_valid_slug("r-a1b2"));
        let taken = ["offline".to_owned(), "offline-2".to_owned()]
            .into_iter()
            .collect();
        assert_eq!(unique_slug("offline", &taken), "offline-3");
    }
}
