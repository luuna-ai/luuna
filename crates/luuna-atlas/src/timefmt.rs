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

use chrono::{DateTime, Utc};

/// The current time as `2006-01-02T15:04:05.000Z` (`NowISO`).
pub fn now_iso() -> String {
    iso(Utc::now())
}

/// `t` in UTC as `2006-01-02T15:04:05.000Z` (`ISO`): milliseconds truncated, never
/// rounded; the year has at least four digits.
pub fn iso(t: DateTime<Utc>) -> String {
    // stub until t2
    let _ = t;
    String::new()
}

/// `ParseDate`: the first of Go's layouts that reads `text` in Go's order (RFC3339Nano,
/// `2006-01-02`, `2006-01-02T15:04:05`, `2006-01-02 15:04:05`, RFC1123), with a hand
/// parser that follows `time.Parse`. Layouts without a zone read as UTC. `None` where Go
/// returns the zero time.
pub fn parse_date(text: &str) -> Option<DateTime<Utc>> {
    // stub until t2
    let _ = text;
    None
}

/// `FormatTimeAgo(t)` with the clock passed in: `just now` under a minute (a future `t`
/// included), then `Nm ago`, `Nh ago`, `Nd ago`, each floored. As with Go's
/// `Time.Sub`, a difference beyond Go's `time.Duration` range saturates at it.
pub fn format_time_ago(t: DateTime<Utc>, now: DateTime<Utc>) -> String {
    // stub until t2
    let _ = (t, now);
    String::new()
}

/// `FileModTime`: the modification time from the file's metadata (`os.Stat`, so a
/// symbolic link is followed), or `None` when it cannot be read.
pub fn file_mod_time(path: &Path) -> Option<DateTime<Utc>> {
    // stub until t2
    let _ = path;
    None
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
