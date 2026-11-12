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
//! numbers compare as Go compares them, as `f64`. No function here recurses once per
//! nesting level of a value: arrays are joined and values compared with explicit stacks.
//!
//! The Go regular expressions these functions use (`[^\w\s-]`, `[.!?]\s` and the summary
//! token pattern) are hand-written matchers with Go's ASCII classes (`\w` is
//! `[0-9A-Za-z_]`, `\s` is `[\t\n\f\r ]`).
//!
//! [`check_lossless_rewrite`] (in [`rewrite_guard`]) is the A8 guard every rewrite of an
//! expertise file calls inside the lock, before [`crate::store::write_records`].

pub mod rewrite_guard;
mod unicode_tables;

pub use rewrite_guard::check_lossless_rewrite;

use std::collections::{HashMap, HashSet};

use crate::evidence;
use crate::json::{Object, Value};
use crate::registry::{go_text, Registry};
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
    match value {
        Some(Value::Object(object)) => object,
        _ => empty_object(),
    }
}

/// `List(v)`: the string elements of an array, in order; anything else is empty.
pub fn list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

/// `AnyList(v)`: the elements of an array; anything else is empty.
pub fn any_list(value: Option<&Value>) -> &[Value] {
    match value {
        Some(Value::Array(items)) => items,
        _ => &[],
    }
}

/// `RecordList(v)`: for each element of an array, the object it holds; an array element
/// reads as an empty record (`RecordFromValue`); other elements are skipped.
pub fn record_list(value: Option<&Value>) -> Vec<&Object> {
    any_list(value)
        .iter()
        .filter_map(|item| match item {
            Value::Object(object) => Some(object),
            Value::Array(_) => Some(empty_object()),
            _ => None,
        })
        .collect()
}

/// `Number(v)`: a JSON number; an integer in the `int64` range as `f64`; an integer above
/// it (a Go `uint64`) is 0 (CF6); a string through `strconv.ParseFloat` (0 when it does
/// not parse, ±Inf when out of range); anything else 0.
pub fn number(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::Number(n)) => *n,
        // yaml.v3 gives a Go `int` for an integer in the int64 range and a `uint64` above
        // it; `Number` has no `uint64` case.
        Some(Value::Integer(i)) => match i64::try_from(*i) {
            Ok(n) => n as f64,
            Err(_) => 0.0,
        },
        Some(Value::String(s)) => go_parse_float(s),
        _ => 0.0,
    }
}

/// `NumberDefault(v, fallback)`: `fallback` when `v` is absent or null, else [`number`].
pub fn number_default(value: Option<&Value>, fallback: f64) -> f64 {
    match value {
        None | Some(Value::Null) => fallback,
        Some(_) => number(value),
    }
}

/// `JSLower`: full Unicode lowercasing (`cases.Lower(language.Und)`), as
/// `str::to_lowercase` does.
pub fn js_lower(text: &str) -> String {
    text.to_lowercase()
}

/// Go's `strings.ToLower`: each character to its simple lowercase mapping, so U+0130
/// becomes `i` (P12).
pub fn go_lower(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c == '\u{130}' {
                // The only character whose full lowercase mapping is longer than one
                // character; its simple mapping (Go's `unicode.ToLower`) is `i`.
                'i'
            } else {
                c.to_lowercase().next().unwrap_or(c)
            }
        })
        .collect()
}

/// `SplitCSV`: split on `,`, each part trimmed, empty parts dropped.
pub fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `Unique`: the first occurrence of each value, in order.
pub fn unique(values: &[String]) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    values
        .iter()
        .filter(|value| seen.insert(value.as_str()))
        .cloned()
        .collect()
}

/// `Contains`.
pub fn contains(values: &[String], value: &str) -> bool {
    values.iter().any(|v| v.as_str() == value)
}

/// Whether two values are equal as Go's `reflect.DeepEqual` sees the values the Go tool
/// holds: numbers as `f64` (so `Integer(100)` equals `Number(100.0)`, and NaN equals
/// nothing), objects by content whatever their key order, arrays by order. Walks with an
/// explicit stack, never recursing per nesting level (CF1). Shared with
/// [`rewrite_guard`].
#[cfg_attr(not(test), allow(dead_code))] // called by t7's code
pub(crate) fn go_deep_equal(a: &Value, b: &Value) -> bool {
    let mut pending: Vec<(&Value, &Value)> = vec![(a, b)];
    while let Some((x, y)) = pending.pop() {
        match (x, y) {
            (Value::Null, Value::Null) => {}
            (Value::Bool(p), Value::Bool(q)) => {
                if p != q {
                    return false;
                }
            }
            (Value::String(p), Value::String(q)) => {
                if p != q {
                    return false;
                }
            }
            (Value::Number(_) | Value::Integer(_), Value::Number(_) | Value::Integer(_)) => {
                // NaN compares unequal to everything, itself included, as in Go.
                if go_float(x) != go_float(y) {
                    return false;
                }
            }
            (Value::Array(p), Value::Array(q)) => {
                if p.len() != q.len() {
                    return false;
                }
                pending.extend(p.iter().zip(q.iter()));
            }
            (Value::Object(p), Value::Object(q)) => {
                if p.len() != q.len() {
                    return false;
                }
                if q.len() <= 16 {
                    for (key, value) in p.iter() {
                        match q.get(key) {
                            Some(other) => pending.push((value, other)),
                            None => return false,
                        }
                    }
                } else {
                    // A lookup table keeps large objects linear.
                    let index: HashMap<&str, &Value> = q.iter().collect();
                    for (key, value) in p.iter() {
                        match index.get(key) {
                            Some(other) => pending.push((value, *other)),
                            None => return false,
                        }
                    }
                }
            }
            _ => return false,
        }
    }
    true
}

