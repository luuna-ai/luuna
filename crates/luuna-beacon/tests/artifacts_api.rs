//! Owner: plan s3-beacon-lib-3. Behaviour of the artifact commands, doctor,
//! onboard and sync that needs no Go and no real git: the stored line
//! format, digests, base64, names, every error text, relative sources,
//! doctor's summary, the three git starts of sync (through a test-made
//! fake `git`), the Pair guard, the normalisation hook, and the three A8
//! refusals (RD55-RD57) with the files left byte for byte unchanged.
//!
//! Every root is a `tempfile` folder; none of these tests builds the Go
//! tool (`fresh_root` would).
//!
//! These tests are written against the library contract of the plan and
//! compile once the library exists.
#![cfg(unix)]

mod support;

use luuna_beacon::{
    ArtifactExport, ArtifactKind, ArtifactsError, Beacon, BeaconError, Clock, DoctorCheck,
    DoctorReport, DoctorStatus, GitEnvironment, GitRunner, OnboardMode, OnboardStatus, Onboarded,
    SyncOptions, Synced,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use support::artifacts::*;
use support::normalise::sort_map_ordered_arrays;
use tempfile::TempDir;

/// sha256 of empty input.
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
/// sha256 of `abc`.
const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
/// sha256 of `[]`: the manifest digest of zero files (`printf '[]' | sha256sum`).
const NO_FILES_SHA256: &str = "4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945";
/// 2001-02-03T04:05:06Z.
const FIXED_SECONDS: u64 = 981_173_106;
/// ETXTBSY on Linux.
const TEXT_FILE_BUSY: i32 = 26;

struct FixedClock(SystemTime);

impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        self.0
    }
}

/// A fresh canonical root in the system temporary folder.
fn root() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temporary folder");
    let path = dir.path().canonicalize().expect("canonical path");
    (dir, path)
}

/// A root with `init` run.
fn store() -> (TempDir, PathBuf, Beacon) {
    let (dir, path) = root();
    let beacon = Beacon::open(&path).expect("open");
    beacon.init().expect("init");
    (dir, path, beacon)
}

fn write_tree(base: &Path, entries: &[(&[u8], &[u8])]) {
    for (path, contents) in entries {
        let target = base.join(OsStr::from_bytes(path));
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, contents).unwrap();
    }
}

fn err<T>(result: Result<T, BeaconError>) -> String {
    match result {
        Ok(_) => panic!("expected an error"),
        Err(error) => error.to_string(),
    }
}

fn artifacts_file(root: &Path) -> PathBuf {
    root.join(".luuna/beacon/artifacts.jsonl")
}

#[test]
fn artifact_records_round_trip_go_json_byte_for_byte() {
    let (_dir, root, beacon) = store();
    let records = [
        record(SKILL, "plain", 1, vec![utf8_file("SKILL.md", "# Plain\n")]),
        record(
            SKILL,
            "escapes",
            3,
            vec![
                utf8_file(
                    "SKILL.md",
                    "<b>&</b> \u{2028}\u{2029} \"q\" \\ \t\r\n\u{1}\u{7f}",
                ),
                utf8_file("a<b>&c.md", "x").executable(),
                utf8_file("ünï/çødé 🌙.md", "ü"),
                base64_file("blob.bin", &[0x00, 0xff, 0xfe]),
            ],
        )
        .with_id("artifact-b1"),
        record(SKILL, "big", i64::MAX, vec![utf8_file("SKILL.md", "")]).with_id("artifact-b2"),
    ];
    let content: String = records.iter().map(Record::line).collect();
    fs::write(artifacts_file(&root), &content).unwrap();
    let listed = beacon.artifact_list(ArtifactKind::Skill).expect("list");
    let mut by_name: Vec<(String, String)> = listed
        .iter()
        .map(|artifact| (artifact.name.clone(), artifact.to_go_json()))
        .collect();
    by_name.sort();
    let mut expected: Vec<(String, String)> = records
        .iter()
        .map(|record| (record.name.clone(), record.json()))
        .collect();
    expected.sort();
    assert_eq!(by_name, expected);
    let shown = beacon
        .artifact_show(ArtifactKind::Skill, Some("escapes"))
        .expect("show");
    assert_eq!(shown.created_at, STAMP);
    assert_eq!(shown.files[1].path, "a<b>&c.md");
    assert!(shown.files[1].executable);

    // A stored import appends exactly `to_go_json` and a newline.
    write_tree(&root.join("src/s"), &[(b"SKILL.md", b"<new>\n")]);
    let imported = beacon
        .artifact_import(ArtifactKind::Skill, Some(Path::new("src/s")), "fresh")
        .expect("import");
    let after = fs::read_to_string(artifacts_file(&root)).unwrap();
    assert_eq!(after, format!("{content}{}\n", imported.to_go_json()));
    let parsed: Value = serde_json::from_str(&imported.to_go_json()).unwrap();
    let keys: Vec<&String> = parsed.as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 7);
    assert!(imported
        .to_go_json()
        .starts_with(&format!("{{\"id\":\"{}\",\"kind\":\"skill\",\"name\":\"fresh\",\"version\":1,\"files\":[{{\"path\":\"SKILL.md\",\"encoding\":\"utf8\",\"content\":\"\\u003cnew\\u003e\\n\",", imported.id)));
    assert!(imported.id.starts_with("artifact-"));
}

