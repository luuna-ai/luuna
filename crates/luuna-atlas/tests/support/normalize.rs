//! The normalisation applied before the Go tool's output is compared with the crate's
//! (plan s5-atlas-lib, "Normalisation"). Exactly three rules, nothing else:
//!
//! 1. each project's absolute path, raw and canonical, becomes `<PROJECT>`;
//! 2. a timestamp in the exact Go form `YYYY-MM-DDTHH:MM:SS.mmmZ`, dated after test start
//!    minus 60 s, becomes `<GENERATED-TIMESTAMP>`, whether it is the whole value or
//!    embedded in a longer string;
//! 3. CRLF becomes LF.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const PROJECT: &str = "<PROJECT>";
pub const GENERATED_TIMESTAMP: &str = "<GENERATED-TIMESTAMP>";

/// Length of `YYYY-MM-DDTHH:MM:SS.mmmZ`.
const GO_TIMESTAMP_LEN: usize = 24;

/// Window before the test start in which a timestamp still counts as generated.
const GENERATED_WINDOW_MS: i64 = 60_000;

/// The normaliser for one project.
#[derive(Debug, Clone)]
pub struct Normalizer {
    /// The project's path spellings, longest first so that no spelling is cut in half.
    paths: Vec<String>,
    /// Timestamps strictly after this instant (ms since the epoch) are generated.
    cutoff_ms: i64,
}

impl Normalizer {
    /// A normaliser for `project`, for a test that started at `started`.
    pub fn new(project: &Path, started: SystemTime) -> Self {
        let mut paths = vec![project.to_string_lossy().into_owned()];
        if let Ok(canonical) = std::fs::canonicalize(project) {
            paths.push(canonical.to_string_lossy().into_owned());
        }
        paths.retain(|p| !p.is_empty());
        paths.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        paths.dedup();
        let started_ms = started
            .duration_since(UNIX_EPOCH)
            .expect("the test start is after 1970")
            .as_millis() as i64;
        Normalizer {
            paths,
            cutoff_ms: started_ms - GENERATED_WINDOW_MS,
        }
    }

    /// Applies the three rules to `text`.
    pub fn normalize(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for path in &self.paths {
            out = out.replace(path.as_str(), PROJECT);
        }
        out = replace_generated_timestamps(&out, self.cutoff_ms);
        out.replace("\r\n", "\n")
    }
}

/// Replaces every Go-form timestamp dated strictly after `cutoff_ms`.
fn replace_generated_timestamps(text: &str, cutoff_ms: i64) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i + GO_TIMESTAMP_LEN <= bytes.len() {
        let window = &bytes[i..i + GO_TIMESTAMP_LEN];
        match go_timestamp_ms(window) {
            Some(ms) if ms > cutoff_ms => {
                out.push_str(&text[copied..i]);
                out.push_str(GENERATED_TIMESTAMP);
                i += GO_TIMESTAMP_LEN;
                copied = i;
            }
            _ => i += 1,
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// Milliseconds since the epoch of `window` if it is exactly `YYYY-MM-DDTHH:MM:SS.mmmZ`
/// and a valid date and time.
fn go_timestamp_ms(window: &[u8]) -> Option<i64> {
    const SHAPE: &[u8; GO_TIMESTAMP_LEN] = b"dddd-dd-ddTdd:dd:dd.dddZ";
    if window.len() != GO_TIMESTAMP_LEN {
        return None;
    }
    for (byte, shape) in window.iter().zip(SHAPE.iter()) {
        let ok = match shape {
            b'd' => byte.is_ascii_digit(),
            other => byte == other,
        };
        if !ok {
            return None;
        }
    }
    let number = |range: std::ops::Range<usize>| -> u32 {
        window[range]
            .iter()
            .fold(0, |acc, b| acc * 10 + u32::from(b - b'0'))
    };
    let date = chrono::NaiveDate::from_ymd_opt(number(0..4) as i32, number(5..7), number(8..10))?;
    let time = date.and_hms_milli_opt(
        number(11..13),
        number(14..16),
        number(17..19),
        number(20..23),
    )?;
    Some(time.and_utc().timestamp_millis())
}