/// A JSON number as the Go tool's `float64`.
fn go_float(value: &Value) -> f64 {
    match value {
        Value::Number(n) => *n,
        // float64(n): rounds to the nearest f64.
        Value::Integer(i) => *i as f64,
        _ => 0.0,
    }
}

/// Go's `==` on two `any` values as `FindDuplicate` uses it on record types: absent and
/// null are both `nil`; scalars of the same kind compare by value (numbers as `f64`).
/// Arrays and objects are never equal here; Go would panic comparing two of them.
fn go_interface_equal(a: Option<&Value>, b: Option<&Value>) -> bool {
    let nil = Value::Null;
    let a = a.unwrap_or(&nil);
    let b = b.unwrap_or(&nil);
    match (a, b) {
        (Value::Array(_) | Value::Object(_), _) | (_, Value::Array(_) | Value::Object(_)) => false,
        _ => go_deep_equal(a, b),
    }
}

fn empty_object() -> &'static Object {
    static EMPTY: std::sync::OnceLock<Object> = std::sync::OnceLock::new();
    EMPTY.get_or_init(Object::new)
}

// ---------------------------------------------------------------------------------------
// Go's strconv.ParseFloat (bitSize 64), for `Number` of a string

/// `strconv.ParseFloat(s, 64)` as `Number` uses it: the value Go returns, so 0 on a
/// syntax error and ±Inf on a range error. Accepts what Go accepts: an optional sign,
/// decimal or `0x` hexadecimal mantissas (hexadecimal needs a `p` exponent), `_` digit
/// separators where Go allows them, and `inf`, `infinity` and `nan` in any case.
fn go_parse_float(s: &str) -> f64 {
    if let Some((value, n)) = go_float_special(s.as_bytes()) {
        return if n == s.len() { value } else { 0.0 };
    }
    let Some(read) = go_read_float(s.as_bytes()) else {
        return 0.0;
    };
    if read.end != s.len() {
        return 0.0;
    }
    if read.hex {
        return go_atof_hex(read.mantissa, read.exp, read.neg, read.trunc);
    }
    // Decimal: Go rounds correctly, as Rust's parser does; overflow is ±Inf and underflow
    // ±0 in both. The syntax was checked above, so only the separators are removed.
    let cleaned: String = s.chars().filter(|&c| c != '_').collect();
    cleaned.parse::<f64>().unwrap_or(0.0)
}

/// Go's `special`: `inf`, `infinity` (optionally signed) or `nan`, any case, and the
/// number of bytes it took.
fn go_float_special(s: &[u8]) -> Option<(f64, usize)> {
    let first = *s.first()?;
    let common = |rest: &[u8], word: &[u8]| {
        rest.iter()
            .zip(word)
            .take_while(|(a, b)| a.to_ascii_lowercase() == **b)
            .count()
    };
    match first {
        b'+' | b'-' | b'i' | b'I' => {
            let (sign, rest, nsign) = match first {
                b'-' => (-1.0, &s[1..], 1),
                b'+' => (1.0, &s[1..], 1),
                _ => (1.0, s, 0),
            };
            let mut n = common(rest, b"infinity");
            if 3 < n && n < 8 {
                n = 3;
            }
            (n == 3 || n == 8).then_some((sign * f64::INFINITY, nsign + n))
        }
        b'n' | b'N' => (common(s, b"nan") == 3).then_some((f64::NAN, 3)),
        _ => None,
    }
}

/// What Go's `readFloat` found at the start of a string.
struct ReadFloat {
    mantissa: u64,
    exp: i64,
    neg: bool,
    trunc: bool,
    hex: bool,
    /// Bytes taken.
    end: usize,
}