#[test]
fn digests_of_known_inputs() {
    assert_eq!(digest(b""), EMPTY_SHA256);
    assert_eq!(digest(b"abc"), ABC_SHA256);
    assert_eq!(digest(b"[]"), NO_FILES_SHA256);
    assert_eq!(manifest_json(&[]), "[]");

    let (_dir, root, beacon) = store();
    write_tree(&root.join("src/empty-file"), &[(b"SKILL.md", b"")]);
    let artifact = beacon
        .artifact_import(ArtifactKind::Skill, Some(Path::new("src/empty-file")), "e")
        .expect("import");
    assert_eq!(artifact.files[0].sha256, EMPTY_SHA256);
    let manifest =
        format!("[{{\"path\":\"SKILL.md\",\"sha256\":\"{EMPTY_SHA256}\",\"executable\":false}}]");
    assert_eq!(artifact.sha256, digest(manifest.as_bytes()));

    write_tree(&root.join("src/abc"), &[(b"abc.txt", b"abc")]);
    let artifact = beacon
        .artifact_import(ArtifactKind::Agent, Some(Path::new("src/abc")), "abc")
        .expect("import");
    assert_eq!(artifact.files[0].sha256, ABC_SHA256);

    fs::create_dir_all(root.join("src/none")).unwrap();
    let artifact = beacon
        .artifact_import(ArtifactKind::Agent, Some(Path::new("src/none")), "none")
        .expect("an empty agent folder is stored (GD50)");
    assert!(artifact.files.is_empty());
    assert_eq!(artifact.sha256, NO_FILES_SHA256);
}

#[test]
fn base64_round_trips_with_padding_and_skips_cr_lf() {
    let (_dir, root, beacon) = store();
    let source: [u8; 5] = [0xff, 0xfe, 0xfd, 0xfc, 0xfb];
    let expected = ["/w==", "//4=", "//79", "//79/A==", "//79/Ps="];
    for length in 1..=5 {
        assert_eq!(base64_std(&source[..length]), expected[length - 1]);
        let dir = root.join(format!("src/b{length}"));
        write_tree(
            &dir,
            &[(b"SKILL.md", b"b\n"), (b"blob.bin", &source[..length])],
        );
        let name = format!("b{length}");
        let artifact = beacon
            .artifact_import(
                ArtifactKind::Skill,
                Some(Path::new(&format!("src/b{length}"))),
                &name,
            )
            .expect("import");
        let blob = artifact
            .files
            .iter()
            .find(|file| file.path == "blob.bin")
            .unwrap();
        assert_eq!(blob.encoding, "base64");
        assert_eq!(blob.content, expected[length - 1]);
        let read = beacon
            .artifact_read(ArtifactKind::Skill, Some(&name), Some("blob.bin"))
            .expect("read");
        assert_eq!(read.file.content, expected[length - 1]);
        let options = ArtifactExport {
            out: format!("out/{name}"),
            ..ArtifactExport::default()
        };
        beacon
            .artifact_export(ArtifactKind::Skill, Some(&name), &options)
            .expect("export");
        assert_eq!(
            fs::read(root.join(format!("out/{name}/blob.bin"))).unwrap(),
            &source[..length]
        );
    }

    let crlf = record(
        AGENT,
        "crlf",
        1,
        vec![base64_file("a.md", b"hello").with_encoded("base64", "aGVs\r\nbG8=")],
    );
    fs::write(artifacts_file(&root), crlf.line()).unwrap();
    assert_eq!(
        beacon
            .artifact_verify(ArtifactKind::Agent, None)
            .expect("verify"),
        1
    );
    let options = ArtifactExport {
        out: "out/crlf".to_owned(),
        ..ArtifactExport::default()
    };
    beacon
        .artifact_export(ArtifactKind::Agent, Some("crlf"), &options)
        .expect("export");
    assert_eq!(fs::read(root.join("out/crlf/a.md")).unwrap(), b"hello");

    let bad = record(
        AGENT,
        "bad",
        1,
        vec![base64_file("a.md", b"abcdef").with_encoded("base64", "AAAA!AAA")],
    );
    fs::write(artifacts_file(&root), bad.line()).unwrap();
    assert_eq!(
        err(beacon.artifact_list(ArtifactKind::Agent)),
        "invalid artifact record agent bad@1: illegal base64 data at input byte 4"
    );
}

#[test]
fn artifact_names_follow_go_rules() {
    let (_dir, root, beacon) = store();
    write_tree(&root.join("src/n"), &[(b"SKILL.md", b"n\n")]);
    let source = Some(Path::new("src/n"));
    let rule = "artifact name must be 1-80 lowercase letters, digits, dots, dashes, or underscores, beginning with a letter or digit";
    let long_ok = "a".repeat(80);
    let too_long = "a".repeat(81);
    for name in [
        "a",
        "0",
        "9lives",
        "ok.name_1-2",
        "a..b",
        "a.",
        long_ok.as_str(),
    ] {
        let artifact = beacon
            .artifact_import(ArtifactKind::Skill, source, name)
            .unwrap_or_else(|error| panic!("{name:?}: {error}"));
        assert_eq!(artifact.name, name);
    }
    for name in [
        "A",
        "Upper",
        "-a",
        "_a",
        ".a",
        ".",
        "..",
        "a/b",
        "a b",
        "é",
        "aé",
        "a\u{0}",
        too_long.as_str(),
    ] {
        assert_eq!(
            err(beacon.artifact_import(ArtifactKind::Skill, source, name)),
            rule,
            "{name:?}"
        );
        assert_eq!(
            err(beacon.artifact_update(ArtifactKind::Skill, Some(name), source)),
            rule,
            "{name:?}"
        );
    }
    assert_eq!(
        err(beacon.artifact_import(ArtifactKind::Skill, source, "")),
        "--name is required"
    );
    assert_eq!(
        err(beacon.artifact_update(ArtifactKind::Skill, Some(""), source)),
        rule
    );
    assert_eq!(
        err(beacon.artifact_import(ArtifactKind::Skill, None, "x")),
        "source path required"
    );
    assert_eq!(
        err(beacon.artifact_update(ArtifactKind::Skill, Some("x"), None)),
        "name and source path required"
    );
    assert_eq!(ArtifactKind::Skill.as_str(), "skill");
    assert_eq!(ArtifactKind::Agent.as_str(), "agent");
    assert_eq!(ArtifactKind::Bundle.as_str(), "bundle");
}

