//! Ticket records as stored in `.luuna/missions/issues.jsonl`.
//!
//! Go calls these records issues; this API calls them tickets (D10). The
//! stored format is unchanged. A ticket read from the store keeps the raw
//! JSON of every member, its key order and every unknown member, exactly as
//! Go's `recordState` does (`internal/model/json.go`): when the ticket is
//! written again, unknown members and known members whose value did not
//! change are written with their original bytes (compacted), changed members
//! get their new value in their original position, and members that are new
//! are appended in Go's struct order.
//!
//! Known members are decoded one by one, as Go's `decodeKnownObject` does: a
//! member of the wrong type leaves its field at the zero value instead of
//! rejecting the record.

use std::collections::{BTreeMap, HashSet};

use crate::error::{Error, Result};
use crate::json::{self, Members, UnsupportedValue};

pub use crate::json::Value;

/// The JSON keys of the fields Go's `Issue` type declares, in struct order.
const KNOWN_KEYS: [&str; 21] = [
    "id",
    "title",
    "status",
    "type",
    "priority",
    "createdAt",
    "updatedAt",
    "assignee",
    "description",
    "closeReason",
    "blocks",
    "blockedBy",
    "labels",
    "convoy",
    "plan_id",
    "plan_step_index",
    "requires_plan",
    "discoveredFrom",
    "intent",
    "extensions",
    "closedAt",
];

/// One ticket record (Go's `model.Issue`).
///
/// The fields mirror Go's. A string field that is empty, a list that is
/// `None` or empty, `plan_step_index` that is `None`, `requires_plan` that is
/// false, `intent` that is `None` and `extensions` that is `None` or empty
/// are left out when the ticket is written, as Go's `omitempty` leaves them
/// out; a list that is `Some` and empty is written as `[]`, after every other
/// member, as Go writes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ticket {
    /// `id`, for example `project-1a2b`.
    pub id: String,
    /// `title`.
    pub title: String,
    /// `status`: `open`, `in_progress` or `closed`.
    pub status: String,
    /// `type`: `task`, `bug`, `feature` or `epic`.
    pub ticket_type: String,
    /// `priority`, 0 (critical) to 4 (backlog).
    pub priority: i64,
    /// `createdAt`.
    pub created_at: String,
    /// `updatedAt`.
    pub updated_at: String,
    /// `assignee`.
    pub assignee: String,
    /// `description`.
    pub description: String,
    /// `closeReason`.
    pub close_reason: String,
    /// `blocks`.
    pub blocks: Option<Vec<String>>,
    /// `blockedBy`.
    pub blocked_by: Option<Vec<String>>,
    /// `labels`.
    pub labels: Option<Vec<String>>,
    /// `convoy`.
    pub convoy: String,
    /// `plan_id`.
    pub plan_id: String,
    /// `plan_step_index`; `Some(0)` is written, `None` is not.
    pub plan_step_index: Option<i64>,
    /// `requires_plan`.
    pub requires_plan: bool,
    /// `discoveredFrom`.
    pub discovered_from: String,
    /// `intent`, any JSON value except `null`.
    pub intent: Option<Value>,
    /// `extensions`, a JSON object.
    pub extensions: Option<BTreeMap<String, Value>>,
    /// `closedAt`.
    pub closed_at: String,
    /// The record as it was read; `None` for a ticket created here.
    state: Option<RecordState>,
}

/// The raw members of a record as read, and the members Go's own encoding of
/// the decoded fields gave at that moment (`recordState`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordState {
    original: Members,
    baseline: Members,
}

