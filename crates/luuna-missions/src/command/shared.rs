//! Helpers that create, update and the read commands share
//! (`internal/cli/issue_create.go`, `issue_read.go`, `issue_mutate.go`).

use std::collections::HashSet;

use crate::model::{Ticket, Value};

/// `model.ValidTypes`.
pub(super) const TYPES: [&str; 4] = ["task", "bug", "feature", "epic"];
/// `model.ValidStatuses`.
pub(super) const STATUSES: [&str; 3] = ["open", "in_progress", "closed"];

/// Go's `strings.ToLower`: each character maps to its simple lower case, so
/// `İ` becomes `i` rather than `i` and a combining dot.
pub(super) fn go_lower(text: &str) -> String {
    text.chars()
        .flat_map(|ch| {
            let mut lower = ch.to_lowercase();
            match (lower.next(), lower.next()) {
                (Some(first), None) => vec![first],
                _ => vec!['i'],
            }
        })
        .collect()
}

/// Go's `parsePriority`: `0` to `4`, optionally with a `P` or `p`; empty is 2.
pub(super) fn parse_priority(raw: &str) -> Result<i64, String> {
    const MESSAGE: &str = "--priority must be 0-4 or P0-P4";
    if raw.is_empty() {
        return Ok(2);
    }
    let digits = if raw.starts_with(['p', 'P']) {
        &raw[1..]
    } else {
        raw
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(MESSAGE.to_owned());
    }
    match digits.parse::<i64>() {
        Ok(number) if (0..=4).contains(&number) => Ok(number),
        _ => Err(MESSAGE.to_owned()),
    }
}

/// Go's `parseIntent`: ids like `r-a1b2`, from comma lists, lower-cased,
/// without repeats. None for no id, a string for one, an array for more.
pub(super) fn parse_intent(values: &[String]) -> Result<Option<Value>, String> {
    let mut ids: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for value in values {
        for token in value.split(',') {
            let id = go_lower(token.trim());
            if id.is_empty() {
                continue;
            }
            if !is_intent_id(&id) {
                return Err(format!(
                    "Invalid --intent value: {id}. Expected a roots idea id like r-a1b2"
                ));
            }
            if seen.insert(id.clone()) {
                ids.push(id);
            }
        }
    }
    Ok(match ids.len() {
        0 => None,
        1 => Some(Value::String(ids.remove(0))),
        _ => Some(Value::Array(ids.into_iter().map(Value::String).collect())),
    })
}

/// `^r-[0-9a-f]{4,8}$`.
pub(super) fn is_intent_id(id: &str) -> bool {
    id.strip_prefix("r-").is_some_and(|hex| {
        (4..=8).contains(&hex.len())
            && hex
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// Go's `validateType`: empty is accepted.
pub(super) fn validate_type(value: &str) -> Result<(), String> {
    if value.is_empty() || TYPES.contains(&value) {
        Ok(())
    } else {
        Err(format!(
            "Invalid --type value: {value}. Valid: task|bug|feature|epic"
        ))
    }
}

/// Go's `validateStatus`: empty is accepted.
pub(super) fn validate_status(value: &str) -> Result<(), String> {
    if value.is_empty() || STATUSES.contains(&value) {
        Ok(())
    } else {
        Err(format!(
            "Invalid --status value: {value}. Valid: open|in_progress|closed"
        ))
    }
}

/// Go's `splitLabels`: comma list, trimmed and lower-cased, empties dropped.
/// An empty result is nil (`None`).
pub(super) fn split_labels(value: &str) -> Option<Vec<String>> {
    let labels: Vec<String> = value
        .split(',')
        .map(|label| go_lower(label.trim()))
        .filter(|label| !label.is_empty())
        .collect();
    (!labels.is_empty()).then_some(labels)
}

/// Go's `uniqueStrings`: first occurrences, never nil.
pub(super) fn unique_strings(items: Vec<String>) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.clone()))
        .collect()
}

