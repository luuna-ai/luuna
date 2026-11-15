//! Owner: plan s3-beacon-lib-3. Reference tests of the artifacts commands against the Go tool.
//!
//! Each Go-dependent test of the plan carries the ignore string
//! `#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]`.
//!
//! The skill, agent and bundle commands run step by step through the frozen
//! `Case` (`Step::Plan` with `ArtifactStep`, AS54), which compares exit
//! status, documents, errors and the store after every step; the files
//! outside the store are compared by `compare_outside`. Cases whose
//! documents name a folder outside the project, and the A8 refusals (where
//! Go and the library differ by design), run through `Pair` (AS53).
//!
//! These tests are written against the library contract of the plan and
//! compile once the library exists.
#![cfg(unix)]

mod support;

use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use support::artifacts::*;
use support::case::{store_paths, Case, Seen, Step};

/// A case in the `artifact` project (AS50), initialised on both sides.
fn ready() -> Case {
    let mut case = Case::with_project(PROJECT);
    plain(&mut case, Step::Init);
    case
}

/// A skill source tree with `SKILL.md` and one more file.
fn skill_tree(dir: &str, body: &str) -> ArtifactStep {
    tree(
        dir,
        vec![entry("SKILL.md", body), entry("notes/a.txt", "a\n")],
    )
}

fn error(seen: &Seen) -> &str {
    assert_eq!(seen.code, 1, "the step was expected to fail");
    seen.error.as_deref().expect("an error document")
}

fn ok(seen: &Seen) -> &Value {
    assert_eq!(
        seen.code, 0,
        "the step was expected to succeed: {:?}",
        seen.error
    );
    &seen.raw[0]
}