impl Ticket {
    /// The ticket id, for example `project-1a2b`.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Decodes one record line as Go's `json.Unmarshal` into `model.Issue`
    /// does. `None` means Go rejects the line, and its store reader skips it:
    /// the line is not exactly one JSON value, the value is not an object,
    /// or a decoded member holds a number Go cannot encode again.
    pub fn from_json(line: &[u8]) -> Option<Self> {
        if !json::is_valid(line) {
            return None;
        }
        let original = json::parse_object(line).ok()?;
        let mut ticket = Self::default();
        for key in KNOWN_KEYS {
            if let Some(raw) = original.get(key) {
                ticket.decode_member(key, raw);
            }
        }
        let baseline = json::parse_object(&ticket.marshal_wire().ok()?).ok()?;
        ticket.state = Some(RecordState { original, baseline });
        Some(ticket)
    }

    /// The JSON object Go's `Issue.MarshalJSON` returns: the record merged
    /// with what was read. Strings in members Go encodes itself are HTML
    /// escaped; raw members keep their bytes.
    pub fn marshal_json(&self) -> Result<Vec<u8>> {
        let current = self.marshal_wire().map_err(encode_error)?;
        match &self.state {
            None => Ok(current),
            Some(state) => state.merge(&current),
        }
    }

    /// The record line Go's store writes (`json.Marshal`), without the
    /// newline. Literal `<`, `>`, `&`, U+2028 and U+2029 are escaped in every
    /// string, including raw members kept from the file (SD17).
    pub fn to_json_line(&self) -> Result<Vec<u8>> {
        let merged = self.marshal_json()?;
        json::reformat(&merged, json::Format::MARSHAL).map_err(|error| Error::Encode {
            message: format!("json: error calling MarshalJSON: {error}"),
        })
    }

    /// Decodes one known member as Go's `json.Unmarshal` into the field
    /// does, ignoring a type mismatch.
    fn decode_member(&mut self, key: &str, raw: &[u8]) {
        match key {
            "id" => self.id = decode_string(raw),
            "title" => self.title = decode_string(raw),
            "status" => self.status = decode_string(raw),
            "type" => self.ticket_type = decode_string(raw),
            "priority" => self.priority = decode_int(raw).unwrap_or(0),
            "createdAt" => self.created_at = decode_string(raw),
            "updatedAt" => self.updated_at = decode_string(raw),
            "assignee" => self.assignee = decode_string(raw),
            "description" => self.description = decode_string(raw),
            "closeReason" => self.close_reason = decode_string(raw),
            "blocks" => self.blocks = decode_strings(raw),
            "blockedBy" => self.blocked_by = decode_strings(raw),
            "labels" => self.labels = decode_strings(raw),
            "convoy" => self.convoy = decode_string(raw),
            "plan_id" => self.plan_id = decode_string(raw),
            // Go allocates the pointer for any non-null value before the
            // type check, so a mismatch leaves Some(0).
            "plan_step_index" => {
                self.plan_step_index = (raw != b"null").then(|| decode_int(raw).unwrap_or(0));
            }
            "requires_plan" => self.requires_plan = raw == b"true",
            "discoveredFrom" => self.discovered_from = decode_string(raw),
            "intent" => {
                self.intent = match Value::decode(raw) {
                    Ok(Value::Null) | Err(_) => None,
                    Ok(value) => Some(value),
                };
            }
            "extensions" => {
                self.extensions = Value::decode(raw).ok().and_then(Value::into_object);
            }
            "closedAt" => self.closed_at = decode_string(raw),
            _ => {}
        }
    }

