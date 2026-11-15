//! The plan index the read and workflow commands consult (Go's
//! `loadPlanIndex`, `internal/cli/issue_read.go:87-127`), owned by
//! s4-missions-lib-2.
//!
//! [`load`] reads `plans.jsonl` as Go's `store.ReadPlans` does
//! (`internal/store/store.go:116-143`): a missing file is an empty index;
//! the content is split on `\n`, each line trimmed as `bytes.TrimSpace`
//! trims it, and a blank line skipped. A line is skipped exactly when Go's
//! Plan decoder rejects its syntax/object shape or cannot marshal its known
//! baseline values. Acceptance is shared with the full model introduced by
//! s4-missions-lib-4; unknown root values stay raw. Index fields retain Go's
//! independently ignored field type errors. Repeated ids keep the last record
//! at the first one's position.
//!
//! The index holds only what the commands of this plan and s4-missions-lib-4
//! read ([`IndexedPlan`]); s4-missions-lib-4 may replace it by its full plan
//! model.

use std::collections::{BTreeMap, HashMap};
use std::io;

use crate::error::{Error, Result};
use crate::json::{self, Value};
use crate::model::Ticket;
use crate::store::{trim_space, DataFile, Store};

/// The plans of `plans.jsonl`, by plan id and by seed ticket.
#[allow(
    dead_code,
    reason = "read by the commands of s4-missions-lib-2 and by s4-missions-lib-3's prime"
)]
pub(crate) struct PlanIndex {
    /// The plans in file order, one per id (Go's `ReadPlans` result).
    plans: Vec<IndexedPlan>,
    /// The position of each plan, by plan id, the empty id included (Go's
    /// `byID`).
    by_id: HashMap<String, usize>,
    /// The position of the latest plan seeded by each ticket, the empty
    /// seed included (Go's `bySeed`).
    by_seed: HashMap<String, usize>,
}

/// The fields of a `model.Plan` record the index uses, decoded as Go's
/// `json.Unmarshal` into the field leaves them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "read by ready and blocked of s4-missions-lib-2 and by s4-missions-lib-4"
)]
pub(crate) struct IndexedPlan {
    /// `id`: "" unless the raw value is a JSON string.
    id: String,
    /// `seed`: "" unless the raw value is a JSON string.
    seed: String,
    /// `status`: "" unless the raw value is a JSON string.
    status: String,
    /// `children`: `None` for `null`, a value that is not an array or an
    /// absent key; otherwise the elements, "" for each that is not a string.
    children: Option<Vec<String>>,
    /// `updatedAt`: "" unless the raw value is a JSON string.
    updated_at: String,
}

impl IndexedPlan {
    /// Decodes one trimmed, non-empty line as Go's `json.Unmarshal` into
    /// `model.Plan` does; `None` where Go rejects the line and its store
    /// reader skips it.
    fn from_line(line: &[u8]) -> Option<Self> {
        if !super::plans::accepts_record(line) {
            return None;
        }
        let members = json::parse_object(line).ok()?;
        let string = |key: &str| members.get(key).map(decode_string).unwrap_or_default();
        Some(Self {
            id: string("id"),
            seed: string("seed"),
            status: string("status"),
            children: members.get("children").and_then(decode_strings),
            updated_at: string("updatedAt"),
        })
    }

    /// The plan id.
    #[allow(
        dead_code,
        reason = "called by ready and blocked of s4-missions-lib-2 and by s4-missions-lib-4"
    )]
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    /// The plan status ("" when the record's value is not a string).
    #[allow(
        dead_code,
        reason = "called by ready and blocked of s4-missions-lib-2 and by s4-missions-lib-4"
    )]
    pub(crate) fn status(&self) -> &str {
        &self.status
    }

    /// The plan's children (Go's `nil` is `None`).
    #[allow(
        dead_code,
        reason = "called by ready and blocked of s4-missions-lib-2 and by s4-missions-lib-4"
    )]
    pub(crate) fn children(&self) -> Option<&[String]> {
        self.children.as_deref()
    }
}

/// A string member as Go decodes it into `string`: anything but a JSON
/// string leaves "".
fn decode_string(raw: &[u8]) -> String {
    if raw.first() == Some(&b'"') {
        json::unquote(raw)
    } else {
        String::new()
    }
}

