//! Pins the exact bytes and digests of the environment fingerprints, so a
//! change in how their identity is serialised cannot go unnoticed.

use super::{sha256_hex, Executor, ToolVersion};
use std::path::Path;

const TOOL: &str = "/opt/luuna-pin/tool";

fn arch() -> &'static str {
    std::env::consts::ARCH
}

fn os() -> &'static str {
    std::env::consts::OS
}

#[test]
fn fingerprint_with_version_matches_sorted_compact_json() {
    let text = format!(
        r#"{{"arch":"{}","executable":"{TOOL}","os":"{}","tool_version":"tool 1.2.3"}}"#,
        arch(),
        os()
    );
    assert_eq!(
        Executor::fingerprint(Path::new(TOOL), Some("tool 1.2.3")),
        sha256_hex(text.as_bytes())
    );
}

#[test]
fn fingerprint_without_version_matches_sorted_compact_json() {
    let text = format!(
        r#"{{"arch":"{}","executable":"{TOOL}","os":"{}","tool_version":null}}"#,
        arch(),
        os()
    );
    assert_eq!(
        Executor::fingerprint(Path::new(TOOL), None),
        sha256_hex(text.as_bytes())
    );
}

#[test]
fn combined_fingerprint_matches_sorted_compact_json() {
    let text = format!(
        r#"{{"arch":"{}","os":"{}","parts":["a","b"]}}"#,
        arch(),
        os()
    );
    let parts = ["b".to_owned(), "a".to_owned(), "a".to_owned()];
    assert_eq!(
        Executor::combine_fingerprints(&parts),
        sha256_hex(text.as_bytes())
    );
}

#[test]
fn failed_probe_fingerprint_matches_sorted_compact_json() {
    let text = format!(
        r#"{{"arch":"{}","executable":"{TOOL}","os":"{}","probe_failure":"probe exited 2","tool_version":null}}"#,
        arch(),
        os()
    );
    assert_eq!(
        Executor::fingerprint_for(
            Path::new(TOOL),
            &ToolVersion::Failed("probe exited 2".to_owned())
        ),
        sha256_hex(text.as_bytes())
    );
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod literal {
    use super::*;

    #[test]
    fn fingerprint_with_version_literal() {
        assert_eq!(
            Executor::fingerprint(Path::new(TOOL), Some("tool 1.2.3")),
            "sha256:674953e9190379118c6ac81f062c713e0047389e5e461be62c0bc6925e3a786e"
        );
    }

    #[test]
    fn fingerprint_without_version_literal() {
        assert_eq!(
            Executor::fingerprint(Path::new(TOOL), None),
            "sha256:ef4c7d7a2946d7cf13a3b8548ff2ff70d4eb36da5abf6ed59fa951c888c73750"
        );
    }

    #[test]
    fn combined_fingerprint_literal() {
        let parts = ["b".to_owned(), "a".to_owned(), "a".to_owned()];
        assert_eq!(
            Executor::combine_fingerprints(&parts),
            "sha256:3693f13e105d6de20e22a8f5ad3bad7d5501e9bf712b9cf6ed3321c653bde587"
        );
    }

    #[test]
    fn failed_probe_fingerprint_literal() {
        assert_eq!(
            Executor::fingerprint_for(
                Path::new(TOOL),
                &ToolVersion::Failed("probe exited 2".to_owned())
            ),
            "sha256:beb7ff800ff40adcabaec1dae8f307eca98f96c44c2d3dddc56727dd459e76b8"
        );
    }
}