    /// Go's `marshalIssueWire`: the fields in struct order with `omitempty`,
    /// then the empty lists that are present.
    fn marshal_wire(&self) -> std::result::Result<Vec<u8>, UnsupportedValue> {
        let mut out = Vec::with_capacity(256);
        let mut first = true;
        let mut member = |out: &mut Vec<u8>, key: &str| {
            if !first {
                out.push(b',');
            }
            first = false;
            json::quote(out, key, true);
            out.push(b':');
        };
        let string = |out: &mut Vec<u8>, text: &str| json::quote(out, text, true);
        out.push(b'{');
        for (key, value) in [
            ("id", &self.id),
            ("title", &self.title),
            ("status", &self.status),
            ("type", &self.ticket_type),
        ] {
            member(&mut out, key);
            string(&mut out, value);
        }
        member(&mut out, "priority");
        out.extend_from_slice(self.priority.to_string().as_bytes());
        for (key, value) in [
            ("createdAt", &self.created_at),
            ("updatedAt", &self.updated_at),
        ] {
            member(&mut out, key);
            string(&mut out, value);
        }
        for (key, value) in [
            ("assignee", &self.assignee),
            ("description", &self.description),
            ("closeReason", &self.close_reason),
        ] {
            if !value.is_empty() {
                member(&mut out, key);
                string(&mut out, value);
            }
        }
        for (key, list) in [
            ("blocks", &self.blocks),
            ("blockedBy", &self.blocked_by),
            ("labels", &self.labels),
        ] {
            if let Some(list) = list.as_ref().filter(|list| !list.is_empty()) {
                member(&mut out, key);
                out.push(b'[');
                for (index, item) in list.iter().enumerate() {
                    if index > 0 {
                        out.push(b',');
                    }
                    string(&mut out, item);
                }
                out.push(b']');
            }
        }
        for (key, value) in [("convoy", &self.convoy), ("plan_id", &self.plan_id)] {
            if !value.is_empty() {
                member(&mut out, key);
                string(&mut out, value);
            }
        }
        if let Some(index) = self.plan_step_index {
            member(&mut out, "plan_step_index");
            out.extend_from_slice(index.to_string().as_bytes());
        }
        if self.requires_plan {
            member(&mut out, "requires_plan");
            out.extend_from_slice(b"true");
        }
        if !self.discovered_from.is_empty() {
            member(&mut out, "discoveredFrom");
            string(&mut out, &self.discovered_from);
        }
        if let Some(intent) = self.intent.as_ref().filter(|value| **value != Value::Null) {
            member(&mut out, "intent");
            intent.encode(&mut out, true)?;
        }
        if let Some(extensions) = self.extensions.as_ref().filter(|map| !map.is_empty()) {
            member(&mut out, "extensions");
            json::encode_map(&mut out, extensions, true)?;
        }
        if !self.closed_at.is_empty() {
            member(&mut out, "closedAt");
            string(&mut out, &self.closed_at);
        }
        for (key, list) in [
            ("blocks", &self.blocks),
            ("blockedBy", &self.blocked_by),
            ("labels", &self.labels),
        ] {
            if list.as_ref().is_some_and(Vec::is_empty) {
                member(&mut out, key);
                out.extend_from_slice(b"[]");
            }
        }
        out.push(b'}');
        Ok(out)
    }
}

impl RecordState {
    /// Go's `recordState.merge`: members in the original order, unknown and
    /// unchanged members with their original bytes (compacted), changed
    /// members with their new value, removed members left out, and members
    /// that are new appended in struct order.
    fn merge(&self, current: &[u8]) -> Result<Vec<u8>> {
        let values = json::parse_object(current).map_err(|error| Error::Encode {
            message: format!("json: invalid record encoding: {error}"),
        })?;
        let mut out = Vec::with_capacity(current.len() + 64);
        let mut written: HashSet<String> = HashSet::new();
        out.push(b'{');
        for key in self.original.keys() {
            let original = self.original.get(key).unwrap_or_default();
            if !KNOWN_KEYS.contains(&key.as_str()) {
                append_member(&mut out, &mut written, key, original);
                continue;
            }
            match (values.get(key), self.baseline.get(key)) {
                (None, None) => append_member(&mut out, &mut written, key, original),
                (Some(value), Some(baseline)) if value == baseline => {
                    append_member(&mut out, &mut written, key, original);
                }
                (Some(value), _) => append_member(&mut out, &mut written, key, value),
                (None, Some(_)) => {}
            }
        }
        for key in values.keys() {
            let value = values.get(key).unwrap_or_default();
            append_member(&mut out, &mut written, key, value);
        }
        out.push(b'}');
        Ok(out)
    }
}

