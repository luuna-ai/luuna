//! Owner: plan s3-beacon-lib-3. Reference tests of prime, onboard, doctor and
//! sync against the Go tool.
//!
//! Each test carries the ignore string
//! `#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test artifacts_commands -- --ignored"]`.
//!
//! Steps run through the frozen `Case` where the comparison can be exact.
//! `Pair` (AS53) runs sync (in repositories the Pair's helper made with
//! `git init`, never anywhere else), onboard `--stdout` (which prints no
//! JSON), prime with a refused legacy move, onboard with a legacy layout,
//! the onboard refusal (RD55, where Go and the library differ by design),
//! and doctor cases whose details carry Go parser or OS texts (AS55).
//!
//! These tests are written against the library contract of the plan and
//! compile once the library exists.
#![cfg(unix)]

mod support;

use luuna_beacon::ONBOARD_SNIPPET;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use support::artifacts::*;
use support::case::{prompt_record, set_mtime, store_paths, write_file, Case, Seen, Step};

fn ready() -> Case {
    let mut case = Case::with_project(PROJECT);
    plain(&mut case, Step::Init);
    case
}

fn error(seen: &Seen) -> &str {
    assert_eq!(seen.code, 1, "the step was expected to fail");
    seen.error.as_deref().expect("an error document")
}

fn doc(seen: &Seen) -> &Value {
    assert_eq!(
        seen.code, 0,
        "the step was expected to succeed: {:?}",
        seen.error
    );
    &seen.raw[0]
}

fn content(seen: &Seen) -> &str {
    doc(seen)["content"].as_str().expect("prime content")
}

fn check_names(report: &Value) -> Vec<String> {
    report["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .map(|check| check["name"].as_str().unwrap().to_owned())
        .collect()
}

fn check<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|check| check["name"] == json!(name))
        .unwrap_or_else(|| panic!("no {name} check in {report}"))
}

