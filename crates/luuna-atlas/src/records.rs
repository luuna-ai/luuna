//! Record helpers: duplicate search, text normalisation, filters, confirmation scores, quality
//! checks, summaries and list helpers. Owner: s5-atlas-lib-2.
//!
//! A port of the Go tool's record helpers: `NormalizeText`, `RecordBodyText`,
//! `FindDuplicate` and `FilterBy*` (`store.go`); `Tokenize`, `ExtractRecordText`,
//! `SearchBM25`, `SearchRecords`, the confirmation scores, `FindNearDuplicates`,
//! `HasRuleSignal` and `CheckRecordQuality` (`search.go`); `RecordSummary`, `Truncate` and
//! `formatJS` (`format.go`); and the loose value helpers of `model.go` (`Obj`, `List`,
//! `AnyList`, `RecordList`, `Number`, `NumberDefault`, `JSLower`, `SplitCSV`, `Unique`,
//! `Contains`).
//!
//! Values from user data are never compared with `==` or printed with `{:?}` (CF1);
//! numbers compare as Go compares them, as `f64`.
//!
//! [`check_lossless_rewrite`] (in [`rewrite_guard`]) is the A8 guard every rewrite of an
//! expertise file calls inside the lock, before [`crate::store::write_records`].

pub mod rewrite_guard;

pub use rewrite_guard::check_lossless_rewrite;

use crate::json::{Object, Value};
use crate::registry::Registry;
use crate::store::Record;

/// One hit of [`search_bm25`] (`BM25Result`).
#[derive(Debug, Clone)]
pub struct Bm25Result {
    pub record: Record,
    pub score: f64,
    /// The record's fields that hold a query token, in extraction order.
    pub matched_fields: Vec<String>,
}

/// One hit of [`find_near_duplicates`] (`NearDuplicate`).
#[derive(Debug, Clone)]
pub struct NearDuplicate {
    pub record: Record,
    pub similarity: f64,
    pub shared_anchors: Vec<String>,
}

// ---------------------------------------------------------------------------------------
// Loose value helpers (model.go)

/// `Obj(v)`: the object `v` holds, or an empty object for anything else (absent, null,
/// an array, a scalar).
pub fn obj(value: Option<&Value>) -> &Object {
    // stub until t3: always the empty object.
    let _ = value;
    empty_object()
}

/// `List(v)`: the string elements of an array, in order; anything else is empty.
pub fn list(value: Option<&Value>) -> Vec<String> {
    // stub until t3
    let _ = value;
    Vec::new()
}

/// `AnyList(v)`: the elements of an array; anything else is empty.
pub fn any_list(value: Option<&Value>) -> &[Value] {
    // stub until t3
    let _ = value;
    &[]
}

/// `RecordList(v)`: for each element of an array, the object it holds; an array element
/// reads as an empty record (`RecordFromValue`); other elements are skipped.
pub fn record_list(value: Option<&Value>) -> Vec<&Object> {
    // stub until t3
    let _ = value;
    Vec::new()
}

/// `Number(v)`: a JSON number; an integer in the `int64` range as `f64`; an integer above
/// it (a Go `uint64`) is 0 (CF6); a string through `strconv.ParseFloat` (0 when it does
/// not parse, ±Inf when out of range); anything else 0.
pub fn number(value: Option<&Value>) -> f64 {
    // stub until t3
    let _ = value;
    0.0
}

/// `NumberDefault(v, fallback)`: `fallback` when `v` is absent or null, else [`number`].
pub fn number_default(value: Option<&Value>, fallback: f64) -> f64 {
    // stub until t3
    let _ = (value, fallback);
    0.0
}

/// `JSLower`: full Unicode lowercasing (`cases.Lower(language.Und)`), as
/// `str::to_lowercase` does.
pub fn js_lower(text: &str) -> String {
    // stub until t3
    let _ = text;
    String::new()
}

/// Go's `strings.ToLower`: each character to its simple lowercase mapping, so U+0130
/// becomes `i` (P12).
pub fn go_lower(text: &str) -> String {
    // stub until t3
    let _ = text;
    String::new()
}

/// `SplitCSV`: split on `,`, each part trimmed, empty parts dropped.
pub fn split_csv(value: &str) -> Vec<String> {
    // stub until t3
    let _ = value;
    Vec::new()
}

/// `Unique`: the first occurrence of each value, in order.
pub fn unique(values: &[String]) -> Vec<String> {
    // stub until t3
    let _ = values;
    Vec::new()
}

