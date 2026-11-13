//! `--respect-schedule` for `ready` (Go's `scheduledOut`,
//! `internal/cli/issue_read.go:495-514`).
//!
//! A ticket is scheduled out when its `queued` extension is boolean `true`,
//! or when its `scheduledFor` extension is a string that, trimmed, parses
//! with Go's `time.Parse` as `time.RFC3339Nano` or else as `time.DateOnly`
//! to an instant after now. The parsers below follow Go's `time.Parse`
//! (`parse` in `time/format.go`, which the RFC 3339 fast path falls back to):
//!
//! - RFC3339Nano is `2006-01-02T15:04:05.999999999Z07:00`: a four digit
//!   year, two digit month and day, `T`, an hour of one or two digits below
//!   24, two digit minute and second below 60, an optional fraction (`.` or
//!   `,` and any number of digits, nine kept), then `Z` or a sign with two
//!   digit hours (24 at most) and minutes (60 at most) separated by `:`.
//! - DateOnly is `2006-01-02`, read as midnight UTC.
//!
//! The day must exist in the month. Anything left over is an error.

use crate::json::Value;
use crate::model::Ticket;

/// Go's `scheduledOut`; `now` is nanoseconds since the Unix epoch, read once
/// per command call.
pub(super) fn scheduled_out(ticket: &Ticket, now: i128) -> bool {
    let Some(extensions) = &ticket.extensions else {
        return false;
    };
    if let Some(Value::Bool(true)) = extensions.get("queued") {
        return true;
    }
    if let Some(Value::String(value)) = extensions.get("scheduledFor") {
        // `strings.TrimSpace` trims the Unicode White_Space characters.
        let value = value.trim();
        let when = parse_rfc3339_nano(value).or_else(|| parse_date_only(value));
        if let Some(when) = when {
            return when > now;
        }
    }
    false
}

/// The wall clock now, in nanoseconds since the Unix epoch (Go's
/// `time.Now()`).
pub(super) fn now_nanos() -> i128 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(after) => i128::try_from(after.as_nanos()).unwrap_or(i128::MAX),
        Err(before) => -i128::try_from(before.duration().as_nanos()).unwrap_or(i128::MAX),
    }
}

/// A scanner over the bytes of a time string.
struct Scan<'a> {
    rest: &'a [u8],
}

impl Scan<'_> {
    fn expect(&mut self, byte: u8) -> Option<()> {
        match self.rest.split_first() {
            Some((first, rest)) if *first == byte => {
                self.rest = rest;
                Some(())
            }
            _ => None,
        }
    }

    /// `n` digits.
    fn digits(&mut self, n: usize) -> Option<i64> {
        let head = self.rest.get(..n)?;
        if !head.iter().all(u8::is_ascii_digit) {
            return None;
        }
        self.rest = &self.rest[n..];
        Some(head.iter().fold(0, |x, d| x * 10 + i64::from(d - b'0')))
    }

    /// Go's `getnum(s, false)`: one or two digits.
    fn one_or_two_digits(&mut self) -> Option<i64> {
        match self.digits(2) {
            Some(number) => Some(number),
            None => self.digits(1),
        }
    }
}