/// Go's `appendField` inside `merge`: each key once, its value compacted.
fn append_member(out: &mut Vec<u8>, written: &mut HashSet<String>, key: &str, value: &[u8]) {
    if written.contains(key) || value.is_empty() {
        return;
    }
    if !written.is_empty() {
        out.push(b',');
    }
    out.extend_from_slice(&json::quoted(key));
    out.push(b':');
    match json::compact(value) {
        Ok(compacted) => out.extend_from_slice(&compacted),
        Err(_) => out.extend_from_slice(value),
    }
    written.insert(key.to_owned());
}

fn encode_error(error: UnsupportedValue) -> Error {
    Error::Encode {
        message: error.to_string(),
    }
}

/// A string member; anything but a JSON string leaves the zero value.
fn decode_string(raw: &[u8]) -> String {
    if raw.first() == Some(&b'"') {
        json::unquote(raw)
    } else {
        String::new()
    }
}

/// An integer member as Go decodes it into `int`: a JSON number that is an
/// integer within 64 bits; anything else (`2.0`, `1e2`, a string) is `None`.
fn decode_int(raw: &[u8]) -> Option<i64> {
    std::str::from_utf8(raw).ok()?.parse::<i64>().ok()
}

/// A `[]string` member: `null` or a non-array gives `None`; an array gives
/// its strings, with the zero value `""` for an element of another type.
fn decode_strings(raw: &[u8]) -> Option<Vec<String>> {
    let items = Value::decode(raw).ok().and_then(Value::into_array)?;
    Some(
        items
            .into_iter()
            .map(|item| item.into_string().unwrap_or_default())
            .collect(),
    )
}

/// The current time as Go's `model.NowISO` writes it: UTC with exactly three
/// fractional digits and a literal `Z`, for example
/// `2026-01-02T03:04:05.006Z`.
pub fn now_iso() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// A new id as Go's `model.GenerateID` makes it: `<prefix>-` and 4 random
/// lowercase hex digits, retried while the id is in `existing`, and after 100
/// collisions 8 hex digits until one is free.
///
/// `existing` holds only the ids in the local file, so creates on two
/// branches merged with `merge=union` can produce the same id (SD15).
pub fn generate_id(prefix: &str, existing: &HashSet<String>) -> String {
    generate_id_with(prefix, existing, &mut random_bytes)
}

fn generate_id_with(
    prefix: &str,
    existing: &HashSet<String>,
    fill: &mut dyn FnMut(&mut [u8]),
) -> String {
    let mut make = |digits: usize| {
        let mut buffer = vec![0; digits.div_ceil(2)];
        fill(&mut buffer);
        format!("{prefix}-{}", &hex(&buffer)[..digits])
    };
    for _ in 0..100 {
        let id = make(4);
        if !existing.contains(&id) {
            return id;
        }
    }
    loop {
        let id = make(8);
        if !existing.contains(&id) {
            return id;
        }
    }
}

/// Lowercase hex, as Go's `hex.EncodeToString`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0xf)]));
    }
    out
}

