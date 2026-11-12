//! Timestamp helpers with the JavaScript reference implementation's precision.

use time::OffsetDateTime;

#[must_use]
pub fn iso_now() -> String {
    iso_at(OffsetDateTime::now_utc())
}

#[must_use]
pub fn iso_at(value: OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        value.year(),
        u8::from(value.month()),
        value.day(),
        value.hour(),
        value.minute(),
        value.second()
    )
}

#[must_use]
pub fn short_time(iso: &str) -> String {
    let bytes = iso.as_bytes();
    if bytes.len() >= 16
        && bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && bytes.get(10) == Some(&b'T')
        && bytes.get(13) == Some(&b':')
    {
        format!("{} {}", &iso[..10], &iso[11..16])
    } else {
        iso.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn emits_utc_seconds_only() {
        assert_eq!(
            iso_at(datetime!(2026-10-22 10:11:12.999 UTC)),
            "2026-10-22T10:11:12Z"
        );
        assert_eq!(short_time("2026-10-22T10:11:12Z"), "2026-10-22 10:11");
    }
}