/// A `[]string` member: `null` or a non-array gives `None`; an array gives
/// its strings, with "" for an element of another type.
fn decode_strings(raw: &[u8]) -> Option<Vec<String>> {
    let items = Value::decode(raw).ok().and_then(Value::into_array)?;
    Some(
        items
            .into_iter()
            .map(|item| item.into_string().unwrap_or_default())
            .collect(),
    )
}

/// Go's `loadPlanIndex` for the store.
#[allow(
    dead_code,
    reason = "called by the commands of s4-missions-lib-2 and by s4-missions-lib-3's prime"
)]
pub(crate) fn load(store: &Store) -> Result<PlanIndex> {
    let path = store.path(DataFile::Plans);
    let content = match std::fs::read(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(Error::io("read", path, error)),
    };
    // Go's `readRecords`: the last record of an id at the first one's
    // position.
    let mut plans: Vec<IndexedPlan> = Vec::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();
    for line in content.split(|byte| *byte == b'\n') {
        let line = trim_space(line);
        if line.is_empty() {
            continue;
        }
        let Some(plan) = IndexedPlan::from_line(line) else {
            continue;
        };
        match by_id.get(&plan.id) {
            Some(&position) => plans[position] = plan,
            None => {
                by_id.insert(plan.id.clone(), plans.len());
                plans.push(plan);
            }
        }
    }
    // `bySeed`: a later plan replaces an earlier one only when its
    // `updatedAt` string is strictly greater.
    let mut by_seed: HashMap<String, usize> = HashMap::new();
    for (position, plan) in plans.iter().enumerate() {
        match by_seed.get(&plan.seed) {
            Some(&old) if plans[old].updated_at >= plan.updated_at => {}
            _ => {
                by_seed.insert(plan.seed.clone(), position);
            }
        }
    }
    Ok(PlanIndex {
        plans,
        by_id,
        by_seed,
    })
}