fn text_of(root: &Path, relative: &str) -> String {
    fs::read_to_string(root.join(relative))
        .unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

fn write(path: &str, text: &str) -> Step {
    write_file(path, text.as_bytes())
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test artifacts_commands -- --ignored"]
fn prime_default_compact_export_and_custom_file() {
    let mut case = Case::with_project(PROJECT);
    assert_eq!(content(&art(&mut case, prime(false, false))), PRIME_DEFAULT);
    assert_eq!(content(&art(&mut case, prime(true, false))), PRIME_COMPACT);
    assert!(
        store_paths(case.lib_root()).is_empty(),
        "prime writes nothing"
    );
    plain(&mut case, Step::Init);
    assert_eq!(content(&art(&mut case, prime(false, false))), PRIME_DEFAULT);
    plain(
        &mut case,
        write(".luuna/beacon/PRIME.md", "# Custom\n<b>&</b>\n"),
    );
    assert_eq!(
        content(&art(&mut case, prime(false, false))),
        "# Custom\n<b>&</b>\n"
    );
    assert_eq!(
        content(&art(&mut case, prime(true, false))),
        "# Custom\n<b>&</b>\n",
        "GD69: a custom PRIME.md wins over --compact"
    );
    assert_eq!(content(&art(&mut case, prime(false, true))), PRIME_DEFAULT);
    assert_eq!(content(&art(&mut case, prime(true, true))), PRIME_COMPACT);
    plain(&mut case, write(".luuna/beacon/PRIME.md", ""));
    assert_eq!(content(&art(&mut case, prime(false, false))), "");
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test artifacts_commands -- --ignored"]
fn prime_reads_through_a_symlink_and_keeps_invalid_utf8_like_go() {
    let mut case = ready();
    art(
        &mut case,
        tree(
            "docs",
            vec![entry_bytes(b"prime-source.md", b"bad \xff byte\n", false)],
        ),
    );
    on_both(&mut case, |root| {
        std::os::unix::fs::symlink(
            "../../docs/prime-source.md",
            root.join(".luuna/beacon/PRIME.md"),
        )
        .unwrap();
    });
    let seen = art(&mut case, prime(false, false));
    assert_eq!(
        content(&seen),
        "bad \u{FFFD} byte\n",
        "DL11: invalid UTF-8 prints as U+FFFD"
    );
    // (A dangling link is not used: the frozen store snapshot reads every
    // store entry.) A PRIME.md that cannot be read is ignored.
    on_both(&mut case, |root| {
        fs::remove_file(root.join(".luuna/beacon/PRIME.md")).unwrap();
        fs::create_dir(root.join(".luuna/beacon/PRIME.md")).unwrap();
    });
    let seen = art(&mut case, prime(false, false));
    assert_eq!(
        content(&seen),
        PRIME_DEFAULT,
        "a PRIME.md that cannot be read is ignored"
    );
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test artifacts_commands -- --ignored"]
fn prime_ignores_a_refused_legacy_migration() {
    let mut pair = Pair::new(Git::Available);
    pair.step(Step::Init);
    pair.step(write(".luuna/beacon/PRIME.md", "custom\n"));
    pair.step(write(".canopy/config.yaml", "project: legacy\n"));
    let seen = pair.art(prime(false, false));
    assert_eq!(seen.code, 0, "GD70: prime swallows the refused move");
    assert!(!seen.migrated);
    assert_eq!(
        seen.raw[0]["content"],
        json!(PRIME_DEFAULT),
        "with the move refused, Go never reads PRIME.md"
    );

    let mut pair = Pair::new(Git::Available);
    pair.step(write(".canopy/PRIME.md", "legacy prime\n"));
    let seen = pair.art(prime(false, false));
    assert!(seen.migrated, "prime runs the legacy move");
    assert_eq!(seen.raw[0]["content"], json!("legacy prime\n"));
    assert!(pair.lib_root().join(".luuna/beacon/PRIME.md").is_file());
    assert!(!pair.lib_root().join(".canopy").exists());
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test artifacts_commands -- --ignored"]
fn onboard_creates_appends_updates_and_keeps_current() {
    assert_eq!(
        ONBOARD_SNIPPET, GO_SNIPPET,
        "the snippet is Go's, byte for byte"
    );
    let mut case = Case::with_project(PROJECT);
    let seen = art(&mut case, onboard(Onboard::Apply));
    assert_eq!(doc(&seen)["action"], json!("created"));
    assert_eq!(seen.documents[0]["file"], json!("<root>/CLAUDE.md"));
    assert_eq!(text_of(case.lib_root(), "CLAUDE.md"), GO_SNIPPET);
    assert_eq!(mode_of(&case.lib_root().join("CLAUDE.md")), 0o644);
    assert_eq!(
        doc(&art(&mut case, onboard(Onboard::Apply)))["action"],
        json!("unchanged")
    );

    let cases: [(&str, &str, String); 7] = [
        ("", "created", GO_SNIPPET.to_owned()),
        ("# Notes\nkeep", "appended", format!("# Notes\nkeep\n\n{GO_SNIPPET}")),
        ("# Notes\n", "appended", format!("# Notes\n\n{GO_SNIPPET}")),
        (
            "before\n<!-- luuna-beacon:start -->\nold\n<!-- luuna-beacon:end -->\nafter\n",
            "updated",
            format!("before\n{GO_SNIPPET}\nafter\n"),
        ),
        (
            "x\n<!-- luuna-beacon:start -->\nold\n<!-- luuna-beacon:end -->",
            "updated",
            format!("x\n{GO_SNIPPET}"),
        ),
        (
            "a\r\n<!-- luuna-beacon:start -->\r\nold\r\n<!-- luuna-beacon:end -->\r\nz\r\n",
            "updated",
            format!("a\r\n{GO_SNIPPET}\r\nz\r\n"),
        ),
        (
            "<!-- luuna-beacon:start -->\nv1\n<!-- luuna-beacon:end -->\nmid\n<!-- luuna-beacon:start -->\nv2\n<!-- luuna-beacon:end -->\n",
            "updated",
            format!("{GO_SNIPPET}\nmid\n<!-- luuna-beacon:start -->\nv2\n<!-- luuna-beacon:end -->\n"),
        ),
    ];
    for (before, action, after) in cases {
        plain(&mut case, write("CLAUDE.md", before));
        let seen = art(&mut case, onboard(Onboard::Apply));
        assert_eq!(doc(&seen)["action"], json!(action), "from {before:?}");
        assert_eq!(
            text_of(case.lib_root(), "CLAUDE.md"),
            after,
            "from {before:?}"
        );
    }
    let seen = art(&mut case, onboard(Onboard::Apply));
    assert_eq!(
        doc(&seen)["action"],
        json!("unchanged"),
        "only the first block is considered"
    );

    // GD66: every update keeps the newline after the old end marker.
    plain(
        &mut case,
        write(
            "CLAUDE.md",
            "<!-- luuna-beacon:start -->\nold\n<!-- luuna-beacon:end -->\n",
        ),
    );
    art(&mut case, onboard(Onboard::Apply));
    let once = text_of(case.lib_root(), "CLAUDE.md");
    assert_eq!(once, format!("{GO_SNIPPET}\n"));
    plain(
        &mut case,
        write("CLAUDE.md", &once.replace(ONBOARD_VERSION, "<!-- v:2 -->")),
    );
    art(&mut case, onboard(Onboard::Apply));
    assert_eq!(
        text_of(case.lib_root(), "CLAUDE.md"),
        format!("{GO_SNIPPET}\n\n")
    );
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test artifacts_commands -- --ignored"]
fn onboard_check_reports_missing_outdated_current_including_disagreement() {
    let mut case = Case::with_project(PROJECT);
    let seen = art(&mut case, onboard(Onboard::Check));
    assert_eq!(doc(&seen)["status"], json!("missing"));
    assert_eq!(seen.documents[0]["file"], json!("<root>/CLAUDE.md"));
    assert!(
        !case.lib_root().join("CLAUDE.md").exists(),
        "--check writes nothing"
    );

    let reversed = format!("{ONBOARD_END}\n{ONBOARD_START}\n");
    plain(&mut case, write("CLAUDE.md", &reversed));
    assert_eq!(
        doc(&art(&mut case, onboard(Onboard::Check)))["status"],
        json!("outdated")
    );
    let reversed_current = format!("{reversed}{ONBOARD_VERSION}\n");
    plain(&mut case, write("CLAUDE.md", &reversed_current));
    assert_eq!(
        doc(&art(&mut case, onboard(Onboard::Check)))["status"],
        json!("current"),
        "GD65: --check accepts the markers in any order"
    );
    let seen = art(&mut case, onboard(Onboard::Apply));
    assert_eq!(
        doc(&seen)["action"],
        json!("appended"),
        "GD65: apply needs end after start"
    );
    assert_eq!(
        text_of(case.lib_root(), "CLAUDE.md"),
        format!("{reversed_current}\n{GO_SNIPPET}")
    );
    assert_eq!(
        doc(&art(&mut case, onboard(Onboard::Check)))["status"],
        json!("current")
    );

    plain(
        &mut case,
        write(
            "CLAUDE.md",
            &format!("{ONBOARD_START}\nold\n{ONBOARD_END}\n"),
        ),
    );
    assert_eq!(
        doc(&art(&mut case, onboard(Onboard::Check)))["status"],
        json!("outdated")
    );
    art(&mut case, onboard(Onboard::Apply));
    assert_eq!(
        doc(&art(&mut case, onboard(Onboard::Check)))["status"],
        json!("current")
    );
    plain(
        &mut case,
        write("CLAUDE.md", &format!("{ONBOARD_START} only\n")),
    );
    assert_eq!(
        doc(&art(&mut case, onboard(Onboard::Check)))["status"],
        json!("missing")
    );
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test artifacts_commands -- --ignored"]
fn onboard_target_order_and_symlinks_like_go() {
    let mut case = Case::with_project(PROJECT);
    plain(&mut case, write("AGENTS.md", "agents\n"));
    let seen = art(&mut case, onboard(Onboard::Apply));
    assert_eq!(seen.documents[0]["file"], json!("<root>/AGENTS.md"));
    plain(&mut case, write(".claude/CLAUDE.md", "dot\n"));
    let seen = art(&mut case, onboard(Onboard::Apply));
    assert_eq!(seen.documents[0]["file"], json!("<root>/.claude/CLAUDE.md"));
    plain(&mut case, write("CLAUDE.md", "top\n"));
    let seen = art(&mut case, onboard(Onboard::Apply));
    assert_eq!(seen.documents[0]["file"], json!("<root>/CLAUDE.md"));
    let seen = art(&mut case, onboard(Onboard::Check));
    assert_eq!(seen.documents[0]["file"], json!("<root>/CLAUDE.md"));

    // GD68: a CLAUDE.md link is followed and its target written.
    let mut case = Case::with_project(PROJECT);
    art(&mut case, tree("docs", vec![entry("real.md", "real\n")]));
    art(&mut case, symlink("CLAUDE.md", "docs/real.md"));
    let seen = art(&mut case, onboard(Onboard::Apply));
    assert_eq!(doc(&seen)["action"], json!("appended"));
    assert_eq!(
        text_of(case.lib_root(), "docs/real.md"),
        format!("real\n\n{GO_SNIPPET}")
    );
    assert!(fs::symlink_metadata(case.lib_root().join("CLAUDE.md"))
        .unwrap()
        .file_type()
        .is_symlink());

    // A dangling CLAUDE.md link: Go creates the link's target.
    let mut case = Case::with_project(PROJECT);
    art(&mut case, folder("notes"));
    art(&mut case, symlink("CLAUDE.md", "notes/missing.md"));
    let seen = art(&mut case, onboard(Onboard::Apply));
    assert_eq!(doc(&seen)["action"], json!("created"));
    assert_eq!(text_of(case.lib_root(), "notes/missing.md"), GO_SNIPPET);

    // A symlinked .claude folder.
    let mut case = Case::with_project(PROJECT);
    art(
        &mut case,
        tree("shared", vec![entry("CLAUDE.md", "shared\n")]),
    );
    art(&mut case, symlink(".claude", "shared"));
    let seen = art(&mut case, onboard(Onboard::Apply));
    assert_eq!(seen.documents[0]["file"], json!("<root>/.claude/CLAUDE.md"));
    assert_eq!(
        text_of(case.lib_root(), "shared/CLAUDE.md"),
        format!("shared\n\n{GO_SNIPPET}")
    );
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test artifacts_commands -- --ignored"]
fn onboard_stdout_prints_the_snippet_and_writes_nothing() {
    let mut pair = Pair::new(Git::Available);
    let seen = pair.art(onboard(Onboard::Stdout));
    assert_eq!(
        seen.stdout,
        GO_SNIPPET.as_bytes(),
        "GD67: --stdout ignores --json"
    );
    assert!(seen.documents.is_empty());
    assert!(
        names_under(pair.lib_root()).is_empty(),
        "--stdout writes nothing"
    );
    pair.step(write("CLAUDE.md", "keep\n"));
    let seen = pair.art(onboard(Onboard::Stdout));
    assert_eq!(seen.stdout, GO_SNIPPET.as_bytes());
    assert_eq!(text_of(pair.lib_root(), "CLAUDE.md"), "keep\n");
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test artifacts_commands -- --ignored"]
fn onboard_never_migrates_legacy_layout() {
    let mut pair = Pair::new(Git::Available);
    pair.step(write(".canopy/config.yaml", "project: legacy\n"));
    pair.step(write(".gitignore", ".canopy/\n"));
    let seen = pair.art(onboard(Onboard::Apply));
    assert_eq!(seen.code, 0);
    assert!(!seen.migrated, "GD70: onboard never runs the legacy move");
    assert!(pair.lib_root().join(".canopy/config.yaml").is_file());
    assert!(!pair.lib_root().join(".luuna").exists());
    assert_eq!(text_of(pair.lib_root(), ".gitignore"), ".canopy/\n");
    pair.art(onboard(Onboard::Check));
    pair.step(write(".luuna/beacon/config.yaml", "project: both\n"));
    let seen = pair.art(onboard(Onboard::Apply));
    assert_eq!(seen.code, 0, "both layouts do not stop onboard");
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test artifacts_commands -- --ignored"]
fn onboard_write_errors_are_reported_like_go() {
    let mut case = Case::with_project(PROJECT);
    plain(&mut case, write("CLAUDE.md", "keep\n"));
    art(&mut case, chmod("CLAUDE.md", 0o444));
    assert_unwritable(&case.lib_root().join("CLAUDE.md"));
    art_os_error(
        &mut case,
        onboard(Onboard::Apply),
        "open <root>/CLAUDE.md: ",
    );
    assert_eq!(
        text_of(case.lib_root(), "CLAUDE.md"),
        "keep\n",
        "DL7: the error is reported"
    );
    assert_eq!(
        doc(&art(&mut case, onboard(Onboard::Check)))["status"],
        json!("missing")
    );

    art(&mut case, chmod("CLAUDE.md", 0o644));
    plain(
        &mut case,
        write(
            "CLAUDE.md",
            &format!("{ONBOARD_START}\nold\n{ONBOARD_END}\n"),
        ),
    );
    art(&mut case, chmod("CLAUDE.md", 0o444));
    art_os_error(
        &mut case,
        onboard(Onboard::Apply),
        "open <root>/CLAUDE.md: ",
    );
    assert_eq!(
        doc(&art(&mut case, onboard(Onboard::Check)))["status"],
        json!("outdated")
    );
    art(&mut case, chmod("CLAUDE.md", 0o644));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test artifacts_commands -- --ignored"]
fn onboard_refuses_an_unreadable_target_and_leaves_it_unchanged() {
    // A target that can be written but not read: Go reads it as empty and
    // writes the snippet over it (GD64); the library refuses (RD55).
    let mut pair = Pair::new(Git::Available);
    pair.step(write("CLAUDE.md", "the user's notes\n"));
    pair.art(chmod("CLAUDE.md", 0o200));
    let target = pair.lib_root().join("CLAUDE.md");
    assert_unreadable(&target);
    let seen = pair.art(onboard(Onboard::Check));
    assert_eq!(
        seen.raw[0]["status"],
        json!("missing"),
        "--check keeps Go's behaviour"
    );
    let names_before = names_under(pair.lib_root());

    let apart = pair.apart(onboard(Onboard::Apply));
    assert_eq!(apart.go.code, 0);
    assert_eq!(apart.go.raw[0]["action"], json!("created"));
    assert_eq!(
        read_protected(&pair.go_root().join("CLAUDE.md")),
        GO_SNIPPET.as_bytes(),
        "evidence: Go replaced the user's notes with the snippet"
    );
    let os_error = fs::read(&target).expect_err("still unreadable").to_string();
    assert_eq!(apart.lib.code, 1);
    assert_eq!(apart.lib.raw[0]["command"], json!("onboard"));
    assert_eq!(
        apart.lib.raw[0]["error"],
        json!(format!(
            "refusing to overwrite {}: it cannot be read: {os_error}",
            target.display()
        ))
    );
    assert_eq!(mode_of(&target), 0o200);
    assert_eq!(read_protected(&target), b"the user's notes\n");
    assert_eq!(names_under(pair.lib_root()), names_before, "no new file");

    // A target that is a folder: Go fails at the write, the library refuses.
    let mut pair = Pair::new(Git::Available);
    pair.step(write("CLAUDE.md/inner.md", "inner\n"));
    let target = pair.lib_root().join("CLAUDE.md");
    let names_before = names_under(pair.lib_root());
    let apart = pair.apart(onboard(Onboard::Apply));
    assert_eq!(apart.go.code, 1);
    assert!(
        apart
            .go
            .error
            .as_deref()
            .unwrap()
            .starts_with("open <root>/CLAUDE.md: "),
        "{:?}",
        apart.go.error
    );
    let os_error = fs::read(&target)
        .expect_err("a folder cannot be read")
        .to_string();
    assert_eq!(
        apart.lib.raw[0]["error"],
        json!(format!(
            "refusing to overwrite {}: it cannot be read: {os_error}",
            target.display()
        ))
    );
    assert_eq!(text_of(pair.lib_root(), "CLAUDE.md/inner.md"), "inner\n");
    assert_eq!(names_under(pair.lib_root()), names_before);
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test artifacts_commands -- --ignored"]
fn doctor_on_an_empty_root_and_a_fresh_store() {
    let mut pair = Pair::new(Git::Available);
    let seen = pair.art(doctor(false));
    assert_eq!(seen.code, 1);
    let report = &seen.raw[0];
    assert_eq!(
        check_names(report),
        [
            "config",
            "prompts-integrity",
            "schemas-integrity",
            "artifacts-integrity",
            "inheritance",
            "stale-locks"
        ]
    );
    assert_eq!(check(report, "config")["status"], json!("fail"));
    assert_eq!(report["summary"], json!({"pass": 5, "warn": 0, "fail": 1}));
    assert_eq!(
        seen.raw[1]["error"],
        json!("doctor found failures"),
        "GD71: two documents"
    );
    assert!(
        names_under(pair.lib_root()).is_empty(),
        "doctor writes nothing"
    );

    pair.step(Step::Init);
    let seen = pair.art(doctor(false));
    assert_eq!(seen.code, 0);
    assert_eq!(seen.raw.len(), 1);
    assert_eq!(seen.raw[0]["success"], json!(true));
    assert_eq!(
        seen.raw[0]["summary"],
        json!({"pass": 6, "warn": 0, "fail": 0})
    );
    assert_eq!(check(&seen.raw[0], "config")["details"], Value::Null);
    let seen = pair.art(doctor(true));
    assert_eq!(seen.code, 0);
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test artifacts_commands -- --ignored"]
fn doctor_reports_corrupt_prompt_schema_and_artifact_stores() {
    let mut pair = Pair::new(Git::Available);
    pair.step(Step::Init);
    pair.step(write(".luuna/beacon/prompts.jsonl", "not json\n"));
    pair.step(write(".luuna/beacon/schemas.jsonl", "{\"name\":5}\n"));
    let good = record(SKILL, "good", 1, vec![utf8_file("SKILL.md", "g\n")]);
    let bad = record(
        SKILL,
        "bad",
        1,
        vec![utf8_file("SKILL.md", "b\n").with_sha256(&digest(b"x"))],
    )
    .with_id("artifact-b1");
    let empty = record(AGENT, "empty", 1, Vec::new()).with_id("artifact-b2");
    pair.step(write_artifacts(&[good.line(), bad.line(), empty.line()]));
    let seen = pair.art(doctor(false));
    assert_eq!(seen.code, 1);
    let report = &seen.raw[0];
    assert_eq!(
        check_names(report),
        [
            "config",
            "prompts-integrity",
            "schemas-integrity",
            "artifacts-integrity",
            "stale-locks"
        ],
        "GD73: no inheritance check when prompts do not parse"
    );
    assert_eq!(
        check(report, "artifacts-integrity")["details"],
        json!([
            "artifact hash mismatch: SKILL.md",
            "invalid artifact record"
        ]),
        "GD72: verify details carry no name or version"
    );
    assert_eq!(
        check(report, "artifacts-integrity")["message"],
        json!("Artifact store is invalid")
    );

    pair.step(write(".luuna/beacon/artifacts.jsonl", "{\n"));
    let seen = pair.art(doctor(false));
    assert_eq!(
        check(&seen.raw[0], "artifacts-integrity")["message"],
        json!("Artifact store is invalid"),
        "GD72: the same message for a parse failure"
    );
    pair.step(write(".luuna/beacon/config.yaml", "project: [\n"));
    let seen = pair.art(doctor(false));
    assert_eq!(check(&seen.raw[0], "config")["status"], json!("fail"));
    pair.both(|root, _| fs::remove_file(root.join(".luuna/beacon/config.yaml")).unwrap());
    pair.both(|root, _| fs::remove_file(root.join(".luuna/beacon/prompts.jsonl")).unwrap());
    let seen = pair.art(doctor(false));
    assert_eq!(
        check(&seen.raw[0], "prompts-integrity")["status"],
        json!("pass")
    );
    pair.both(|root, _| fs::create_dir(root.join(".luuna/beacon/prompts.jsonl")).unwrap());
    let seen = pair.art(doctor(true));
    assert_eq!(
        check(&seen.raw[0], "prompts-integrity")["status"],
        json!("fail")
    );
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test artifacts_commands -- --ignored"]
fn doctor_reports_broken_inheritance_sorted() {
    let mut case = ready();
    let config = project_config(&case);
    plain(&mut case, config);
    let prompt = |id: &str, name: &str, extra: Value| {
        prompt_record(id, name, 1, STAMP, &[("Task", name)], extra)
    };
    let lines = [
        prompt("artifact-a1", "base", json!({})),
        prompt("artifact-a2", "child", json!({"extends": "base"})),
        prompt("artifact-a3", "orphan", json!({"extends": "ghost"})),
        prompt("artifact-a4", "mixed", json!({"mixins": ["nowhere"]})),
        prompt("artifact-a5", "loop-a", json!({"extends": "loop-b"})),
        prompt("artifact-a6", "loop-b", json!({"extends": "loop-a"})),
        prompt(
            "artifact-a7",
            "also-orphan",
            json!({"extends": "ghost", "mixins": ["base"]}),
        ),
    ];
    plain(
        &mut case,
        write(".luuna/beacon/prompts.jsonl", &lines.concat()),
    );
    let seen = art(&mut case, doctor(false));
    assert_eq!(seen.code, 1);
    let inheritance = check(&seen.documents[0], "inheritance");
    assert_eq!(inheritance["status"], json!("fail"));
    assert_eq!(
        inheritance["message"],
        json!("Broken inheritance references")
    );
    let details: Vec<String> = inheritance["details"]
        .as_array()
        .unwrap()
        .iter()
        .map(|detail| detail.as_str().unwrap().to_owned())
        .collect();
    let mut sorted = details.clone();
    sorted.sort();
    assert_eq!(details, sorted, "the hook sorts the map-ordered details");
    assert_eq!(details.len(), 5);
    assert!(details.contains(&"orphan: prompt \"ghost\" not found".to_owned()));
    assert!(details.contains(&"mixed: prompt \"nowhere\" not found".to_owned()));
    assert!(details.iter().any(|detail| detail.starts_with("loop-a: ")));
    assert!(details.iter().any(|detail| detail.starts_with("loop-b: ")));
    let go_details = check(&seen.raw[0], "inheritance")["details"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(go_details, 5, "the hook keeps every value");
    art(&mut case, doctor(false));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test artifacts_commands -- --ignored"]
fn doctor_stale_locks_warn_and_fix() {
    let mut case = ready();
    plain(&mut case, write(".luuna/beacon/old.lock", ""));
    plain(&mut case, set_mtime(".luuna/beacon/old.lock", 120));
    plain(&mut case, write(".luuna/beacon/prompts.jsonl.lock", "123"));
    plain(
        &mut case,
        set_mtime(".luuna/beacon/prompts.jsonl.lock", 3600),
    );
    plain(&mut case, write(".luuna/beacon/fresh.lock", ""));
    plain(&mut case, write(".luuna/beacon/notalock.txt", ""));
    plain(&mut case, set_mtime(".luuna/beacon/notalock.txt", 3600));
    let seen = art(&mut case, doctor(false));
    assert_eq!(seen.code, 0, "a warning is not a failure");
    let stale = check(&seen.raw[0], "stale-locks");
    assert_eq!(stale["status"], json!("warn"));
    assert_eq!(stale["message"], json!("2 stale lock file(s)"));
    assert_eq!(stale["details"], json!(["old.lock", "prompts.jsonl.lock"]));
    assert_eq!(stale["fixable"], json!(true));
    assert_eq!(
        seen.raw[0]["summary"],
        json!({"pass": 5, "warn": 1, "fail": 0})
    );

    let seen = art(&mut case, doctor(true));
    let stale = check(&seen.raw[0], "stale-locks");
    assert_eq!(
        stale["status"],
        json!("pass"),
        "DL8: --fix removes them and clears the warning"
    );
    assert_eq!(stale["fixable"], json!(false));
    let store = case.lib_root().join(".luuna/beacon");
    assert!(!store.join("old.lock").exists());
    assert!(!store.join("prompts.jsonl.lock").exists());
    assert!(store.join("fresh.lock").exists());
    assert!(store.join("notalock.txt").exists());
    art(&mut case, doctor(false));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test artifacts_commands -- --ignored"]
fn doctor_refused_migration_is_an_error() {
    let mut case = ready();
    plain(&mut case, write(".canopy/config.yaml", "project: legacy\n"));
    let seen = art(&mut case, doctor(false));
    assert_eq!(
        error(&seen),
        "both .canopy and .luuna/beacon exist; merge or remove one before continuing"
    );
    assert_eq!(seen.raw.len(), 1, "no report is printed");
    assert_eq!(seen.raw[0]["command"], json!("doctor"));

    let mut case = Case::with_project(PROJECT);
    art(
        &mut case,
        tree(
            "legacy-real",
            vec![entry("config.yaml", "project: legacy\n")],
        ),
    );
    on_both(&mut case, |root| {
        std::os::unix::fs::symlink("legacy-real", root.join(".canopy")).unwrap();
    });
    let seen = art(&mut case, doctor(true));
    assert_eq!(
        error(&seen),
        "refusing to migrate symlinked legacy directory"
    );

    let mut case = Case::with_project(PROJECT);
    plain(&mut case, write(".canopy/config.yaml", "project: legacy\n"));
    let seen = art(&mut case, doctor(false));
    assert!(seen.migrated, "doctor runs the legacy move");
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test artifacts_commands -- --ignored"]
fn sync_status_nothing_and_commit_in_a_temporary_repository() {
    let mut pair = Pair::new(Git::Repository);
    pair.step(Step::Init);
    let seen = pair.art(sync(true));
    assert_eq!(seen.code, 0);
    assert_eq!(seen.raw[0]["uncommitted"], json!(true));
    assert!(!seen.raw[0]["files"].as_array().unwrap().is_empty());
    assert!(
        pair.lib_git(&["log", "--format=%s"]).is_empty(),
        "--status commits nothing"
    );

    let seen = pair.art(sync(false));
    assert_eq!(seen.raw[0]["committed"], json!(true));
    assert_eq!(
        seen.documents[0]["message"],
        json!(format!("luuna-beacon: sync {}", utc_day()))
    );
    assert_eq!(
        pair.lib_git(&["log", "--format=%s%x00%an%x00%ae"]),
        format!(
            "luuna-beacon: sync {}\0Beacon Test\0beacon-test@example.invalid\n",
            utc_day()
        )
    );

    let seen = pair.art(sync(true));
    assert_eq!(
        seen.raw[0],
        json!({"success": true, "command": "sync", "uncommitted": false, "files": []})
    );
    let seen = pair.art(sync(false));
    assert_eq!(
        seen.raw[0],
        json!({"success": true, "command": "sync", "committed": false, "message": "Nothing to commit"})
    );
    assert_eq!(pair.lib_git(&["log", "--format=%s"]).lines().count(), 1);
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test artifacts_commands -- --ignored"]
fn sync_commits_the_whole_index_like_go() {
    let mut pair = Pair::new(Git::Repository);
    pair.step(Step::Init);
    pair.art(tree(
        "docs",
        vec![entry("notes.txt", "staged by the user\n")],
    ));
    pair.art(tree("loose", vec![entry("untracked.txt", "not staged\n")]));
    pair.git_both(&["add", "docs/notes.txt"]);
    let seen = pair.art(sync(false));
    assert_eq!(seen.raw[0]["committed"], json!(true));
    let committed = pair.lib_git(&["show", "--name-status", "--format=", "HEAD"]);
    assert!(
        committed.contains("A\tdocs/notes.txt"),
        "GD75: commit takes the whole index: {committed}"
    );
    assert!(committed.contains("A\t.luuna/beacon/config.yaml"));
    assert!(!committed.contains("untracked.txt"));
    let status = pair.lib_git(&["status", "--porcelain=v1", "--untracked-files=all"]);
    assert!(status.contains("?? loose/untracked.txt"));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test artifacts_commands -- --ignored"]
fn sync_status_first_line_loses_a_character_like_go() {
    let mut pair = Pair::new(Git::Repository);
    pair.step(Step::Init);
    pair.git_both(&["add", ".luuna/beacon"]);
    pair.git_both(&["add", ".gitattributes"]);
    pair.git_both(&["commit", "-q", "-m", "seed"]);
    let prompt = prompt_record("artifact-a1", "p", 1, STAMP, &[("Task", "t")], json!({}));
    pair.step(write(".luuna/beacon/prompts.jsonl", &prompt));
    pair.step(write(".luuna/beacon/schemas.jsonl", "\n"));
    pair.both(|root, _| {
        fs::write(root.join(".luuna/beacon/caf\u{e9} file.txt"), b"quoted\n").unwrap();
    });
    let seen = pair.art(sync(true));
    let files: Vec<String> = seen.raw[0]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        files[0], "luuna/beacon/prompts.jsonl",
        "GD76: the first line loses a character"
    );
    assert_eq!(files[1], ".luuna/beacon/schemas.jsonl");
    assert!(
        files[2].starts_with('"'),
        "GD76: quoted paths keep their quotes: {files:?}"
    );
    let seen = pair.art(sync(false));
    assert_eq!(seen.raw[0]["files"][0], json!("luuna/beacon/prompts.jsonl"));
    pair.art(sync(true));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test artifacts_commands -- --ignored"]
fn sync_outside_a_repository_and_without_git() {
    let mut pair = Pair::new(Git::Available);
    pair.step(Step::Init);
    let seen = pair.art(sync(true));
    assert_eq!(seen.code, 1);
    assert!(
        seen.error
            .as_deref()
            .unwrap()
            .starts_with("git status: fatal: not a git repository"),
        "{:?}",
        seen.error
    );
    pair.art(sync(false));

    let mut pair = Pair::new(Git::Missing);
    pair.step(Step::Init);
    let seen = pair.art(sync(false));
    assert_eq!(
        seen.error.as_deref(),
        Some("git status: "),
        "GD77: no detail when git cannot start"
    );
    pair.art(sync(true));

    let mut pair = Pair::new(Git::Missing);
    pair.step(write(".canopy/config.yaml", "project: legacy\n"));
    let seen = pair.art(sync(true));
    assert!(seen.migrated, "sync runs the legacy move before git");
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test artifacts_commands -- --ignored"]
fn sync_reports_git_failures_with_combined_output() {
    let mut pair = Pair::new(Git::Repository);
    pair.step(Step::Init);
    pair.both(|root, _| fs::write(root.join(".git/index.lock"), b"").unwrap());
    let seen = pair.art(sync(false));
    assert!(
        seen.error
            .as_deref()
            .unwrap()
            .starts_with("git add: fatal: Unable to create '<root>/.git/index.lock'"),
        "{:?}",
        seen.error
    );
    pair.both(|root, _| fs::remove_file(root.join(".git/index.lock")).unwrap());

    // A held branch lock: `git add` ran, then the commit fails (GD77).
    let head = fs::read_to_string(pair.lib_root().join(".git/HEAD")).unwrap();
    let branch = head
        .trim_end()
        .strip_prefix("ref: ")
        .expect("HEAD names a branch")
        .to_owned();
    let lock = format!("{branch}.lock");
    pair.both(|root, _| fs::write(root.join(".git").join(&lock), b"").unwrap());
    let seen = pair.art(sync(false));
    assert!(
        seen.error.as_deref().unwrap().starts_with("git commit: "),
        "{:?}",
        seen.error
    );
    assert!(
        pair.lib_git(&["diff", "--cached", "--name-status"])
            .contains(".luuna/beacon/config.yaml"),
        "the store was staged before the commit failed"
    );
    pair.both(|root, _| fs::remove_file(root.join(".git").join(&lock)).unwrap());
    let seen = pair.art(sync(false));
    assert_eq!(seen.raw[0]["committed"], json!(true));
}
