//! Time helpers: ISO timestamps, date parsing, relative time and file modification time.
//! Owner: s5-atlas-lib-2.
//!
//! A port of the Go tool's `NowISO`, `ISO` and `ParseDate` (`model.go:331-340`),
//! `FormatTimeAgo` (`format.go:56-69`) and `FileModTime` (`store.go:310-317`).
//!
//! Go's zero `time.Time` (what `ParseDate` returns for text it cannot read) is
//! represented as `None`. A caller that compares, subtracts or formats such a time as Go
//! does uses 0001-01-01T00:00:00Z in its place.

use std::path::Path;

use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Timelike, Utc};

/// The current time as `2006-01-02T15:04:05.000Z` (`NowISO`).
pub fn now_iso() -> String {
    iso(Utc::now())
}

/// `t` in UTC as `2006-01-02T15:04:05.000Z` (`ISO`): milliseconds truncated, never
/// rounded; the year has at least four digits.
pub fn iso(t: DateTime<Utc>) -> String {
    let year = t.year();
    let sign = if year < 0 { "-" } else { "" };
    // A leap second (nanosecond 1_000_000_000 and over) has no Go counterpart.
    let millis = t.nanosecond().min(999_999_999) / 1_000_000;
    format!(
        "{sign}{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        year.unsigned_abs(),
        t.month(),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    )
}

/// `ParseDate`: the first of Go's layouts that reads `text` in Go's order (RFC3339Nano,
/// `2006-01-02`, `2006-01-02T15:04:05`, `2006-01-02 15:04:05`, RFC1123), with a hand
/// parser that follows `time.Parse`. Layouts without a zone read as UTC. `None` where Go
/// returns the zero time.
///
/// One thing is not copied: for RFC1123 with a zone abbreviation (`MST`), Go asks the
/// process's local time zone whether it knows the abbreviation and, if so, shifts the
/// instant by that zone's offset. Here the abbreviation is read the way Go reads one the
/// local zone does not know: the wall-clock time is taken as UTC.
pub fn parse_date(text: &str) -> Option<DateTime<Utc>> {
    let value = text.as_bytes();
    LAYOUTS
        .iter()
        .find_map(|layout| parse_layout(layout, value))
}

/// The parts of Go's layouts, in the order `time.Parse` meets them.
#[derive(Clone, Copy)]
enum Part {
    /// A literal run; a space in it matches a run of spaces.
    Lit(&'static str),
    /// `2006`
    Year,
    /// `01`
    Month,
    /// `02`
    Day,
    /// `15`: one or two digits.
    Hour,
    /// `04`
    Minute,
    /// `05`, followed by a fractional second when the text has one.
    Second,
    /// `Z07:00`
    ColonZone,
    /// `Mon`
    ShortWeekday,
    /// `Jan`
    ShortMonth,
    /// `MST`
    ZoneName,
}

use Part::*;

const LAYOUTS: [&[Part]; 5] = [
    // time.RFC3339Nano: "2006-01-02T15:04:05.999999999Z07:00"
    &[
        Year,
        Lit("-"),
        Month,
        Lit("-"),
        Day,
        Lit("T"),
        Hour,
        Lit(":"),
        Minute,
        Lit(":"),
        Second,
        ColonZone,
    ],
    &[Year, Lit("-"), Month, Lit("-"), Day],
    &[
        Year,
        Lit("-"),
        Month,
        Lit("-"),
        Day,
        Lit("T"),
        Hour,
        Lit(":"),
        Minute,
        Lit(":"),
        Second,
    ],
    &[
        Year,
        Lit("-"),
        Month,
        Lit("-"),
        Day,
        Lit(" "),
        Hour,
        Lit(":"),
        Minute,
        Lit(":"),
        Second,
    ],
    // time.RFC1123: "Mon, 02 Jan 2006 15:04:05 MST"
    &[
        ShortWeekday,
        Lit(", "),
        Day,
        Lit(" "),
        ShortMonth,
        Lit(" "),
        Year,
        Lit(" "),
        Hour,
        Lit(":"),
        Minute,
        Lit(":"),
        Second,
        Lit(" "),
        ZoneName,
    ],
];

const SHORT_DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const SHORT_MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn is_digit(s: &[u8], i: usize) -> bool {
    s.get(i).is_some_and(u8::is_ascii_digit)
}

/// Go's `getnum`: one or two digits (two when `fixed`).
fn getnum(s: &[u8], fixed: bool) -> Option<(i64, &[u8])> {
    if !is_digit(s, 0) {
        return None;
    }
    if !is_digit(s, 1) {
        if fixed {
            return None;
        }
        return Some((i64::from(s[0] - b'0'), &s[1..]));
    }
    Some((
        i64::from(s[0] - b'0') * 10 + i64::from(s[1] - b'0'),
        &s[2..],
    ))
}

fn cut_space(mut s: &[u8]) -> &[u8] {
    while let [b' ', rest @ ..] = s {
        s = rest;
    }
    s
}

/// Go's `skip`: a literal prefix, where a run of spaces matches any run of spaces. As in
/// Go, a space in the prefix also matches the end of the text.
fn skip<'a>(mut value: &'a [u8], prefix: &str) -> Option<&'a [u8]> {
    let mut prefix = prefix.as_bytes();
    while let Some(&first) = prefix.first() {
        if first == b' ' {
            if value.first().is_some_and(|&c| c != b' ') {
                return None;
            }
            prefix = cut_space(prefix);
            value = cut_space(value);
            continue;
        }
        if value.first() != Some(&first) {
            return None;
        }
        prefix = &prefix[1..];
        value = &value[1..];
    }
    Some(value)
}