/// `Contains`.
pub fn contains(values: &[String], value: &str) -> bool {
    // stub until t3
    let _ = (values, value);
    false
}

/// Whether two values are equal as Go's `reflect.DeepEqual` sees the values the Go tool
/// holds: numbers as `f64` (so `Integer(100)` equals `Number(100.0)`, and NaN equals
/// nothing), objects by content whatever their key order, arrays by order. Walks with an
/// explicit stack, never recursing per nesting level (CF1). Shared with
/// [`rewrite_guard`].
#[cfg_attr(not(test), allow(dead_code))] // called by t3's and t7's code
pub(crate) fn go_deep_equal(a: &Value, b: &Value) -> bool {
    // stub until t3
    let _ = (a, b);
    false
}

fn empty_object() -> &'static Object {
    static EMPTY: std::sync::OnceLock<Object> = std::sync::OnceLock::new();
    EMPTY.get_or_init(Object::new)
}

// ---------------------------------------------------------------------------------------
// Text, duplicates and filters (store.go)

/// `NormalizeText`: [`js_lower`], then only the characters Go's `unicode.IsLetter` or
/// `unicode.IsNumber` accepts (Go 1.27.0's tables, P11), runs of other characters
/// becoming one space between kept runs.
pub fn normalize_text(text: &str) -> String {
    // stub until t3
    let _ = text;
    String::new()
}

/// `RecordBodyText(record, registry)`: the extracted text fields other than tags and
/// files, joined with spaces; for types with none, the string values of the type's
/// required then optional fields.
pub fn record_body_text(record: &Record, registry: &Registry) -> String {
    // stub until t3
    let _ = (record, registry);
    String::new()
}

/// `FindDuplicate(records, candidate, registry)`: the index of the first record of the
/// candidate's type whose normalised dedup value (the `dedup_key` field, or the body text
/// for `content_hash`) equals the candidate's; `None` when the type is unknown, the
/// candidate's value is empty or nothing matches.
pub fn find_duplicate(
    records: &[Record],
    candidate: &Record,
    registry: &Registry,
) -> Option<usize> {
    // stub until t3
    let _ = (records, candidate, registry);
    None
}

/// `FilterByType`: records whose `type` text equals `type_name` (cloned, in order).
pub fn filter_by_type(records: &[Record], type_name: &str) -> Vec<Record> {
    // stub until t3
    let _ = (records, type_name);
    Vec::new()
}

/// `FilterByClassification`: records whose `classification` text equals `class`.
pub fn filter_by_classification(records: &[Record], class: &str) -> Vec<Record> {
    // stub until t3
    let _ = (records, class);
    Vec::new()
}

/// `FilterByFile`: records with a `files` entry containing `file`, both [`js_lower`]ed.
pub fn filter_by_file(records: &[Record], file: &str) -> Vec<Record> {
    // stub until t3
    let _ = (records, file);
    Vec::new()
}

/// The `--tag` filter of `query` and `search` (`read_commands.go:61-70`): records with a
/// `tags` entry equal to `tag`, both [`js_lower`]ed.
pub fn filter_by_tag(records: &[Record], tag: &str) -> Vec<Record> {
    // stub until t3
    let _ = (records, tag);
    Vec::new()
}

// ---------------------------------------------------------------------------------------
// Search and scores (search.go)

/// `Tokenize`: [`js_lower`], every character other than ASCII letters, digits, `_`, `-`
/// and the ASCII spaces `\t \n \f \r` and space replaced by a space, then split on
/// Unicode white space (`strings.Fields`).
pub fn tokenize(text: &str) -> Vec<String> {
    // stub until t3
    let _ = text;
    Vec::new()
}

/// `ExtractRecordText(r)`: the searchable text of a record and its non-blank fields in
/// extraction order (by type, then `tags`), as `(text, [(field, value)])`.
pub fn extract_record_text(record: &Record) -> (String, Vec<(String, String)>) {
    // stub until t3
    let _ = record;
    (String::new(), Vec::new())
}

/// `SearchBM25(records, query)`: BM25 (k1 1.5, b 0.75) over the extracted text, hits
/// with a positive score, stably sorted by descending score.
pub fn search_bm25(records: &[Record], query: &str) -> Vec<Bm25Result> {
    // stub until t3
    let _ = (records, query);
    Vec::new()
}