/// The Display text of every variant. The match has no wildcard, so a
/// variant added without a text here does not compile.
fn go_text(error: &ArtifactsError) -> &'static str {
    match error {
        ArtifactsError::SubcommandRequired => "artifact subcommand required",
        ArtifactsError::UnknownSubcommand { .. } => "unknown skill subcommand: frob",
        ArtifactsError::InvalidRecord { .. } => "invalid artifact record skill x@3: invalid artifact record",
        ArtifactsError::ConflictingVersion { .. } => "conflicting artifact version: agent y@2",
        ArtifactsError::NotFound { .. } => "skill \"x@\\\"y\" not found",
        ArtifactsError::SourcePathRequired => "source path required",
        ArtifactsError::NameRequired => "--name is required",
        ArtifactsError::NameAndSourceRequired => "name and source path required",
        ArtifactsError::ArtifactNameRequired => "artifact name required",
        ArtifactsError::FilePathRequired => "file path required",
        ArtifactsError::FileNotFound(_) => "file \"a\\\"b\\n\" not found",
        ArtifactsError::OutRequired => "--out is required for this export",
        ArtifactsError::RefusingDataDirectory => {
            "refusing to export over a project or Beacon data directory"
        }
        ArtifactsError::DestinationExists(_) => "destination already exists: /p/.claude/skills/x",
        ArtifactsError::AgentExportNeedsOneFile => "agent export requires exactly one stored file",
        ArtifactsError::NotManaged { .. } => "refusing to replace an export not managed by bundle \"x\"",
        ArtifactsError::InvalidName => "artifact name must be 1-80 lowercase letters, digits, dots, dashes, or underscores, beginning with a letter or digit",
        ArtifactsError::SourceSymlink => "source symlinks are not allowed",
        ArtifactsError::SourceSymlinkAt(_) => "source symlinks are not allowed: a/b.md",
        ArtifactsError::FileLimit => "artifact file limit exceeded",
        ArtifactsError::TooLarge(_) => "artifact file exceeds 4 MiB: big.bin",
        ArtifactsError::CredentialLike(_) => "credential-like file rejected: api.token",
        ArtifactsError::SkillNeedsSkillMd => "skill must contain SKILL.md",
        ArtifactsError::BundleNeedsPlugin => "bundle must contain .claude-plugin/plugin.json",
        ArtifactsError::InvalidKind => "invalid artifact kind",
        ArtifactsError::InvalidArtifactName(_) => "invalid artifact name: Bad",
        ArtifactsError::InvalidArtifactRecord => "invalid artifact record",
        ArtifactsError::HashMismatch(_) => "artifact hash mismatch: SKILL.md",
        ArtifactsError::ManifestMismatch => "artifact manifest hash mismatch",
        ArtifactsError::Base64 { .. } => "illegal base64 data at input byte 4",
        ArtifactsError::DoctorFailed => "doctor found failures",
        ArtifactsError::Git { .. } => "git commit: nothing to commit, working tree clean",
        ArtifactsError::OnboardUnreadable { .. } => {
            "refusing to overwrite /p/CLAUDE.md: it cannot be read: permission denied"
        }
        ArtifactsError::NonUtf8Path(_) => "refusing to store a path that is not valid UTF-8: caf\u{FFFD}.md",
        ArtifactsError::ExportWouldOverwrite(_) => {
            "refusing to overwrite /p/out/escaped.txt outside the export directory"
        }
    }
}