/// Go's `withoutString`: nil (`None`) when nothing is left, and for nil.
pub(super) fn without_string(items: Option<Vec<String>>, target: &str) -> Option<Vec<String>> {
    let next: Vec<String> = items
        .into_iter()
        .flatten()
        .filter(|item| item != target)
        .collect();
    (!next.is_empty()).then_some(next)
}

/// Go's `issueIndex`: the first ticket with `id`.
pub(super) fn ticket_index(tickets: &[Ticket], id: &str) -> Option<usize> {
    tickets.iter().position(|ticket| ticket.id() == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn priorities_follow_go() {
        for (raw, expected) in [
            ("", 2),
            ("0", 0),
            ("4", 4),
            ("P1", 1),
            ("p3", 3),
            ("000", 0),
            ("P04", 4),
        ] {
            assert_eq!(parse_priority(raw), Ok(expected), "{raw}");
        }
        for raw in [
            "5",
            "P",
            "p",
            "x",
            "-1",
            "1.5",
            "P9",
            " 1",
            "P-1",
            "٣",
            "99999999999999999999",
        ] {
            assert_eq!(
                parse_priority(raw),
                Err("--priority must be 0-4 or P0-P4".to_owned()),
                "{raw}"
            );
        }
    }

    #[test]
    fn intents_follow_go() {
        assert_eq!(parse_intent(&[]), Ok(None));
        assert_eq!(parse_intent(&strings(&[" , "])), Ok(None));
        assert_eq!(
            parse_intent(&strings(&["R-A1B2"])),
            Ok(Some(Value::String("r-a1b2".to_owned())))
        );
        assert_eq!(
            parse_intent(&strings(&["r-a1b2, r-00ff00ff", "r-a1b2"])),
            Ok(Some(Value::Array(vec![
                Value::String("r-a1b2".to_owned()),
                Value::String("r-00ff00ff".to_owned())
            ])))
        );
        for bad in ["r-abc", "r-123456789", "x-abcd", "r-abcg", "abcd"] {
            assert_eq!(
                parse_intent(&strings(&[bad])),
                Err(format!(
                    "Invalid --intent value: {bad}. Expected a roots idea id like r-a1b2"
                ))
            );
        }
    }

    #[test]
    fn validation_accepts_empty_values() {
        assert_eq!(validate_type(""), Ok(()));
        assert_eq!(validate_type("epic"), Ok(()));
        assert_eq!(
            validate_type("story"),
            Err("Invalid --type value: story. Valid: task|bug|feature|epic".to_owned())
        );
        assert_eq!(validate_status(""), Ok(()));
        assert_eq!(validate_status("in_progress"), Ok(()));
        assert_eq!(
            validate_status("done"),
            Err("Invalid --status value: done. Valid: open|in_progress|closed".to_owned())
        );
    }

    #[test]
    fn label_helpers_distinguish_nil_from_empty() {
        assert_eq!(split_labels(""), None);
        assert_eq!(split_labels(" , "), None);
        assert_eq!(split_labels(" A, b ,,A"), Some(strings(&["a", "b", "a"])));
        assert_eq!(
            unique_strings(strings(&["a", "b", "a"])),
            strings(&["a", "b"])
        );
        assert_eq!(unique_strings(Vec::new()), Vec::<String>::new());
        assert_eq!(without_string(None, "a"), None);
        assert_eq!(without_string(Some(Vec::new()), "a"), None);
        assert_eq!(without_string(Some(strings(&["a"])), "a"), None);
        assert_eq!(
            without_string(Some(strings(&["a", "b", "a"])), "a"),
            Some(strings(&["b"]))
        );
    }

    #[test]
    fn lower_casing_is_per_character() {
        assert_eq!(go_lower("ÀBC İ"), "àbc i");
        assert_eq!(go_lower("ΑΣ"), "ασ");
    }
}