/// Fills `buffer` with random bytes from the operating system's generator
/// (through version 4 UUIDs; their six fixed version and variant bits are
/// skipped).
pub(crate) fn random_bytes(buffer: &mut [u8]) {
    let mut filled = 0;
    while filled < buffer.len() {
        let uuid = uuid::Uuid::new_v4();
        for (index, byte) in uuid.as_bytes().iter().enumerate() {
            if index == 6 || index == 8 {
                continue;
            }
            if filled == buffer.len() {
                break;
            }
            buffer[filled] = *byte;
            filled += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(ticket: &Ticket) -> String {
        String::from_utf8(ticket.to_json_line().unwrap()).unwrap()
    }

    fn read(line: &str) -> Ticket {
        Ticket::from_json(line.as_bytes()).unwrap_or_else(|| panic!("rejected: {line}"))
    }

    const PLAIN: &str = r#"{"id":"p-0001","title":"Plain","status":"open","type":"task","priority":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#;

    #[test]
    fn a_new_ticket_is_written_in_go_struct_order() {
        let ticket = Ticket {
            id: "p-0001".into(),
            title: "<x>&\u{2028}".into(),
            status: "open".into(),
            ticket_type: "task".into(),
            priority: 2,
            created_at: "c".into(),
            updated_at: "u".into(),
            labels: Some(Vec::new()),
            blocks: Some(vec!["p-0002".into()]),
            plan_step_index: Some(0),
            extensions: Some(BTreeMap::from([
                (
                    "keep".into(),
                    Value::decode(b"{\"n\":[1,2.5,null]}").unwrap(),
                ),
                ("html".into(), Value::String("<&>".into())),
            ])),
            ..Ticket::default()
        };
        assert_eq!(
            line(&ticket),
            r#"{"id":"p-0001","title":"\u003cx\u003e\u0026\u2028","status":"open","type":"task","priority":2,"createdAt":"c","updatedAt":"u","blocks":["p-0002"],"plan_step_index":0,"extensions":{"html":"\u003c\u0026\u003e","keep":{"n":[1,2.5,null]}},"labels":[]}"#
        );
    }

    #[test]
    fn an_unchanged_record_is_written_with_its_original_bytes() {
        let raw = r#"{"title":"Escaped \u003cb\u003e \u00fc","id":"p-r002","zzz":{"b":1,"a":[true,null,1.50,1e2,"\u003c"]},"status":"in_progress","type":"bug","priority":0,"createdAt":"c","updatedAt":"u","assignee":null,"labels":[],"extensions":{"k":"v","n":{"x":[]}}}"#;
        assert_eq!(line(&read(raw)), raw);
        assert_eq!(line(&read(PLAIN)), PLAIN);
    }

    #[test]
    fn unknown_keys_null_values_and_key_order_survive_a_change() {
        let raw = r#"{"x_first":{"keep":[1,null,"x"]},"id":"p-u001","priority":2,"title":"Old","assignee":null,"status":"open","type":"task","createdAt":"c","updatedAt":"u","x_last":null}"#;
        let mut ticket = read(raw);
        ticket.title = "New".into();
        ticket.priority = 3;
        assert_eq!(
            line(&ticket),
            r#"{"x_first":{"keep":[1,null,"x"]},"id":"p-u001","priority":3,"title":"New","assignee":null,"status":"open","type":"task","createdAt":"c","updatedAt":"u","x_last":null}"#
        );
    }

    #[test]
    fn a_present_optional_field_can_be_removed_and_new_fields_are_appended() {
        let raw = r#"{"id":"p-1","description":"before","title":"T","status":"open","type":"task","priority":2,"createdAt":"c","updatedAt":"u"}"#;
        let mut ticket = read(raw);
        ticket.description.clear();
        ticket.closed_at = "z".into();
        ticket.assignee = "me".into();
        assert_eq!(
            line(&ticket),
            r#"{"id":"p-1","title":"T","status":"open","type":"task","priority":2,"createdAt":"c","updatedAt":"u","assignee":"me","closedAt":"z"}"#
        );
    }

    #[test]
    fn empty_lists_stay_distinct_from_absent_ones() {
        let raw = r#"{"id":"p-1","title":"T","status":"open","type":"task","priority":2,"createdAt":"c","updatedAt":"u","blockedBy":[],"blocks":[]}"#;
        let ticket = read(raw);
        assert_eq!(ticket.blocks, Some(Vec::new()));
        assert_eq!(ticket.blocked_by, Some(Vec::new()));
        assert_eq!(ticket.labels, None);
        assert_eq!(line(&ticket), raw);
        let mut changed = ticket.clone();
        changed.labels = Some(Vec::new());
        changed.blocks = None;
        assert_eq!(
            line(&changed),
            r#"{"id":"p-1","title":"T","status":"open","type":"task","priority":2,"createdAt":"c","updatedAt":"u","blockedBy":[],"labels":[]}"#
        );
        let mut fresh = Ticket {
            id: "p-2".into(),
            ..Ticket::default()
        };
        assert!(!line(&fresh).contains("labels"));
        fresh.labels = Some(Vec::new());
        assert!(line(&fresh).ends_with(r#","labels":[]}"#));
    }

    #[test]
    fn written_lines_escape_html_and_js_characters_as_go_marshal() {
        // Literal characters in an unchanged record and in unknown members
        // are escaped too when the record is written (SD17).
        let raw = "{\"id\":\"p-1\",\"title\":\"Literal <i> & \u{fc} \u{2028}\",\"x\":\"<&>\u{2029}\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"c\",\"updatedAt\":\"u\"}";
        let ticket = read(raw);
        assert_eq!(ticket.title, "Literal <i> & \u{fc} \u{2028}");
        assert_eq!(
            line(&ticket),
            "{\"id\":\"p-1\",\"title\":\"Literal \\u003ci\\u003e \\u0026 \u{fc} \\u2028\",\"x\":\"\\u003c\\u0026\\u003e\\u2029\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"c\",\"updatedAt\":\"u\"}"
        );
        // MarshalJSON keeps them literal; only json.Marshal escapes.
        assert_eq!(ticket.marshal_json().unwrap(), raw.as_bytes());
    }

    #[test]
    fn missing_known_fields_are_appended_and_whitespace_is_compacted() {
        let ticket = read(r#"  {"id":"r3","x":"v" , "y" : [ 1 , 2 ] }  "#);
        assert_eq!(
            line(&ticket),
            r#"{"id":"r3","x":"v","y":[1,2],"title":"","status":"","type":"","priority":0,"createdAt":"","updatedAt":""}"#
        );
    }

    #[test]
    fn members_of_the_wrong_type_decode_to_zero_values() {
        let raw = r#"{"id":"r4","priority":1e2,"plan_step_index":1.5,"labels":["a",1,null],"requires_plan":"yes","extensions":[1],"intent":null,"title":5,"blocks":"x"}"#;
        let ticket = read(raw);
        assert_eq!(ticket.priority, 0);
        assert_eq!(ticket.plan_step_index, Some(0));
        assert_eq!(
            ticket.labels,
            Some(vec!["a".into(), String::new(), String::new()])
        );
        assert!(!ticket.requires_plan);
        assert_eq!(ticket.extensions, None);
        assert_eq!(ticket.intent, None);
        assert_eq!(ticket.title, "");
        assert_eq!(ticket.blocks, None);
        // Unchanged, so every odd value is written back as it was.
        assert!(line(&ticket).starts_with(r#"{"id":"r4","priority":1e2,"plan_step_index":1.5,"labels":["a",1,null],"requires_plan":"yes","extensions":[1],"intent":null,"title":5,"blocks":"x","#));
    }

    #[test]
    fn repeated_keys_keep_the_first_position_and_the_last_value() {
        let ticket = read(r#"{"id":"r6","title":"first","x":1,"x":2,"title":"again"}"#);
        assert_eq!(ticket.title, "again");
        assert!(line(&ticket).starts_with(r#"{"id":"r6","title":"again","x":2,"#));
    }

    #[test]
    fn lines_go_rejects_are_rejected() {
        for bad in [
            "[1]",
            "null",
            "\"x\"",
            "{not json",
            "{\"id\":\"r9\"} x",
            "{\"id\":\"r5\",\"intent\":{\"a\":[1e400]}}",
            "{\"id\":\"r5\",\"extensions\":{\"a\":-1e400}}",
        ] {
            assert!(Ticket::from_json(bad.as_bytes()).is_none(), "{bad}");
        }
        assert_eq!(read("{}").id(), "");
    }

    #[test]
    fn invalid_utf8_is_kept_in_raw_members_and_replaced_in_decoded_ones() {
        let raw = b"{\"id\":\"r7\",\"title\":\"a\xffb\",\"u\":\"\xfe\",\"status\":\"open\",\"type\":\"task\",\"priority\":2,\"createdAt\":\"c\",\"updatedAt\":\"u\"}";
        let ticket = Ticket::from_json(raw).unwrap();
        assert_eq!(ticket.title, "a\u{fffd}b");
        assert_eq!(ticket.to_json_line().unwrap(), raw.to_vec());
    }

    #[test]
    fn timestamps_have_millisecond_precision_and_a_z() {
        let now = now_iso();
        let bytes = now.as_bytes();
        assert_eq!(bytes.len(), 24, "{now}");
        for (index, byte) in bytes.iter().enumerate() {
            match index {
                4 | 7 => assert_eq!(*byte, b'-', "{now}"),
                10 => assert_eq!(*byte, b'T', "{now}"),
                13 | 16 => assert_eq!(*byte, b':', "{now}"),
                19 => assert_eq!(*byte, b'.', "{now}"),
                23 => assert_eq!(*byte, b'Z', "{now}"),
                _ => assert!(byte.is_ascii_digit(), "{now}"),
            }
        }
    }

    #[test]
    fn ids_are_prefix_and_four_hex_digits() {
        let id = generate_id("project", &HashSet::new());
        let (prefix, digits) = id.split_once('-').unwrap();
        assert_eq!(prefix, "project");
        assert_eq!(digits.len(), 4);
        assert!(digits
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')));
    }

    #[test]
    fn ids_fall_back_to_eight_hex_digits_after_100_collisions() {
        let existing = HashSet::from(["p-abcd".to_owned()]);
        let mut calls = 0;
        let id = generate_id_with("p", &existing, &mut |buffer: &mut [u8]| {
            calls += 1;
            for (index, byte) in buffer.iter_mut().enumerate() {
                *byte = [0xab, 0xcd, 0x01, 0x23][index];
            }
        });
        assert_eq!(id, "p-abcd0123");
        assert_eq!(calls, 101);
        let mut calls = 0;
        let id = generate_id_with("p", &existing, &mut |buffer: &mut [u8]| {
            calls += 1;
            let first = if calls < 50 { 0xab } else { 0x12 };
            buffer[0] = first;
            buffer[1] = 0xcd;
        });
        assert_eq!(id, "p-12cd");
        assert_eq!(calls, 50);
    }

    #[test]
    fn a_record_as_deep_as_decode_accepts_needs_no_deep_stack() {
        // The record object holds `intent` and `extensions` -> `x`, so these
        // members reach MAX_DEPTH (10000), the deepest nesting Go accepts.
        let deep = |depth: usize| "[".repeat(depth) + "1" + &"]".repeat(depth);
        let record = move |title: &str| {
            format!(
                r#"{{"id":"p-d001","title":"{title}","status":"open","type":"task","priority":2,"createdAt":"c","updatedAt":"u","intent":{},"extensions":{{"x":{}}}}}"#,
                deep(9_999),
                deep(9_998)
            )
        };
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || {
                let ticket = Ticket::from_json(record("Old").as_bytes()).expect("Go keeps it");
                let copy = ticket.clone();
                // `assert!` rather than `assert_eq!`: the derived `Debug` of
                // a failure message would recurse.
                assert!(copy == ticket);
                let mut changed = copy.clone();
                changed.title = "New".into();
                assert!(changed != ticket);
                let written = changed.to_json_line().unwrap();
                assert!(written == record("New").into_bytes());
                assert!(Ticket::from_json(&written).is_some_and(|read| read.title == "New"));
                drop(changed);
                drop(copy);
                drop(ticket);
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