/// Go's `readFloat` (internal/strconv/atof.go): the longest prefix that is a number,
/// or `None` where Go returns `ok == false`.
fn go_read_float(s: &[u8]) -> Option<ReadFloat> {
    let mut i = 0;
    let mut neg = false;
    let mut underscores = false;
    match s.first()? {
        b'+' => i += 1,
        b'-' => {
            i += 1;
            neg = true;
        }
        _ => {}
    }
    let mut base: u64 = 10;
    let mut max_mant_digits = 19;
    let mut exp_char = b'e';
    let mut hex = false;
    if i + 2 < s.len() && s[i] == b'0' && s[i + 1].eq_ignore_ascii_case(&b'x') {
        base = 16;
        max_mant_digits = 16;
        i += 2;
        exp_char = b'p';
        hex = true;
    }
    let mut sawdot = false;
    let mut sawdigits = false;
    let mut nd: i64 = 0;
    let mut nd_mant: i64 = 0;
    let mut dp: i64 = 0;
    let mut mantissa: u64 = 0;
    let mut trunc = false;
    while i < s.len() {
        let c = s[i];
        let lower = c.to_ascii_lowercase();
        if c == b'_' {
            underscores = true;
        } else if c == b'.' {
            if sawdot {
                break;
            }
            sawdot = true;
            dp = nd;
        } else if c.is_ascii_digit() {
            sawdigits = true;
            if c == b'0' && nd == 0 {
                dp -= 1;
            } else {
                nd += 1;
                if nd_mant < max_mant_digits {
                    mantissa = mantissa * base + u64::from(c - b'0');
                    nd_mant += 1;
                } else if c != b'0' {
                    trunc = true;
                }
            }
        } else if base == 16 && (b'a'..=b'f').contains(&lower) {
            sawdigits = true;
            nd += 1;
            if nd_mant < max_mant_digits {
                mantissa = mantissa * 16 + u64::from(lower - b'a' + 10);
                nd_mant += 1;
            } else {
                trunc = true;
            }
        } else {
            break;
        }
        i += 1;
    }
    if !sawdigits {
        return None;
    }
    if !sawdot {
        dp = nd;
    }
    if base == 16 {
        dp *= 4;
        nd_mant *= 4;
    }
    if i < s.len() && s[i].to_ascii_lowercase() == exp_char {
        i += 1;
        if i >= s.len() {
            return None;
        }
        let mut esign = 1;
        match s[i] {
            b'+' => i += 1,
            b'-' => {
                i += 1;
                esign = -1;
            }
            _ => {}
        }
        if i >= s.len() || !s[i].is_ascii_digit() {
            return None;
        }
        let mut e: i64 = 0;
        while i < s.len() && (s[i].is_ascii_digit() || s[i] == b'_') {
            if s[i] == b'_' {
                underscores = true;
            } else if e < 10000 {
                e = e * 10 + i64::from(s[i] - b'0');
            }
            i += 1;
        }
        dp += e * esign;
    } else if base == 16 {
        return None;
    }
    let exp = if mantissa != 0 { dp - nd_mant } else { 0 };
    if underscores && !go_underscore_ok(&s[..i]) {
        return None;
    }
    Some(ReadFloat {
        mantissa,
        exp,
        neg,
        trunc,
        hex,
        end: i,
    })
}

/// Go's `underscoreOK`: each `_` sits between digits (or after a base prefix).
fn go_underscore_ok(s: &[u8]) -> bool {
    let mut saw = b'^';
    let mut s = s;
    if let Some(b'+' | b'-') = s.first() {
        s = &s[1..];
    }
    let mut i = 0;
    let mut hex = false;
    if s.len() >= 2 && s[0] == b'0' && matches!(s[1].to_ascii_lowercase(), b'b' | b'o' | b'x') {
        i = 2;
        saw = b'0';
        hex = s[1].eq_ignore_ascii_case(&b'x');
    }
    while i < s.len() {
        let c = s[i];
        if c.is_ascii_digit() || (hex && (b'a'..=b'f').contains(&c.to_ascii_lowercase())) {
            saw = b'0';
        } else if c == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
        } else {
            if saw == b'_' {
                return false;
            }
            saw = b'!';
        }
        i += 1;
    }
    saw != b'_'
}

/// Go's `atofHex` for `float64`: `mantissa * 2^exp` rounded to nearest even, ±Inf on
/// overflow.
fn go_atof_hex(mut mantissa: u64, exp: i64, neg: bool, trunc: bool) -> f64 {
    const MANTBITS: u32 = 52;
    const EXPBITS: u32 = 11;
    const BIAS: i64 = -1023;
    let max_exp: i64 = (1 << EXPBITS) + BIAS - 2;
    let min_exp: i64 = BIAS + 1;
    let mut exp = exp + i64::from(MANTBITS);
    while mantissa != 0 && mantissa >> (MANTBITS + 2) == 0 {
        mantissa <<= 1;
        exp -= 1;
    }
    if trunc {
        mantissa |= 1;
    }
    while mantissa >> (1 + MANTBITS + 2) != 0 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp += 1;
    }
    while mantissa > 1 && exp < min_exp - 2 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp += 1;
    }
    let mut round = mantissa & 3;
    mantissa >>= 2;
    round |= mantissa & 1;
    exp += 2;
    if round == 3 {
        mantissa += 1;
        if mantissa == 1 << (1 + MANTBITS) {
            mantissa >>= 1;
            exp += 1;
        }
    }
    if mantissa >> MANTBITS == 0 {
        exp = BIAS;
    }
    if exp > max_exp {
        mantissa = 1 << MANTBITS;
        exp = max_exp + 1;
    }
    let mut bits = mantissa & ((1 << MANTBITS) - 1);
    bits |= (((exp - BIAS) as u64) & ((1 << EXPBITS) - 1)) << MANTBITS;
    if neg {
        bits |= 1 << 63;
    }
    f64::from_bits(bits)
}

// ---------------------------------------------------------------------------------------
// Text, duplicates and filters (store.go)