/// `SearchRecords(records, query, boostFactor)`: the BM25 hits' records, re-sorted
/// stably after [`apply_confirmation_boost`] when `boost_factor > 0`.
pub fn search_records(records: &[Record], query: &str, boost_factor: f64) -> Vec<Record> {
    // stub until t3
    let _ = (records, query, boost_factor);
    Vec::new()
}

/// `SuccessCount`: outcomes with status `success`.
pub fn success_count(record: &Record) -> usize {
    // stub until t3
    let _ = record;
    0
}

/// `FailureCount`: outcomes with status `failure`.
pub fn failure_count(record: &Record) -> usize {
    // stub until t3
    let _ = record;
    0
}

/// `TotalApplications`: the number of `outcomes` elements of any kind.
pub fn total_applications(record: &Record) -> usize {
    // stub until t3
    let _ = record;
    0
}

/// `ConfirmationScore`: 1 per `success` outcome, 0.5 per `partial`.
pub fn confirmation_score(record: &Record) -> f64 {
    // stub until t3
    let _ = record;
    0.0
}

/// `SuccessRate`: confirmation score over total applications, 0 without any.
pub fn success_rate(record: &Record) -> f64 {
    // stub until t3
    let _ = record;
    0.0
}

/// `ApplyConfirmationBoost(score, r, factor)`: `score * (1 + factor * confirmation)`, or
/// `score` when the confirmation score is 0.
pub fn apply_confirmation_boost(score: f64, record: &Record, factor: f64) -> f64 {
    // stub until t3
    let _ = (score, record, factor);
    0.0
}

/// `SortByConfirmationScore`: a stable sort by descending confirmation score (cloned).
pub fn sort_by_confirmation_score(records: &[Record]) -> Vec<Record> {
    // stub until t3
    let _ = records;
    Vec::new()
}

/// `FindNearDuplicates(existing, candidate, registry)`: among the top ten BM25 hits for
/// the candidate's body text, those not superseded by it whose token-set similarity
/// reaches 0.6 (0.45 with a shared anchor), stably sorted by descending similarity.
pub fn find_near_duplicates(
    existing: &[Record],
    candidate: &Record,
    registry: &Registry,
) -> Vec<NearDuplicate> {
    // stub until t3
    let _ = (existing, candidate, registry);
    Vec::new()
}

/// `HasRuleSignal`: the [`js_lower`]ed text contains one of the 13 rule words.
pub fn has_rule_signal(content: &str) -> bool {
    // stub until t3
    let _ = content;
    false
}

/// `CheckRecordQuality(r, autoCommit)`: Go's two warnings (an ungrounded convention or
/// pattern; a convention without a rule word).
pub fn check_record_quality(record: &Record, auto_commit: bool) -> Vec<String> {
    // stub until t3
    let _ = (record, auto_commit);
    Vec::new()
}

// ---------------------------------------------------------------------------------------
// Summaries (format.go)

/// `RecordSummary(r, registry)`: the built-in types' summary field (conventions and
/// failures truncated to 60), an inherited convention or failure summary, or the custom
/// type's template with `{field}` and `{{field}}` replaced by [`format_js`] of the value.
pub fn record_summary(record: &Record, registry: &Registry) -> String {
    // stub until t3
    let _ = (record, registry);
    String::new()
}

/// `Truncate(text, max)` in UTF-16 code units: the text if it fits; else up to its first
/// `[.!?]` followed by ASCII white space when that comes after the start and before
/// `max`; else the first `max` units (a split surrogate becomes U+FFFD) and `...`.
pub fn truncate(text: &str, max: usize) -> String {
    // stub until t3
    let _ = (text, max);
    String::new()
}

