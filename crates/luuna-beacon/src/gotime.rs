//! Time as the Go tool writes it: `time.Now().UTC().Format(time.RFC3339Nano)`
//! for `createdAt` and `updatedAt`, and `time.Now().UnixNano()` for ids. The
//! current time comes from a [`Clock`] so that callers and tests can fix it.

use std::time::{SystemTime, UNIX_EPOCH};

/// The source of the current time for every record the library writes.
pub trait Clock: Send + Sync {
    /// The current wall-clock time.
    fn now(&self) -> SystemTime;
}

/// The operating system's clock; the default.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Seconds and nanoseconds since the Unix epoch, the nanoseconds always in
/// `0..1_000_000_000` (so a time before the epoch has negative seconds).
fn unix_parts(time: SystemTime) -> (i64, u32) {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => (
            i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
            after.subsec_nanos(),
        ),
        Err(error) => {
            let before = error.duration();
            let seconds = i64::try_from(before.as_secs()).unwrap_or(i64::MAX);
            match before.subsec_nanos() {
                0 => (-seconds, 0),
                nanos => (-seconds - 1, 1_000_000_000 - nanos),
            }
        }
    }
}

/// Go's `t.UnixNano()`: nanoseconds since the Unix epoch. Like Go, the result
/// wraps when it does not fit an `int64` (before 1678 or after 2262).
pub(crate) fn unix_nano(time: SystemTime) -> i64 {
    let (seconds, nanos) = unix_parts(time);
    seconds
        .wrapping_mul(1_000_000_000)
        .wrapping_add(i64::from(nanos))
}

/// Go's `t.UTC().Format(time.RFC3339Nano)`: `YYYY-MM-DDTHH:MM:SS`, then a
/// `.` and the nanoseconds with trailing zeros removed (nothing at all when
/// they are zero), then `Z`. The year is written as Go's `appendInt(year, 4)`
/// does: at least four digits, with a leading `-` when negative.
pub(crate) fn rfc3339_nano(time: SystemTime) -> String {
    let (seconds, nanos) = unix_parts(time);
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let mut out = String::with_capacity(30);
    if year < 0 {
        out.push('-');
    }
    out.push_str(&format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        year.unsigned_abs(),
        month,
        day,
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60
    ));
    if nanos != 0 {
        let fraction = format!("{nanos:09}");
        out.push('.');
        out.push_str(fraction.trim_end_matches('0'));
    }
    out.push('Z');
    out
}

/// The proleptic Gregorian date of a day count since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Howard Hinnant's days_from_civil inverse, in 400-year eras.
    let shifted = i128::from(days) + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i128::from(month <= 2);
    (
        i64::try_from(year).unwrap_or(i64::MAX),
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(seconds: u64, nanos: u32) -> SystemTime {
        UNIX_EPOCH + Duration::new(seconds, nanos)
    }

    #[test]
    fn trims_trailing_zeros_like_rfc3339nano() {
        // 2024-01-02T03:04:05Z
        let base = 1_704_164_645;
        let cases: &[(u32, &str)] = &[
            (0, "2024-01-02T03:04:05Z"),
            (100_000_000, "2024-01-02T03:04:05.1Z"),
            (120_000_000, "2024-01-02T03:04:05.12Z"),
            (123_000_000, "2024-01-02T03:04:05.123Z"),
            (123_400_000, "2024-01-02T03:04:05.1234Z"),
            (123_450_000, "2024-01-02T03:04:05.12345Z"),
            (123_456_000, "2024-01-02T03:04:05.123456Z"),
            (123_456_700, "2024-01-02T03:04:05.1234567Z"),
            (123_456_780, "2024-01-02T03:04:05.12345678Z"),
            (123_456_789, "2024-01-02T03:04:05.123456789Z"),
            (1, "2024-01-02T03:04:05.000000001Z"),
            (10, "2024-01-02T03:04:05.00000001Z"),
            (500_000_000, "2024-01-02T03:04:05.5Z"),
            (100_000_010, "2024-01-02T03:04:05.10000001Z"),
            (999_999_999, "2024-01-02T03:04:05.999999999Z"),
        ];
        for (nanos, expected) in cases {
            assert_eq!(rfc3339_nano(at(base, *nanos)), *expected, "nanos {nanos}");
        }
    }

    #[test]
    fn formats_calendar_edges() {
        assert_eq!(rfc3339_nano(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_nano(at(951_782_400, 0)), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339_nano(at(4_107_542_399, 0)), "2100-02-28T23:59:59Z");
        assert_eq!(rfc3339_nano(at(253_402_300_799, 0)), "9999-12-31T23:59:59Z");
        assert_eq!(
            rfc3339_nano(at(253_402_300_800, 0)),
            "10000-01-01T00:00:00Z"
        );
        let before = UNIX_EPOCH - Duration::new(1, 500_000_000);
        assert_eq!(rfc3339_nano(before), "1969-12-31T23:59:58.5Z");
        let year_zero = UNIX_EPOCH - Duration::from_secs(62_167_219_200);
        assert_eq!(rfc3339_nano(year_zero), "0000-01-01T00:00:00Z");
        // Year 0 is a leap year; year -1 is not.
        let year_minus_one = UNIX_EPOCH - Duration::from_secs(62_167_219_200 + 365 * 86_400);
        assert_eq!(rfc3339_nano(year_minus_one), "-0001-01-01T00:00:00Z");
    }

    #[test]
    fn unix_nano_counts_from_the_epoch() {
        assert_eq!(unix_nano(UNIX_EPOCH), 0);
        assert_eq!(unix_nano(at(1, 5)), 1_000_000_005);
        assert_eq!(unix_nano(UNIX_EPOCH - Duration::new(0, 1)), -1);
        assert_eq!(unix_nano(UNIX_EPOCH - Duration::new(2, 0)), -2_000_000_000);
    }
}