fn paths_of(artifact: &Value) -> Vec<String> {
    artifact["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|file| file["path"].as_str().expect("path").to_owned())
        .collect()
}

fn lines_of(root: &Path) -> usize {
    fs::read(root.join(".luuna/beacon/artifacts.jsonl"))
        .map(|bytes| {
            bytes
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .count()
        })
        .unwrap_or(0)
}

const NAME_RULE: &str = "artifact name must be 1-80 lowercase letters, digits, dots, dashes, or underscores, beginning with a letter or digit";

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn skill_import_list_show_verify_history_read() {
    let mut case = ready();
    art(&mut case, skill_tree("src/hello", "# Hello\n"));
    let seen = art(&mut case, import(SKILL, "src/hello", "hello"));
    let stored = &ok(&seen)["artifact"];
    assert_eq!(seen.raw[0]["command"], json!("skill import"));
    assert_eq!(stored["kind"], json!("skill"));
    assert_eq!(stored["name"], json!("hello"));
    assert_eq!(stored["version"], json!(1));
    assert_eq!(paths_of(stored), ["SKILL.md", "notes/a.txt"]);
    assert_eq!(stored["files"][0]["encoding"], json!("utf8"));
    assert_eq!(stored["files"][0]["content"], json!("# Hello\n"));
    assert!(stored["id"].as_str().unwrap().starts_with("artifact-"));

    let seen = art(&mut case, list(SKILL));
    assert_eq!(ok(&seen)["count"], json!(1));
    art(&mut case, show(SKILL, "hello"));
    assert_eq!(
        ok(&art(&mut case, verify(SKILL, None)))["checked"],
        json!(1)
    );
    assert_eq!(
        ok(&art(&mut case, verify(SKILL, Some("hello"))))["checked"],
        json!(1)
    );
    let seen = art(&mut case, history(SKILL, "hello"));
    assert_eq!(ok(&seen)["name"], json!("hello"));
    let seen = art(&mut case, read(SKILL, "hello", "SKILL.md"));
    assert_eq!(ok(&seen)["file"]["content"], json!("# Hello\n"));
    assert_eq!(seen.raw[0]["version"], json!(1));
    let seen = art(&mut case, read(SKILL, "hello", "missing.md"));
    assert_eq!(error(&seen), "file \"missing.md\" not found");

    art(
        &mut case,
        tree("src/hello", vec![entry("SKILL.md", "# Hello again\n")]),
    );
    let seen = art(&mut case, update(SKILL, "hello", "src/hello"));
    assert_eq!(seen.raw[0]["command"], json!("skill update"));
    assert_eq!(ok(&seen)["artifact"]["version"], json!(2));
    let seen = art(&mut case, list(SKILL));
    assert_eq!(ok(&seen)["artifacts"][0]["version"], json!(2));
    let seen = art(&mut case, history(SKILL, "hello"));
    let versions: Vec<&Value> = ok(&seen)["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|version| &version["version"])
        .collect();
    assert_eq!(versions, [&json!(1), &json!(2)]);
    assert_eq!(
        ok(&art(&mut case, show(SKILL, "hello@1")))["artifact"]["version"],
        json!(1)
    );
    let seen = art(&mut case, read(SKILL, "hello@1", "SKILL.md"));
    assert_eq!(ok(&seen)["file"]["content"], json!("# Hello\n"));
    assert_eq!(
        ok(&art(&mut case, verify(SKILL, None)))["checked"],
        json!(2)
    );

    assert_eq!(ok(&art(&mut case, list(AGENT)))["count"], json!(0));
    let seen = art(&mut case, show(AGENT, "hello"));
    assert_eq!(error(&seen), "agent \"hello\" not found");
    assert_eq!(seen.raw[0]["command"], json!("agent"));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn import_is_idempotent_on_the_manifest_hash_and_versions_keep_the_first_id() {
    let mut case = ready();
    art(&mut case, skill_tree("src/s", "one\n"));
    let first = art(&mut case, import(SKILL, "src/s", "same"));
    let first_id = ok(&first)["artifact"]["id"].clone();
    let first_lib_id = first.lib_raw[0]["artifact"]["id"].clone();
    assert_eq!(lines_of(case.lib_root()), 1);

    let again = art(&mut case, import(SKILL, "src/s", "same"));
    assert_eq!(
        ok(&again)["artifact"],
        first.raw[0]["artifact"],
        "the stored record is returned"
    );
    assert_eq!(again.lib_raw[0]["artifact"], first.lib_raw[0]["artifact"]);
    assert_eq!(
        lines_of(case.lib_root()),
        1,
        "an identical import writes nothing"
    );

    let same_by_update = art(&mut case, update(SKILL, "same", "src/s"));
    assert_eq!(ok(&same_by_update)["artifact"]["version"], json!(1));
    assert_eq!(lines_of(case.lib_root()), 1);

    art(&mut case, tree("src/s", vec![entry("SKILL.md", "two\n")]));
    let second = art(&mut case, import(SKILL, "src/s", "same"));
    assert_eq!(ok(&second)["artifact"]["version"], json!(2));
    assert_eq!(
        second.raw[0]["artifact"]["id"], first_id,
        "versions keep the first id"
    );
    assert_eq!(second.lib_raw[0]["artifact"]["id"], first_lib_id);
    assert_eq!(lines_of(case.lib_root()), 2);
    art(&mut case, import(SKILL, "src/s", "same"));
    assert_eq!(lines_of(case.lib_root()), 2);

    art(&mut case, chmod("src/s/notes/a.txt", 0o755));
    let third = art(&mut case, import(SKILL, "src/s", "same"));
    assert_eq!(
        ok(&third)["artifact"]["version"],
        json!(3),
        "the executable flag is in the manifest"
    );
    assert_eq!(
        third.raw[0]["artifact"]["files"][1]["executable"],
        json!(true)
    );
    assert_eq!(lines_of(case.lib_root()), 3);
    art(&mut case, history(SKILL, "same"));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn update_of_an_unknown_name_creates_version_one() {
    let mut case = ready();
    art(&mut case, skill_tree("src/u", "u\n"));
    let seen = art(&mut case, update(SKILL, "fresh", "src/u"));
    assert_eq!(seen.raw[0]["command"], json!("skill update"));
    assert_eq!(ok(&seen)["artifact"]["version"], json!(1));
    let seen = art(&mut case, update_args(SKILL, Some("fresh"), None));
    assert_eq!(error(&seen), "name and source path required");
    let seen = art(&mut case, update_args(SKILL, None, None));
    assert_eq!(error(&seen), "name and source path required");
    let seen = art(&mut case, update(SKILL, "Bad", "src/u"));
    assert_eq!(error(&seen), NAME_RULE);
    art_os_error(
        &mut case,
        update(SKILL, "other", "src/missing"),
        "lstat src/missing: ",
    );
    let seen = art(&mut case, update(AGENT, "fresh", "src/u"));
    assert_eq!(
        ok(&seen)["artifact"]["version"],
        json!(1),
        "kinds are separate"
    );
    art(&mut case, list(SKILL));
    art(&mut case, list(AGENT));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn agent_and_bundle_requirements_and_single_file_sources() {
    let mut case = ready();
    art(
        &mut case,
        tree("src/noskill", vec![entry("readme.md", "r\n")]),
    );
    let seen = art(&mut case, import(SKILL, "src/noskill", "noskill"));
    assert_eq!(error(&seen), "skill must contain SKILL.md");
    let seen = art(&mut case, import(BUNDLE, "src/noskill", "nobundle"));
    assert_eq!(
        error(&seen),
        "bundle must contain .claude-plugin/plugin.json"
    );
    let seen = art(&mut case, import(AGENT, "src/noskill", "anything"));
    assert_eq!(paths_of(&ok(&seen)["artifact"]), ["readme.md"]);

    art(
        &mut case,
        tree(
            "src/bundle",
            vec![
                entry(".claude-plugin/plugin.json", "{\"name\":\"b\"}\n"),
                entry("skills/x/SKILL.md", "x\n"),
                entry("README.md", "b\n"),
            ],
        ),
    );
    let seen = art(&mut case, import(BUNDLE, "src/bundle", "bundled"));
    assert_eq!(
        paths_of(&ok(&seen)["artifact"]),
        [
            ".claude-plugin/plugin.json",
            "README.md",
            "skills/x/SKILL.md"
        ]
    );

    art(
        &mut case,
        tree("src/single", vec![entry("reviewer.md", "# Reviewer\n")]),
    );
    let seen = art(
        &mut case,
        import(AGENT, "src/single/reviewer.md", "reviewer"),
    );
    assert_eq!(paths_of(&ok(&seen)["artifact"]), ["reviewer.md"]);
    let seen = art(&mut case, import(SKILL, "src/single/reviewer.md", "loose"));
    assert_eq!(paths_of(&ok(&seen)["artifact"]), ["reviewer.md"], "GD55");
    let seen = art(
        &mut case,
        import(BUNDLE, "src/single/reviewer.md", "loosebundle"),
    );
    assert_eq!(ok(&seen)["artifact"]["kind"], json!("bundle"));
    art_os_error(
        &mut case,
        import(SKILL, "src/missing", "missing"),
        "lstat src/missing: ",
    );
    art(&mut case, list(SKILL));
    art(&mut case, list(AGENT));
    art(&mut case, list(BUNDLE));
    art(&mut case, verify(BUNDLE, None));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn snapshot_skips_only_top_level_git_node_modules_and_luuna() {
    let mut case = ready();
    art(
        &mut case,
        tree(
            "src/s",
            vec![
                entry("SKILL.md", "s\n"),
                entry(".git/config", "[core]\n"),
                entry("node_modules/x.js", "x\n"),
                entry(".luuna/beacon/y.jsonl", "{}\n"),
                entry("nested/node_modules/z.js", "z\n"),
                entry("nested/.git/HEAD", "ref: refs/heads/x\n"),
                entry("nested/.luuna/k.txt", "k\n"),
                entry(".github/w.yml", "w\n"),
                entry("node_modulesx/a.txt", "a\n"),
            ],
        ),
    );
    art(&mut case, folder("src/s/empty-dir"));
    art(&mut case, folder("src/s/nested/empty"));
    let seen = art(&mut case, import(SKILL, "src/s", "skips"));
    assert_eq!(
        paths_of(&ok(&seen)["artifact"]),
        [
            ".github/w.yml",
            "SKILL.md",
            "nested/.git/HEAD",
            "nested/.luuna/k.txt",
            "nested/node_modules/z.js",
            "node_modulesx/a.txt",
        ]
    );

    art(
        &mut case,
        tree(
            "src/gitfile",
            vec![
                entry("SKILL.md", "g\n"),
                entry(".git", "gitdir: elsewhere\n"),
            ],
        ),
    );
    let seen = art(&mut case, import(SKILL, "src/gitfile", "gitfile"));
    assert_eq!(
        paths_of(&ok(&seen)["artifact"]),
        [".git", "SKILL.md"],
        "GD54"
    );
    art(&mut case, export(SKILL, "skips").out("out/skips"));
    art(&mut case, export(SKILL, "gitfile").out("out/gitfile"));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn snapshot_refuses_symlinks_and_credential_like_names() {
    let mut case = ready();
    art(&mut case, tree("src/real", vec![entry("SKILL.md", "r\n")]));
    art(&mut case, symlink("src/link", "real"));
    let seen = art(&mut case, import(SKILL, "src/link", "link"));
    assert_eq!(error(&seen), "source symlinks are not allowed");

    art(
        &mut case,
        tree("src/withlink", vec![entry("SKILL.md", "w\n")]),
    );
    art(&mut case, symlink("src/withlink/docs.md", "SKILL.md"));
    let seen = art(&mut case, import(SKILL, "src/withlink", "withlink"));
    assert_eq!(error(&seen), "source symlinks are not allowed: docs.md");

    art(
        &mut case,
        tree("src/dirlink", vec![entry("SKILL.md", "d\n")]),
    );
    art(&mut case, symlink("src/dirlink/sub", "../real"));
    let seen = art(&mut case, import(SKILL, "src/dirlink", "dirlink"));
    assert_eq!(error(&seen), "source symlinks are not allowed: sub");

    art(
        &mut case,
        tree("src/parent/skill", vec![entry("SKILL.md", "p\n")]),
    );
    art(&mut case, symlink("src/alias", "parent"));
    let seen = art(&mut case, import(SKILL, "src/alias/skill", "via-alias"));
    assert_eq!(
        paths_of(&ok(&seen)["artifact"]),
        ["SKILL.md"],
        "GD80: ancestor links are followed"
    );

    for (index, name) in [
        "api.token",
        "my-secret.txt",
        "PASSWORD.md",
        "tokenizer.py",
        "db.Secrets.yaml",
        "config/prod.password",
    ]
    .iter()
    .enumerate()
    {
        let dir = format!("src/cred{index}");
        art(
            &mut case,
            tree(&dir, vec![entry("SKILL.md", "c\n"), entry(name, "x\n")]),
        );
        let seen = art(&mut case, import(SKILL, &dir, &format!("cred{index}")));
        assert_eq!(
            error(&seen),
            format!("credential-like file rejected: {name}")
        );
    }
    art(
        &mut case,
        tree(
            "src/okdirs",
            vec![
                entry("SKILL.md", "o\n"),
                entry("secrets/readme.md", "r\n"),
                entry("tokens/list.md", "l\n"),
            ],
        ),
    );
    let seen = art(&mut case, import(SKILL, "src/okdirs", "okdirs"));
    assert_eq!(
        paths_of(&ok(&seen)["artifact"]),
        ["SKILL.md", "secrets/readme.md", "tokens/list.md"],
        "GD56: folder names are not checked"
    );
    art(
        &mut case,
        tree("src/one", vec![entry("deploy.token", "t\n")]),
    );
    let seen = art(&mut case, import(AGENT, "src/one/deploy.token", "deploy"));
    assert_eq!(error(&seen), "credential-like file rejected: deploy.token");
    assert_eq!(lines_of(case.lib_root()), 2);
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn artifact_names_are_validated_like_go() {
    let mut case = ready();
    art(&mut case, skill_tree("src/n", "n\n"));
    let long_ok = "a".repeat(80);
    let too_long = "a".repeat(81);
    for name in ["a", "0", "9lives", "ok.name_1-2", "a..b", long_ok.as_str()] {
        let seen = art(&mut case, import(SKILL, "src/n", name));
        assert_eq!(ok(&seen)["artifact"]["name"], json!(name));
    }
    for name in [
        "Upper",
        "-dash",
        "_under",
        ".dot",
        ".",
        "..",
        "a/b",
        "a b",
        "é",
        "aé",
        too_long.as_str(),
    ] {
        let seen = art(&mut case, import(SKILL, "src/n", name));
        assert_eq!(error(&seen), NAME_RULE, "name {name:?}");
    }
    let seen = art(&mut case, import(SKILL, "src/n", ""));
    assert_eq!(error(&seen), "--name is required");
    let seen = art(&mut case, update(SKILL, "Upper", "src/n"));
    assert_eq!(error(&seen), NAME_RULE);
    let seen = art(&mut case, show(SKILL, "Upper"));
    assert_eq!(error(&seen), "skill \"Upper\" not found");
    art(&mut case, list(SKILL));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn binary_empty_and_executable_files_round_trip() {
    let mut case = ready();
    art(
        &mut case,
        tree(
            "src/b",
            vec![
                entry("SKILL.md", "b\n"),
                exec_entry("bin/run.sh", "#!/bin/sh\necho hi\n"),
                entry("empty.txt", ""),
                entry_bytes(b"blob.bin", &[0x00, 0x9f, 0x92, 0x96, 0xff], false),
                entry_bytes(b"latin.txt", b"caf\xe9\n", false),
                entry_bytes(b"nul.txt", b"a\x00b", false),
                entry("replacement.txt", "\u{FFFD} is valid\n"),
            ],
        ),
    );
    let seen = art(&mut case, import(SKILL, "src/b", "binary"));
    let stored = &ok(&seen)["artifact"];
    let file = |path: &str| {
        stored["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["path"] == json!(path))
            .unwrap_or_else(|| panic!("{path} is stored"))
            .clone()
    };
    assert_eq!(file("blob.bin")["encoding"], json!("base64"));
    assert_eq!(
        file("blob.bin")["content"],
        json!(base64_std(&[0x00, 0x9f, 0x92, 0x96, 0xff]))
    );
    assert_eq!(file("latin.txt")["encoding"], json!("base64"));
    assert_eq!(file("nul.txt")["encoding"], json!("utf8"));
    assert_eq!(file("replacement.txt")["encoding"], json!("utf8"));
    assert_eq!(file("empty.txt")["content"], json!(""));
    assert_eq!(file("empty.txt")["sha256"], json!(digest(b"")));
    assert_eq!(file("bin/run.sh")["executable"], json!(true));
    assert_eq!(file("SKILL.md")["executable"], json!(false));

    art(&mut case, read(SKILL, "binary", "blob.bin"));
    art(&mut case, read(SKILL, "binary", "empty.txt"));
    art(&mut case, export(SKILL, "binary").target("claude"));
    let exported = case.lib_root().join(".claude/skills/binary");
    assert_eq!(mode_of(&exported.join("bin/run.sh")), 0o755);
    assert_eq!(mode_of(&exported.join("blob.bin")), 0o644);
    assert_eq!(
        fs::read(exported.join("blob.bin")).unwrap(),
        [0x00, 0x9f, 0x92, 0x96, 0xff]
    );
    assert_eq!(fs::read(exported.join("latin.txt")).unwrap(), b"caf\xe9\n");
    assert_eq!(fs::read(exported.join("empty.txt")).unwrap(), b"");
    art(&mut case, export(SKILL, "binary").target("codex"));

    art(&mut case, chmod("src/b/bin/run.sh", 0o644));
    let seen = art(&mut case, import(SKILL, "src/b", "binary"));
    assert_eq!(ok(&seen)["artifact"]["version"], json!(2));
    art(&mut case, export(SKILL, "binary").target("claude").force());
    assert_eq!(mode_of(&exported.join("bin/run.sh")), 0o644);
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn versioned_references_select_like_go() {
    let mut case = ready();
    for body in ["one\n", "two\n", "three\n"] {
        art(&mut case, tree("src/v", vec![entry("SKILL.md", body)]));
        art(&mut case, import(SKILL, "src/v", "v"));
    }
    let version = |seen: &Seen| seen.raw[0]["artifact"]["version"].clone();
    assert_eq!(version(&art(&mut case, show(SKILL, "v"))), json!(3));
    assert_eq!(version(&art(&mut case, show(SKILL, "v@2"))), json!(2));
    assert_eq!(version(&art(&mut case, show(SKILL, "v@2abc"))), json!(2));
    for reference in ["v@", "v@y", "v@4", "missing", "missing@1"] {
        let seen = art(&mut case, show(SKILL, reference));
        assert_eq!(error(&seen), format!("skill \"{reference}\" not found"));
    }
    for reference in ["v@0", "v@+2", "v@-1", "v@ 2", "v@2@3", "v@02", "v@1.5"] {
        art(&mut case, show(SKILL, reference));
    }
    let seen = art(&mut case, read(SKILL, "v@1", "SKILL.md"));
    assert_eq!(ok(&seen)["file"]["content"], json!("one\n"));
    let seen = art(&mut case, history(SKILL, "v@1"));
    assert_eq!(ok(&seen)["versions"].as_array().unwrap().len(), 3);
    art(&mut case, history(SKILL, "v@9"));
    assert_eq!(
        ok(&art(&mut case, verify(SKILL, Some("v@2"))))["checked"],
        json!(1)
    );
    art(&mut case, verify(SKILL, Some("v@9")));
    let seen = art(&mut case, export(SKILL, "v@1").out("out/v1"));
    assert_eq!(ok(&seen)["version"], json!(1));
    assert_eq!(
        fs::read_to_string(case.lib_root().join("out/v1/SKILL.md")).unwrap(),
        "one\n"
    );
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn argument_errors_come_after_the_store_prologue() {
    let mut case = ready();
    let expect = |case: &mut Case, step: ArtifactStep, text: &str| {
        let seen = art(case, step);
        assert_eq!(error(&seen), text);
        assert_eq!(
            seen.raw[0]["command"],
            json!("skill"),
            "GD60: the error's command is the kind"
        );
    };
    expect(
        &mut case,
        no_subcommand(SKILL),
        "artifact subcommand required",
    );
    expect(
        &mut case,
        import_args(SKILL, None, None),
        "source path required",
    );
    art(&mut case, skill_tree("src/a", "a\n"));
    expect(
        &mut case,
        import_args(SKILL, Some("src/a"), None),
        "--name is required",
    );
    expect(
        &mut case,
        import_args(SKILL, Some("src/a"), Some("")),
        "--name is required",
    );
    expect(
        &mut case,
        update_args(SKILL, None, None),
        "name and source path required",
    );
    expect(
        &mut case,
        update_args(SKILL, Some("a"), None),
        "name and source path required",
    );
    expect(&mut case, show_args(SKILL, None), "artifact name required");
    expect(
        &mut case,
        history_args(SKILL, None),
        "artifact name required",
    );
    expect(
        &mut case,
        read_args(SKILL, None, None),
        "artifact name required",
    );
    expect(
        &mut case,
        export_args(SKILL, None),
        "artifact name required",
    );
    expect(
        &mut case,
        unknown(SKILL, "frob"),
        "unknown skill subcommand: frob",
    );
    art(&mut case, import(SKILL, "src/a", "a"));
    expect(
        &mut case,
        read_args(SKILL, Some("a"), None),
        "file path required",
    );
    expect(
        &mut case,
        export(SKILL, "a"),
        "--out is required for this export",
    );
    expect(
        &mut case,
        export(SKILL, "a").target("other"),
        "--out is required for this export",
    );
    assert_eq!(
        ok(&art(&mut case, verify(AGENT, None)))["checked"],
        json!(0)
    );

    let bad = record("tool", "x", 1, vec![utf8_file("SKILL.md", "x\n")]);
    let good = record(SKILL, "a", 1, vec![utf8_file("SKILL.md", "a\n")]).with_id("artifact-b0");
    plain(&mut case, write_artifacts(&[good.line(), bad.line()]));
    let prologue = "invalid artifact record tool x@1: invalid artifact kind";
    expect(
        &mut case,
        no_subcommand(SKILL),
        "artifact subcommand required",
    );
    expect(&mut case, import_args(SKILL, None, None), prologue);
    expect(&mut case, import_args(SKILL, Some("src/a"), None), prologue);
    expect(&mut case, update_args(SKILL, None, None), prologue);
    expect(&mut case, show_args(SKILL, None), prologue);
    expect(&mut case, read_args(SKILL, Some("a"), None), prologue);
    expect(&mut case, export(SKILL, "a"), prologue);
    expect(&mut case, unknown(SKILL, "frob"), prologue);
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn corrupt_or_conflicting_records_fail_every_artifact_command() {
    let good = record(SKILL, "good", 1, vec![utf8_file("SKILL.md", "ok\n")]).with_id("artifact-b0");
    let files = || vec![utf8_file("SKILL.md", "x\n")];
    let conflicting = record(SKILL, "x", 1, vec![utf8_file("SKILL.md", "other\n")]);
    let cases: Vec<(Vec<String>, String)> = vec![
        (
            vec![record("tool", "x", 1, files()).line()],
            "invalid artifact record tool x@1: invalid artifact kind".to_owned(),
        ),
        (
            vec![record(SKILL, "Bad", 1, files()).line()],
            "invalid artifact record skill Bad@1: invalid artifact name: Bad".to_owned(),
        ),
        (
            vec![record(SKILL, "x", 0, files()).line()],
            "invalid artifact record skill x@0: invalid artifact record".to_owned(),
        ),
        (
            vec![record(SKILL, "x", 1, Vec::new()).line()],
            "invalid artifact record skill x@1: invalid artifact record".to_owned(),
        ),
        (
            vec![record(
                SKILL,
                "x",
                1,
                vec![utf8_file("SKILL.md", "x\n").with_sha256(&digest(b"y"))],
            )
            .line()],
            "invalid artifact record skill x@1: artifact hash mismatch: SKILL.md".to_owned(),
        ),
        (
            vec![record(SKILL, "x", 1, files())
                .with_sha256(&digest(b"[]"))
                .line()],
            "invalid artifact record skill x@1: artifact manifest hash mismatch".to_owned(),
        ),
        (
            vec![record(
                SKILL,
                "x",
                1,
                vec![base64_file("SKILL.md", b"abcdef").with_encoded("base64", "AAAA!AAA")],
            )
            .line()],
            "invalid artifact record skill x@1: illegal base64 data at input byte 4".to_owned(),
        ),
        (
            vec![record(SKILL, "x", 1, files()).line(), conflicting.line()],
            "conflicting artifact version: skill x@1".to_owned(),
        ),
    ];
    for (lines, expected) in cases {
        let mut case = ready();
        art(&mut case, skill_tree("src/n", "n\n"));
        let mut all = vec![good.line()];
        all.extend(lines);
        plain(&mut case, write_artifacts(&all));
        let before = fs::read(case.lib_root().join(".luuna/beacon/artifacts.jsonl")).unwrap();
        for step in [
            list(SKILL),
            list(AGENT),
            verify(SKILL, None),
            verify(SKILL, Some("good")),
            show(SKILL, "good"),
            history(SKILL, "good"),
            read(SKILL, "good", "SKILL.md"),
            export(SKILL, "good").out("out/good"),
            import(SKILL, "src/n", "n"),
            update(SKILL, "n", "src/n"),
            unknown(SKILL, "frob"),
        ] {
            let seen = art(&mut case, step);
            assert_eq!(error(&seen), expected);
        }
        assert_eq!(
            fs::read(case.lib_root().join(".luuna/beacon/artifacts.jsonl")).unwrap(),
            before,
            "a failing prologue writes nothing"
        );
        assert!(!case.lib_root().join("out").exists());
    }

    // Records Go reads as valid although they look odd (GD57).
    let crlf = record(
        SKILL,
        "crlf",
        1,
        vec![base64_file("SKILL.md", b"hello").with_encoded("base64", "aGVs\r\nbG8=")],
    );
    let hex = record(
        SKILL,
        "hexed",
        1,
        vec![utf8_file("SKILL.md", "68656c6c6f").with_encoded("hex", "68656c6c6f")],
    )
    .with_id("artifact-b1");
    let mut case = ready();
    plain(
        &mut case,
        write_artifacts(&[good.line(), crlf.line(), hex.line()]),
    );
    assert_eq!(
        ok(&art(&mut case, verify(SKILL, None)))["checked"],
        json!(3)
    );
    art(&mut case, read(SKILL, "crlf", "SKILL.md"));
    art(&mut case, read(SKILL, "hexed", "SKILL.md"));
    art(&mut case, export(SKILL, "crlf").out("out/crlf"));
    art(&mut case, export(SKILL, "hexed").out("out/hexed"));
    assert_eq!(
        fs::read(case.lib_root().join("out/crlf/SKILL.md")).unwrap(),
        b"hello"
    );
    assert_eq!(
        fs::read(case.lib_root().join("out/hexed/SKILL.md")).unwrap(),
        b"68656c6c6f"
    );
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn empty_agent_import_poisons_the_store_like_go() {
    let mut case = ready();
    art(&mut case, folder("src/empty"));
    let seen = art(&mut case, import(AGENT, "src/empty", "hollow"));
    assert_eq!(ok(&seen)["artifact"]["files"], json!([]));
    assert_eq!(lines_of(case.lib_root()), 1);
    let poisoned = "invalid artifact record agent hollow@1: invalid artifact record";
    assert_eq!(error(&art(&mut case, list(AGENT))), poisoned);
    assert_eq!(error(&art(&mut case, list(SKILL))), poisoned);
    art(&mut case, skill_tree("src/s", "s\n"));
    assert_eq!(
        error(&art(&mut case, import(SKILL, "src/s", "s"))),
        poisoned
    );
    let seen = art(&mut case, doctor(false));
    assert_eq!(seen.code, 1);
    assert_eq!(
        seen.raw[0]["checks"][3]["details"],
        json!(["invalid artifact record"])
    );
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn version_overflow_wraps_and_poisons_the_store_like_go() {
    let mut case = ready();
    let max = record(SKILL, "max", i64::MAX, vec![utf8_file("SKILL.md", "a\n")]);
    plain(&mut case, write_artifacts(&[max.line()]));
    let seen = art(&mut case, list(SKILL));
    assert_eq!(ok(&seen)["artifacts"][0]["version"], json!(i64::MAX));
    art(&mut case, tree("src/m", vec![entry("SKILL.md", "b\n")]));
    let seen = art(&mut case, import(SKILL, "src/m", "max"));
    assert_eq!(
        ok(&seen)["artifact"]["version"],
        json!(i64::MIN),
        "GD82: version + 1 wraps"
    );
    assert_eq!(lines_of(case.lib_root()), 2);
    let seen = art(&mut case, list(SKILL));
    assert_eq!(
        error(&seen),
        "invalid artifact record skill max@-9223372036854775808: invalid artifact record"
    );
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn unknown_and_case_variant_keys_are_kept_in_the_file_like_go() {
    let mut case = ready();
    let odd = record(SKILL, "keys", 1, vec![utf8_file("SKILL.md", "k\n")])
        .with_key("color", "\"blue\"")
        .with_key("nested", "{\"a\":[1,2,{\"b\":null}]}")
        .with_key("Kind", "\"skill\"")
        .with_key("NAME", "\"renamed\"");
    let variant = record(AGENT, "other", 2, vec![utf8_file("a.md", "a\n")])
        .with_id("artifact-b1")
        .with_key("VERSION", "3");
    let original = format!("{}{}", odd.line(), variant.line());
    plain(
        &mut case,
        Step::WriteFile {
            path: ".luuna/beacon/artifacts.jsonl".to_owned(),
            bytes: original.clone().into_bytes(),
        },
    );
    art(&mut case, list(SKILL));
    art(&mut case, list(AGENT));
    art(&mut case, show(SKILL, "renamed"));
    art(&mut case, show(SKILL, "keys"));
    art(&mut case, verify(SKILL, None));
    art(&mut case, verify(AGENT, None));
    art(&mut case, history(AGENT, "other"));
    art(&mut case, skill_tree("src/f", "f\n"));
    art(&mut case, import(SKILL, "src/f", "fresh"));
    let bytes = fs::read(case.lib_root().join(".luuna/beacon/artifacts.jsonl")).unwrap();
    assert!(
        bytes.starts_with(original.as_bytes()),
        "DL10: appending keeps every existing byte, unknown keys included"
    );
    assert_eq!(lines_of(case.lib_root()), 3);
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn skill_export_targets_claude_and_codex_with_a_marker() {
    let mut case = ready();
    art(&mut case, skill_tree("src/s", "s\n"));
    art(&mut case, import(SKILL, "src/s", "s"));
    let seen = art(&mut case, export(SKILL, "s").target("claude"));
    assert_eq!(seen.documents[0]["path"], json!("<root>/.claude/skills/s"));
    assert_eq!(seen.raw[0]["command"], json!("skill export"));
    let marker = case.lib_root().join(".claude/skills/s/.lbc-artifact.json");
    assert_eq!(
        fs::read_to_string(&marker).unwrap(),
        "{\"kind\":\"skill\",\"name\":\"s\"}"
    );
    assert_eq!(mode_of(&marker), 0o644);
    let seen = art(&mut case, export(SKILL, "s").target("codex"));
    assert_eq!(seen.documents[0]["path"], json!("<root>/.agents/skills/s"));
    let seen = art(&mut case, export(SKILL, "s").target("claude"));
    assert_eq!(
        error(&seen),
        "destination already exists: <root>/.claude/skills/s"
    );
    art(&mut case, export(SKILL, "s").target("claude").force());
    art(&mut case, export(SKILL, "s").target("codex").force());

    art(
        &mut case,
        tree("src/b", vec![entry(".claude-plugin/plugin.json", "{}\n")]),
    );
    art(&mut case, import(BUNDLE, "src/b", "b"));
    let seen = art(&mut case, export(BUNDLE, "b").target("claude"));
    assert_eq!(error(&seen), "--out is required for this export");
    art(&mut case, export(BUNDLE, "b").out("out/b"));
    let seen = art(&mut case, export(SKILL, "s").target("claude").out("out/s"));
    assert_eq!(
        seen.documents[0]["path"],
        json!("<root>/out/s"),
        "--out wins over --target"
    );
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn agent_export_single_file_and_directory_forms() {
    let mut case = ready();
    art(
        &mut case,
        tree("src/a", vec![entry("reviewer.md", "# R\n")]),
    );
    art(&mut case, import(AGENT, "src/a/reviewer.md", "reviewer"));
    let seen = art(&mut case, export(AGENT, "reviewer").target("claude"));
    assert_eq!(
        seen.documents[0]["path"],
        json!("<root>/.claude/agents/reviewer.md")
    );
    let file = case.lib_root().join(".claude/agents/reviewer.md");
    assert_eq!(fs::read_to_string(&file).unwrap(), "# R\n");
    assert_eq!(mode_of(&file), 0o644);
    let seen = art(&mut case, export(AGENT, "reviewer").target("claude"));
    assert_eq!(
        error(&seen),
        "destination already exists: <root>/.claude/agents/reviewer.md"
    );

    plain(
        &mut case,
        Step::WriteFile {
            path: ".claude/agents/reviewer.md".to_owned(),
            bytes: b"user edit\n".to_vec(),
        },
    );
    art(
        &mut case,
        export(AGENT, "reviewer").target("claude").force(),
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "# R\n",
        "DL5: --force replaces the file"
    );

    let seen = art(&mut case, export(AGENT, "reviewer").out("out/Custom.MD"));
    assert_eq!(seen.documents[0]["path"], json!("<root>/out/Custom.MD"));
    assert!(case.lib_root().join("out/Custom.MD").is_file());
    let seen = art(&mut case, export(AGENT, "reviewer").out("out/revdir"));
    assert_eq!(seen.documents[0]["path"], json!("<root>/out/revdir"));
    assert!(case.lib_root().join("out/revdir/reviewer.md").is_file());
    assert!(case
        .lib_root()
        .join("out/revdir/.lbc-artifact.json")
        .is_file());

    art(
        &mut case,
        tree("src/two", vec![entry("a.md", "a\n"), entry("b.md", "b\n")]),
    );
    art(&mut case, import(AGENT, "src/two", "pair"));
    let seen = art(&mut case, export(AGENT, "pair").target("claude"));
    assert_eq!(
        error(&seen),
        "agent export requires exactly one stored file"
    );
    art(&mut case, export(AGENT, "pair").out("out/pairdir"));
    let seen = art(&mut case, export(AGENT, "pair").target("codex"));
    assert_eq!(error(&seen), "--out is required for this export");
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn export_refuses_existing_unmanaged_and_data_directories() {
    let mut case = ready();
    art(&mut case, skill_tree("src/s", "s\n"));
    art(&mut case, import(SKILL, "src/s", "s"));
    for out in [
        ".",
        "./",
        ".luuna",
        ".luuna/beacon",
        ".luuna/beacon/x",
        "sub/../.luuna",
        ".luuna/other",
    ] {
        let seen = art(&mut case, export(SKILL, "s").out(out));
        assert_eq!(
            error(&seen),
            "refusing to export over a project or Beacon data directory",
            "--out {out}"
        );
    }
    let seen = art(&mut case, export(SKILL, "s").out(".luuna-not"));
    assert_eq!(
        seen.documents[0]["path"],
        json!("<root>/.luuna-not"),
        "GD52: only a lexical check"
    );

    art(
        &mut case,
        tree("out/plain", vec![entry("file.txt", "mine\n")]),
    );
    let seen = art(&mut case, export(SKILL, "s").out("out/plain"));
    assert_eq!(error(&seen), "destination already exists: <root>/out/plain");
    let refused = "refusing to replace an export not managed by skill \"s\"";
    let seen = art(&mut case, export(SKILL, "s").out("out/plain").force());
    assert_eq!(error(&seen), refused);
    for (dir, marker) in [
        ("out/other", "{\"kind\":\"skill\",\"name\":\"other\"}"),
        ("out/badmarker", "not json"),
        ("out/kindmarker", "{\"kind\":\"agent\",\"name\":\"s\"}"),
        ("out/numeric", "{\"kind\":\"skill\",\"name\":\"s\",\"n\":1}"),
    ] {
        art(
            &mut case,
            tree(
                dir,
                vec![
                    entry(".lbc-artifact.json", marker),
                    entry("keep.txt", "k\n"),
                ],
            ),
        );
        let seen = art(&mut case, export(SKILL, "s").out(dir).force());
        assert_eq!(error(&seen), refused, "marker {marker}");
        assert!(case.lib_root().join(dir).join("keep.txt").is_file());
    }
    art(
        &mut case,
        tree(
            "out/extra",
            vec![entry(
                ".lbc-artifact.json",
                "{\"kind\":\"skill\",\"name\":\"s\",\"extra\":\"1\"}",
            )],
        ),
    );
    art(&mut case, export(SKILL, "s").out("out/extra").force());
    art(&mut case, tree("out", vec![entry("file.txt", "f\n")]));
    let seen = art(&mut case, export(SKILL, "s").out("out/file.txt").force());
    assert_eq!(error(&seen), refused);
    let seen = art(&mut case, export(SKILL, "s").target("elsewhere"));
    assert_eq!(error(&seen), "--out is required for this export");
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn export_force_replaces_only_managed_exports() {
    let mut case = ready();
    art(&mut case, skill_tree("src/s", "one\n"));
    art(&mut case, import(SKILL, "src/s", "s"));
    art(&mut case, export(SKILL, "s").out("out/s"));
    art(&mut case, tree("out/s", vec![entry("user.txt", "mine\n")]));
    art(&mut case, export(SKILL, "s").out("out/s").force());
    assert!(
        !case.lib_root().join("out/s/user.txt").exists(),
        "DL4: a managed export is replaced whole on --force"
    );
    art(&mut case, tree("src/s", vec![entry("SKILL.md", "two\n")]));
    art(&mut case, import(SKILL, "src/s", "s"));
    art(&mut case, export(SKILL, "s").out("out/s").force());
    assert_eq!(
        fs::read_to_string(case.lib_root().join("out/s/SKILL.md")).unwrap(),
        "two\n"
    );
    art(&mut case, export(SKILL, "s@1").out("out/s").force());
    assert_eq!(
        fs::read_to_string(case.lib_root().join("out/s/SKILL.md")).unwrap(),
        "one\n"
    );

    art(
        &mut case,
        tree("src/b", vec![entry(".claude-plugin/plugin.json", "{}\n")]),
    );
    art(&mut case, import(BUNDLE, "src/b", "s"));
    let seen = art(&mut case, export(BUNDLE, "s").out("out/s").force());
    assert_eq!(
        error(&seen),
        "refusing to replace an export not managed by bundle \"s\""
    );
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn export_out_may_point_outside_the_project_like_go() {
    let mut pair = Pair::new(Git::Available);
    pair.step(Step::Init);
    pair.art(skill_tree("src/s", "s\n"));
    pair.art(import(SKILL, "src/s", "s"));
    let seen = pair.art(export(SKILL, "s").out("<outside>/dest"));
    assert_eq!(seen.documents[0]["path"], json!("<outside>/dest"), "GD52");
    assert!(pair.lib_outside().join("dest/SKILL.md").is_file());
    let seen = pair.art(export(SKILL, "s").out("<outside>/nested/deeper/dest"));
    assert_eq!(
        seen.documents[0]["path"],
        json!("<outside>/nested/deeper/dest")
    );
    let seen = pair.art(export(SKILL, "s").out("<outside>/dest"));
    assert_eq!(
        seen.error.as_deref(),
        Some("destination already exists: <outside>/dest")
    );
    pair.art(export(SKILL, "s").out("<outside>/dest").force());

    pair.art(tree("src/a", vec![entry("agent.md", "a\n")]));
    pair.art(import(AGENT, "src/a/agent.md", "agent"));
    let seen = pair.art(export(AGENT, "agent").out("<outside>/agents/agent.md"));
    assert_eq!(
        seen.documents[0]["path"],
        json!("<outside>/agents/agent.md")
    );
    assert!(pair.lib_outside().join("agents/agent.md").is_file());
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn stored_path_traversal_escapes_the_export_like_go() {
    let mut case = ready();
    let traversal = record(
        SKILL,
        "trav",
        1,
        vec![
            utf8_file("../escaped.txt", "escaped\n"),
            utf8_file("SKILL.md", "x\n"),
        ],
    );
    let deeper = record(
        SKILL,
        "deeper",
        1,
        vec![
            utf8_file("../../up.txt", "up\n"),
            utf8_file("SKILL.md", "y\n"),
        ],
    )
    .with_id("artifact-b1");
    plain(
        &mut case,
        write_artifacts(&[traversal.line(), deeper.line()]),
    );
    let seen = art(&mut case, export(SKILL, "trav").out("out/dest"));
    assert_eq!(
        ok(&seen)["path"],
        json!(format!("{}/out/dest", case.go_root().display()))
    );
    assert_eq!(
        fs::read_to_string(case.lib_root().join("out/escaped.txt")).unwrap(),
        "escaped\n",
        "GD51: a stored `../` path writes beside the destination"
    );
    assert!(case.lib_root().join("out/dest/SKILL.md").is_file());
    assert!(!case.lib_root().join("out/dest/escaped.txt").exists());
    art(&mut case, export(SKILL, "deeper").out("out/a/dest"));
    assert_eq!(
        fs::read_to_string(case.lib_root().join("out/up.txt")).unwrap(),
        "up\n"
    );
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn stored_path_escaping_onto_an_existing_file_is_refused_and_the_file_is_unchanged() {
    let mut pair = Pair::new(Git::Available);
    pair.step(Step::Init);
    let traversal = record(
        SKILL,
        "trav",
        1,
        vec![
            utf8_file("../escaped.txt", "from the record\n"),
            utf8_file("SKILL.md", "x\n"),
        ],
    );
    pair.step(write_artifacts(&[traversal.line()]));
    pair.art(tree("out", vec![entry("escaped.txt", "user data\n")]));
    pair.art(chmod("out/escaped.txt", 0o640));

    let victim = pair.lib_root().join("out/escaped.txt");
    let store = pair.lib_root().join(".luuna/beacon/artifacts.jsonl");
    let (bytes, mode, store_bytes) = (
        fs::read(&victim).unwrap(),
        mode_of(&victim),
        fs::read(&store).unwrap(),
    );
    let names_before = names_under(pair.lib_root());

    let apart = pair.apart(export(SKILL, "trav").out("out/dest"));

    // Go: the user's file is overwritten (the loss RD57 refuses).
    assert_eq!(apart.go.code, 0, "Go exports: {:?}", apart.go.error);
    assert_eq!(
        fs::read_to_string(pair.go_root().join("out/escaped.txt")).unwrap(),
        "from the record\n",
        "evidence: Go truncates and rewrites the existing file"
    );
    // The library: refused, nothing written.
    assert_eq!(apart.lib.code, 1);
    assert_eq!(
        apart.lib.raw[0]["error"],
        json!(format!(
            "refusing to overwrite {} outside the export directory",
            victim.display()
        ))
    );
    assert_eq!(apart.lib.raw[0]["command"], json!("skill"));
    assert_eq!(fs::read(&victim).unwrap(), bytes);
    assert_eq!(mode_of(&victim), mode);
    assert_eq!(fs::read(&store).unwrap(), store_bytes);
    assert!(!pair.lib_root().join("out/dest").exists());
    assert_eq!(
        names_under(pair.lib_root()),
        names_before,
        "no new file under the root"
    );
    assert!(names_under(pair.lib_outside()).is_empty());
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn duplicate_and_marker_named_stored_paths_export_like_go() {
    let mut case = ready();
    let duplicate = record(
        SKILL,
        "dup",
        1,
        vec![
            utf8_file("a.md", "first\n"),
            utf8_file("a.md", "second\n"),
            utf8_file(".lbc-artifact.json", "user marker\n"),
            utf8_file("SKILL.md", "d\n"),
        ],
    );
    plain(&mut case, write_artifacts(&[duplicate.line()]));
    let seen = art(&mut case, read(SKILL, "dup", "a.md"));
    assert_eq!(ok(&seen)["file"]["content"], json!("first\n"));
    art(&mut case, export(SKILL, "dup").out("out/dup"));
    let dir = case.lib_root().join("out/dup");
    assert_eq!(
        fs::read_to_string(dir.join("a.md")).unwrap(),
        "second\n",
        "DL14"
    );
    assert_eq!(
        fs::read_to_string(dir.join(".lbc-artifact.json")).unwrap(),
        "{\"kind\":\"skill\",\"name\":\"dup\"}"
    );
    art(&mut case, export(SKILL, "dup").out("out/dup").force());
    art(&mut case, verify(SKILL, Some("dup")));
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn export_follows_symlinked_ancestors_and_destinations_like_go() {
    let mut pair = Pair::new(Git::Available);
    pair.step(Step::Init);
    pair.art(skill_tree("src/s", "s\n"));
    pair.art(import(SKILL, "src/s", "s"));
    pair.both(|_, outside| fs::create_dir_all(outside.join("claude-home")).unwrap());
    pair.art(symlink(".claude", "<outside>/claude-home"));
    let seen = pair.art(export(SKILL, "s").target("claude"));
    assert_eq!(seen.documents[0]["path"], json!("<root>/.claude/skills/s"));
    assert!(
        pair.lib_outside()
            .join("claude-home/skills/s/SKILL.md")
            .is_file(),
        "GD52"
    );

    pair.art(export(SKILL, "s").out("<outside>/managed"));
    pair.art(symlink("out/link", "<outside>/managed"));
    let seen = pair.art(export(SKILL, "s").out("out/link"));
    assert_eq!(
        seen.error.as_deref(),
        Some("destination already exists: <root>/out/link")
    );
    pair.art(export(SKILL, "s").out("out/link").force());
    let link = pair.lib_root().join("out/link");
    assert!(
        fs::symlink_metadata(&link).unwrap().is_dir(),
        "GD53: the link is replaced by a real folder"
    );
    assert!(
        pair.lib_outside().join("managed/SKILL.md").is_file(),
        "the link's target is kept"
    );

    pair.art(tree("src/a", vec![entry("agent.md", "a\n")]));
    pair.art(import(AGENT, "src/a/agent.md", "agent"));
    pair.art(export(AGENT, "agent").target("claude"));
    assert!(pair
        .lib_outside()
        .join("claude-home/agents/agent.md")
        .is_file());
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn source_paths_that_are_not_utf8_are_refused_and_the_store_is_unchanged() {
    let mut pair = Pair::new(Git::Available);
    pair.step(Step::Init);
    pair.art(tree(
        "src/s",
        vec![
            entry("SKILL.md", "s\n"),
            entry_bytes(b"caf\xe9.md", b"c\n", false),
        ],
    ));
    pair.art(tree(
        "src/d",
        vec![entry_bytes(b"\xffdir/a.md", b"o\n", false)],
    ));
    let store = pair.lib_root().join(".luuna/beacon/artifacts.jsonl");
    let dir = pair.lib_root().join(".luuna/beacon");
    let store_before = fs::read(&store).unwrap();
    let store_names = names_under(&dir);
    let root_names = names_under(pair.lib_root());

    for (kind, source, name, shown) in [
        (SKILL, "src/s", "s", "caf\u{FFFD}.md"),
        (AGENT, "src/d", "d", "\u{FFFD}dir/a.md"),
    ] {
        let apart = pair.apart(import(kind, source, name));
        // Go stores the path with U+FFFD: the value it reads back differs.
        assert_eq!(apart.go.code, 0, "Go stores it: {:?}", apart.go.error);
        let go_paths: Vec<&str> = apart.go.raw[0]["artifact"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["path"].as_str().unwrap())
            .collect();
        assert!(
            go_paths.contains(&shown),
            "evidence: Go stores {shown:?}: {go_paths:?}"
        );
        let go_store =
            fs::read_to_string(pair.go_root().join(".luuna/beacon/artifacts.jsonl")).unwrap();
        assert!(go_store.contains("\\ufffd") || go_store.contains('\u{FFFD}'));
        // The library refuses and writes nothing.
        assert_eq!(apart.lib.code, 1);
        assert_eq!(apart.lib.raw[0]["command"], json!(kind));
        assert_eq!(
            apart.lib.raw[0]["error"],
            json!(format!(
                "refusing to store a path that is not valid UTF-8: {shown}"
            ))
        );
        assert_eq!(
            fs::read(&store).unwrap(),
            store_before,
            "artifacts.jsonl is unchanged"
        );
        assert_eq!(
            names_under(&dir),
            store_names,
            "no lock or temporary file remains"
        );
        assert_eq!(
            names_under(pair.lib_root()),
            root_names,
            "no new file under the root"
        );
        assert!(names_under(pair.lib_outside()).is_empty());
    }
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn artifact_commands_before_init_and_with_a_legacy_layout() {
    let mut case = Case::with_project(PROJECT);
    let seen = art(&mut case, list(SKILL));
    assert_eq!(ok(&seen)["count"], json!(0));
    assert_eq!(
        ok(&art(&mut case, verify(SKILL, None)))["checked"],
        json!(0)
    );
    art(&mut case, skill_tree("src/a", "a\n"));
    art_os_error(
        &mut case,
        import(SKILL, "src/a", "a"),
        "acquire lock <root>/.luuna/beacon/artifacts.jsonl.lock: open <root>/.luuna/beacon/artifacts.jsonl.lock: ",
    );
    assert_eq!(
        error(&art(&mut case, show(SKILL, "a"))),
        "skill \"a\" not found"
    );
    assert!(
        store_paths(case.lib_root()).is_empty(),
        "nothing is created before init"
    );

    let legacy = record(SKILL, "old", 1, vec![utf8_file("SKILL.md", "old\n")]);
    plain(
        &mut case,
        Step::WriteFile {
            path: ".canopy/artifacts.jsonl".to_owned(),
            bytes: legacy.line().into_bytes(),
        },
    );
    plain(
        &mut case,
        Step::WriteFile {
            path: ".gitignore".to_owned(),
            bytes: b".canopy/*.lock\n".to_vec(),
        },
    );
    let seen = art(&mut case, list(SKILL));
    assert!(
        seen.migrated,
        "DL12: the artifact commands run the legacy move"
    );
    assert_eq!(ok(&seen)["artifacts"][0]["name"], json!("old"));
    assert_eq!(
        fs::read_to_string(case.lib_root().join(".gitignore")).unwrap(),
        ".luuna/beacon/*.lock\n"
    );
    plain(
        &mut case,
        Step::WriteFile {
            path: ".canopy/config.yaml".to_owned(),
            bytes: b"project: legacy\n".to_vec(),
        },
    );
    let seen = art(&mut case, show(SKILL, "old"));
    assert_eq!(
        error(&seen),
        "both .canopy and .luuna/beacon exist; merge or remove one before continuing"
    );
    let seen = art(&mut case, unknown(SKILL, "frob"));
    assert_eq!(
        error(&seen),
        "both .canopy and .luuna/beacon exist; merge or remove one before continuing"
    );
}

#[test]
#[ignore = "requires Go 1.27 and the luuna-beacon reference clone; run with LUUNA_BEACON_REFERENCE=<clone> LUUNA_BEACON_GO=<go> cargo test -p luuna-beacon --locked --test parity_artifacts -- --ignored"]
fn html_unicode_and_invalid_utf8_in_names_paths_and_contents() {
    let mut case = ready();
    art(
        &mut case,
        tree(
            "src/h",
            vec![
                entry(
                    "SKILL.md",
                    "<b>bold</b> & \u{2028} \u{2029} \"quoted\" \\ \t\n",
                ),
                entry("a<b>&c.md", "html path\n"),
                entry("ünï/çødé.md", "ü\n"),
                entry("tab\tname.md", "t\n"),
                entry("emoji-🌙.md", "🌙\n"),
                entry_bytes(b"bad.md", b"\xff\xfe<>&", false),
                entry("ctrl.md", "\u{1}\u{1f}\u{7f}\n"),
            ],
        ),
    );
    let seen = art(&mut case, import(SKILL, "src/h", "h"));
    assert_eq!(ok(&seen)["artifact"]["files"].as_array().unwrap().len(), 7);
    art(&mut case, show(SKILL, "h"));
    art(&mut case, list(SKILL));
    for file in [
        "a<b>&c.md",
        "ünï/çødé.md",
        "tab\tname.md",
        "emoji-🌙.md",
        "bad.md",
        "SKILL.md",
        "ctrl.md",
    ] {
        art(&mut case, read(SKILL, "h", file));
    }
    art(&mut case, export(SKILL, "h").out("out/h"));
    assert_eq!(
        fs::read(case.lib_root().join("out/h/bad.md")).unwrap(),
        b"\xff\xfe<>&"
    );
    for reference in ["<script>&", "ü\u{2028}", "tab\there", "quote\"d"] {
        let seen = art(&mut case, show(SKILL, reference));
        assert_eq!(seen.code, 1);
    }
    let seen = art(&mut case, read(SKILL, "h", "<missing>&"));
    assert_eq!(error(&seen), "file \"<missing>&\" not found");
}