#[test]
fn artifact_errors_display_go_messages() {
    let errors = vec![
        ArtifactsError::SubcommandRequired,
        ArtifactsError::UnknownSubcommand {
            kind: "skill".into(),
            subcommand: "frob".into(),
        },
        ArtifactsError::InvalidRecord {
            kind: "skill".into(),
            name: "x".into(),
            version: 3,
            cause: "invalid artifact record".into(),
        },
        ArtifactsError::ConflictingVersion {
            kind: "agent".into(),
            name: "y".into(),
            version: 2,
        },
        ArtifactsError::NotFound {
            kind: "skill".into(),
            reference: "x@\"y".into(),
        },
        ArtifactsError::SourcePathRequired,
        ArtifactsError::NameRequired,
        ArtifactsError::NameAndSourceRequired,
        ArtifactsError::ArtifactNameRequired,
        ArtifactsError::FilePathRequired,
        ArtifactsError::FileNotFound("a\"b\n".into()),
        ArtifactsError::OutRequired,
        ArtifactsError::RefusingDataDirectory,
        ArtifactsError::DestinationExists("/p/.claude/skills/x".into()),
        ArtifactsError::AgentExportNeedsOneFile,
        ArtifactsError::NotManaged {
            kind: "bundle".into(),
            name: "x".into(),
        },
        ArtifactsError::InvalidName,
        ArtifactsError::SourceSymlink,
        ArtifactsError::SourceSymlinkAt("a/b.md".into()),
        ArtifactsError::FileLimit,
        ArtifactsError::TooLarge("big.bin".into()),
        ArtifactsError::CredentialLike("api.token".into()),
        ArtifactsError::SkillNeedsSkillMd,
        ArtifactsError::BundleNeedsPlugin,
        ArtifactsError::InvalidKind,
        ArtifactsError::InvalidArtifactName("Bad".into()),
        ArtifactsError::InvalidArtifactRecord,
        ArtifactsError::HashMismatch("SKILL.md".into()),
        ArtifactsError::ManifestMismatch,
        ArtifactsError::Base64 { offset: 4 },
        ArtifactsError::DoctorFailed,
        ArtifactsError::Git {
            step: "commit".into(),
            output: "nothing to commit, working tree clean".into(),
        },
        ArtifactsError::OnboardUnreadable {
            file: "/p/CLAUDE.md".into(),
            cause: "permission denied".into(),
        },
        ArtifactsError::NonUtf8Path("caf\u{FFFD}.md".into()),
        ArtifactsError::ExportWouldOverwrite("/p/out/escaped.txt".into()),
    ];
    assert_eq!(errors.len(), 35);
    for error in errors {
        let expected = go_text(&error);
        assert_eq!(error.to_string(), expected);
        assert_eq!(BeaconError::from(error).to_string(), expected);
    }
    let empty = ArtifactsError::Git {
        step: "status".into(),
        output: "".into(),
    };
    assert_eq!(empty.to_string(), "git status: ");
}

#[test]
fn relative_sources_are_joined_to_the_project_root() {
    let (_dir, root, beacon) = store();
    assert_ne!(
        std::env::current_dir().unwrap().canonicalize().unwrap(),
        root,
        "the test runs with a working directory other than the root"
    );
    write_tree(&root.join("src/rel"), &[(b"SKILL.md", b"rel\n")]);
    let artifact = beacon
        .artifact_import(ArtifactKind::Skill, Some(Path::new("src/rel")), "rel")
        .expect("a relative source is read under the project root");
    assert_eq!(artifact.files[0].content, "rel\n");
    let artifact = beacon
        .artifact_update(
            ArtifactKind::Skill,
            Some("abs"),
            Some(&root.join("src/rel")),
        )
        .expect("an absolute source");
    assert_eq!(artifact.version, 1);
    write_tree(&root.join("src/one"), &[(b"agent.md", b"a\n")]);
    let artifact = beacon
        .artifact_import(
            ArtifactKind::Agent,
            Some(Path::new("src/one/agent.md")),
            "one",
        )
        .expect("a relative single file");
    assert_eq!(artifact.files[0].path, "agent.md");
    let message =
        err(beacon.artifact_import(ArtifactKind::Skill, Some(Path::new("src/missing")), "m"));
    assert!(
        message.starts_with("lstat src/missing: "),
        "RD50: the message shows the path as given: {message}"
    );
}

fn check(name: &str, status: DoctorStatus) -> DoctorCheck {
    DoctorCheck {
        name: name.to_owned(),
        status,
        message: "m".to_owned(),
        details: None,
        fixable: false,
    }
}

#[test]
fn doctor_summary_counts_and_success() {
    let report = DoctorReport {
        checks: vec![
            check("a", DoctorStatus::Pass),
            check("b", DoctorStatus::Warn),
            check("c", DoctorStatus::Fail),
            check("d", DoctorStatus::Fail),
        ],
    };
    let summary = report.summary();
    assert_eq!((summary.pass, summary.warn, summary.fail), (1, 1, 2));
    assert!(!report.success());
    let warn_only = DoctorReport {
        checks: vec![
            check("a", DoctorStatus::Pass),
            check("b", DoctorStatus::Warn),
        ],
    };
    assert!(warn_only.success(), "a warning is not a failure");

    let names = [
        "config",
        "prompts-integrity",
        "schemas-integrity",
        "artifacts-integrity",
        "inheritance",
        "stale-locks",
    ];
    let (_dir, root) = root();
    let beacon = Beacon::open(&root).unwrap();
    let report = beacon.doctor(false).expect("doctor on an empty root");
    let got: Vec<&str> = report
        .checks
        .iter()
        .map(|check| check.name.as_str())
        .collect();
    assert_eq!(got, names);
    assert!(matches!(report.checks[0].status, DoctorStatus::Fail));
    assert!(!report.success());
    assert!(
        fs::read_dir(&root).unwrap().next().is_none(),
        "doctor writes nothing"
    );

    beacon.init().unwrap();
    let report = beacon.doctor(false).expect("doctor on a fresh store");
    let summary = report.summary();
    assert_eq!((summary.pass, summary.warn, summary.fail), (6, 0, 0));
    assert!(report.success());
    assert!(report
        .checks
        .iter()
        .all(|check| check.details.is_none() && !check.fixable));

    let lock = root.join(".luuna/beacon/old.lock");
    fs::write(&lock, b"").unwrap();
    fs::File::options()
        .write(true)
        .open(&lock)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(120))
        .unwrap();
    let report = beacon.doctor(false).expect("doctor with a stale lock");
    let stale = report.checks.last().unwrap();
    assert!(matches!(stale.status, DoctorStatus::Warn));
    assert_eq!(stale.details.as_deref(), Some(&["old.lock".to_owned()][..]));
    assert!(stale.fixable);
    assert!(report.success());
    let report = beacon.doctor(true).expect("doctor --fix");
    assert!(matches!(
        report.checks.last().unwrap().status,
        DoctorStatus::Pass
    ));
    assert!(!lock.exists());
}