impl PlanIndex {
    /// The plan `ticket` names in its `plan_id`, if that plan exists (Go's
    /// `forIssue`): an empty `plan_id` names no plan.
    #[allow(
        dead_code,
        reason = "called by ready and blocked of s4-missions-lib-2 and by s4-missions-lib-4"
    )]
    pub(crate) fn plan_for_ticket(&self, ticket: &Ticket) -> Option<&IndexedPlan> {
        if ticket.plan_id.is_empty() {
            return None;
        }
        self.by_id
            .get(&ticket.plan_id)
            .map(|&position| &self.plans[position])
    }

    /// The latest plan seeded by the ticket `ticket_id` (Go's `bySeed`).
    #[allow(
        dead_code,
        reason = "called by ready of s4-missions-lib-2 and by s4-missions-lib-4"
    )]
    pub(crate) fn plan_for_seed(&self, ticket_id: &str) -> Option<&IndexedPlan> {
        self.by_seed
            .get(ticket_id)
            .map(|&position| &self.plans[position])
    }

    /// The status of the plan `ticket` names in its `plan_id`, if that plan
    /// exists (Go's `forIssue`).
    #[allow(
        dead_code,
        reason = "called by the commands of s4-missions-lib-2 and by s4-missions-lib-3's prime"
    )]
    pub(crate) fn status_for_ticket(&self, ticket: &Ticket) -> Option<&str> {
        self.plan_for_ticket(ticket).map(IndexedPlan::status)
    }

    /// The status of the latest plan seeded by the ticket `ticket_id`.
    #[allow(
        dead_code,
        reason = "called by the commands of s4-missions-lib-2 and by s4-missions-lib-3's prime"
    )]
    pub(crate) fn status_for_seed(&self, ticket_id: &str) -> Option<&str> {
        self.plan_for_seed(ticket_id).map(IndexedPlan::status)
    }

    /// The JSON object Go's `issueWithPlan.MarshalJSON` returns for the
    /// ticket annotated by `annotateIssue` (`issue_read.go:20-58`,
    /// :116-127): the ticket's own JSON when it names no known plan;
    /// otherwise `plan_status` (omitted when the plan's status is "") and
    /// `plan_children` (`[]` when the plan has none) appended before the
    /// closing brace, or, when the ticket's JSON already holds either key,
    /// the whole object re-encoded by `json.Marshal` of a
    /// `map[string]json.RawMessage` with those keys replaced (SD217).
    #[allow(
        dead_code,
        reason = "called by ready and blocked of s4-missions-lib-2 and by s4-missions-lib-4"
    )]
    pub(crate) fn annotated_json(&self, ticket: &Ticket) -> Result<Vec<u8>> {
        let issue = ticket.marshal_json()?;
        let Some(plan) = self.plan_for_ticket(ticket) else {
            return Ok(issue);
        };
        if issue.len() < 2 || issue[0] != b'{' || issue[issue.len() - 1] != b'}' {
            return Err(annotation_error("issue JSON not an object"));
        }
        let status = (!plan.status.is_empty()).then(|| json::quoted(&plan.status));
        let children = string_array(plan.children().unwrap_or_default());
        let existing = json::parse_object(&issue).map_err(annotation_error)?;
        if existing.get("plan_status").is_some() || existing.get("plan_children").is_some() {
            let mut members: BTreeMap<String, Vec<u8>> = existing
                .keys()
                .iter()
                .map(|key| (key.clone(), existing.get(key).unwrap_or_default().to_vec()))
                .collect();
            if let Some(status) = status {
                members.insert("plan_status".to_owned(), status);
            }
            members.insert("plan_children".to_owned(), children);
            let mut out = vec![b'{'];
            for (index, (key, raw)) in members.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                json::quote(&mut out, key, true);
                out.push(b':');
                let value = json::reformat(raw, json::Format::MARSHAL).map_err(annotation_error)?;
                out.extend_from_slice(&value);
            }
            out.push(b'}');
            return Ok(out);
        }
        let mut out = Vec::with_capacity(issue.len() + children.len() + 48);
        out.extend_from_slice(&issue[..issue.len() - 1]);
        if issue.len() > 2 {
            out.push(b',');
        }
        if let Some(status) = &status {
            out.extend_from_slice(b"\"plan_status\":");
            out.extend_from_slice(status);
            out.push(b',');
        }
        out.extend_from_slice(b"\"plan_children\":");
        out.extend_from_slice(&children);
        out.push(b'}');
        Ok(out)
    }

    /// Go's `planSuffix` (`issue_read.go:335-344`): the plan context a text
    /// line of ready, blocked, list and search ends with.
    #[allow(
        dead_code,
        reason = "called by ready and blocked of s4-missions-lib-2 and by s4-missions-lib-4"
    )]
    pub(crate) fn plan_suffix(&self, ticket: &Ticket) -> String {
        match self.plan_for_ticket(ticket) {
            None => String::new(),
            Some(plan) if plan.status == "draft" => {
                " [plan in draft \u{2014} run luuna-missions plan submit]".to_owned()
            }
            Some(plan) => format!(" [plan {}]", plan.status),
        }
    }
}

/// `strings` as `json.Marshal` writes a `[]string`.
fn string_array(strings: &[String]) -> Vec<u8> {
    let mut out = vec![b'['];
    for (index, text) in strings.iter().enumerate() {
        if index > 0 {
            out.push(b',');
        }
        json::quote(&mut out, text, true);
    }
    out.push(b']');
    out
}

/// An error of `issueWithPlan.MarshalJSON` as Go's encoder reports it. The
/// ticket's own JSON is always an object Go's decoder accepts, so this is
/// not reached by a record read from the store.
fn annotation_error(detail: impl std::fmt::Display) -> Error {
    Error::Encode {
        message: format!("json: error calling MarshalJSON for type cli.issueWithPlan: {detail}"),
    }
}

/// The expectations of `the_plan_index_answers_as_its_owner_states` in
/// `command/mod.rs`.
#[cfg(test)]
pub(super) mod checks {
    use std::fs;

    use super::load;
    use crate::model::Ticket;
    use crate::store::Store;

    /// Two plan records seeded by the same ticket, the later one with a
    /// child.
    const TWO_PLANS: &str = concat!(
        r#"{"id":"plan-a","seed":"p-1","template":"t","status":"draft","revision":1,"sections":{},"children":[],"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
        "\n",
        r#"{"id":"plan-b","seed":"p-1","template":"t","status":"approved","revision":2,"sections":{},"children":["p-2"],"createdAt":"2026-01-02T00:00:00.000Z","updatedAt":"2026-01-02T00:00:00.000Z"}"#,
        "\n",
    );