/// Go's `isLeap`.
fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Go's `daysIn`.
fn days_in(month: i64, year: i64) -> i64 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a valid proleptic Gregorian date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted = (month + 9) % 12;
    let day_of_year = (153 * shifted + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The date `2006-01-02` of `scan`, validated.
fn date(scan: &mut Scan<'_>) -> Option<i64> {
    // `stdLongYear`: four characters, the first a digit, all digits for `atoi`.
    let year = scan.digits(4)?;
    scan.expect(b'-')?;
    let month = scan.digits(2)?;
    if !(1..=12).contains(&month) {
        return None;
    }
    scan.expect(b'-')?;
    let day = scan.digits(2)?;
    if day < 1 || day > days_in(month, year) {
        return None;
    }
    Some(days_from_civil(year, month, day))
}

/// Go's `time.Parse(time.DateOnly, value)`, as nanoseconds since the epoch.
fn parse_date_only(value: &str) -> Option<i128> {
    let mut scan = Scan {
        rest: value.as_bytes(),
    };
    let days = date(&mut scan)?;
    scan.rest
        .is_empty()
        .then(|| i128::from(days) * 86_400 * 1_000_000_000)
}

/// Go's `time.Parse(time.RFC3339Nano, value)`, as nanoseconds since the
/// epoch.
fn parse_rfc3339_nano(value: &str) -> Option<i128> {
    let mut scan = Scan {
        rest: value.as_bytes(),
    };
    let days = date(&mut scan)?;
    scan.expect(b'T')?;
    let hour = scan.one_or_two_digits()?;
    if hour >= 24 {
        return None;
    }
    scan.expect(b':')?;
    let minute = scan.digits(2)?;
    scan.expect(b':')?;
    let second = scan.digits(2)?;
    if minute >= 60 || second >= 60 {
        return None;
    }
    // A fraction needs a digit after its separator; nine digits are kept.
    let mut nanos: i64 = 0;
    if let [b'.' | b',', first, ..] = scan.rest {
        if first.is_ascii_digit() {
            let count = scan.rest[1..]
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count();
            let kept = count.min(9);
            nanos = scan.rest[1..=kept]
                .iter()
                .fold(0, |x, d| x * 10 + i64::from(d - b'0'));
            for _ in kept..9 {
                nanos *= 10;
            }
            scan.rest = &scan.rest[1 + count..];
        }
    }
    let offset = if scan.expect(b'Z').is_some() {
        0
    } else {
        let sign = match scan.rest.first()? {
            b'+' => 1,
            b'-' => -1,
            _ => return None,
        };
        if scan.rest.len() < 6 || scan.rest[3] != b':' {
            return None;
        }
        scan.rest = &scan.rest[1..];
        let hours = scan.digits(2)?;
        scan.expect(b':')?;
        let minutes = scan.digits(2)?;
        // Go allows offsets of 24 hours and 60 minutes.
        if hours > 24 || minutes > 60 {
            return None;
        }
        sign * (hours * 60 + minutes) * 60
    };
    if !scan.rest.is_empty() {
        return None;
    }
    let seconds = i128::from(days) * 86_400 + i128::from(hour * 3600 + minute * 60 + second)
        - i128::from(offset);
    Some(seconds * 1_000_000_000 + i128::from(nanos))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// 2026-06-15T12:00:00Z.
    fn now() -> i128 {
        parse_rfc3339_nano("2026-06-15T12:00:00Z").unwrap()
    }

    fn ticket(extensions: &[(&str, Value)]) -> Ticket {
        let mut ticket = Ticket::default();
        ticket.extensions = Some(
            extensions
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect::<BTreeMap<_, _>>(),
        );
        ticket
    }

    fn scheduled(value: &str) -> bool {
        scheduled_out(
            &ticket(&[("scheduledFor", Value::String(value.to_owned()))]),
            now(),
        )
    }

    #[test]
    fn scheduled_out_follows_go() {
        // Go: time.Parse of these gives UnixNano 1781524800000000000 and
        // 1781524799999999999.
        assert_eq!(now(), 1_781_524_800_000_000_000);
        assert_eq!(
            parse_rfc3339_nano("2026-06-15T11:59:59.999999999Z"),
            Some(1_781_524_799_999_999_999)
        );
        assert_eq!(parse_date_only("1970-01-01"), Some(0));
        assert_eq!(
            parse_rfc3339_nano("1970-01-01T00:00:01.5Z"),
            Some(1_500_000_000)
        );
        assert_eq!(
            parse_rfc3339_nano("1970-01-02T01:00:00+01:00"),
            Some(86_400 * 1_000_000_000)
        );

        // No extensions, or neither key: kept.
        assert!(!scheduled_out(&Ticket::default(), now()));
        assert!(!scheduled_out(&ticket(&[]), now()));
        // `queued` is out only when it is boolean true.
        assert!(scheduled_out(
            &ticket(&[("queued", Value::Bool(true))]),
            now()
        ));
        assert!(!scheduled_out(
            &ticket(&[("queued", Value::Bool(false))]),
            now()
        ));
        assert!(!scheduled_out(
            &ticket(&[("queued", Value::String("true".to_owned()))]),
            now()
        ));
        // `queued: true` wins over a past date.
        assert!(scheduled_out(
            &ticket(&[
                ("queued", Value::Bool(true)),
                ("scheduledFor", Value::String("2000-01-01".to_owned())),
            ]),
            now()
        ));
        // `scheduledFor` of another type is ignored.
        assert!(!scheduled_out(
            &ticket(&[("scheduledFor", Value::Bool(true))]),
            now()
        ));

        // Strictly after now: out; equal or before: kept.
        assert!(scheduled("2026-06-15T12:00:00.000000001Z"));
        assert!(!scheduled("2026-06-15T12:00:00Z"));
        assert!(!scheduled("2026-06-15T11:59:59.999999999Z"));
        assert!(scheduled("2026-06-16"));
        assert!(!scheduled("2026-06-15"));
        assert!(scheduled(" \t2999-01-01 \n"));
        assert!(scheduled("2999-01-01\u{a0}"));
        // Offsets move the instant.
        assert!(scheduled("2026-06-15T12:00:00-00:01"));
        assert!(!scheduled("2026-06-15T12:00:00+00:01"));
        assert!(scheduled("2026-06-15T13:00:01+01:00"));
        assert!(!scheduled("2026-06-15T13:00:00+01:00"));
        // Go accepts a comma fraction, a one digit hour, offsets up to 24:60
        // and a fraction of any length.
        assert!(scheduled("2999-01-01T0:00:00,5Z"));
        assert!(scheduled("2999-01-01T9:00:00Z"));
        assert!(scheduled("2999-01-01T00:00:00+24:60"));
        assert!(scheduled("2999-01-01T00:00:00.123456789123Z"));
        // Everything else is not a time: kept.
        for text in [
            "",
            "2999-01-01T00:00:00",
            "2999-01-01t00:00:00Z",
            "2999-01-01 00:00:00Z",
            "2999-1-01",
            "2999-01-1",
            "999-01-01",
            "2999-13-01",
            "2999-02-29",
            "2999-04-31",
            "2999-01-01T24:00:00Z",
            "2999-01-01T00:60:00Z",
            "2999-01-01T00:00:60Z",
            "2999-01-01T00:0:00Z",
            "2999-01-01T00:00:00.Z",
            "2999-01-01T00:00:00z",
            "2999-01-01T00:00:00+25:00",
            "2999-01-01T00:00:00+00:61",
            "2999-01-01T00:00:00+0100",
            "2999-01-01T00:00:00+01",
            "2999-01-01T00:00:00 Z",
            "2999-01-01Z",
            "2999-01-01T00:00:00Z ",
            "+999-01-01",
            "now",
        ] {
            // A trailing space is trimmed first: it is the only text of the
            // list that parses once trimmed.
            let expected = text == "2999-01-01T00:00:00Z ";
            assert_eq!(scheduled(text), expected, "{text:?}");
        }
        // 2024 is a leap year, 2100 is not.
        assert!(scheduled_out(
            &ticket(&[("scheduledFor", Value::String("2999-02-28".to_owned()))]),
            now()
        ));
        assert!(!scheduled("2100-02-29"));
        assert!(!scheduled("2024-02-29"));
        assert!(parse_date_only("2024-02-29").is_some());
        assert!(parse_date_only("2100-02-29").is_none());
    }
}