/// `formatJS(v)`: a value as JavaScript's `String()` would print it in the Go port:
/// absent or null `""`, arrays joined with `,`, objects `[object Object]`, numbers as
/// Go's `%v` (`1e+21`, `1.5`).
pub fn format_js(value: Option<&Value>) -> String {
    // stub until t3
    let _ = value;
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;
    use crate::yaml;

    fn record(text: &str) -> Record {
        match &mut json::parse(text).expect("record json") {
            Value::Object(object) => std::mem::take(object),
            _ => panic!("not an object"),
        }
    }

    fn builtin() -> Registry {
        Registry::from_config(None).expect("built-in registry")
    }

    fn fixture_registry() -> Registry {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/s5-atlas-lib-2/custom-types.yaml"
        );
        let text = std::fs::read_to_string(path).expect("read custom-types.yaml");
        let parsed = yaml::parse(&text).expect("parse custom-types.yaml");
        let Value::Object(config) = &parsed else {
            panic!("custom-types.yaml is not a mapping");
        };
        Registry::from_config(Some(config)).expect("registry from custom-types.yaml")
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn normalize_text_keeps_letters_and_numbers_only() {
        assert_eq!(normalize_text("Hello, World! 42"), "hello world 42");
        assert_eq!(normalize_text("  --a--b--  "), "a b");
        assert_eq!(normalize_text(""), "");
        assert_eq!(normalize_text("!!!"), "");
        assert_eq!(normalize_text("snake_case-name"), "snake case name");
        assert_eq!(normalize_text("ÀÉÎ ß"), "àéî ß");
        assert_eq!(normalize_text("a\u{1F600}b"), "a b");
        assert_eq!(
            normalize_text("x\u{00B2} \u{0663}\u{0664}"),
            "x\u{00B2} \u{0663}\u{0664}"
        );
        assert_eq!(normalize_text("漢字、テスト"), "漢字 テスト");
    }

    #[test]
    fn normalize_text_drops_combining_marks_and_circled_letters() {
        // U+0301 COMBINING ACUTE ACCENT (Mn) is neither a letter nor a number in Go.
        assert_eq!(normalize_text("e\u{0301}te"), "e te");
        // U+093E DEVANAGARI VOWEL SIGN AA (Mc) is Alphabetic in Rust but not a Go letter.
        assert_eq!(
            normalize_text("\u{0915}\u{093E}\u{0930}"),
            "\u{0915} \u{0930}"
        );
        // Circled letters lower to U+24D0.. (So): dropped; a circled digit (No) is kept.
        assert_eq!(normalize_text("a\u{24B6}b"), "a b");
        assert_eq!(normalize_text("\u{24B6}b"), "b");
        assert_eq!(normalize_text("\u{2460}"), "\u{2460}");
    }

    #[test]
    fn tokenize_is_ascii_only() {
        assert_eq!(tokenize("Héllo wörld"), strings(&["h", "llo", "w", "rld"]));
        assert_eq!(tokenize("a_b-c d"), strings(&["a_b-c", "d"]));
        assert_eq!(tokenize("x\u{00A0}y"), strings(&["x", "y"]));
        assert_eq!(tokenize("x\u{000B}y"), strings(&["x", "y"]));
        assert!(tokenize("漢字").is_empty());
        assert_eq!(tokenize("ÀB"), strings(&["b"]));
        // The Kelvin sign lowercases to an ASCII k first.
        assert_eq!(tokenize("\u{212A}elvin"), strings(&["kelvin"]));
        assert_eq!(
            tokenize("  Use the LOCK.  "),
            strings(&["use", "the", "lock"])
        );
    }

    #[test]
    fn bm25_does_not_match_substrings() {
        let records = vec![
            record(r#"{"type":"convention","content":"testing framework"}"#),
            record(r#"{"type":"convention","content":"test runner"}"#),
            record(r#"{"type":"failure","description":"flaky test","resolution":"retry"}"#),
        ];
        let hits = search_bm25(&records, "test");
        assert_eq!(hits.len(), 2);
        let contents: Vec<String> = hits
            .iter()
            .map(|h| {
                format!(
                    "{}{}",
                    h.record
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or(""),
                    h.record
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                )
            })
            .collect();
        assert!(contents.contains(&"test runner".to_owned()));
        assert!(contents.contains(&"flaky test".to_owned()));
        assert!(hits.iter().all(|h| h.score > 0.0));
        let runner = hits
            .iter()
            .find(|h| h.record.get("content").is_some())
            .expect("runner hit");
        assert_eq!(runner.matched_fields, strings(&["content"]));
        assert!(search_bm25(&records, "tes").is_empty());
        assert!(search_bm25(&records, "").is_empty());
        assert!(search_bm25(&[], "test").is_empty());
        assert_eq!(search_records(&records, "runner", 0.0).len(), 1);
    }

    #[test]
    fn confirmation_score_and_boost() {
        let r = record(
            r#"{"type":"convention","content":"x","outcomes":[{"status":"success"},{"status":"partial"},{"status":"failure"},"x",["y"]]}"#,
        );
        assert_eq!(confirmation_score(&r), 1.5);
        assert_eq!(success_count(&r), 1);
        assert_eq!(failure_count(&r), 1);
        assert_eq!(total_applications(&r), 5);
        assert_eq!(success_rate(&r), 0.3);
        assert_eq!(apply_confirmation_boost(2.0, &r, 0.5), 3.5);
        let none = record(r#"{"type":"convention","content":"y"}"#);
        assert_eq!(confirmation_score(&none), 0.0);
        assert_eq!(success_rate(&none), 0.0);
        assert_eq!(apply_confirmation_boost(2.0, &none, 0.5), 2.0);
        let one =
            record(r#"{"type":"convention","content":"z","outcomes":[{"status":"success"}]}"#);
        let sorted = sort_by_confirmation_score(&[none.clone(), r.clone(), one.clone(), none]);
        let contents: Vec<&str> = sorted
            .iter()
            .map(|s| s.get("content").and_then(Value::as_str).unwrap_or(""))
            .collect();
        assert_eq!(contents, ["x", "z", "y", "y"]);
    }

    #[test]
    fn find_duplicate_content_hash_and_named_keys() {
        let registry = fixture_registry();
        // content_hash: the normalised body text (required then optional string fields).
        let existing = vec![
            record(r#"{"type":"runbook","title":"Use the lock","steps":"a"}"#),
            record(r#"{"type":"snippet","body":"Use  the Lock!","language":"rust"}"#),
        ];
        let candidate = record(r#"{"type":"snippet","body":"use the lock","language":"Rust"}"#);
        assert_eq!(find_duplicate(&existing, &candidate, &registry), Some(1));
        let other = record(r#"{"type":"snippet","body":"use the lock","language":"go"}"#);
        assert_eq!(find_duplicate(&existing, &other, &registry), None);
        // A named key: only that field, normalised, within the same type.
        let existing = vec![
            record(r#"{"type":"snippet","body":"Restart the API","language":"x"}"#),
            record(r#"{"type":"runbook","title":"Restart the API","steps":"a"}"#),
        ];
        let candidate = record(r#"{"type":"runbook","title":"restart  the api!","steps":"b"}"#);
        assert_eq!(find_duplicate(&existing, &candidate, &registry), Some(1));
        let empty = record(r#"{"type":"runbook","title":"","steps":"b"}"#);
        assert_eq!(find_duplicate(&existing, &empty, &registry), None);
        let not_string = record(r#"{"type":"runbook","title":5,"steps":"b"}"#);
        assert_eq!(find_duplicate(&existing, &not_string, &registry), None);
        let unknown = record(r#"{"type":"nope","title":"Restart the API"}"#);
        assert_eq!(find_duplicate(&existing, &unknown, &registry), None);
        // Built-in types dedupe on their key field.
        let existing = vec![record(r#"{"type":"convention","content":"Always lock."}"#)];
        let candidate = record(r#"{"type":"convention","content":"always LOCK"}"#);
        assert_eq!(find_duplicate(&existing, &candidate, &builtin()), Some(0));
    }

    #[test]
    fn has_rule_signal_needles() {
        for needle in [
            "because",
            "must not",
            "do not",
            "don't",
            "avoid",
            "always ",
            "never ",
            "prefer",
            "required for",
            "reason:",
            "rationale",
            "so that",
            "otherwise",
        ] {
            assert!(has_rule_signal(&format!("x {needle} y")), "{needle:?}");
        }
        assert!(has_rule_signal("AVOID globals"));
        assert!(!has_rule_signal("Do it always"));
        assert!(!has_rule_signal("never"));
        assert!(!has_rule_signal("Nothing here"));
        let r = record(r#"{"type":"convention","content":"Use tabs"}"#);
        assert_eq!(
            check_record_quality(&r, false),
            [
                "convention has no file/dir anchor or evidence; add --files, --dir-anchor, or an --evidence-* flag so it can be scoped and verified.",
                "convention reads like a restatement of code (no rule words such as \"because\", \"avoid\", \"never\", \"prefer\"); state the rule and why, or skip it.",
            ]
        );
        let r = record(
            r#"{"type":"convention","content":"Use tabs because x","evidence":{"commit":"abc"}}"#,
        );
        assert_eq!(check_record_quality(&r, false), Vec::<String>::new());
        assert_eq!(check_record_quality(&r, true).len(), 1);
    }

    #[test]
    fn truncate_utf16_and_sentence_break() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("exactly10!", 10), "exactly10!");
        assert_eq!(
            truncate("First sentence. Second sentence is longer", 20),
            "First sentence."
        );
        assert_eq!(truncate("abcdefghij", 5), "abcde...");
        assert_eq!(truncate("a. bbbbbbbbbb. c", 5), "a.");
        // The break must come after the start and before max.
        assert_eq!(truncate(". abcdefgh", 4), ". ab...");
        assert_eq!(truncate("abcd. efgh", 4), "abcd...");
        // Only ASCII white space ends a sentence.
        assert_eq!(truncate("Done.\u{00A0}More text", 6), "Done.\u{00A0}...");
        // Lengths in UTF-16 units; a split surrogate pair becomes U+FFFD.
        assert_eq!(
            truncate("\u{1F600}\u{1F600}\u{1F600}", 6),
            "\u{1F600}\u{1F600}\u{1F600}"
        );
        assert_eq!(
            truncate("\u{1F600}\u{1F600}\u{1F600}", 4),
            "\u{1F600}\u{1F600}..."
        );
        assert_eq!(
            truncate("\u{1F600}\u{1F600}\u{1F600}", 3),
            "\u{1F600}\u{FFFD}..."
        );
    }

    #[test]
    fn record_summary_custom_template() {
        let registry = fixture_registry();
        let r = record(r#"{"type":"runbook","title":"Restart","steps":"a"}"#);
        assert_eq!(record_summary(&r, &registry), "Restart ()");
        let r = record(r#"{"type":"runbook","title":"Restart","owner_team":3}"#);
        assert_eq!(record_summary(&r, &registry), "Restart (3)");
        let r = record(r#"{"type":"runbook","title":["a","b"],"owner_team":{"x":1}}"#);
        assert_eq!(record_summary(&r, &registry), "a,b ([object Object])");
        let r = record(r#"{"type":"runbook","title":true,"owner_team":1e21}"#);
        assert_eq!(record_summary(&r, &registry), "true (1e+21)");
        let r = record(r#"{"type":"runbook","title":1.5,"owner_team":null}"#);
        assert_eq!(record_summary(&r, &registry), "1.5 ()");
        let r = record(r#"{"type":"snippet","body":"let x = 1;","language":"rust"}"#);
        assert_eq!(record_summary(&r, &registry), "rust: let x = 1;");
        // An inherited convention summary is truncated like a convention's.
        let long = "x".repeat(70);
        let r = record(&format!(r#"{{"type":"lesson","content":"{long}"}}"#));
        assert_eq!(
            record_summary(&r, &registry),
            format!("{}...", "x".repeat(60))
        );
        let r = record(r#"{"type":"decision","title":"Pick A","rationale":"r"}"#);
        assert_eq!(record_summary(&r, &registry), "Pick A");
        let r = record(r#"{"type":"unknown","title":"t"}"#);
        assert_eq!(record_summary(&r, &registry), "");
        assert_eq!(format_js(None), "");
        assert_eq!(format_js(Some(&Value::Number(100.0))), "100");
        assert_eq!(format_js(Some(&Value::Number(1e-7))), "1e-07");
        assert_eq!(format_js(Some(&Value::Integer(7))), "7");
    }

    #[test]
    fn go_lower_dotted_capital_i() {
        assert_eq!(go_lower("\u{0130}STANBUL"), "istanbul");
        assert_eq!(js_lower("\u{0130}"), "i\u{0307}");
        assert_eq!(go_lower("\u{01C5}"), "\u{01C6}");
        assert_eq!(go_lower("\u{212A}"), "k");
        assert_eq!(go_lower("ÀB"), "àb");
        // normalize_text uses js_lower: the dot above is a mark and splits the word.
        assert_eq!(normalize_text("\u{0130}x"), "i x");
    }

    #[test]
    fn unicode_table_matches_samples() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/s5-atlas-lib-2/unicode-samples.txt"
        );
        let text = std::fs::read_to_string(path).expect("read unicode-samples.txt");
        let mut cases = 0;
        for (n, line) in text.lines().enumerate() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            assert_eq!(
                fields.len(),
                3,
                "line {}: three tab-separated fields",
                n + 1
            );
            let (function, input, want) = (fields[0], fields[1], fields[2]);
            let got = match function {
                "normalize_text" => normalize_text(input),
                "js_lower" => js_lower(input),
                "go_lower" => go_lower(input),
                other => panic!("line {}: unknown function {other}", n + 1),
            };
            assert_eq!(got, want, "line {}: {function}({input:?})", n + 1);
            cases += 1;
        }
        assert!(cases >= 10, "only {cases} cases");
    }

    #[test]
    fn number_of_uint64_is_zero() {
        assert_eq!(number(Some(&Value::Integer(i128::from(u64::MAX)))), 0.0);
        assert_eq!(number(Some(&Value::Integer(i128::from(i64::MAX) + 1))), 0.0);
        assert_eq!(
            number(Some(&Value::Integer(i128::from(i64::MAX)))),
            i64::MAX as f64
        );
        assert_eq!(number(Some(&Value::Integer(-5))), -5.0);
        assert_eq!(number(Some(&Value::Number(2.5))), 2.5);
        assert_eq!(number(Some(&Value::from("12.5"))), 12.5);
        assert_eq!(number(Some(&Value::from("abc"))), 0.0);
        assert_eq!(number(Some(&Value::from(" 1"))), 0.0);
        assert_eq!(number(Some(&Value::from("1e400"))), f64::INFINITY);
        assert_eq!(number(Some(&Value::Bool(true))), 0.0);
        assert_eq!(number(Some(&Value::Null)), 0.0);
        assert_eq!(number(None), 0.0);
        assert_eq!(number_default(None, 7.0), 7.0);
        assert_eq!(number_default(Some(&Value::Null), 7.0), 7.0);
        assert_eq!(
            number_default(Some(&Value::Integer(i128::from(u64::MAX))), 7.0),
            0.0
        );
        assert_eq!(number_default(Some(&Value::Number(3.0)), 7.0), 3.0);
    }

    #[test]
    fn integer_and_float_compare_as_go() {
        let eq = |a: &str, b: &str| {
            go_deep_equal(
                &json::parse(a).expect("json a"),
                &json::parse(b).expect("json b"),
            )
        };
        assert!(go_deep_equal(&Value::Integer(100), &Value::Number(100.0)));
        assert!(go_deep_equal(&Value::Number(100.0), &Value::Integer(100)));
        assert!(!go_deep_equal(&Value::Integer(100), &Value::Number(100.5)));
        assert!(!go_deep_equal(
            &Value::Number(f64::NAN),
            &Value::Number(f64::NAN)
        ));
        assert!(!go_deep_equal(&Value::from("100"), &Value::Number(100.0)));
        assert!(go_deep_equal(&Value::Null, &Value::Null));
        assert!(!go_deep_equal(&Value::Null, &Value::from("")));
        assert!(eq(
            r#"{"a":1,"b":[1,{"c":"d"}]}"#,
            r#"{"b":[1.0,{"c":"d"}],"a":1e0}"#
        ));
        assert!(!eq(r#"[1,2]"#, r#"[2,1]"#));
        assert!(!eq(r#"{"a":1}"#, r#"{"a":1,"b":null}"#));
        assert!(!eq(r#"{}"#, r#"[]"#));
        assert!(eq(r#"[]"#, r#"[]"#));
        // Through the helpers that compare record values.
        let a = record(r#"{"type":"convention","content":"x","n":100}"#);
        let mut b = a.clone();
        b.insert("n", Value::Integer(100));
        assert!(go_deep_equal(&Value::Object(a), &Value::Object(b)));
    }

    #[test]
    #[ignore = "hostile input: run under prlimit (PIPELINE rule 10)"]
    fn hostile_long_text_is_linear() {
        // 1 MiB inputs built here: alternating kept and dropped characters, and one long
        // run without a sentence break.
        let mixed: String = "a\u{0301}-".repeat(1 << 18);
        assert!(mixed.len() >= 1 << 20);
        let start = std::time::Instant::now();
        let normalized = normalize_text(&mixed);
        assert_eq!(normalized.len(), 2 * (1 << 18) - 1);
        // "a -a -a ... -a -": the first "a", then "-a" for each later group, then "-".
        let tokens = tokenize(&mixed);
        assert_eq!(tokens.len(), (1 << 18) + 1);
        let plain = "x".repeat(1 << 20);
        assert_eq!(truncate(&plain, 60).len(), 63);
        let dots = ".".repeat(1 << 20);
        assert_eq!(truncate(&dots, 60).len(), 63);
        assert!(!has_rule_signal(&plain));
        assert!(
            start.elapsed() < std::time::Duration::from_secs(60),
            "took {:?}",
            start.elapsed()
        );
    }
}