/// Go's `lookup`, matching ASCII letters without regard to case.
fn lookup<'a>(table: &[&str], value: &'a [u8]) -> Option<(usize, &'a [u8])> {
    table.iter().enumerate().find_map(|(i, name)| {
        let n = name.len();
        (value.len() >= n && value[..n].eq_ignore_ascii_case(name.as_bytes()))
            .then(|| (i, &value[n..]))
    })
}

/// Go's `leadingInt`: the leading digits, or `None` on overflow of Go's limit.
fn leading_int(s: &[u8]) -> Option<(u64, &[u8])> {
    let mut x: u64 = 0;
    let mut i = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        if x > (1u64 << 63) / 10 {
            return None;
        }
        x = x * 10 + u64::from(s[i] - b'0');
        if x > 1u64 << 63 {
            return None;
        }
        i += 1;
    }
    Some((x, &s[i..]))
}

/// Go's `parseSignedOffset`: the length of a `+03` / `-04` offset, 0 when there is none.
fn parse_signed_offset(value: &[u8]) -> usize {
    if value.first() != Some(&b'-') && value.first() != Some(&b'+') {
        return 0;
    }
    let Some((x, rem)) = leading_int(&value[1..]) else {
        return 0;
    };
    if rem.len() == value.len() - 1 || x > 23 {
        return 0;
    }
    value.len() - rem.len()
}

/// Go's `parseTimeZone`: the length of the zone name at the start of `value`.
fn parse_time_zone(value: &[u8]) -> Option<usize> {
    if value.len() < 3 {
        return None;
    }
    if value.len() >= 4 && (&value[..4] == b"ChST" || &value[..4] == b"MeST") {
        return Some(4);
    }
    if &value[..3] == b"GMT" {
        let rest = &value[3..];
        return Some(if rest.is_empty() {
            3
        } else {
            3 + parse_signed_offset(rest)
        });
    }
    if value[0] == b'+' || value[0] == b'-' {
        let n = parse_signed_offset(value);
        return (n > 0).then_some(n);
    }
    let upper = value
        .iter()
        .take(6)
        .take_while(|c| c.is_ascii_uppercase())
        .count();
    match upper {
        5 if value[4] == b'T' => Some(5),
        4 if value[3] == b'T' || &value[..4] == b"WITA" => Some(4),
        3 => Some(3),
        _ => None,
    }
}

/// Go's `parseNanoseconds` for a separator followed by `digits` (a non-empty run of
/// digits): digits past the ninth are dropped.
fn parse_nanoseconds(digits: &[u8]) -> i64 {
    let mut ns: i64 = 0;
    let mut used = 0;
    for &d in digits.iter().take(9) {
        ns = ns * 10 + i64::from(d - b'0');
        used += 1;
    }
    for _ in used..9 {
        ns *= 10;
    }
    ns
}

fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in(month: i64, year: i64) -> i64 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `time.Parse(layout, text)` for one of the layouts above.
fn parse_layout(layout: &[Part], mut value: &[u8]) -> Option<DateTime<Utc>> {
    let (mut year, mut month, mut day) = (0i64, -1i64, -1i64);
    let (mut hour, mut minute, mut second, mut nsec) = (0i64, 0i64, 0i64, 0i64);
    // Seconds east of UTC, from a numeric zone.
    let mut zone_offset: Option<i64> = None;
    for part in layout {
        match *part {
            Lit(text) => value = skip(value, text)?,
            Year => {
                if value.len() < 4 || !value[..4].iter().all(u8::is_ascii_digit) {
                    return None;
                }
                year = value[..4]
                    .iter()
                    .fold(0, |acc, &d| acc * 10 + i64::from(d - b'0'));
                value = &value[4..];
            }
            Month => {
                let (m, rest) = getnum(value, true)?;
                if !(1..=12).contains(&m) {
                    return None;
                }
                month = m;
                value = rest;
            }
            Day => {
                let (d, rest) = getnum(value, true)?;
                day = d;
                value = rest;
            }
            Hour => {
                let (h, rest) = getnum(value, false)?;
                if h >= 24 {
                    return None;
                }
                hour = h;
                value = rest;
            }
            Minute => {
                let (m, rest) = getnum(value, true)?;
                if m >= 60 {
                    return None;
                }
                minute = m;
                value = rest;
            }
            Second => {
                let (s, rest) = getnum(value, true)?;
                if s >= 60 {
                    return None;
                }
                second = s;
                value = rest;
                // A fractional second is read whether or not the layout names one.
                if value.len() >= 2 && (value[0] == b'.' || value[0] == b',') && is_digit(value, 1)
                {
                    let n = 1 + value[1..].iter().take_while(|c| c.is_ascii_digit()).count();
                    nsec = parse_nanoseconds(&value[1..n]);
                    value = &value[n..];
                }
            }
            ColonZone => {
                if value.first() == Some(&b'Z') {
                    value = &value[1..];
                    zone_offset = Some(0);
                } else {
                    if value.len() < 6 || value[3] != b':' {
                        return None;
                    }
                    let (hr, _) = getnum(&value[1..3], true)?;
                    let (mm, _) = getnum(&value[4..6], true)?;
                    if hr > 24 || mm > 60 {
                        return None;
                    }
                    let offset = (hr * 60 + mm) * 60;
                    zone_offset = Some(match value[0] {
                        b'+' => offset,
                        b'-' => -offset,
                        _ => return None,
                    });
                    value = &value[6..];
                }
            }
            ShortWeekday => {
                // Only checked to be a day name, not to fit the date.
                value = lookup(&SHORT_DAYS, value)?.1;
            }
            ShortMonth => {
                let (i, rest) = lookup(&SHORT_MONTHS, value)?;
                month = i as i64 + 1;
                value = rest;
            }
            ZoneName => {
                if value.len() >= 3 && &value[..3] == b"UTC" {
                    value = &value[3..];
                } else {
                    // Any other name is read as Go reads one its local zone does not
                    // know: the wall-clock time stays as it is.
                    let n = parse_time_zone(value)?;
                    value = &value[n..];
                }
            }
        }
    }
    if !value.is_empty() {
        return None;
    }
    if month < 0 {
        month = 1;
    }
    if day < 0 {
        day = 1;
    }
    if day < 1 || day > days_in(month, year) {
        return None;
    }
    let date = NaiveDate::from_ymd_opt(year as i32, month as u32, day as u32)?;
    let naive = date.and_hms_nano_opt(hour as u32, minute as u32, second as u32, nsec as u32)?;
    let utc = Utc.from_utc_datetime(&naive);
    let offset = chrono::TimeDelta::try_seconds(zone_offset.unwrap_or(0))?;
    utc.checked_sub_signed(offset)
}

/// `FormatTimeAgo(t)` with the clock passed in: `just now` under a minute (a future `t`
/// included), then `Nm ago`, `Nh ago`, `Nd ago`, each floored. As with Go's
/// `Time.Sub`, a difference beyond Go's `time.Duration` range saturates at it.
pub fn format_time_ago(t: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let diff = now.signed_duration_since(t);
    let nanos = i128::from(diff.num_seconds()) * 1_000_000_000 + i128::from(diff.subsec_nanos());
    let d = nanos.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
    // Go's Duration.Minutes and Duration.Hours, float arithmetic included.
    let minutes = (d / 60_000_000_000) as f64 + (d % 60_000_000_000) as f64 / 60e9;
    let mins = minutes.floor() as i64;
    if mins < 1 {
        return "just now".to_string();
    }
    if mins < 60 {
        return format!("{mins}m ago");
    }
    let hours_f = (d / 3_600_000_000_000) as f64 + (d % 3_600_000_000_000) as f64 / 3600e9;
    let hours = hours_f.floor() as i64;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    format!("{}d ago", (hours_f / 24.0).floor() as i64)
}