/// Whether Go's `unicode.IsLetter(c) || unicode.IsNumber(c)` holds, from Go 1.27.0's
/// tables (P11).
fn is_go_letter_or_number(c: char) -> bool {
    let c = u32::from(c);
    unicode_tables::LETTER_OR_NUMBER
        .binary_search_by(|&(lo, hi)| {
            if hi < c {
                std::cmp::Ordering::Less
            } else if lo > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// `NormalizeText`: [`js_lower`], then only the characters Go's `unicode.IsLetter` or
/// `unicode.IsNumber` accepts (Go 1.27.0's tables, P11), runs of other characters
/// becoming one space between kept runs.
pub fn normalize_text(text: &str) -> String {
    let mut out = String::new();
    let mut separator = false;
    for c in js_lower(text).chars() {
        if is_go_letter_or_number(c) {
            if separator && !out.is_empty() {
                out.push(' ');
            }
            out.push(c);
            separator = false;
        } else {
            separator = true;
        }
    }
    out
}

/// `RecordBodyText(record, registry)`: the extracted text fields other than tags and
/// files, joined with spaces; for types with none, the string values of the type's
/// required then optional fields.
pub fn record_body_text(record: &Record, registry: &Registry) -> String {
    let (_, fields) = extract_record_text(record);
    let parts: Vec<&str> = fields
        .iter()
        .filter(|(name, _)| name != "tags" && name != "files")
        .map(|(_, value)| value.as_str())
        .collect();
    if !parts.is_empty() {
        return parts.join(" ");
    }
    let Some(definition) = registry.get(&text(record.get("type"))) else {
        return String::new();
    };
    definition
        .required
        .iter()
        .chain(definition.optional.iter())
        .filter_map(|field| record.get(field).and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `FindDuplicate(records, candidate, registry)`: the index of the first record of the
/// candidate's type whose normalised dedup value (the `dedup_key` field, or the body text
/// for `content_hash`) equals the candidate's; `None` when the type is unknown, the
/// candidate's value is empty or nothing matches.
///
/// The types are compared as Go's `==` on two `any` values; where both are arrays or
/// objects Go would panic, and this counts them as different.
pub fn find_duplicate(
    records: &[Record],
    candidate: &Record,
    registry: &Registry,
) -> Option<usize> {
    let definition = registry.get(&text(candidate.get("type")))?;
    let value = |r: &Record| {
        if definition.dedup_key == "content_hash" {
            normalize_text(&record_body_text(r, registry))
        } else {
            match r.get(&definition.dedup_key) {
                Some(Value::String(s)) => normalize_text(s),
                _ => String::new(),
            }
        }
    };
    let wanted = value(candidate);
    if wanted.is_empty() {
        return None;
    }
    records.iter().position(|r| {
        go_interface_equal(r.get("type"), candidate.get("type")) && value(r) == wanted
    })
}

/// `FilterByType`: records whose `type` text equals `type_name` (cloned, in order).
pub fn filter_by_type(records: &[Record], type_name: &str) -> Vec<Record> {
    records
        .iter()
        .filter(|r| text(r.get("type")) == type_name)
        .cloned()
        .collect()
}

/// `FilterByClassification`: records whose `classification` text equals `class`.
pub fn filter_by_classification(records: &[Record], class: &str) -> Vec<Record> {
    records
        .iter()
        .filter(|r| text(r.get("classification")) == class)
        .cloned()
        .collect()
}

/// `FilterByFile`: records with a `files` entry containing `file`, both [`js_lower`]ed.
pub fn filter_by_file(records: &[Record], file: &str) -> Vec<Record> {
    let needle = js_lower(file);
    records
        .iter()
        .filter(|r| {
            list(r.get("files"))
                .iter()
                .any(|f| js_lower(f).contains(needle.as_str()))
        })
        .cloned()
        .collect()
}

/// The `--tag` filter of `query` and `search` (`read_commands.go:61-70`): records with a
/// `tags` entry equal to `tag`, both [`js_lower`]ed.
pub fn filter_by_tag(records: &[Record], tag: &str) -> Vec<Record> {
    let wanted = js_lower(tag);
    records
        .iter()
        .filter(|r| list(r.get("tags")).iter().any(|t| js_lower(t) == wanted))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------------------
// Search and scores (search.go)

/// `Tokenize`: [`js_lower`], every character other than ASCII letters, digits, `_`, `-`
/// and the ASCII spaces `\t \n \f \r` and space replaced by a space, then split on
/// Unicode white space (`strings.Fields`).
pub fn tokenize(text: &str) -> Vec<String> {
    // `[^\w\s-]` with Go's ASCII classes: \w = [0-9A-Za-z_], \s = [\t\n\f\r ].
    let kept = |c: char| {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '\t' | '\n' | '\u{000C}' | '\r' | ' ')
    };
    let replaced: String = js_lower(text)
        .chars()
        .map(|c| if kept(c) { c } else { ' ' })
        .collect();
    replaced.split_whitespace().map(str::to_owned).collect()
}

/// `ExtractRecordText(r)`: the searchable text of a record and its non-blank fields in
/// extraction order (by type, then `tags`), as `(text, [(field, value)])`.
pub fn extract_record_text(record: &Record) -> (String, Vec<(String, String)>) {
    let names: &[&str] = match text(record.get("type")).as_str() {
        "pattern" | "reference" => &["name", "description", "files"],
        "convention" => &["content"],
        "failure" => &["description", "resolution"],
        "decision" => &["title", "rationale"],
        "guide" => &["name", "description"],
        _ => &[],
    };
    let mut fields: Vec<(String, String)> = Vec::new();
    for name in names.iter().chain(std::iter::once(&"tags")) {
        let value = match record.get(name) {
            Some(Value::String(s)) => s.clone(),
            None | Some(Value::Null) => String::new(),
            other => list(other).join(" "),
        };
        if !value.trim().is_empty() {
            fields.push(((*name).to_owned(), value));
        }
    }
    let all = fields
        .iter()
        .map(|(_, value)| value.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    (all, fields)
}

/// `SearchBM25(records, query)`: BM25 (k1 1.5, b 0.75) over the extracted text, hits
/// with a positive score, stably sorted by descending score.
///
/// The arithmetic follows Go's expression order, and the logarithm is Go's own
/// ([`go_log`]), so scores are the same `f64` values the Go tool computes.
pub fn search_bm25(records: &[Record], query: &str) -> Vec<Bm25Result> {
    let query_tokens = tokenize(query);
    if records.is_empty() || query_tokens.is_empty() {
        return Vec::new();
    }
    struct Document<'a> {
        record: &'a Record,
        tokens: Vec<String>,
        fields: Vec<(String, String)>,
    }
    let mut docs: Vec<Document> = Vec::with_capacity(records.len());
    let mut df: HashMap<String, f64> = HashMap::new();
    let mut total: usize = 0;
    for record in records {
        let (all, fields) = extract_record_text(record);
        let tokens = tokenize(&all);
        total += tokens.len();
        for token in unique(&tokens) {
            *df.entry(token).or_insert(0.0) += 1.0;
        }
        docs.push(Document {
            record,
            tokens,
            fields,
        });
    }
    let n_docs = docs.len() as f64;
    let avg = total as f64 / n_docs;
    if avg == 0.0 {
        return Vec::new();
    }
    let idf: HashMap<&str, f64> = df
        .iter()
        .map(|(term, n)| (term.as_str(), go_log((n_docs - n + 0.5) / (n + 0.5) + 1.0)))
        .collect();
    let mut out: Vec<Bm25Result> = Vec::new();
    for doc in &docs {
        let mut freq: HashMap<&str, f64> = HashMap::new();
        for term in &doc.tokens {
            *freq.entry(term.as_str()).or_insert(0.0) += 1.0;
        }
        let doc_len = doc.tokens.len() as f64;
        let mut score = 0.0;
        for q in &query_tokens {
            let tf = freq.get(q.as_str()).copied().unwrap_or(0.0);
            let weight = idf.get(q.as_str()).copied().unwrap_or(0.0);
            // Go: idf[q] * (tf * 2.5) / (tf + 1.5*(1-.75+.75*(len/avg))); 1-.75 is a
            // constant, 0.25.
            score += weight * (tf * 2.5) / (tf + 1.5 * (0.25 + 0.75 * (doc_len / avg)));
        }
        if score > 0.0 {
            let matched_fields = doc
                .fields
                .iter()
                .filter(|(_, value)| {
                    let tokens = tokenize(value);
                    query_tokens.iter().any(|q| contains(&tokens, q))
                })
                .map(|(name, _)| name.clone())
                .collect();
            out.push(Bm25Result {
                record: doc.record.clone(),
                score,
                matched_fields,
            });
        }
    }
    out.sort_by(|a, b| descending(a.score, b.score));
    out
}

/// The order of Go's `sort.SliceStable(..., a > b)` for a stable `sort_by`.
fn descending(a: f64, b: f64) -> std::cmp::Ordering {
    if a > b {
        std::cmp::Ordering::Less
    } else if b > a {
        std::cmp::Ordering::Greater
    } else {
        std::cmp::Ordering::Equal
    }
}

/// Go's `math.Log` as the pinned Go 1.27.0 computes it on amd64 (`archLog`,
/// `math/log_amd64.s`): the FreeBSD algorithm with the same constants, the same operation
/// order and no fused multiply-add, so the result is the same `f64` bit for bit, which a
/// platform `ln` does not promise.
fn go_log(x: f64) -> f64 {
    const HSQRT2: f64 = f64::from_bits(0x3FE6_A09E_667F_3BCD);
    const LN2_HI: f64 = f64::from_bits(0x3FE6_2E42_FEE0_0000);
    const LN2_LO: f64 = f64::from_bits(0x3DEA_39EF_3579_3C76);
    const L1: f64 = f64::from_bits(0x3FE5_5555_5555_5593);
    const L2: f64 = f64::from_bits(0x3FD9_9999_9997_FA04);
    const L3: f64 = f64::from_bits(0x3FD2_4924_9422_9359);
    const L4: f64 = f64::from_bits(0x3FCC_71C5_1D8E_78AF);
    const L5: f64 = f64::from_bits(0x3FC7_4664_96CB_03DE);
    const L6: f64 = f64::from_bits(0x3FC3_9A09_D078_C69F);
    const L7: f64 = f64::from_bits(0x3FC2_F112_DF3E_5244);
    let bits = x.to_bits();
    if bits & !(1u64 << 63) == 0 {
        return f64::NEG_INFINITY;
    }
    if (bits as i64) < 0 {
        return f64::NAN;
    }
    if bits >= f64::INFINITY.to_bits() {
        return x;
    }
    // f1, ki := Frexp(x), done on the bits as the assembly does.
    let f1 = f64::from_bits((bits & 0x000F_FFFF_FFFF_FFFF) | 0x3FE0_0000_0000_0000);
    let k = f64::from(((bits >> 52) & 0x7FF) as i32 - 0x3FE);
    // CMPSD $5 (not less than): adjust when !(HSqrt2 < f1).
    let adjust = if HSQRT2 < f1 { 0.0 } else { 1.0 };
    let k = k - adjust;
    let f1 = f1 * (adjust + 1.0);
    let f = f1 - 1.0;
    let s = f / (2.0 + f);
    let s2 = s * s;
    let s4 = s2 * s2;
    let t1 = s2 * (L1 + s4 * (L3 + s4 * (L5 + s4 * L7)));
    let t2 = s4 * (L2 + s4 * (L4 + s4 * L6));
    let r = t1 + t2;
    let hfsq = 0.5 * f * f;
    k * LN2_HI - ((hfsq - (s * (hfsq + r) + k * LN2_LO)) - f)
}

/// `SearchRecords(records, query, boostFactor)`: the BM25 hits' records, re-sorted
/// stably after [`apply_confirmation_boost`] when `boost_factor > 0`.
pub fn search_records(records: &[Record], query: &str, boost_factor: f64) -> Vec<Record> {
    let mut hits = search_bm25(records, query);
    if boost_factor > 0.0 {
        for hit in &mut hits {
            hit.score = apply_confirmation_boost(hit.score, &hit.record, boost_factor);
        }
        hits.sort_by(|a, b| descending(a.score, b.score));
    }
    hits.into_iter().map(|hit| hit.record).collect()
}

/// The `status` texts of a record's outcomes (`RecordList(r["outcomes"])`).
fn outcome_statuses(record: &Record) -> impl Iterator<Item = String> + '_ {
    record_list(record.get("outcomes"))
        .into_iter()
        .map(|outcome| text(outcome.get("status")))
}

/// `SuccessCount`: outcomes with status `success`.
pub fn success_count(record: &Record) -> usize {
    outcome_statuses(record).filter(|s| s == "success").count()
}

/// `FailureCount`: outcomes with status `failure`.
pub fn failure_count(record: &Record) -> usize {
    outcome_statuses(record).filter(|s| s == "failure").count()
}

/// `TotalApplications`: the number of `outcomes` elements of any kind.
pub fn total_applications(record: &Record) -> usize {
    any_list(record.get("outcomes")).len()
}

/// `ConfirmationScore`: 1 per `success` outcome, 0.5 per `partial`.
pub fn confirmation_score(record: &Record) -> f64 {
    let mut score = 0.0;
    for status in outcome_statuses(record) {
        match status.as_str() {
            "success" => score += 1.0,
            "partial" => score += 0.5,
            _ => {}
        }
    }
    score
}

/// `SuccessRate`: confirmation score over total applications, 0 without any.
pub fn success_rate(record: &Record) -> f64 {
    let n = total_applications(record);
    if n == 0 {
        return 0.0;
    }
    confirmation_score(record) / n as f64
}

/// `ApplyConfirmationBoost(score, r, factor)`: `score * (1 + factor * confirmation)`, or
/// `score` when the confirmation score is 0.
pub fn apply_confirmation_boost(score: f64, record: &Record, factor: f64) -> f64 {
    let confirmation = confirmation_score(record);
    if confirmation == 0.0 {
        return score;
    }
    score * (1.0 + factor * confirmation)
}

/// `SortByConfirmationScore`: a stable sort by descending confirmation score (cloned).
pub fn sort_by_confirmation_score(records: &[Record]) -> Vec<Record> {
    let mut scored: Vec<(f64, &Record)> =
        records.iter().map(|r| (confirmation_score(r), r)).collect();
    scored.sort_by(|a, b| descending(a.0, b.0));
    scored.into_iter().map(|(_, r)| r.clone()).collect()
}

/// `FindNearDuplicates(existing, candidate, registry)`: among the top ten BM25 hits for
/// the candidate's body text, those not superseded by it whose token-set similarity
/// reaches 0.6 (0.45 with a shared anchor), stably sorted by descending similarity.
pub fn find_near_duplicates(
    existing: &[Record],
    candidate: &Record,
    registry: &Registry,
) -> Vec<NearDuplicate> {
    let token_set = |s: &str| {
        let long: Vec<String> = tokenize(s).into_iter().filter(|t| t.len() >= 3).collect();
        unique(&long)
    };
    let body = record_body_text(candidate, registry);
    let tokens = token_set(&body);
    if tokens.is_empty() {
        return Vec::new();
    }
    let superseded = list(candidate.get("supersedes"));
    let anchors = quality_anchors(candidate);
    let mut hits = search_bm25(existing, &body);
    hits.truncate(10);
    let mut out: Vec<NearDuplicate> = Vec::new();
    for hit in hits {
        let r = hit.record;
        if contains(&superseded, &text(r.get("id"))) {
            continue;
        }
        let other = token_set(&record_body_text(&r, registry));
        let shared = tokens.iter().filter(|t| contains(&other, t)).count();
        let mut both = tokens.clone();
        both.extend(other.iter().cloned());
        let similarity = shared as f64 / unique(&both).len() as f64;
        let shared_anchors: Vec<String> = quality_anchors(&r)
            .into_iter()
            .filter(|v| contains(&anchors, v))
            .collect();
        let threshold = if shared_anchors.is_empty() { 0.6 } else { 0.45 };
        if similarity >= threshold {
            out.push(NearDuplicate {
                record: r,
                similarity,
                shared_anchors,
            });
        }
    }
    out.sort_by(|a, b| descending(a.similarity, b.similarity));
    out
}

/// `qualityAnchors`: `files`, then `dir_anchors`, then a non-empty `evidence.file`.
fn quality_anchors(record: &Record) -> Vec<String> {
    let mut out = list(record.get("files"));
    out.extend(list(record.get("dir_anchors")));
    let file = text(obj(record.get("evidence")).get("file"));
    if !file.is_empty() {
        out.push(file);
    }
    out
}

/// The 13 words `HasRuleSignal` looks for, in Go's order.
const RULE_WORDS: [&str; 13] = [
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
];

/// `HasRuleSignal`: the [`js_lower`]ed text contains one of the 13 rule words.
pub fn has_rule_signal(content: &str) -> bool {
    let lower = js_lower(content);
    RULE_WORDS.iter().any(|word| lower.contains(word))
}

/// `CheckRecordQuality(r, autoCommit)`: Go's two warnings (an ungrounded convention or
/// pattern; a convention without a rule word).
pub fn check_record_quality(record: &Record, auto_commit: bool) -> Vec<String> {
    let mut out = Vec::new();
    let type_name = text(record.get("type"));
    // Obj(r["evidence"]): anything but an object reads as an empty one.
    let evidence_value = match record.get("evidence") {
        Some(value @ Value::Object(_)) => Some(value),
        _ => None,
    };
    let ev = obj(evidence_value);
    let set = |key: &str| !text(ev.get(key)).is_empty();
    let grounded = !quality_anchors(record).is_empty()
        || set("issue")
        || !evidence::evidence_missions(evidence_value).is_empty()
        || set("gh")
        || set("linear")
        || set("bead")
        || (set("commit") && !auto_commit);
    if (type_name == "convention" || type_name == "pattern") && !grounded {
        out.push(format!(
            "{type_name} has no file/dir anchor or evidence; add --files, --dir-anchor, or an --evidence-* flag so it can be scoped and verified."
        ));
    }
    if type_name == "convention" && !has_rule_signal(&text(record.get("content"))) {
        out.push(
            "convention reads like a restatement of code (no rule words such as \"because\", \"avoid\", \"never\", \"prefer\"); state the rule and why, or skip it."
                .to_owned(),
        );
    }
    out
}

// ---------------------------------------------------------------------------------------
// Summaries (format.go)

/// `RecordSummary(r, registry)`: the built-in types' summary field (conventions and
/// failures truncated to 60), an inherited convention or failure summary, or the custom
/// type's template with `{field}` and `{{field}}` replaced by [`format_js`] of the value.
pub fn record_summary(record: &Record, registry: &Registry) -> String {
    let type_name = text(record.get("type"));
    match type_name.as_str() {
        "convention" => return truncate(&text(record.get("content")), 60),
        "failure" => return truncate(&text(record.get("description")), 60),
        "decision" => return text(record.get("title")),
        "pattern" | "reference" | "guide" => return text(record.get("name")),
        _ => {}
    }
    let Some(definition) = registry.get(&type_name) else {
        return String::new();
    };
    if definition.summary_inherited {
        match definition.parent.as_str() {
            "convention" => return truncate(&text(record.get("content")), 60),
            "failure" => return truncate(&text(record.get("description")), 60),
            _ => {}
        }
    }
    replace_summary_tokens(&definition.summary, |key| format_js(record.get(key)))
}

/// Go's `summaryToken.ReplaceAllStringFunc` for
/// `(?i)\{\{([a-z_][a-z0-9_]*)\}\}|\{([a-z_][a-z0-9_]*)\}`: each leftmost match, the
/// double-brace form tried first, replaced by `value(name)`; other text is kept.
fn replace_summary_tokens(template: &str, value: impl Fn(&str) -> String) -> String {
    // (?i)[a-z_] folds the Kelvin sign and the long s to k and s.
    fn start(c: char) -> bool {
        c.is_ascii_alphabetic() || c == '_' || c == '\u{17F}' || c == '\u{212A}'
    }
    /// The identifier at the start of `s` when a `}` follows it.
    fn ident(s: &str) -> Option<&str> {
        let mut chars = s.char_indices();
        let (_, first) = chars.next()?;
        if !start(first) {
            return None;
        }
        let end = chars
            .find(|(_, c)| !(start(*c) || c.is_ascii_digit()))
            .map_or(s.len(), |(i, _)| i);
        s[end..].starts_with('}').then(|| &s[..end])
    }
    let mut out = String::with_capacity(template.len());
    let mut i = 0;
    while i < template.len() {
        let rest = &template[i..];
        if let Some(after) = rest.strip_prefix("{{") {
            if let Some(name) = ident(after) {
                if after[name.len() + 1..].starts_with('}') {
                    out.push_str(&value(name));
                    i += 2 + name.len() + 2;
                    continue;
                }
            }
        }
        if let Some(after) = rest.strip_prefix('{') {
            if let Some(name) = ident(after) {
                out.push_str(&value(name));
                i += 1 + name.len() + 1;
                continue;
            }
        }
        let c = rest.chars().next().unwrap_or(' ');
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// `Truncate(text, max)` in UTF-16 code units: the text if it fits; else up to its first
/// `[.!?]` followed by ASCII white space when that comes after the start and before
/// `max`; else the first `max` units (a split surrogate becomes U+FFFD) and `...`.
pub fn truncate(text: &str, max: usize) -> String {
    if !utf16_longer_than(text, max) {
        return text.to_owned();
    }
    // `[.!?]\s`, leftmost match, \s being Go's ASCII [\t\n\f\r ].
    let bytes = text.as_bytes();
    let sentence_end = bytes.windows(2).position(|pair| {
        matches!(pair[0], b'.' | b'!' | b'?')
            && matches!(pair[1], b'\t' | b'\n' | 0x0C | b'\r' | b' ')
    });
    if let Some(at) = sentence_end {
        // formatLen(text[:at]) < max.
        if at > 0 && max > 0 && !utf16_longer_than(&text[..at], max - 1) {
            return text[..=at].to_owned();
        }
    }
    let mut out = utf16_prefix(text, max);
    out.push_str("...");
    out
}

/// Whether `text` is longer than `max` UTF-16 code units (stops counting past `max`).
fn utf16_longer_than(text: &str, max: usize) -> bool {
    let mut units = 0;
    for c in text.chars() {
        units += c.len_utf16();
        if units > max {
            return true;
        }
    }
    false
}

/// `formatSlice(text, n)`: the first `n` UTF-16 code units, decoded; half of a split
/// surrogate pair becomes U+FFFD, as Go's `utf16.Decode` makes it.
fn utf16_prefix(text: &str, n: usize) -> String {
    let mut out = String::new();
    let mut units = 0;
    for c in text.chars() {
        let width = c.len_utf16();
        if units + width > n {
            if units < n {
                out.push('\u{FFFD}');
            }
            break;
        }
        out.push(c);
        units += width;
    }
    out
}

/// `formatJS(v)`: a value as JavaScript's `String()` would print it in the Go port:
/// absent or null `""`, arrays joined with `,`, objects `[object Object]`, numbers as
/// Go's `%v` (`1e+21`, `1.5`).
pub fn format_js(value: Option<&Value>) -> String {
    joined(value, go_format_v)
}

/// `Text(v)` (`model.go`) without recursion: [`go_text`] for every number, arrays joined
/// with `,` at every level.
fn text(value: Option<&Value>) -> String {
    joined(value, |n| go_text(Some(&Value::Number(n))))
}

/// Go's `Text` and `formatJS` share their shape: `""` for nil, strings as they are,
/// `true`/`false`, integers in decimal, `[object Object]` for objects, and arrays as
/// their elements' texts joined with `,` (recursively in Go; here with an explicit
/// stack). They differ only in how a `float64` prints.
fn joined(value: Option<&Value>, float: impl Fn(f64) -> String) -> String {
    let Some(value) = value else {
        return String::new();
    };
    let mut out = String::new();
    let scalar = |out: &mut String, v: &Value| match v {
        Value::Null | Value::Array(_) => {}
        Value::String(s) => out.push_str(s),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&float(*n)),
        // An `int` or `uint64` from config.yaml prints its decimal digits in both.
        Value::Integer(i) => out.push_str(&i.to_string()),
        Value::Object(_) => out.push_str("[object Object]"),
    };
    let Value::Array(items) = value else {
        scalar(&mut out, value);
        return out;
    };
    // Each frame: the array's remaining elements and whether one was written already.
    let mut stack: Vec<(std::slice::Iter<Value>, bool)> = vec![(items.iter(), false)];
    while let Some((iter, started)) = stack.last_mut() {
        let Some(item) = iter.next() else {
            stack.pop();
            continue;
        };
        if *started {
            out.push(',');
        }
        *started = true;
        match item {
            Value::Array(inner) => stack.push((inner.iter(), false)),
            other => scalar(&mut out, other),
        }
    }
    out
}

/// Go's `fmt.Sprint` of a `float64` (`%v`, that is `strconv.FormatFloat(n, 'g', -1,
/// 64)`): the shortest digits that read back as `n`; exponent form (`1e+06`,
/// `1.234567e+06`, `1e-07`, at least two exponent digits) when the decimal exponent is
/// below -4 or at least 6, else plain (`123456`, `0.0001`); `NaN`, `+Inf`, `-Inf`.
/// Checked against the pinned Go (t3 build log).
fn go_format_v(n: f64) -> String {
    if n.is_nan() {
        return "NaN".to_owned();
    }
    if n.is_infinite() {
        return if n > 0.0 { "+Inf" } else { "-Inf" }.to_owned();
    }
    let sign = if n.is_sign_negative() { "-" } else { "" };
    // Rust's `{:e}` gives the same shortest round-trip digits as Go, as `d.ddde<exp>`.
    let scientific = format!("{:e}", n.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    if (-4..6).contains(&exponent) {
        // Plain: Rust's `Display` prints the same shortest digits without an exponent.
        return format!("{sign}{}", n.abs());
    }
    let exp_sign = if exponent < 0 { '-' } else { '+' };
    format!("{sign}{mantissa}e{exp_sign}{:02}", exponent.unsigned_abs())
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