    /// A missing `plans.jsonl` and one holding only white space give an
    /// index that knows no plan; [`TWO_PLANS`] gives the statuses Go's
    /// `loadPlanIndex` gives, as literals.
    pub(in crate::command) fn plans_file() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".luuna/missions");
        fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir).unwrap();
        let mut planned = Ticket::default();
        planned.id = "p-2".to_owned();
        planned.plan_id = "plan-b".to_owned();
        let mut seed = Ticket::default();
        seed.id = "p-1".to_owned();

        // Missing, then white space only: always empty.
        for content in [None, Some(" \n\t\r\n\n")] {
            if let Some(content) = content {
                fs::write(dir.join("plans.jsonl"), content).unwrap();
            }
            let index = load(&store).unwrap();
            for ticket in [&planned, &seed] {
                assert_eq!(index.status_for_ticket(ticket), None, "{content:?}");
                assert_eq!(index.status_for_seed(ticket.id()), None, "{content:?}");
            }
        }

        // Go: byID["plan-b"] is plan-b; bySeed["p-1"] is plan-b, whose
        // updatedAt is greater; p-1 names no plan and seeds plan-b; p-2
        // seeds none.
        fs::write(dir.join("plans.jsonl"), TWO_PLANS).unwrap();
        let index = load(&store).unwrap();
        assert_eq!(index.status_for_ticket(&planned), Some("approved"));
        assert_eq!(index.status_for_ticket(&seed), None);
        assert_eq!(index.status_for_seed("p-1"), Some("approved"));
        assert_eq!(index.status_for_seed("p-2"), None);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{load, PlanIndex};
    use crate::model::Ticket;
    use crate::store::Store;

    /// The index of a store whose `plans.jsonl` holds `content` (`None`:
    /// no file).
    fn index_of(content: Option<&str>) -> PlanIndex {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".luuna/missions");
        fs::create_dir_all(&dir).unwrap();
        if let Some(content) = content {
            fs::write(dir.join("plans.jsonl"), content).unwrap();
        }
        load(&Store::open(&dir).unwrap()).unwrap()
    }

    /// A ticket whose `plan_id` is `plan_id`.
    fn naming(plan_id: &str) -> Ticket {
        let mut ticket = Ticket::default();
        ticket.id = "t-0".to_owned();
        ticket.plan_id = plan_id.to_owned();
        ticket
    }

    #[test]
    fn a_missing_or_blank_plans_file_is_an_empty_index() {
        // Go's TrimSpace also trims Unicode white space (U+0085, U+00A0).
        for content in [
            None,
            Some(""),
            Some(" \n\t\r\n\n"),
            Some("\u{85}\u{a0}\n\x0b\x0c"),
        ] {
            let index = index_of(content);
            assert!(index.plans.is_empty(), "{content:?}");
            assert!(
                index.plan_for_ticket(&naming("pl-1")).is_none(),
                "{content:?}"
            );
            assert!(index.plan_for_seed("").is_none(), "{content:?}");
            assert!(index.plan_for_seed("t-1").is_none(), "{content:?}");
        }
    }

    #[test]
    fn the_latest_plan_by_updated_at_seeds_a_ticket() {
        // pl-b is newer than pl-a; pl-c ties pl-b and pl-d's updatedAt is
        // "", so neither replaces it (Go's strict `<`).
        let index = index_of(Some(concat!(
            r#"{"id":"pl-a","seed":"t-1","status":"draft","updatedAt":"2026-01-02T00:00:00.000Z"}"#,
            "\n",
            r#"{"id":"pl-b","seed":"t-1","status":"approved","updatedAt":"2026-01-03T00:00:00.000Z"}"#,
            "\n",
            r#"{"id":"pl-c","seed":"t-1","status":"completed","updatedAt":"2026-01-03T00:00:00.000Z"}"#,
            "\n",
            r#"{"id":"pl-d","seed":"t-1","status":"draft"}"#,
            "\n",
        )));
        assert_eq!(
            index.plan_for_seed("t-1").map(|plan| plan.id()),
            Some("pl-b")
        );
        assert_eq!(index.status_for_seed("t-1"), Some("approved"));
        assert_eq!(index.status_for_seed("t-2"), None);

        // A repeated id keeps its last record at its first position, so on
        // a tie the re-recorded p-1 still comes first.
        let index = index_of(Some(concat!(
            r#"{"id":"p-1","seed":"s","status":"draft","updatedAt":"2026-01-05T00:00:00.000Z"}"#,
            "\n",
            r#"{"id":"p-2","seed":"s","status":"draft","updatedAt":"2026-01-05T00:00:00.000Z"}"#,
            "\n",
            r#"{"id":"p-1","seed":"s","status":"approved","updatedAt":"2026-01-05T00:00:00.000Z"}"#,
            "\n",
        )));
        assert_eq!(index.plans.len(), 2);
        let latest = index.plan_for_seed("s").unwrap();
        assert_eq!((latest.id(), latest.status()), ("p-1", "approved"));
        assert_eq!(index.status_for_ticket(&naming("p-1")), Some("approved"));
    }

    #[test]
    fn a_ticket_finds_its_plan_by_plan_id() {
        let index = index_of(Some(concat!(
            r#"{"id":"pl-1","seed":"t-1","status":"approved","children":["t-2","t-3"],"updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            "\n",
            r#"{"seed":"t-9","status":"draft","children":[]}"#,
            "\n",
        )));
        let plan = index.plan_for_ticket(&naming("pl-1")).unwrap();
        assert_eq!(plan.id(), "pl-1");
        assert_eq!(plan.status(), "approved");
        assert_eq!(
            plan.children(),
            Some(&["t-2".to_owned(), "t-3".to_owned()][..])
        );
        assert_eq!(index.status_for_ticket(&naming("pl-1")), Some("approved"));
        assert!(index.plan_for_ticket(&naming("pl-missing")).is_none());
        // An empty plan_id names no plan, although a plan with the empty id
        // is indexed (by id and by its seed).
        assert!(index.plan_for_ticket(&naming("")).is_none());
        assert_eq!(index.status_for_ticket(&naming("")), None);
        let unnamed = index.plan_for_seed("t-9").unwrap();
        assert_eq!(unnamed.id(), "");
        assert_eq!(unnamed.children(), Some(&[][..]));
        assert!(index.by_id.contains_key(""));
    }

    #[test]
    fn unreadable_plan_lines_are_skipped_and_wrong_typed_ones_kept() {
        let index = index_of(Some(concat!(
            r#"{"id":"pl-good","seed":"t-1","template":"","status":"approved","revision":1,"sections":{},"children":["t-2"],"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#,
            "\n",
            "not json",
            "\n",
            "[1,2]",
            "\n",
            r#"{"id":"pl-bad","seed":"t-1","status":5,"name":["x"],"children":"t-3","updatedAt":7}"#,
            "\n",
            r#"{"id":"pl-mixed","seed":5,"status":"draft","children":["t-4",5,null],"updatedAt":"2026-02-01T00:00:00.000Z"}"#,
            "\n",
        )));
        let good = index.plan_for_ticket(&naming("pl-good")).unwrap();
        assert_eq!(good.id(), "pl-good");
        assert_eq!(good.status(), "approved");
        assert_eq!(good.children(), Some(&["t-2".to_owned()][..]));
        // The wrong-typed line is kept (PROBE:25172-25175).
        let bad = index.plan_for_ticket(&naming("pl-bad")).unwrap();
        assert_eq!(bad.id(), "pl-bad");
        assert_eq!(bad.status(), "");
        assert_eq!(bad.children(), None);
        let mixed = index.plan_for_ticket(&naming("pl-mixed")).unwrap();
        assert_eq!(mixed.status(), "draft");
        assert_eq!(
            mixed.children(),
            Some(&["t-4".to_owned(), String::new(), String::new()][..])
        );
        assert!(index.plan_for_ticket(&naming("not json")).is_none());
        assert_eq!(index.plans.len(), 3);
        // pl-bad's updatedAt decodes to "", which is not greater.
        assert_eq!(
            index.plan_for_seed("t-1").map(|plan| plan.id()),
            Some("pl-good")
        );
        // pl-mixed's wrong-typed seed joins the empty seed.
        assert_eq!(
            index.plan_for_seed("").map(|plan| plan.id()),
            Some("pl-mixed")
        );
        assert_eq!(index.status_for_ticket(&naming("pl-bad")), Some(""));
        let annotated = index.annotated_json(&naming("pl-bad")).unwrap();
        let annotated = String::from_utf8(annotated).unwrap();
        assert!(!annotated.contains("plan_status"), "{annotated}");
        assert!(annotated.ends_with(r#""plan_children":[]}"#), "{annotated}");
    }

    #[test]
    fn annotations_append_or_replace_plan_keys() {
        let index = index_of(Some(concat!(
            r#"{"id":"pl-1","seed":"s","status":"approved","children":["t-2","a&b"]}"#,
            "\n",
            r#"{"id":"pl-2","seed":"s","status":5}"#,
            "\n",
        )));
        let ticket = |line: &str| Ticket::from_json(line.as_bytes()).unwrap();
        let annotated =
            |line: &str| String::from_utf8(index.annotated_json(&ticket(line)).unwrap()).unwrap();

        // No plan_id, or one naming no plan: the ticket's own JSON.
        let line = r#"{"id":"t-1","title":"A","status":"open","type":"task","priority":2,"createdAt":"c","updatedAt":"u"}"#;
        assert_eq!(annotated(line), line);
        let line = r#"{"id":"t-1","title":"A","status":"open","type":"task","priority":2,"createdAt":"c","updatedAt":"u","plan_id":"pl-9"}"#;
        assert_eq!(annotated(line), line);

        // Appended before the closing brace, children escaped as
        // json.Marshal escapes them.
        assert_eq!(
            annotated(
                r#"{"id":"t-1","title":"A","status":"open","type":"task","priority":2,"createdAt":"c","updatedAt":"u","plan_id":"pl-1"}"#
            ),
            r#"{"id":"t-1","title":"A","status":"open","type":"task","priority":2,"createdAt":"c","updatedAt":"u","plan_id":"pl-1","plan_status":"approved","plan_children":["t-2","a\u0026b"]}"#
        );
        // A plan whose status is "" adds plan_children only, [] for nil.
        assert_eq!(
            annotated(
                r#"{"id":"t-1","title":"A","status":"open","type":"task","priority":2,"createdAt":"c","updatedAt":"u","plan_id":"pl-2"}"#
            ),
            r#"{"id":"t-1","title":"A","status":"open","type":"task","priority":2,"createdAt":"c","updatedAt":"u","plan_id":"pl-2","plan_children":[]}"#
        );

        // A record already holding a plan key is re-encoded with its keys
        // sorted and raw values HTML-escaped (json.Marshal of
        // map[string]json.RawMessage), the plan keys replaced (SD217).
        assert_eq!(
            annotated(
                r#"{"id":"t-1","title":"a<b","status":"open","type":"task","priority":2,"createdAt":"c","updatedAt":"u","plan_id":"pl-1","zz":{"k":"x>y"},"plan_status":"old","plan_children":["old"]}"#
            ),
            r#"{"createdAt":"c","id":"t-1","plan_children":["t-2","a\u0026b"],"plan_id":"pl-1","plan_status":"approved","priority":2,"status":"open","title":"a\u003cb","type":"task","updatedAt":"u","zz":{"k":"x\u003ey"}}"#
        );
        // ... and a plan whose status is "" keeps the record's plan_status.
        assert_eq!(
            annotated(
                r#"{"id":"t-1","title":"A","status":"open","type":"task","priority":2,"createdAt":"c","updatedAt":"u","plan_id":"pl-2","plan_status":"old"}"#
            ),
            r#"{"createdAt":"c","id":"t-1","plan_children":[],"plan_id":"pl-2","plan_status":"old","priority":2,"status":"open","title":"A","type":"task","updatedAt":"u"}"#
        );
    }

    #[test]
    fn the_plan_suffix_follows_go() {
        let index = index_of(Some(concat!(
            r#"{"id":"pl-draft","status":"draft"}"#,
            "\n",
            r#"{"id":"pl-approved","status":"approved"}"#,
            "\n",
            r#"{"id":"pl-typed","status":5}"#,
            "\n",
        )));
        assert_eq!(
            index.plan_suffix(&naming("pl-draft")),
            " [plan in draft \u{2014} run luuna-missions plan submit]"
        );
        assert_eq!(
            index.plan_suffix(&naming("pl-approved")),
            " [plan approved]"
        );
        assert_eq!(index.plan_suffix(&naming("pl-typed")), " [plan ]");
        assert_eq!(index.plan_suffix(&naming("pl-missing")), "");
        assert_eq!(index.plan_suffix(&naming("")), "");
    }
}
