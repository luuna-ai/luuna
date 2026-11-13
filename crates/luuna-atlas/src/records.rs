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
pub(crate) fn go_deep_equal(a: &Value, b: &Value) -> bool {
    let mut pending: Vec<(&Value, &Value)> = vec![(a, b)];
    while let Some((x, y)) = pending.pop() {
        if let (Some(x), Some(y)) = (x.string_bytes(), y.string_bytes()) {
            if x != y {
                return false;
            }
            continue;
        }
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
                        match q.get_go_key(p.key_bytes(key)) {
                            Some(other) => pending.push((value, other)),
                            None => return false,
                        }
                    }
                } else {
                    // A lookup table keeps large objects linear.
                    let index: HashMap<&[u8], &Value> = q
                        .iter()
                        .map(|(key, value)| (q.key_bytes(key), value))
                        .collect();
                    for (key, value) in p.iter() {
                        match index.get(p.key_bytes(key)) {
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
        Value::RawString(s) => out.push_str(s.text()),
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

// ---------------------------------------------------------------------------------------
// Additional command-layer preservation checks (s5-atlas-lib-2b).

/// Refuse normalization of a retained negative zero. Pairs associate the same
/// record before and after the requested mutation; deleted records have no pair.
/// A removed or explicitly replaced value is not retained. Traversal is iterative
/// and compares only primitive float signs, never recursive Value equality.
pub fn check_retained_numbers<'a>(
    path: &std::path::Path,
    pairs: impl IntoIterator<Item = (&'a Record, &'a Record)>,
) -> crate::Result<()> {
    for (index, (before, after)) in pairs.into_iter().enumerate() {
        let mut stack: Vec<(&Value, &Value)> = before
            .iter()
            .filter_map(|(key, value)| after.get(key).map(|next| (value, next)))
            .collect();
        while let Some((before, after)) = stack.pop() {
            match (before, after) {
                (Value::Number(a), Value::Number(b))
                    if *a == 0.0 && a.is_sign_negative() && *b == 0.0 && b.is_sign_negative() =>
                {
                    return Err(crate::AtlasError::Message(format!(
                        "Refusing to rewrite {}: retained record {} contains negative zero that would become positive zero. The file is unchanged.",
                        path.display(), index + 1
                    )));
                }
                (Value::Array(a), Value::Array(b)) => stack.extend(a.iter().zip(b)),
                (Value::Object(a), Value::Object(b)) => stack.extend(
                    a.iter()
                        .filter_map(|(key, value)| b.get(key).map(|next| (value, next))),
                ),
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod retained_number_checks {
    use super::*;
    #[test]
    #[ignore = "hostile input; run under the documented address-space and time limits"]
    fn hostile_retained_zero_walk_is_iterative() {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                let text = format!("{{\"extra\":{}-0{}}}", "[".repeat(9999), "]".repeat(9999));
                assert!(text.len() < 64 * 1024);
                let parsed = crate::json::parse(&text).unwrap();
                let before = parsed.as_object().unwrap().clone();
                let mut after = before.clone();
                let path = std::path::Path::new("fixture.jsonl");
                assert!(check_retained_numbers(path, [(&before, &after)]).is_err());
                after.remove("extra");
                check_retained_numbers(path, [(&before, &after)]).unwrap();
            })
            .unwrap()
            .join()
            .unwrap();
    }
}

/// Go strconv.Quote, including Unicode printability from the pinned Go version.
/// This corrects the inherited D10 gap without changing the frozen foundation helper.
pub fn go_quote(text: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{b}' => out.push_str("\\v"),
            ch => {
                let code = ch as u32;
                let printable = GO_PRINTABLE_RANGES
                    .binary_search_by(|&(lo, hi)| {
                        if code < lo {
                            std::cmp::Ordering::Greater
                        } else if code > hi {
                            std::cmp::Ordering::Less
                        } else {
                            std::cmp::Ordering::Equal
                        }
                    })
                    .is_ok();
                if printable {
                    out.push(ch);
                } else if code < 0x80 {
                    write!(out, "\\x{code:02x}").unwrap();
                } else if code < 0x10000 {
                    write!(out, "\\u{code:04x}").unwrap();
                } else {
                    write!(out, "\\U{code:08x}").unwrap();
                }
            }
        }
    }
    out.push('"');
    out
}

// Generated from go1.27.0 unicode.IsPrint (Unicode 17.0.0).
const GO_PRINTABLE_RANGES: &[(u32, u32)] = &[
    (0x20, 0x7e),
    (0xa1, 0xac),
    (0xae, 0x377),
    (0x37a, 0x37f),
    (0x384, 0x38a),
    (0x38c, 0x38c),
    (0x38e, 0x3a1),
    (0x3a3, 0x52f),
    (0x531, 0x556),
    (0x559, 0x58a),
    (0x58d, 0x58f),
    (0x591, 0x5c7),
    (0x5d0, 0x5ea),
    (0x5ef, 0x5f4),
    (0x606, 0x61b),
    (0x61d, 0x6dc),
    (0x6de, 0x70d),
    (0x710, 0x74a),
    (0x74d, 0x7b1),
    (0x7c0, 0x7fa),
    (0x7fd, 0x82d),
    (0x830, 0x83e),
    (0x840, 0x85b),
    (0x85e, 0x85e),
    (0x860, 0x86a),
    (0x870, 0x88f),
    (0x897, 0x8e1),
    (0x8e3, 0x983),
    (0x985, 0x98c),
    (0x98f, 0x990),
    (0x993, 0x9a8),
    (0x9aa, 0x9b0),
    (0x9b2, 0x9b2),
    (0x9b6, 0x9b9),
    (0x9bc, 0x9c4),
    (0x9c7, 0x9c8),
    (0x9cb, 0x9ce),
    (0x9d7, 0x9d7),
    (0x9dc, 0x9dd),
    (0x9df, 0x9e3),
    (0x9e6, 0x9fe),
    (0xa01, 0xa03),
    (0xa05, 0xa0a),
    (0xa0f, 0xa10),
    (0xa13, 0xa28),
    (0xa2a, 0xa30),
    (0xa32, 0xa33),
    (0xa35, 0xa36),
    (0xa38, 0xa39),
    (0xa3c, 0xa3c),
    (0xa3e, 0xa42),
    (0xa47, 0xa48),
    (0xa4b, 0xa4d),
    (0xa51, 0xa51),
    (0xa59, 0xa5c),
    (0xa5e, 0xa5e),
    (0xa66, 0xa76),
    (0xa81, 0xa83),
    (0xa85, 0xa8d),
    (0xa8f, 0xa91),
    (0xa93, 0xaa8),
    (0xaaa, 0xab0),
    (0xab2, 0xab3),
    (0xab5, 0xab9),
    (0xabc, 0xac5),
    (0xac7, 0xac9),
    (0xacb, 0xacd),
    (0xad0, 0xad0),
    (0xae0, 0xae3),
    (0xae6, 0xaf1),
    (0xaf9, 0xaff),
    (0xb01, 0xb03),
    (0xb05, 0xb0c),
    (0xb0f, 0xb10),
    (0xb13, 0xb28),
    (0xb2a, 0xb30),
    (0xb32, 0xb33),
    (0xb35, 0xb39),
    (0xb3c, 0xb44),
    (0xb47, 0xb48),
    (0xb4b, 0xb4d),
    (0xb55, 0xb57),
    (0xb5c, 0xb5d),
    (0xb5f, 0xb63),
    (0xb66, 0xb77),
    (0xb82, 0xb83),
    (0xb85, 0xb8a),
    (0xb8e, 0xb90),
    (0xb92, 0xb95),
    (0xb99, 0xb9a),
    (0xb9c, 0xb9c),
    (0xb9e, 0xb9f),
    (0xba3, 0xba4),
    (0xba8, 0xbaa),
    (0xbae, 0xbb9),
    (0xbbe, 0xbc2),
    (0xbc6, 0xbc8),
    (0xbca, 0xbcd),
    (0xbd0, 0xbd0),
    (0xbd7, 0xbd7),
    (0xbe6, 0xbfa),
    (0xc00, 0xc0c),
    (0xc0e, 0xc10),
    (0xc12, 0xc28),
    (0xc2a, 0xc39),
    (0xc3c, 0xc44),
    (0xc46, 0xc48),
    (0xc4a, 0xc4d),
    (0xc55, 0xc56),
    (0xc58, 0xc5a),
    (0xc5c, 0xc5d),
    (0xc60, 0xc63),
    (0xc66, 0xc6f),
    (0xc77, 0xc8c),
    (0xc8e, 0xc90),
    (0xc92, 0xca8),
    (0xcaa, 0xcb3),
    (0xcb5, 0xcb9),
    (0xcbc, 0xcc4),
    (0xcc6, 0xcc8),
    (0xcca, 0xccd),
    (0xcd5, 0xcd6),
    (0xcdc, 0xcde),
    (0xce0, 0xce3),
    (0xce6, 0xcef),
    (0xcf1, 0xcf3),
    (0xd00, 0xd0c),
    (0xd0e, 0xd10),
    (0xd12, 0xd44),
    (0xd46, 0xd48),
    (0xd4a, 0xd4f),
    (0xd54, 0xd63),
    (0xd66, 0xd7f),
    (0xd81, 0xd83),
    (0xd85, 0xd96),
    (0xd9a, 0xdb1),
    (0xdb3, 0xdbb),
    (0xdbd, 0xdbd),
    (0xdc0, 0xdc6),
    (0xdca, 0xdca),
    (0xdcf, 0xdd4),
    (0xdd6, 0xdd6),
    (0xdd8, 0xddf),
    (0xde6, 0xdef),
    (0xdf2, 0xdf4),
    (0xe01, 0xe3a),
    (0xe3f, 0xe5b),
    (0xe81, 0xe82),
    (0xe84, 0xe84),
    (0xe86, 0xe8a),
    (0xe8c, 0xea3),
    (0xea5, 0xea5),
    (0xea7, 0xebd),
    (0xec0, 0xec4),
    (0xec6, 0xec6),
    (0xec8, 0xece),
    (0xed0, 0xed9),
    (0xedc, 0xedf),
    (0xf00, 0xf47),
    (0xf49, 0xf6c),
    (0xf71, 0xf97),
    (0xf99, 0xfbc),
    (0xfbe, 0xfcc),
    (0xfce, 0xfda),
    (0x1000, 0x10c5),
    (0x10c7, 0x10c7),
    (0x10cd, 0x10cd),
    (0x10d0, 0x1248),
    (0x124a, 0x124d),
    (0x1250, 0x1256),
    (0x1258, 0x1258),
    (0x125a, 0x125d),
    (0x1260, 0x1288),
    (0x128a, 0x128d),
    (0x1290, 0x12b0),
    (0x12b2, 0x12b5),
    (0x12b8, 0x12be),
    (0x12c0, 0x12c0),
    (0x12c2, 0x12c5),
    (0x12c8, 0x12d6),
    (0x12d8, 0x1310),
    (0x1312, 0x1315),
    (0x1318, 0x135a),
    (0x135d, 0x137c),
    (0x1380, 0x1399),
    (0x13a0, 0x13f5),
    (0x13f8, 0x13fd),
    (0x1400, 0x167f),
    (0x1681, 0x169c),
    (0x16a0, 0x16f8),
    (0x1700, 0x1715),
    (0x171f, 0x1736),
    (0x1740, 0x1753),
    (0x1760, 0x176c),
    (0x176e, 0x1770),
    (0x1772, 0x1773),
    (0x1780, 0x17dd),
    (0x17e0, 0x17e9),
    (0x17f0, 0x17f9),
    (0x1800, 0x180d),
    (0x180f, 0x1819),
    (0x1820, 0x1878),
    (0x1880, 0x18aa),
    (0x18b0, 0x18f5),
    (0x1900, 0x191e),
    (0x1920, 0x192b),
    (0x1930, 0x193b),
    (0x1940, 0x1940),
    (0x1944, 0x196d),
    (0x1970, 0x1974),
    (0x1980, 0x19ab),
    (0x19b0, 0x19c9),
    (0x19d0, 0x19da),
    (0x19de, 0x1a1b),
    (0x1a1e, 0x1a5e),
    (0x1a60, 0x1a7c),
    (0x1a7f, 0x1a89),
    (0x1a90, 0x1a99),
    (0x1aa0, 0x1aad),
    (0x1ab0, 0x1add),
    (0x1ae0, 0x1aeb),
    (0x1b00, 0x1b4c),
    (0x1b4e, 0x1bf3),
    (0x1bfc, 0x1c37),
    (0x1c3b, 0x1c49),
    (0x1c4d, 0x1c8a),
    (0x1c90, 0x1cba),
    (0x1cbd, 0x1cc7),
    (0x1cd0, 0x1cfa),
    (0x1d00, 0x1f15),
    (0x1f18, 0x1f1d),
    (0x1f20, 0x1f45),
    (0x1f48, 0x1f4d),
    (0x1f50, 0x1f57),
    (0x1f59, 0x1f59),
    (0x1f5b, 0x1f5b),
    (0x1f5d, 0x1f5d),
    (0x1f5f, 0x1f7d),
    (0x1f80, 0x1fb4),
    (0x1fb6, 0x1fc4),
    (0x1fc6, 0x1fd3),
    (0x1fd6, 0x1fdb),
    (0x1fdd, 0x1fef),
    (0x1ff2, 0x1ff4),
    (0x1ff6, 0x1ffe),
    (0x2010, 0x2027),
    (0x2030, 0x205e),
    (0x2070, 0x2071),
    (0x2074, 0x208e),
    (0x2090, 0x209c),
    (0x20a0, 0x20c1),
    (0x20d0, 0x20f0),
    (0x2100, 0x218b),
    (0x2190, 0x2429),
    (0x2440, 0x244a),
    (0x2460, 0x2b73),
    (0x2b76, 0x2cf3),
    (0x2cf9, 0x2d25),
    (0x2d27, 0x2d27),
    (0x2d2d, 0x2d2d),
    (0x2d30, 0x2d67),
    (0x2d6f, 0x2d70),
    (0x2d7f, 0x2d96),
    (0x2da0, 0x2da6),
    (0x2da8, 0x2dae),
    (0x2db0, 0x2db6),
    (0x2db8, 0x2dbe),
    (0x2dc0, 0x2dc6),
    (0x2dc8, 0x2dce),
    (0x2dd0, 0x2dd6),
    (0x2dd8, 0x2dde),
    (0x2de0, 0x2e5d),
    (0x2e80, 0x2e99),
    (0x2e9b, 0x2ef3),
    (0x2f00, 0x2fd5),
    (0x2ff0, 0x2fff),
    (0x3001, 0x303f),
    (0x3041, 0x3096),
    (0x3099, 0x30ff),
    (0x3105, 0x312f),
    (0x3131, 0x318e),
    (0x3190, 0x31e5),
    (0x31ef, 0x321e),
    (0x3220, 0xa48c),
    (0xa490, 0xa4c6),
    (0xa4d0, 0xa62b),
    (0xa640, 0xa6f7),
    (0xa700, 0xa7dc),
    (0xa7f1, 0xa82c),
    (0xa830, 0xa839),
    (0xa840, 0xa877),
    (0xa880, 0xa8c5),
    (0xa8ce, 0xa8d9),
    (0xa8e0, 0xa953),
    (0xa95f, 0xa97c),
    (0xa980, 0xa9cd),
    (0xa9cf, 0xa9d9),
    (0xa9de, 0xa9fe),
    (0xaa00, 0xaa36),
    (0xaa40, 0xaa4d),
    (0xaa50, 0xaa59),
    (0xaa5c, 0xaac2),
    (0xaadb, 0xaaf6),
    (0xab01, 0xab06),
    (0xab09, 0xab0e),
    (0xab11, 0xab16),
    (0xab20, 0xab26),
    (0xab28, 0xab2e),
    (0xab30, 0xab6b),
    (0xab70, 0xabed),
    (0xabf0, 0xabf9),
    (0xac00, 0xd7a3),
    (0xd7b0, 0xd7c6),
    (0xd7cb, 0xd7fb),
    (0xf900, 0xfa6d),
    (0xfa70, 0xfad9),
    (0xfb00, 0xfb06),
    (0xfb13, 0xfb17),
    (0xfb1d, 0xfb36),
    (0xfb38, 0xfb3c),
    (0xfb3e, 0xfb3e),
    (0xfb40, 0xfb41),
    (0xfb43, 0xfb44),
    (0xfb46, 0xfdcf),
    (0xfdf0, 0xfe19),
    (0xfe20, 0xfe52),
    (0xfe54, 0xfe66),
    (0xfe68, 0xfe6b),
    (0xfe70, 0xfe74),
    (0xfe76, 0xfefc),
    (0xff01, 0xffbe),
    (0xffc2, 0xffc7),
    (0xffca, 0xffcf),
    (0xffd2, 0xffd7),
    (0xffda, 0xffdc),
    (0xffe0, 0xffe6),
    (0xffe8, 0xffee),
    (0xfffc, 0xfffd),
    (0x10000, 0x1000b),
    (0x1000d, 0x10026),
    (0x10028, 0x1003a),
    (0x1003c, 0x1003d),
    (0x1003f, 0x1004d),
    (0x10050, 0x1005d),
    (0x10080, 0x100fa),
    (0x10100, 0x10102),
    (0x10107, 0x10133),
    (0x10137, 0x1018e),
    (0x10190, 0x1019c),
    (0x101a0, 0x101a0),
    (0x101d0, 0x101fd),
    (0x10280, 0x1029c),
    (0x102a0, 0x102d0),
    (0x102e0, 0x102fb),
    (0x10300, 0x10323),
    (0x1032d, 0x1034a),
    (0x10350, 0x1037a),
    (0x10380, 0x1039d),
    (0x1039f, 0x103c3),
    (0x103c8, 0x103d5),
    (0x10400, 0x1049d),
    (0x104a0, 0x104a9),
    (0x104b0, 0x104d3),
    (0x104d8, 0x104fb),
    (0x10500, 0x10527),
    (0x10530, 0x10563),
    (0x1056f, 0x1057a),
    (0x1057c, 0x1058a),
    (0x1058c, 0x10592),
    (0x10594, 0x10595),
    (0x10597, 0x105a1),
    (0x105a3, 0x105b1),
    (0x105b3, 0x105b9),
    (0x105bb, 0x105bc),
    (0x105c0, 0x105f3),
    (0x10600, 0x10736),
    (0x10740, 0x10755),
    (0x10760, 0x10767),
    (0x10780, 0x10785),
    (0x10787, 0x107b0),
    (0x107b2, 0x107ba),
    (0x10800, 0x10805),
    (0x10808, 0x10808),
    (0x1080a, 0x10835),
    (0x10837, 0x10838),
    (0x1083c, 0x1083c),
    (0x1083f, 0x10855),
    (0x10857, 0x1089e),
    (0x108a7, 0x108af),
    (0x108e0, 0x108f2),
    (0x108f4, 0x108f5),
    (0x108fb, 0x1091b),
    (0x1091f, 0x10939),
    (0x1093f, 0x10959),
    (0x10980, 0x109b7),
    (0x109bc, 0x109cf),
    (0x109d2, 0x10a03),
    (0x10a05, 0x10a06),
    (0x10a0c, 0x10a13),
    (0x10a15, 0x10a17),
    (0x10a19, 0x10a35),
    (0x10a38, 0x10a3a),
    (0x10a3f, 0x10a48),
    (0x10a50, 0x10a58),
    (0x10a60, 0x10a9f),
    (0x10ac0, 0x10ae6),
    (0x10aeb, 0x10af6),
    (0x10b00, 0x10b35),
    (0x10b39, 0x10b55),
    (0x10b58, 0x10b72),
    (0x10b78, 0x10b91),
    (0x10b99, 0x10b9c),
    (0x10ba9, 0x10baf),
    (0x10c00, 0x10c48),
    (0x10c80, 0x10cb2),
    (0x10cc0, 0x10cf2),
    (0x10cfa, 0x10d27),
    (0x10d30, 0x10d39),
    (0x10d40, 0x10d65),
    (0x10d69, 0x10d85),
    (0x10d8e, 0x10d8f),
    (0x10e60, 0x10e7e),
    (0x10e80, 0x10ea9),
    (0x10eab, 0x10ead),
    (0x10eb0, 0x10eb1),
    (0x10ec2, 0x10ec7),
    (0x10ed0, 0x10ed8),
    (0x10efa, 0x10f27),
    (0x10f30, 0x10f59),
    (0x10f70, 0x10f89),
    (0x10fb0, 0x10fcb),
    (0x10fe0, 0x10ff6),
    (0x11000, 0x1104d),
    (0x11052, 0x11075),
    (0x1107f, 0x110bc),
    (0x110be, 0x110c2),
    (0x110d0, 0x110e8),
    (0x110f0, 0x110f9),
    (0x11100, 0x11134),
    (0x11136, 0x11147),
    (0x11150, 0x11176),
    (0x11180, 0x111df),
    (0x111e1, 0x111f4),
    (0x11200, 0x11211),
    (0x11213, 0x11241),
    (0x11280, 0x11286),
    (0x11288, 0x11288),
    (0x1128a, 0x1128d),
    (0x1128f, 0x1129d),
    (0x1129f, 0x112a9),
    (0x112b0, 0x112ea),
    (0x112f0, 0x112f9),
    (0x11300, 0x11303),
    (0x11305, 0x1130c),
    (0x1130f, 0x11310),
    (0x11313, 0x11328),
    (0x1132a, 0x11330),
    (0x11332, 0x11333),
    (0x11335, 0x11339),
    (0x1133b, 0x11344),
    (0x11347, 0x11348),
    (0x1134b, 0x1134d),
    (0x11350, 0x11350),
    (0x11357, 0x11357),
    (0x1135d, 0x11363),
    (0x11366, 0x1136c),
    (0x11370, 0x11374),
    (0x11380, 0x11389),
    (0x1138b, 0x1138b),
    (0x1138e, 0x1138e),
    (0x11390, 0x113b5),
    (0x113b7, 0x113c0),
    (0x113c2, 0x113c2),
    (0x113c5, 0x113c5),
    (0x113c7, 0x113ca),
    (0x113cc, 0x113d5),
    (0x113d7, 0x113d8),
    (0x113e1, 0x113e2),
    (0x11400, 0x1145b),
    (0x1145d, 0x11461),
    (0x11480, 0x114c7),
    (0x114d0, 0x114d9),
    (0x11580, 0x115b5),
    (0x115b8, 0x115dd),
    (0x11600, 0x11644),
    (0x11650, 0x11659),
    (0x11660, 0x1166c),
    (0x11680, 0x116b9),
    (0x116c0, 0x116c9),
    (0x116d0, 0x116e3),
    (0x11700, 0x1171a),
    (0x1171d, 0x1172b),
    (0x11730, 0x11746),
    (0x11800, 0x1183b),
    (0x118a0, 0x118f2),
    (0x118ff, 0x11906),
    (0x11909, 0x11909),
    (0x1190c, 0x11913),
    (0x11915, 0x11916),
    (0x11918, 0x11935),
    (0x11937, 0x11938),
    (0x1193b, 0x11946),
    (0x11950, 0x11959),
    (0x119a0, 0x119a7),
    (0x119aa, 0x119d7),
    (0x119da, 0x119e4),
    (0x11a00, 0x11a47),
    (0x11a50, 0x11aa2),
    (0x11ab0, 0x11af8),
    (0x11b00, 0x11b09),
    (0x11b60, 0x11b67),
    (0x11bc0, 0x11be1),
    (0x11bf0, 0x11bf9),
    (0x11c00, 0x11c08),
    (0x11c0a, 0x11c36),
    (0x11c38, 0x11c45),
    (0x11c50, 0x11c6c),
    (0x11c70, 0x11c8f),
    (0x11c92, 0x11ca7),
    (0x11ca9, 0x11cb6),
    (0x11d00, 0x11d06),
    (0x11d08, 0x11d09),
    (0x11d0b, 0x11d36),
    (0x11d3a, 0x11d3a),
    (0x11d3c, 0x11d3d),
    (0x11d3f, 0x11d47),
    (0x11d50, 0x11d59),
    (0x11d60, 0x11d65),
    (0x11d67, 0x11d68),
    (0x11d6a, 0x11d8e),
    (0x11d90, 0x11d91),
    (0x11d93, 0x11d98),
    (0x11da0, 0x11da9),
    (0x11db0, 0x11ddb),
    (0x11de0, 0x11de9),
    (0x11ee0, 0x11ef8),
    (0x11f00, 0x11f10),
    (0x11f12, 0x11f3a),
    (0x11f3e, 0x11f5a),
    (0x11fb0, 0x11fb0),
    (0x11fc0, 0x11ff1),
    (0x11fff, 0x12399),
    (0x12400, 0x1246e),
    (0x12470, 0x12474),
    (0x12480, 0x12543),
    (0x12f90, 0x12ff2),
    (0x13000, 0x1342f),
    (0x13440, 0x13455),
    (0x13460, 0x143fa),
    (0x14400, 0x14646),
    (0x16100, 0x16139),
    (0x16800, 0x16a38),
    (0x16a40, 0x16a5e),
    (0x16a60, 0x16a69),
    (0x16a6e, 0x16abe),
    (0x16ac0, 0x16ac9),
    (0x16ad0, 0x16aed),
    (0x16af0, 0x16af5),
    (0x16b00, 0x16b45),
    (0x16b50, 0x16b59),
    (0x16b5b, 0x16b61),
    (0x16b63, 0x16b77),
    (0x16b7d, 0x16b8f),
    (0x16d40, 0x16d79),
    (0x16e40, 0x16e9a),
    (0x16ea0, 0x16eb8),
    (0x16ebb, 0x16ed3),
    (0x16f00, 0x16f4a),
    (0x16f4f, 0x16f87),
    (0x16f8f, 0x16f9f),
    (0x16fe0, 0x16fe4),
    (0x16ff0, 0x16ff6),
    (0x17000, 0x18cd5),
    (0x18cff, 0x18d1e),
    (0x18d80, 0x18df2),
    (0x1aff0, 0x1aff3),
    (0x1aff5, 0x1affb),
    (0x1affd, 0x1affe),
    (0x1b000, 0x1b122),
    (0x1b132, 0x1b132),
    (0x1b150, 0x1b152),
    (0x1b155, 0x1b155),
    (0x1b164, 0x1b167),
    (0x1b170, 0x1b2fb),
    (0x1bc00, 0x1bc6a),
    (0x1bc70, 0x1bc7c),
    (0x1bc80, 0x1bc88),
    (0x1bc90, 0x1bc99),
    (0x1bc9c, 0x1bc9f),
    (0x1cc00, 0x1ccfc),
    (0x1cd00, 0x1ceb3),
    (0x1ceba, 0x1ced0),
    (0x1cee0, 0x1cef0),
    (0x1cf00, 0x1cf2d),
    (0x1cf30, 0x1cf46),
    (0x1cf50, 0x1cfc3),
    (0x1d000, 0x1d0f5),
    (0x1d100, 0x1d126),
    (0x1d129, 0x1d172),
    (0x1d17b, 0x1d1ea),
    (0x1d200, 0x1d245),
    (0x1d2c0, 0x1d2d3),
    (0x1d2e0, 0x1d2f3),
    (0x1d300, 0x1d356),
    (0x1d360, 0x1d378),
    (0x1d400, 0x1d454),
    (0x1d456, 0x1d49c),
    (0x1d49e, 0x1d49f),
    (0x1d4a2, 0x1d4a2),
    (0x1d4a5, 0x1d4a6),
    (0x1d4a9, 0x1d4ac),
    (0x1d4ae, 0x1d4b9),
    (0x1d4bb, 0x1d4bb),
    (0x1d4bd, 0x1d4c3),
    (0x1d4c5, 0x1d505),
    (0x1d507, 0x1d50a),
    (0x1d50d, 0x1d514),
    (0x1d516, 0x1d51c),
    (0x1d51e, 0x1d539),
    (0x1d53b, 0x1d53e),
    (0x1d540, 0x1d544),
    (0x1d546, 0x1d546),
    (0x1d54a, 0x1d550),
    (0x1d552, 0x1d6a5),
    (0x1d6a8, 0x1d7cb),
    (0x1d7ce, 0x1da8b),
    (0x1da9b, 0x1da9f),
    (0x1daa1, 0x1daaf),
    (0x1df00, 0x1df1e),
    (0x1df25, 0x1df2a),
    (0x1e000, 0x1e006),
    (0x1e008, 0x1e018),
    (0x1e01b, 0x1e021),
    (0x1e023, 0x1e024),
    (0x1e026, 0x1e02a),
    (0x1e030, 0x1e06d),
    (0x1e08f, 0x1e08f),
    (0x1e100, 0x1e12c),
    (0x1e130, 0x1e13d),
    (0x1e140, 0x1e149),
    (0x1e14e, 0x1e14f),
    (0x1e290, 0x1e2ae),
    (0x1e2c0, 0x1e2f9),
    (0x1e2ff, 0x1e2ff),
    (0x1e4d0, 0x1e4f9),
    (0x1e5d0, 0x1e5fa),
    (0x1e5ff, 0x1e5ff),
    (0x1e6c0, 0x1e6de),
    (0x1e6e0, 0x1e6f5),
    (0x1e6fe, 0x1e6ff),
    (0x1e7e0, 0x1e7e6),
    (0x1e7e8, 0x1e7eb),
    (0x1e7ed, 0x1e7ee),
    (0x1e7f0, 0x1e7fe),
    (0x1e800, 0x1e8c4),
    (0x1e8c7, 0x1e8d6),
    (0x1e900, 0x1e94b),
    (0x1e950, 0x1e959),
    (0x1e95e, 0x1e95f),
    (0x1ec71, 0x1ecb4),
    (0x1ed01, 0x1ed3d),
    (0x1ee00, 0x1ee03),
    (0x1ee05, 0x1ee1f),
    (0x1ee21, 0x1ee22),
    (0x1ee24, 0x1ee24),
    (0x1ee27, 0x1ee27),
    (0x1ee29, 0x1ee32),
    (0x1ee34, 0x1ee37),
    (0x1ee39, 0x1ee39),
    (0x1ee3b, 0x1ee3b),
    (0x1ee42, 0x1ee42),
    (0x1ee47, 0x1ee47),
    (0x1ee49, 0x1ee49),
    (0x1ee4b, 0x1ee4b),
    (0x1ee4d, 0x1ee4f),
    (0x1ee51, 0x1ee52),
    (0x1ee54, 0x1ee54),
    (0x1ee57, 0x1ee57),
    (0x1ee59, 0x1ee59),
    (0x1ee5b, 0x1ee5b),
    (0x1ee5d, 0x1ee5d),
    (0x1ee5f, 0x1ee5f),
    (0x1ee61, 0x1ee62),
    (0x1ee64, 0x1ee64),
    (0x1ee67, 0x1ee6a),
    (0x1ee6c, 0x1ee72),
    (0x1ee74, 0x1ee77),
    (0x1ee79, 0x1ee7c),
    (0x1ee7e, 0x1ee7e),
    (0x1ee80, 0x1ee89),
    (0x1ee8b, 0x1ee9b),
    (0x1eea1, 0x1eea3),
    (0x1eea5, 0x1eea9),
    (0x1eeab, 0x1eebb),
    (0x1eef0, 0x1eef1),
    (0x1f000, 0x1f02b),
    (0x1f030, 0x1f093),
    (0x1f0a0, 0x1f0ae),
    (0x1f0b1, 0x1f0bf),
    (0x1f0c1, 0x1f0cf),
    (0x1f0d1, 0x1f0f5),
    (0x1f100, 0x1f1ad),
    (0x1f1e6, 0x1f202),
    (0x1f210, 0x1f23b),
    (0x1f240, 0x1f248),
    (0x1f250, 0x1f251),
    (0x1f260, 0x1f265),
    (0x1f300, 0x1f6d8),
    (0x1f6dc, 0x1f6ec),
    (0x1f6f0, 0x1f6fc),
    (0x1f700, 0x1f7d9),
    (0x1f7e0, 0x1f7eb),
    (0x1f7f0, 0x1f7f0),
    (0x1f800, 0x1f80b),
    (0x1f810, 0x1f847),
    (0x1f850, 0x1f859),
    (0x1f860, 0x1f887),
    (0x1f890, 0x1f8ad),
    (0x1f8b0, 0x1f8bb),
    (0x1f8c0, 0x1f8c1),
    (0x1f8d0, 0x1f8d8),
    (0x1f900, 0x1fa57),
    (0x1fa60, 0x1fa6d),
    (0x1fa70, 0x1fa7c),
    (0x1fa80, 0x1fa8a),
    (0x1fa8e, 0x1fac6),
    (0x1fac8, 0x1fac8),
    (0x1facd, 0x1fadc),
    (0x1fadf, 0x1faea),
    (0x1faef, 0x1faf8),
    (0x1fb00, 0x1fb92),
    (0x1fb94, 0x1fbfa),
    (0x20000, 0x2a6df),
    (0x2a700, 0x2b81d),
    (0x2b820, 0x2cead),
    (0x2ceb0, 0x2ebe0),
    (0x2ebf0, 0x2ee5d),
    (0x2f800, 0x2fa1d),
    (0x30000, 0x3134a),
    (0x31350, 0x33479),
    (0xe0100, 0xe01ef),
];

/// The Go Text conversion with an explicit stack, also for nested user arrays.
pub fn text_value(value: Option<&Value>) -> String {
    text(value)
}

/// Check an implicit outcomes append/merge before it filters stored values.
/// Objects-only matches the CLI merge; the typed API keeps all array elements.
pub fn check_outcomes_collection(
    path: &std::path::Path,
    record: &Record,
    objects_only: bool,
) -> crate::Result<()> {
    let no_data =
        |value: &Value| value.is_null() || matches!(value,Value::String(s) if s.is_empty());
    let loses = match record.get("outcomes") {
        None => false,
        Some(Value::Array(values)) => {
            objects_only
                && values
                    .iter()
                    .any(|v| !matches!(v, Value::Object(_)) && !no_data(v))
        }
        Some(value) => !no_data(value),
    };
    if loses {
        Err(crate::AtlasError::Message(format!(
            "Refusing to rewrite {}: stored outcomes would lose a value during append or merge. The file is unchanged.",
            path.display()
        )))
    } else {
        Ok(())
    }
}
/// WriteRecords treats false and numeric zero IDs as missing. Check only
/// retained records; an intentional whole-record replacement has no such pair.
pub fn check_retained_ids<'a>(
    path: &std::path::Path,
    pairs: impl IntoIterator<Item = (&'a Record, &'a Record)>,
) -> crate::Result<()> {
    let false_id = |v: Option<&Value>| match v {
        Some(Value::Bool(false)) => true,
        Some(Value::Number(n)) => *n == 0.0,
        Some(Value::Integer(n)) => *n == 0,
        _ => false,
    };
    for (index, (before, after)) in pairs.into_iter().enumerate() {
        if false_id(before.get("id")) && false_id(after.get("id")) {
            return Err(crate::AtlasError::Message(format!(
                "Refusing to rewrite {}: retained record {} has a false or zero id that would be replaced automatically. The file is unchanged.",
                path.display(),index+1
            )));
        }
    }
    Ok(())
}
/// Resolve a Go-style ID without the foundation helper's recursive Text call.
/// Truthy non-string IDs remain readable; only Go's mx-prefixed lookup matches.
pub fn resolve_record_id(records: &[Record], id: &str) -> crate::Result<usize> {
    let hash = id.strip_prefix("mx-").unwrap_or(id);
    let wanted = format!("mx-{hash}");
    if let Some(index) = records.iter().position(|r| text(r.get("id")) == wanted) {
        return Ok(index);
    }
    let matches: Vec<_> = records
        .iter()
        .enumerate()
        .map(|(index, r)| (index, text(r.get("id"))))
        .filter(|(_, value)| value.starts_with(&wanted))
        .collect();
    match matches.as_slice() {
        [(index,_)]=>Ok(*index),
        []=>Err(crate::AtlasError::Message(format!(
            "Record \"{id}\" not found. Run {}luuna-atlas query{} to see record IDs.",
            char::from(96),char::from(96)
        ))),
        _=>Err(crate::AtlasError::Message(format!(
            "Ambiguous identifier \"{id}\" matches {} records: {}. Use more characters to disambiguate.",
            matches.len(),matches.iter().map(|(_,id)|id.as_str()).collect::<Vec<_>>().join(", ")
        )))
    }
}

/// Go's ID derivation using nonrecursive Text for arbitrary typed API values.
pub fn generate_record_id(record: &Record, registry: &Registry) -> crate::Result<String> {
    use sha2::{Digest, Sha256};
    let typ = text(record.get("type"));
    let definition = registry
        .get(&typ)
        .ok_or_else(|| crate::AtlasError::Message(format!("Unknown record type: {typ}")))?;
    let key = format!("{typ}:{}", text(record.get(&definition.id_key)));
    let digest = Sha256::digest(key.as_bytes());
    Ok(format!(
        "mx-{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2]
    ))
}
/// Prepare IDs exactly as the writer does, without recursive Text. Call the
/// preservation checks first when records came from an existing file.
pub fn assign_record_ids(records: &mut [Record], registry: &Registry) -> crate::Result<()> {
    for record in records {
        if !crate::registry::go_bool(record.get("id")) {
            record.insert_unordered("id", generate_record_id(record, registry)?);
        }
    }
    Ok(())
}
