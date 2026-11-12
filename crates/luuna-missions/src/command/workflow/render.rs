//! Compact JSON helpers for the workflow payloads that
//! [`Envelope`](super::super::output::Envelope) has no field method for. Their
//! results are handed to `Envelope::raw`.
//!
//! - [`string_array`]: a Go `[]string` that is never `nil`, so an empty one
//!   is `[]`.
//! - [`sorted_int_map`]: a Go `map[string]int`, whose keys the encoder sorts
//!   bytewise. A map keyed by integers (`map[int]int`) is sorted by the
//!   keys' decimal text, so the caller passes those texts as the keys.
//!
//! Strings are quoted with `json::quote` and HTML escaping off, as the
//! encoder of `Context.JSON` writes them.

use std::collections::BTreeMap;

use crate::json;

/// `items` as a compact JSON array of strings.
#[allow(
    dead_code,
    reason = "used by stats and completions of s4-missions-lib-2, which follow"
)]
pub(super) fn string_array(items: &[String]) -> Vec<u8> {
    let mut out = vec![b'['];
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.push(b',');
        }
        json::quote(&mut out, item, false);
    }
    out.push(b']');
    out
}

/// `counts` as a compact JSON object with its keys in bytewise order (a
/// `BTreeMap<String, _>` iterates in that order).
#[allow(
    dead_code,
    reason = "used by stats of s4-missions-lib-2, which follows"
)]
pub(super) fn sorted_int_map(counts: &BTreeMap<String, i64>) -> Vec<u8> {
    let mut out = vec![b'{'];
    for (index, (key, count)) in counts.iter().enumerate() {
        if index > 0 {
            out.push(b',');
        }
        json::quote(&mut out, key, false);
        out.push(b':');
        out.extend_from_slice(count.to_string().as_bytes());
    }
    out.push(b'}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::output::Envelope;

    #[test]
    fn string_arrays_and_sorted_maps_print_as_go() {
        assert_eq!(string_array(&[]), b"[]");
        assert_eq!(
            string_array(&["a".to_owned(), "<b> & \"c\"\n\u{2028}".to_owned()]),
            "[\"a\",\"<b> & \\\"c\\\"\\n\\u2028\"]".as_bytes()
        );

        let mut counts = BTreeMap::new();
        assert_eq!(sorted_int_map(&counts), b"{}");
        // Go sorts these keys as strings: "10" before "2", "B" before "a".
        for (key, count) in [("2", 1), ("10", 3), ("a", 0), ("B", -2), ("", 7)] {
            counts.insert(key.to_owned(), count);
        }
        assert_eq!(
            sorted_int_map(&counts),
            br#"{"":7,"10":3,"2":1,"B":-2,"a":0}"#
        );

        let rendered = Envelope::new(true, "stats")
            .raw("labels", &string_array(&[]))
            .raw("byPriority", &sorted_int_map(&counts))
            .render();
        assert_eq!(
            String::from_utf8(rendered).unwrap(),
            "{\n  \"success\": true,\n  \"command\": \"stats\",\n  \"labels\": [],\n  \"byPriority\": {\n    \"\": 7,\n    \"10\": 3,\n    \"2\": 1,\n    \"B\": -2,\n    \"a\": 0\n  }\n}\n"
        );
    }
}