/// `FileModTime`: the modification time from the file's metadata (`os.Stat`, so a
/// symbolic link is followed), or `None` when it cannot be read.
pub fn file_mod_time(path: &Path) -> Option<DateTime<Utc>> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(DateTime::<Utc>::from(modified))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, NaiveDate, TimeZone};

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32, nanos: u32) -> DateTime<Utc> {
        let date = NaiveDate::from_ymd_opt(y, mo, d).expect("date");
        let naive = date.and_hms_nano_opt(h, mi, s, nanos).expect("time");
        Utc.from_utc_datetime(&naive)
    }

    #[test]
    fn now_iso_truncates_milliseconds() {
        let t = Utc
            .timestamp_opt(1_700_000_000, 999_999_999)
            .single()
            .expect("t");
        assert_eq!(iso(t), "2023-11-14T22:13:20.999Z");
        let t = Utc
            .timestamp_opt(1_700_000_000, 1_999_999)
            .single()
            .expect("t");
        assert_eq!(iso(t), "2023-11-14T22:13:20.001Z");
        let t = Utc.timestamp_opt(1_700_000_000, 0).single().expect("t");
        assert_eq!(iso(t), "2023-11-14T22:13:20.000Z");
        assert_eq!(iso(utc(1, 1, 1, 0, 0, 0, 0)), "0001-01-01T00:00:00.000Z");
        let now = now_iso();
        assert_eq!(now.len(), 24, "{now}");
        assert!(now.ends_with('Z'));
        assert_eq!(&now[4..5], "-");
        assert_eq!(&now[10..11], "T");
        assert_eq!(&now[19..20], ".");
    }

    #[test]
    fn parse_date_layout_order_and_zero() {
        assert_eq!(
            parse_date("2024-03-05T10:20:30.123456789Z"),
            Some(utc(2024, 3, 5, 10, 20, 30, 123_456_789))
        );
        assert_eq!(
            parse_date("2024-03-05T10:20:30Z"),
            Some(utc(2024, 3, 5, 10, 20, 30, 0))
        );
        assert_eq!(
            parse_date("2024-03-05T10:20:30+02:00"),
            Some(utc(2024, 3, 5, 8, 20, 30, 0))
        );
        assert_eq!(parse_date("2024-03-05"), Some(utc(2024, 3, 5, 0, 0, 0, 0)));
        assert_eq!(
            parse_date("2024-03-05T10:20:30"),
            Some(utc(2024, 3, 5, 10, 20, 30, 0))
        );
        assert_eq!(
            parse_date("2024-03-05 10:20:30"),
            Some(utc(2024, 3, 5, 10, 20, 30, 0))
        );
        assert_eq!(
            parse_date("Tue, 05 Mar 2024 10:20:30 GMT"),
            Some(utc(2024, 3, 5, 10, 20, 30, 0))
        );
        assert_eq!(
            parse_date("Tue, 05 Mar 2024 10:20:30 UTC"),
            Some(utc(2024, 3, 5, 10, 20, 30, 0))
        );
        // Go checks that the day name is a day name, not that it fits the date.
        assert_eq!(
            parse_date("Fri, 05 Mar 2024 10:20:30 GMT"),
            Some(utc(2024, 3, 5, 10, 20, 30, 0))
        );
        // The zero time: nothing Go can read.
        for text in [
            "",
            "garbage",
            "2024-13-01",
            "2024-02-30",
            "2024-3-5",
            "2024-03-05T23:59:60Z",
            "2024-03-05T10:20",
            " 2024-03-05",
            "2024-03-05 ",
        ] {
            assert_eq!(parse_date(text), None, "{text:?}");
        }
    }

    #[test]
    fn format_time_ago_thresholds() {
        let now = utc(2024, 6, 1, 12, 0, 0, 0);
        let ago = |d: Duration| format_time_ago(now - d, now);
        assert_eq!(ago(Duration::seconds(0)), "just now");
        assert_eq!(ago(Duration::seconds(59)), "just now");
        assert_eq!(ago(Duration::seconds(60)), "1m ago");
        assert_eq!(ago(Duration::seconds(59 * 60 + 59)), "59m ago");
        assert_eq!(ago(Duration::minutes(60)), "1h ago");
        assert_eq!(ago(Duration::minutes(24 * 60 - 1)), "23h ago");
        assert_eq!(ago(Duration::hours(24)), "1d ago");
        assert_eq!(ago(Duration::hours(49)), "2d ago");
        // A time in the future is "just now".
        assert_eq!(format_time_ago(now + Duration::hours(1), now), "just now");
        // Go's zero time: Time.Sub saturates at the largest Duration (about 106751.99
        // days).
        assert_eq!(
            format_time_ago(utc(1, 1, 1, 0, 0, 0, 0), now),
            "106751d ago"
        );
    }
}