/// A fake `git` in `<dir>/bin`: a script that appends its arguments, its
/// working directory and its exported environment to `<dir>/calls.log`,
/// and prints one modified store file for `status`.
fn fake_git(dir: &Path) -> PathBuf {
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let script = bin.join("git");
    fs::write(
        &script,
        concat!(
            "#!/bin/sh\n",
            "log=\"${0%/*}/../calls.log\"\n",
            "{\n",
            "  printf 'call\\n'\n",
            "  printf 'pwd=%s\\n' \"$(pwd -P)\"\n",
            "  for argument in \"$@\"; do printf 'arg=%s\\n' \"$argument\"; done\n",
            "  printf 'env-begin\\n'\n",
            "  export -p\n",
            "  printf 'env-end\\n'\n",
            "} >> \"$log\"\n",
            "if [ \"$1\" = status ]; then printf ' M .luuna/beacon/prompts.jsonl\\n'; fi\n",
            "exit 0\n",
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    // Start it once, retrying only while another thread's fork still holds
    // the write descriptor (ETXTBSY), so the library's start cannot hit it.
    let mut started = false;
    for _ in 0..50 {
        match Command::new(&script).arg("warm-up").env_clear().output() {
            Ok(_) => {
                started = true;
                break;
            }
            Err(error) if error.raw_os_error() == Some(TEXT_FILE_BUSY) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("could not start the fake git: {error}"),
        }
    }
    assert!(started, "the fake git stayed busy");
    fs::remove_file(dir.join("calls.log")).unwrap();
    bin
}

struct Call {
    pwd: String,
    args: Vec<String>,
    env: BTreeSet<String>,
}

fn calls(dir: &Path) -> Vec<Call> {
    let log = fs::read_to_string(dir.join("calls.log")).unwrap_or_default();
    let mut calls = Vec::new();
    let mut in_env = false;
    for line in log.lines() {
        if line == "call" {
            calls.push(Call {
                pwd: String::new(),
                args: Vec::new(),
                env: BTreeSet::new(),
            });
            continue;
        }
        let call = calls.last_mut().expect("a call header");
        if line == "env-begin" {
            in_env = true;
        } else if line == "env-end" {
            in_env = false;
        } else if in_env {
            let rest = line
                .strip_prefix("export ")
                .or_else(|| line.strip_prefix("declare -x "));
            if let Some(rest) = rest {
                let name = rest.split('=').next().unwrap_or_default();
                call.env.insert(name.to_owned());
            }
        } else if let Some(pwd) = line.strip_prefix("pwd=") {
            call.pwd = pwd.to_owned();
        } else if let Some(arg) = line.strip_prefix("arg=") {
            call.args.push(arg.to_owned());
        }
    }
    calls
}

fn exact(bin: &Path) -> GitEnvironment {
    GitEnvironment::Exact(vec![
        (OsString::from("PATH"), bin.as_os_str().to_owned()),
        (
            OsString::from("BEACON_FAKE_MARKER"),
            OsString::from("set-only-in-the-list"),
        ),
    ])
}

fn runner(environment: GitEnvironment) -> GitRunner {
    GitRunner {
        program: OsString::from("git"),
        environment,
    }
}

#[test]
fn sync_runs_exactly_three_git_commands_without_a_shell() {
    let (_dir, root, beacon) = store();
    let (_fake_dir, fake) = support_dir();
    let bin = fake_git(&fake);
    let synced = beacon
        .sync(&SyncOptions {
            status_only: false,
            git: runner(exact(&bin)),
        })
        .expect("sync with the fake git");
    let message = match synced {
        Synced::Committed { files, message } => {
            assert_eq!(files, ["luuna/beacon/prompts.jsonl"], "GD76");
            message
        }
        _ => panic!("expected a commit"),
    };
    assert!(message.starts_with("luuna-beacon: sync "));
    let calls = calls(&fake);
    let args: Vec<Vec<String>> = calls.iter().map(|call| call.args.clone()).collect();
    assert_eq!(
        args,
        vec![
            vec![
                "status".to_owned(),
                "--porcelain".to_owned(),
                ".luuna/beacon/".to_owned()
            ],
            vec!["add".to_owned(), ".luuna/beacon/".to_owned()],
            vec!["commit".to_owned(), "-m".to_owned(), message.clone()],
        ]
    );
    let shell_own = [
        "PWD", "OLDPWD", "SHLVL", "_", "IFS", "PS1", "PS2", "PS4", "OPTIND", "PPID",
    ];
    let only_in_test: Vec<String> = std::env::vars_os()
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .filter(|name| {
            !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.starts_with(|c: char| c.is_ascii_digit())
                && name != "PATH"
                && name != "BEACON_FAKE_MARKER"
                && !shell_own.contains(&name.as_str())
        })
        .collect();
    assert!(
        !only_in_test.is_empty(),
        "the test process has variables of its own"
    );
    for call in &calls {
        assert_eq!(Path::new(&call.pwd), root, "cwd is the project root");
        assert!(
            call.env.contains("BEACON_FAKE_MARKER"),
            "the list reaches the child"
        );
        for name in &only_in_test {
            assert!(
                !call.env.contains(name),
                "{name} is set only in the test process and must not reach git"
            );
        }
    }

    let synced = beacon
        .sync(&SyncOptions {
            status_only: true,
            git: runner(exact(&bin)),
        })
        .expect("sync --status");
    match synced {
        Synced::Status { files, uncommitted } => {
            assert_eq!(files, ["luuna/beacon/prompts.jsonl"]);
            assert!(uncommitted);
        }
        _ => panic!("expected a status"),
    }
    assert_eq!(calls_count(&fake), 4, "--status runs only `git status`");
}

fn calls_count(dir: &Path) -> usize {
    calls(dir).len()
}

/// A test-made folder for the fake git and its log.
fn support_dir() -> (TempDir, PathBuf) {
    root()
}

#[test]
fn sync_failure_to_start_is_git_status_with_empty_detail() {
    let (_dir, root, beacon) = store();
    let missing = root.join("no-such-git");
    for environment in [GitEnvironment::Exact(Vec::new()), GitEnvironment::Inherit] {
        let result = beacon.sync(&SyncOptions {
            status_only: false,
            git: GitRunner {
                program: missing.clone().into_os_string(),
                environment,
            },
        });
        assert_eq!(err(result), "git status: ");
    }
    let (_empty_dir, empty) = support_dir();
    let result = beacon.sync(&SyncOptions {
        status_only: true,
        git: runner(GitEnvironment::Exact(vec![(
            OsString::from("PATH"),
            empty.into_os_string(),
        )])),
    });
    assert_eq!(
        err(result),
        "git status: ",
        "git not found on the list's PATH"
    );
    let defaults = SyncOptions::default();
    assert!(!defaults.status_only);
    assert_eq!(defaults.git.program, OsString::from("git"));
    assert!(matches!(defaults.git.environment, GitEnvironment::Inherit));
}

#[test]
fn sync_message_uses_the_handle_clock_date() {
    let (_dir, root, _) = store();
    let fixed: Arc<dyn Clock> =
        Arc::new(FixedClock(UNIX_EPOCH + Duration::from_secs(FIXED_SECONDS)));
    let beacon = Beacon::open(&root).unwrap().with_clock(fixed);
    let (_fake_dir, fake) = support_dir();
    let bin = fake_git(&fake);
    let synced = beacon
        .sync(&SyncOptions {
            status_only: false,
            git: runner(exact(&bin)),
        })
        .expect("sync");
    match synced {
        Synced::Committed { message, .. } => {
            assert_eq!(message, "luuna-beacon: sync 2001-02-03", "RD54");
        }
        _ => panic!("expected a commit"),
    }
    let calls = calls(&fake);
    assert_eq!(
        calls[2].args,
        ["commit", "-m", "luuna-beacon: sync 2001-02-03"]
    );
}

#[test]
#[should_panic(expected = "is inside the git repository")]
fn pair_guard_refuses_a_root_inside_a_git_repository_it_did_not_create() {
    let (_dir, base) = root();
    fs::create_dir_all(base.join("repo/.git")).unwrap();
    fs::write(base.join("repo/.git/HEAD"), b"ref: refs/heads/main\n").unwrap();
    fs::create_dir_all(base.join("repo/inner")).unwrap();
    let inner = base.join("repo/inner").canonicalize().unwrap();
    pair_guard(&inner, None);
}

#[test]
#[should_panic(expected = "is not under the temporary folder")]
fn pair_guard_refuses_a_root_outside_the_temporary_folder() {
    let holder = tempfile::Builder::new()
        .prefix("beacon-guard-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    let outside = holder.path().canonicalize().unwrap();
    let temp = std::env::temp_dir().canonicalize().unwrap();
    if outside.starts_with(&temp) {
        // Not this test's case; fail with a different message.
        panic!("Cargo's target folder lies inside the system temporary folder");
    }
    pair_guard(&outside, None);
}

fn doctor_document() -> Value {
    json!({
        "success": false,
        "command": "doctor",
        "checks": [
            {"name": "config", "status": "fail", "message": "Config is invalid", "details": ["z", "a"], "fixable": false},
            {"name": "inheritance", "status": "fail", "message": "Broken inheritance references",
             "details": ["c: x", "a: y", "b: z", "a: y"], "fixable": false},
            {"name": "stale-locks", "status": "warn", "message": "2 stale lock file(s)",
             "details": ["z.lock", "a.lock"], "fixable": true},
        ],
        "summary": {"pass": 0, "warn": 1, "fail": 2},
    })
}

#[test]
fn sort_map_ordered_sorts_only_the_doctor_inheritance_details_and_keeps_values() {
    for through_frozen_hook in [false, true] {
        let original = doctor_document();
        let mut sorted = original.clone();
        if through_frozen_hook {
            sort_map_ordered_arrays(&mut sorted, PROJECT);
        } else {
            sort_map_ordered("doctor", &mut sorted, PROJECT);
        }
        let details = sorted["checks"][1]["details"].clone();
        assert_eq!(details, json!(["a: y", "a: y", "b: z", "c: x"]));
        let mut before: Vec<String> = original["checks"][1]["details"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect();
        before.sort();
        let after: Vec<String> = details
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(before, after, "the same strings as a multiset");
        assert_eq!(sorted["checks"][2]["details"], json!(["z.lock", "a.lock"]));
        assert_eq!(sorted["checks"][0]["details"], json!(["z", "a"]));
        let mut restored = sorted.clone();
        restored["checks"][1]["details"] = original["checks"][1]["details"].clone();
        assert_eq!(restored, original, "nothing else changes");
    }
    let mut no_details =
        json!({"command": "doctor", "checks": [{"name": "inheritance", "details": null}]});
    let copy = no_details.clone();
    sort_map_ordered("doctor", &mut no_details, PROJECT);
    assert_eq!(no_details, copy);
}

#[test]
fn sort_map_ordered_leaves_documents_of_other_commands_untouched() {
    let artifact = |name: &str, version: i64| {
        json!({"id": "artifact-1", "kind": "skill", "name": name, "version": version,
               "files": [{"path": "b"}, {"path": "a"}], "sha256": "s", "createdAt": STAMP})
    };
    let documents = vec![
        json!({"success": true, "command": "skill list", "artifacts": [artifact("b", 1), artifact("a", 1)], "count": 2}),
        json!({"success": true, "command": "agent history", "name": "x", "versions": [artifact("x", 2), artifact("x", 1)]}),
        json!({"success": true, "command": "prime", "content": "b\na\n"}),
        json!({"success": true, "command": "onboard", "action": "updated", "file": "<root>/CLAUDE.md"}),
        json!({"success": true, "command": "sync", "committed": true, "files": ["b", "a"], "message": "m"}),
        json!({"success": false, "command": "skill", "error": "x"}),
        json!({"success": true, "command": "skill list", "checks": [{"name": "inheritance", "details": ["b", "a"]}]}),
    ];
    for document in documents {
        let text = serde_json::to_string(&document).unwrap();
        let command = document["command"].as_str().unwrap().to_owned();
        let mut hooked = document.clone();
        sort_map_ordered(&command, &mut hooked, PROJECT);
        assert_eq!(serde_json::to_string(&hooked).unwrap(), text, "{command}");
        let mut frozen = document.clone();
        sort_map_ordered_arrays(&mut frozen, PROJECT);
        assert_eq!(serde_json::to_string(&frozen).unwrap(), text, "{command}");
    }
}

/// Every name under `base`, links not followed.
fn names(base: &Path) -> Vec<String> {
    names_under(base)
}

#[test]
fn onboard_unreadable_target_is_refused_and_unchanged_without_go() {
    let (_dir, root) = root();
    let beacon = Beacon::open(&root).unwrap();
    let target = root.join("CLAUDE.md");
    fs::write(&target, b"the user's notes\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o200)).unwrap();
    assert_unreadable(&target);
    let before = names(&root);

    match beacon.onboard(OnboardMode::Check).expect("check") {
        Onboarded::Checked { file, status } => {
            assert_eq!(file, target);
            assert!(
                matches!(status, OnboardStatus::Missing),
                "--check keeps Go's behaviour"
            );
        }
        _ => panic!("expected a check result"),
    }
    let os_error = fs::read(&target).expect_err("unreadable").to_string();
    let message = err(beacon.onboard(OnboardMode::Apply));
    assert_eq!(
        message,
        format!(
            "refusing to overwrite {}: it cannot be read: {os_error}",
            target.display()
        ),
        "RD55"
    );
    assert_eq!(mode_of(&target), 0o200);
    assert_eq!(read_protected(&target), b"the user's notes\n");
    assert_eq!(names(&root), before, "no new file");
    assert!(
        !root.join(".luuna").exists(),
        "onboard never migrates or creates a store"
    );

    // An AGENTS.md target behaves the same.
    let (_dir, root) = support_dir();
    let beacon = Beacon::open(&root).unwrap();
    let agents = root.join("AGENTS.md");
    fs::write(&agents, b"agents\n").unwrap();
    fs::set_permissions(&agents, fs::Permissions::from_mode(0o200)).unwrap();
    let message = err(beacon.onboard(OnboardMode::Apply));
    assert!(message.starts_with(&format!(
        "refusing to overwrite {}: it cannot be read: ",
        agents.display()
    )));
    assert_eq!(read_protected(&agents), b"agents\n");

    // A folder at the target is refused the same way.
    let (_dir, root) = support_dir();
    let beacon = Beacon::open(&root).unwrap();
    fs::create_dir_all(root.join("CLAUDE.md")).unwrap();
    fs::write(root.join("CLAUDE.md/inner.md"), b"inner\n").unwrap();
    let before = names(&root);
    let os_error = fs::read(root.join("CLAUDE.md"))
        .expect_err("a folder")
        .to_string();
    assert_eq!(
        err(beacon.onboard(OnboardMode::Apply)),
        format!(
            "refusing to overwrite {}: it cannot be read: {os_error}",
            root.join("CLAUDE.md").display()
        )
    );
    assert_eq!(
        fs::read(root.join("CLAUDE.md/inner.md")).unwrap(),
        b"inner\n"
    );
    assert_eq!(names(&root), before);

    // A missing target is still created (not-found is not refused).
    let (_dir, root) = support_dir();
    let beacon = Beacon::open(&root).unwrap();
    match beacon.onboard(OnboardMode::Apply).expect("create") {
        Onboarded::Applied { file, .. } => assert_eq!(file, root.join("CLAUDE.md")),
        _ => panic!("expected an applied result"),
    }
}

#[test]
fn non_utf8_source_path_is_refused_and_the_store_is_unchanged_without_go() {
    let (_dir, root, beacon) = store();
    write_tree(&root.join("src/ok"), &[(b"SKILL.md", b"ok\n")]);
    beacon
        .artifact_import(ArtifactKind::Skill, Some(Path::new("src/ok")), "ok")
        .expect("a valid import first");
    write_tree(
        &root.join("src/s"),
        &[(b"SKILL.md", b"s\n"), (b"caf\xe9.md", b"c\n")],
    );
    write_tree(&root.join("src/d"), &[(b"\xffdir/a.md", b"a\n")]);
    write_tree(&root.join("src/one"), &[(b"\xfe.md", b"o\n")]);
    write_tree(
        &root.join("src/cred"),
        &[(b"SKILL.md", b"s\n"), (b"\xff.token", b"t\n")],
    );
    let store_dir = root.join(".luuna/beacon");
    let before = fs::read(artifacts_file(&root)).unwrap();
    let names_before = names(&store_dir);
    let root_before = names(&root);

    assert_eq!(
        err(beacon.artifact_import(ArtifactKind::Skill, Some(Path::new("src/s")), "s")),
        "refusing to store a path that is not valid UTF-8: caf\u{FFFD}.md",
        "RD56"
    );
    assert_eq!(
        err(beacon.artifact_update(ArtifactKind::Skill, Some("ok"), Some(Path::new("src/s")))),
        "refusing to store a path that is not valid UTF-8: caf\u{FFFD}.md"
    );
    assert_eq!(
        err(beacon.artifact_import(ArtifactKind::Agent, Some(Path::new("src/d")), "d")),
        "refusing to store a path that is not valid UTF-8: \u{FFFD}dir/a.md"
    );
    let single = root.join("src/one").join(OsStr::from_bytes(b"\xfe.md"));
    assert_eq!(
        err(beacon.artifact_import(ArtifactKind::Agent, Some(&single), "one")),
        "refusing to store a path that is not valid UTF-8: \u{FFFD}.md"
    );
    // Go's own checks for the file come first.
    assert_eq!(
        err(beacon.artifact_import(ArtifactKind::Skill, Some(Path::new("src/cred")), "cred")),
        "credential-like file rejected: \u{FFFD}.token"
    );
    assert_eq!(
        fs::read(artifacts_file(&root)).unwrap(),
        before,
        "artifacts.jsonl is unchanged"
    );
    assert_eq!(
        names(&store_dir),
        names_before,
        "no lock or temporary file remains"
    );
    assert_eq!(names(&root), root_before, "no new file under the root");
}

#[test]
fn export_escape_onto_an_existing_file_is_refused_and_files_are_unchanged_without_go() {
    let (_dir, root, beacon) = store();
    let lines = [
        record(
            SKILL,
            "trav",
            1,
            vec![
                utf8_file("../escaped.txt", "record\n"),
                utf8_file("SKILL.md", "x\n"),
            ],
        ),
        record(
            SKILL,
            "fresh",
            1,
            vec![
                utf8_file("../fresh.txt", "new\n"),
                utf8_file("SKILL.md", "y\n"),
            ],
        )
        .with_id("artifact-b1"),
        record(
            SKILL,
            "todir",
            1,
            vec![utf8_file("../adir", "z\n"), utf8_file("SKILL.md", "z\n")],
        )
        .with_id("artifact-b2"),
        record(
            SKILL,
            "dangle",
            1,
            vec![
                utf8_file("../dangling", "d\n"),
                utf8_file("SKILL.md", "d\n"),
            ],
        )
        .with_id("artifact-b3"),
    ];
    let content: String = lines.iter().map(Record::line).collect();
    fs::write(artifacts_file(&root), &content).unwrap();
    fs::create_dir_all(root.join("out/adir")).unwrap();
    let victim = root.join("out/escaped.txt");
    fs::write(&victim, b"user data\n").unwrap();
    fs::set_permissions(&victim, fs::Permissions::from_mode(0o640)).unwrap();
    std::os::unix::fs::symlink("nowhere", root.join("out/dangling")).unwrap();
    let before = names(&root);

    let export = |name: &str, out: &str| {
        beacon.artifact_export(
            ArtifactKind::Skill,
            Some(name),
            &ArtifactExport {
                out: out.to_owned(),
                target: String::new(),
                force: false,
            },
        )
    };
    assert_eq!(
        err(export("trav", "out/dest")),
        format!(
            "refusing to overwrite {} outside the export directory",
            victim.display()
        ),
        "RD57"
    );
    assert_eq!(fs::read(&victim).unwrap(), b"user data\n");
    assert_eq!(mode_of(&victim), 0o640);
    assert_eq!(fs::read_to_string(artifacts_file(&root)).unwrap(), content);
    assert!(!root.join("out/dest").exists());
    assert_eq!(
        names(&root),
        before,
        "no destination, stage or backup is left"
    );
    assert!(names(&root)
        .iter()
        .all(|name| !name.contains(".lbc-stage-") && !name.contains(".lbc-backup-")));

    // Recorded differences with no data involved: an existing folder or a
    // dangling link at the escaped name is refused too.
    assert_eq!(
        err(export("todir", "out/dest")),
        format!(
            "refusing to overwrite {} outside the export directory",
            root.join("out/adir").display()
        )
    );
    assert_eq!(
        err(export("dangle", "out/dest")),
        format!(
            "refusing to overwrite {} outside the export directory",
            root.join("out/dangling").display()
        )
    );
    assert_eq!(names(&root), before);

    // GD51 stays copied where nothing exists at the escaped name.
    let exported = export("fresh", "out/dest").expect("an escape onto a free name is written");
    assert_eq!(exported.path, root.join("out/dest"));
    assert_eq!(fs::read(root.join("out/fresh.txt")).unwrap(), b"new\n");
    assert_eq!(fs::read(&victim).unwrap(), b"user data\n");
}
