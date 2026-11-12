//! Permanent, language-independent byte fixtures for the public help surface.
//!
//! These files were captured from the TypeScript implementation before the
//! native cutover.  This test intentionally invokes only the Rust binary: it
//! stays useful after Bun and the old CLI are removed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const COMMANDS: &[&str] = &[
    "init", "setup", "guard", "plant", "think", "flow", "adopt", "mv", "tend", "accept", "reject",
    "link", "scan", "unlink", "commit", "status", "compost", "tier", "context", "prime", "drift",
    "ask", "note", "propose", "sprout", "heading", "show", "blame", "list", "queue", "log", "view",
    "verify",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

fn fixture(path: &str) -> Vec<u8> {
    fs::read(workspace_root().join("tests").join("compat").join(path))
        .unwrap_or_else(|error| panic!("read fixture {path}: {error}"))
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_luuna-roots"))
        .current_dir(workspace_root())
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("run {args:?}: {error}"))
}

fn assert_help(args: &[&str], expected: &[u8]) {
    let output = run(args);
    assert_eq!(output.status.code(), Some(0), "exit for {args:?}");
    assert!(
        output.stderr.is_empty(),
        "stderr for {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout == expected,
        "help bytes for {args:?}\nactual:\n{}\nexpected:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(expected),
    );
}

#[test]
fn public_help_is_byte_stable_without_the_typescript_runtime() {
    let root = fixture("help/root.txt");
    for args in [["--help"].as_slice(), ["-h"].as_slice()] {
        assert_help(args, &root);
    }

    for command in COMMANDS {
        let expected = fixture(&format!("help/{command}.txt"));
        assert_help(&[*command, "--help"], &expected);
        assert_help(&["help", *command], &expected);
        // Help wins before output selection, so a trailing --json remains
        // plaintext help rather than becoming a JSON envelope.
        assert_help(&[*command, "--help", "--json"], &expected);
    }
}
