//! Isolated Go comparisons for the complete tooling front end.
mod support;
use luuna_atlas::{
    options::{Context, Env, GlobalOptions},
    tooling, CommandOutput,
};
use std::process::{Command, Stdio};
use support::project::{assert_same_output, Step, Twin};
fn compare(args: &[&str]) {
    compare_prepared(args, |_| {});
}
fn compare_prepared(args: &[&str], prepare: impl Fn(&support::project::Project)) {
    let t = Twin::new(true);
    t.steps(&[Step::Init, Step::Add("core".into())]);
    t.prepare(prepare);
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let out = Command::new(&support::reference::reference().binary)
        .args(&args)
        .current_dir(&t.go.dir)
        .env_clear()
        .envs(t.go.run_env())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let expected = CommandOutput {
        exit_code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    };
    let ctx = Context {
        root: t.rust.dir.clone(),
        cwd: t.rust.dir.clone(),
        env: Env::fixed(t.rust.run_env(), args.clone(), false),
        globals: GlobalOptions::default(),
    };
    let actual = tooling::run(&ctx, &args, b"");
    assert_same_output(&format!("{args:?}"), &t.go, &expected, &t.rust, &actual);
    t.assert_same_tree(&format!("{args:?}"));
}
#[test]
#[ignore = "requires pinned Go reference"]
fn parser_help_version_and_errors() {
    for args in [
        vec![],
        vec!["--help"],
        vec!["--version"],
        vec!["--json", "--version"],
        vec!["--bad", "--help"],
        vec!["--bad"],
        vec!["--json", "qurey"],
        vec!["config"],
        vec!["config", "--help"],
        vec!["config", "wat"],
        vec!["record"],
        vec!["record", "--help"],
        vec!["search", "--format", "ids"],
        vec!["query", "--format=ids"],
        vec!["search", "--json=false"],
        vec!["record", "core", "--type"],
        vec!["record", "core", "--type", "invalid"],
        vec!["show"],
        vec!["show", "--bad", "--help"],
        vec!["-qv"],
        vec!["--", "--version"],
    ] {
        compare(&args);
    }
}

#[test]
#[ignore = "requires pinned Go reference"]
fn completion_and_stubs() {
    for args in [
        vec!["completions", "bash"],
        vec!["completions", "zsh", "--json"],
        vec!["completions", "fish", "-q"],
        vec!["completions", "BASH"],
        vec!["completions", "unknown", "--json"],
        vec!["upgrade"],
        vec!["upgrade", "--json"],
        vec!["update"],
        vec!["update", "--json"],
    ] {
        compare(&args)
    }
}
#[test]
#[ignore = "requires pinned Go reference"]
fn onboard_new_check_and_stdout() {
    for args in [
        vec!["onboard", "--check"],
        vec!["onboard", "--check", "--json"],
        vec!["onboard", "--stdout"],
        vec!["onboard"],
        vec!["onboard", "--json"],
        vec!["onboard", "--quiet"],
    ] {
        compare(&args)
    }
}

#[test]
#[ignore = "requires pinned Go reference"]
fn setup_basics() {
    for args in [
        vec!["setup"],
        vec!["setup", "--json"],
        vec!["setup", "--list"],
        vec!["setup", "--list", "--json"],
        vec!["setup", "claude"],
        vec!["setup", "codex"],
        vec!["setup", "cursor"],
        vec!["setup", "--hooks"],
        vec!["setup", "claude", "--json"],
        vec!["setup", "codex", "--json"],
        vec!["setup", "cursor", "--json"],
        vec!["setup", "--hooks", "--json"],
        vec!["setup", "claude", "--check"],
        vec!["setup", "codex", "--check"],
        vec!["setup", "cursor", "--check"],
        vec!["setup", "claude", "--remove"],
        vec!["setup", "codex", "--remove"],
        vec!["setup", "cursor", "--remove"],
    ] {
        compare(&args)
    }
}

#[test]
#[ignore = "requires pinned Go reference"]
fn audit_empty() {
    for args in [
        vec!["audit"],
        vec!["audit", "--json"],
        vec!["audit", "--ci"],
        vec!["audit", "--suggest"],
        vec!["audit", "--suggest", "--json"],
        vec!["audit", "--domain", "bad", "--ci"],
        vec!["audit", "--ignore-domains", "core", "--json"],
    ] {
        compare(&args)
    }
}

#[test]
#[ignore = "requires pinned Go reference"]
fn audit_populated() {
    for flags in [
        vec![],
        vec!["--json"],
        vec!["--ci"],
        vec!["--suggest"],
        vec!["--suggest", "--json"],
        vec!["--domain", "core", "--json"],
    ] {
        let mut args = vec!["audit"];
        args.extend(flags);
        compare_prepared(&args, |p| {
            p.write(".luuna/atlas/expertise/core.jsonl",concat!("{\"id\":\"mx-aaaa\",\"type\":\"convention\",\"content\":\"Always check because errors matter\",\"classification\":\"foundational\",\"recorded_at\":\"2020-01-01T00:00:00Z\",\"evidence\":{\"missions\":\"ms-1234\",\"gh\":\"#12\",\"commit\":\"abcdef\"}}\n","{\"id\":\"mx-bbbb\",\"type\":\"convention\",\"content\":\"Uses rust\",\"recorded_at\":\"2020-01-02T00:00:00Z\",\"classification\":\"observational\"}\n","{\"id\":\"mx-cccc\",\"type\":\"failure\",\"description\":\"Flaky check\",\"resolution\":\"Use deterministic checks\",\"evidence\":{\"missions\":\"ms-5678\",\"linear\":\"PROJ-1\"}}\n"));
            p.write(
                ".luuna/missions/issues.jsonl",
                "{\"id\":\"ms-1234\",\"title\":\"Fix the check\",\"status\":\"closed\"}\n",
            );
        });
    }
}

#[test]
#[ignore = "requires pinned Go reference"]
fn doctor_empty() {
    for args in [
        vec!["doctor"],
        vec!["doctor", "--json"],
        vec!["doctor", "--fix"],
        vec!["doctor", "--fix", "--json"],
        vec!["doctor", "--quiet"],
    ] {
        compare(&args)
    }
}
#[test]
#[ignore = "requires pinned Go reference"]
fn doctor_diagnostics() {
    for data in ["broken json\n", "# comment\n", "[]\n", "{\"id\":\"mx-aaaa\",\"type\":\"mystery\",\"x\":1}\n", "{\"id\":\"mx-aaaa\",\"type\":\"pattern\",\"name\":\"Incomplete\"}\n", "{\"id\":\"mx-aaaa\",\"type\":\"convention\",\"content\":\"Old record\",\"recorded_at\":\"2020-01-01T00:00:00Z\",\"classification\":\"observational\",\"files\":[\"missing.rs\"],\"dir_anchors\":[\"missing-dir\"],\"evidence\":{\"file\":\"absent\"}}\n", "{\"id\":\"mx-aaaa\",\"type\":\"convention\",\"content\":\"Has outcome\",\"outcome\":{\"status\":\"success\"}}\n"]{for flags in [vec![],vec!["--json"]]{let mut args=vec!["doctor"];args.extend(flags);compare_prepared(&args,|p|p.write(".luuna/atlas/expertise/core.jsonl",data));}}
}
#[test]
#[ignore = "requires pinned Go reference"]
fn doctor_repair_safe() {
    for data in ["{\"id\":\"mx-aaaa\",\"type\":\"convention\",\"content\":\"Has outcome\",\"classification\":\"foundational\",\"recorded_at\":\"2026-01-01T00:00:00Z\",\"outcome\":{\"status\":\"success\"}}\n", "{\"id\":\"mx-aaaa\",\"type\":\"pattern\",\"name\":\"File anchors\",\"description\":\"Keep body\",\"classification\":\"foundational\",\"recorded_at\":\"2026-01-01T00:00:00Z\",\"files\":[\"missing.rs\"],\"dir_anchors\":[\"missing-dir\"],\"evidence\":{\"file\":\"absent\"}}\n"]{for flags in [vec!["--fix"],vec!["--fix","--json"]]{let mut args=vec!["doctor"];args.extend(flags);compare_prepared(&args,|p|p.write(".luuna/atlas/expertise/core.jsonl",data));}}
}

fn bridge_compare(input: &str) {
    bridge_prepared(input, |_| {});
}
fn bridge_prepared(input: &str, prepare: impl Fn(&support::project::Project)) {
    use std::io::Write;
    let t = Twin::new(true);
    t.steps(&[Step::Init, Step::Add("core".into())]);
    t.prepare(prepare);
    let request =
        |p: &support::project::Project| input.replace("<project>", &p.dir.to_string_lossy());
    let mut vars = t.go.run_env();
    vars.push(("LUUNA_ATLAS_API_BRIDGE".into(), "1".into()));
    let mut child = Command::new(&support::reference::reference().binary)
        .current_dir(&t.go.dir)
        .env_clear()
        .envs(vars)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request(&t.go).as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let expected = CommandOutput {
        exit_code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    };
    let mut vars = t.rust.run_env();
    vars.push(("LUUNA_ATLAS_API_BRIDGE".into(), "1".into()));
    let ctx = Context {
        root: t.rust.dir.clone(),
        cwd: t.rust.dir.clone(),
        env: Env::fixed(vars, Vec::new(), false),
        globals: GlobalOptions::default(),
    };
    let actual = tooling::run(&ctx, &[], request(&t.rust).as_bytes());
    assert_same_output(input, &t.go, &expected, &t.rust, &actual);
    t.assert_same_tree(input);
}
#[test]
#[ignore = "requires pinned Go reference"]
fn bridge_pure_operations() {
    for req in [
        r#"{"operation":"configSchema"}"#,
        r#"{"operation":"recordSchema"}"#,
        r#"{"operation":"defaultConfig"}"#,
        r#"{"operation":"defaultConfig","defaults":{"governance":{"max_entries":7},"extra":"retained"}}"#,
        r#"{"operation":"readConfig","args":["<project>"]}"#,
        r#"{"operation":"readConfig","args":["<project>"],"defaults":{"governance":{"added":7},"classification_defaults":{"extra":true,"shelf_life":{"other":20}}}}"#,
        r#"{"operation":"getExpertisePath","args":["core","<project>"]}"#,
        r#"{"operation":"missing"}"#,
        r#"null"#,
        r#"broken"#,
        r#"{"operation":}"#,
        r#"[1,]"#,
        r#"{"operation":"configSchema"} trailing"#,
        r#"{"operation":"generateRecordId","args":[{"type":"convention","content":"Hello"}]}"#,
        r#"{"operation":"findDuplicate","args":[[{"type":"convention","content":"Hello"}],{"type":"convention","content":"Hello"}]}"#,
        r#"{"operation":"sortByConfirmationScoreIndexes","args":[[{"type":"convention","content":"same"},{"type":"convention","content":"same"}]]}"#,
        r#"{"operation":"searchRecordIndexes","args":[[{"type":"convention","content":"same"},{"type":"convention","content":"same"}],"same"]}"#,
    ] {
        bridge_compare(req)
    }
    for op in [
        "computeConfirmationScore",
        "getSuccessCount",
        "getFailureCount",
        "getTotalApplications",
        "getSuccessRate",
    ] {
        bridge_compare(&format!(
            r#"{{"operation":"{op}","args":[{{"outcomes":[{{"status":"success"}},{{"status":"failure"}},{{"status":"success"}}]}}]}}"#
        ));
    }
    for special in ["NaN", "Infinity", "-Infinity", "-0"] {
        bridge_compare(&format!(
            r#"{{"operation":"defaultConfig","defaults":{{"value":null}},"numbers":[{{"path":["defaults","value"],"value":"{special}"}}]}}"#
        ));
    }
}

#[test]
#[ignore = "requires pinned Go reference"]
fn a8_repair_refuses_and_preserves_all_files() {
    let valid = r#"{"id":"mx-aaaa","type":"convention","content":"Keep","classification":"foundational","recorded_at":"2026-01-01T00:00:00Z","outcome":{"status":"success"}}"#;
    type LossCase<'a> = (&'a str, Vec<(&'a str, String)>, Vec<&'a str>);
    let cases:Vec<LossCase<'_>>=vec![
 ("unbounded onboard",vec![("CLAUDE.md","# Keep\n## Project Expertise (Atlas)\nold\n## Unrelated instructions\nKEEP ME\n".into())],vec!["onboard"]),
 ("modified cursor",vec![(".cursor/rules/luuna-atlas.mdc","KEEP MY RULE\n".into())],vec!["setup","cursor"]),
 ("extra hook group fields",vec![(".claude/settings.json",r#"{"hooks":{"SessionStart":[{"matcher":"", "extra":"KEEP", "hooks":[{"type":"command","command":"luuna-atlas prime"}]}]}}"#.into())],vec!["setup","claude"]),
 ("negative zero settings",vec![(".claude/settings.json",r#"{"extra":-0,"hooks":{}}"#.into())],vec!["setup","claude"]),
 ("duplicate settings keys",vec![(".claude/settings.json",r#"{"extra":1,"extra":2}"#.into())],vec!["setup","claude"]),
 ("invalid record",vec![(".luuna/atlas/expertise/core.jsonl",r#"{"id":"mx-aaaa","type":"pattern","name":"Keep invalid","recorded_at":"2026-01-01T00:00:00Z"}"#.to_owned()+"\n")],vec!["doctor","--fix"]),
 ("stale record",vec![(".luuna/atlas/expertise/core.jsonl",r#"{"id":"mx-aaaa","type":"convention","content":"Keep stale","classification":"observational","recorded_at":"2020-01-01T00:00:00Z"}"#.to_owned()+"\n")],vec!["doctor","--fix"]),
 ("later invalid record before earlier migration",vec![(".luuna/atlas/expertise/core.jsonl",format!("{valid}\n{{\"id\":\"mx-bbbb\",\"type\":\"pattern\",\"name\":\"Keep invalid\"}}\n"))],vec!["doctor","--fix"])
 ];
    for (label, files, args) in cases {
        let t = Twin::new(true);
        t.steps(&[Step::Init, Step::Add("core".into())]);
        t.prepare(|p| {
            for (rel, data) in &files {
                p.write(rel, data);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(
                        p.dir.join(rel),
                        std::fs::Permissions::from_mode(0o640),
                    )
                    .unwrap();
                }
            }
        });
        let before = t.rust.snapshot();
        let go_before = t.go.snapshot();
        let _go = t.go.go(&args);
        assert_ne!(
            go_before,
            t.go.snapshot(),
            "Go loss probe {label} must actually change files"
        );
        let mut args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        args.extend(["--json".into(), "--quiet".into()]);
        let ctx = Context {
            root: t.rust.dir.clone(),
            cwd: t.rust.dir.clone(),
            env: Env::fixed(t.rust.run_env(), args.clone(), false),
            globals: GlobalOptions::default(),
        };
        let out = tooling::run(&ctx, &args, b"");
        assert_eq!(out.exit_code, 1, "{label}: {out:?}");
        assert!(out.stderr.contains("Refusing"), "{label}: {out:?}");
        assert_eq!(
            before,
            t.rust.snapshot(),
            "{label}: all files and modes remain untouched"
        );
        for (rel, data) in &files {
            assert_eq!(
                std::fs::read(t.rust.dir.join(rel)).unwrap(),
                data.as_bytes(),
                "{label}: exact bytes"
            );
        }
    }
}
#[test]
#[ignore = "requires pinned Go reference"]
fn setup_installed_check_repeat_remove() {
    for provider in ["claude", "codex", "cursor"] {
        for extra in [
            vec![],
            vec!["--json"],
            vec!["--check"],
            vec!["--remove"],
            vec!["--remove", "--json"],
        ] {
            let mut args = vec!["setup", provider];
            args.extend(extra);
            compare_prepared(&args, |p| {
                let out = Command::new(&support::reference::reference().binary)
                    .args(["setup", provider, "--json"])
                    .current_dir(&p.dir)
                    .env_clear()
                    .envs(p.run_env())
                    .output()
                    .unwrap();
                assert!(out.status.success());
            });
        }
    }
}

#[test]
#[ignore = "requires pinned Go reference"]
fn every_command_help() {
    let contract = luuna_atlas::lifecycle::contract::contract()
        .as_object()
        .unwrap();
    let root = contract.get("cli").unwrap().as_object().unwrap();
    for command in root.get("commands").unwrap().as_array().unwrap() {
        let c = command.as_object().unwrap();
        let name = c.get("name").unwrap().as_str().unwrap();
        compare(&[name, "--help"]);
        compare(&[name, "--bad", "--help"]);
        for child in luuna_atlas::records::any_list(c.get("commands")) {
            let child = child
                .as_object()
                .unwrap()
                .get("name")
                .unwrap()
                .as_str()
                .unwrap();
            compare(&[name, child, "--help"]);
        }
    }
}
#[test]
#[ignore = "requires pinned Go reference"]
fn dispatch_native_actions_and_defaults() {
    for args in [
        vec!["init"],
        vec!["init", "--json"],
        vec!["add", "new-domain"],
        vec!["add", "new-domain", "--json"],
        vec![
            "record",
            "core",
            "Hello",
            "--type",
            "convention",
            "--classification",
            "foundational",
            "--force",
        ],
        vec!["record", "core", "--stdin"],
        vec!["record", "core", "--batch", "batch.json"],
        vec!["record", "core", "--batch=missing"],
        vec!["query", "core", "--json"],
        vec!["prime", "--json"],
        vec!["status", "--json"],
        vec!["config", "schema"],
        vec!["config", "show", "--json"],
    ] {
        compare_prepared(&args, |p| {
            p.write(
                "batch.json",
                r#"[{"type":"convention","content":"Bulk","classification":"foundational"}]"#,
            )
        });
    }
}

fn hostile_root(name: &str) -> tempfile::TempDir {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/tooling-hostile")
        .join(name);
    std::fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}
fn digest(path: &std::path::Path) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path).unwrap();
    let mut hash = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = f.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    hash.finalize().into()
}
#[test]
#[ignore = "hostile input: compile first, then run with 4GiB and 120s limits"]
fn hostile_bridge_deep_numbers() {
    use std::io::{BufWriter, Write};
    let dir = hostile_root("bridge");
    let input = dir.path().join("input.json");
    let mut f = BufWriter::new(std::fs::File::create(&input).unwrap());
    f.write_all(br#"{"operation":"defaultConfig","defaults":{"deep":"#)
        .unwrap();
    for _ in 0..9990 {
        f.write_all(b"[").unwrap();
    }
    f.write_all(b"null").unwrap();
    for _ in 0..9990 {
        f.write_all(b"]").unwrap();
    }
    f.write_all(br#"},"numbers":[{"path":["defaults","deep""#)
        .unwrap();
    for _ in 0..9990 {
        f.write_all(b",0").unwrap();
    }
    f.write_all(br#"],"value":"NaN"}]}"#).unwrap();
    f.flush().unwrap();
    drop(f);
    let expected = dir.path().join("go.json");
    let error = dir.path().join("go.stderr");
    let env = vec![
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("HOME".into(), dir.path().to_string_lossy().into_owned()),
        ("LUUNA_ATLAS_API_BRIDGE".into(), "1".into()),
    ];
    let status = Command::new(&support::reference::reference().binary)
        .current_dir(dir.path())
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(std::fs::File::open(&input).unwrap())
        .stdout(std::fs::File::create(&expected).unwrap())
        .stderr(std::fs::File::create(&error).unwrap())
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(std::fs::metadata(&error).unwrap().len(), 0);
    let bytes = std::fs::read(&input).unwrap();
    let ctx = Context {
        root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        env: Env::fixed(env, Vec::new(), false),
        globals: GlobalOptions::default(),
    };
    let actual = std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(move || tooling::run(&ctx, &[], &bytes))
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(actual.exit_code, 0);
    assert!(actual.stderr.is_empty());
    let native = dir.path().join("native.json");
    std::fs::write(&native, &actual.stdout).unwrap();
    drop(actual);
    assert!(std::fs::metadata(&expected).unwrap().len() > 100_000_000);
    assert_eq!(digest(&expected), digest(&native));
}
#[test]
#[ignore = "hostile input: compile first, then run with 4GiB and 120s limits"]
fn hostile_parser_many_unknown_flags_help_priority() {
    use std::io::{BufRead, BufReader, BufWriter, Write};
    let dir = hostile_root("parser");
    let file = dir.path().join("args.txt");
    let mut out = BufWriter::new(std::fs::File::create(&file).unwrap());
    writeln!(out, "record").unwrap();
    for _ in 0..20000 {
        writeln!(out, "--unknown").unwrap();
    }
    writeln!(out, "--help").unwrap();
    out.flush().unwrap();
    drop(out);
    let args: Vec<String> = BufReader::new(std::fs::File::open(file).unwrap())
        .lines()
        .map(std::result::Result::unwrap)
        .collect();
    let vars = vec![
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("HOME".into(), dir.path().to_string_lossy().into_owned()),
        ("NO_COLOR".into(), "1".into()),
    ];
    let expected = Command::new(&support::reference::reference().binary)
        .args(&args)
        .current_dir(dir.path())
        .env_clear()
        .envs(vars.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(expected.status.success());
    let ctx = Context {
        root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        env: Env::fixed(vars, args.clone(), false),
        globals: GlobalOptions::default(),
    };
    let actual = std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(move || tooling::run(&ctx, &args, b""))
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(actual.exit_code, 0);
    assert_eq!(actual.stdout.as_bytes(), expected.stdout);
    assert_eq!(actual.stderr.as_bytes(), expected.stderr);
}

#[test]
#[ignore = "requires pinned Go reference"]
fn onboard_preserves_arbitrary_body_bytes() {
    for flags in [vec![], vec!["--check"], vec!["--json"]] {
        let mut args = vec!["onboard"];
        args.extend(flags);
        compare_prepared(&args, |p| {
            let mut body = b"# Instructions\nKEEP \xff\x80 bytes\n".to_vec();
            body.extend_from_slice("\u{f0000}\u{f01ff}\n".as_bytes());
            std::fs::write(p.dir.join("CLAUDE.md"), body).unwrap();
        });
    }
}
#[test]
#[ignore = "requires pinned Go reference"]
fn custom_registry_cli_options_and_help() {
    let custom="\ncustom_types:\n  hypothesis:\n    required: [statement]\n    optional: [confidence]\n    dedup_key: statement\n    summary: '{statement}'\n    section_title: Hypotheses\n";
    for args in [
        vec!["record", "--help"],
        vec!["edit", "--help"],
        vec!["search", "--help"],
        vec!["rank", "--help"],
        vec![
            "record",
            "core",
            "--type",
            "hypothesis",
            "--statement",
            "Evidence-based claim",
            "--confidence",
            "high",
            "--force",
        ],
        vec!["search", "--type", "hypothesis", "--json"],
        vec!["record", "core", "--type", "hypothsis"],
        vec!["record", "core", "--statement"],
    ] {
        compare_prepared(&args, |p| p.append(&p.config_path(), custom));
    }
}
#[test]
#[ignore = "requires pinned Go reference"]
fn marker_update_remove_and_legacy_bounded() {
    let fence = String::from_utf8(vec![96; 3]).unwrap();
    let legacy=format!("# Before\n## Project Expertise (Atlas)\nlegacy\n{fence}bash\nluuna-atlas validate && git add .luuna/atlas/ && git commit -m \"luuna-atlas: record learnings\"\n{fence}\n\n# Keep after\n");
    for body in [
        legacy,
        "# Before\n<!-- luuna-atlas:start -->\nOld\n<!-- luuna-atlas:end -->\n# Keep after\n"
            .into(),
        "# Before\n<!-- mulch:start -->\nOld\n<!-- mulch:end -->\n# Keep after\n".into(),
        "# Before\n<!-- luuna-atlas:start -->\nOld without end\n# Keep after\n".into(),
    ] {
        for args in [
            vec!["onboard"],
            vec!["onboard", "--check", "--json"],
            vec!["setup", "codex", "--remove", "--json"],
        ] {
            compare_prepared(&args, |p| p.write("AGENTS.md", &body));
        }
    }
    for body in [
        "#!/bin/sh\nKEEP PREFIX\n# luuna-atlas:start\nKEEP BODY\n",
        "#!/bin/sh\n# luuna-atlas:end\nKEEP BODY\n# luuna-atlas:start\nKEEP TAIL\n",
    ] {
        compare_prepared(&["setup", "--hooks", "--remove", "--json"], |p| {
            p.write(".git/hooks/pre-commit", body)
        });
    }
}

#[test]
#[ignore = "requires pinned Go reference"]
fn doctor_comments_orphans_and_combined_repairs() {
    for flags in [vec!["--json"], vec!["--fix"], vec!["--fix", "--json"]] {
        let mut args = vec!["doctor"];
        args.extend(flags);
        for which in 0..3 {
            compare_prepared(&args, |p| {
                let row = r#"{"id":"mx-aaaa","type":"pattern","name":"Keep body","description":"Keep body","classification":"foundational","recorded_at":"2026-01-01T00:00:00Z","outcome":{"status":"success"},"files":["missing.rs"]}"#;
                match which {
                    0 => p.write(
                        ".luuna/atlas/expertise/core.jsonl",
                        &format!("# Comment\n{row}\n"),
                    ),
                    1 => {
                        p.write(".luuna/atlas/expertise/orphan.jsonl", "");
                        std::fs::remove_file(p.expertise("core")).unwrap();
                    }
                    _ => p.write(".luuna/atlas/expertise/core.jsonl", &format!("{row}\n")),
                }
            });
        }
    }
}
#[test]
#[ignore = "requires pinned Go reference"]
fn setup_external_discovery_shell_and_javascript() {
    for kind in ["sh", "ts", "npm"] {
        for flags in [
            vec![],
            vec!["--json"],
            vec!["--check"],
            vec!["--remove"],
            vec!["--list", "--json"],
        ] {
            let mut args = vec!["setup", "custom"];
            args.extend(flags);
            compare_prepared(&args, |p| {
                if kind != "sh" {
                    assert!(
                        Env::fixed(p.run_env(), Vec::new(), false)
                            .look_path("bun")
                            .is_some(),
                        "Bun is required only for these explicit recipe-extension comparisons"
                    );
                }
                if kind == "sh" {
                    p.write(
                        ".luuna/atlas/recipes/custom.sh",
                        "#!/bin/sh\nprintf 'custom %s from shell\\n' \"$1\"\n",
                    );
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(
                            p.dir.join(".luuna/atlas/recipes/custom.sh"),
                            std::fs::Permissions::from_mode(0o755),
                        )
                        .unwrap();
                    }
                } else {
                    let code="const recipe={install:async()=>({success:true,message:'installed custom'}),check:async()=>({success:true,message:'checked custom'}),remove:async()=>({success:true,message:'removed custom'})};export default recipe;\n";
                    if kind == "ts" {
                        p.write(".luuna/atlas/recipes/custom.ts", code)
                    } else {
                        p.write("node_modules/luuna-atlas-recipe-custom/package.json",r#"{"name":"luuna-atlas-recipe-custom","type":"module","exports":"./recipe.js"}"#);
                        p.write("node_modules/luuna-atlas-recipe-custom/recipe.js", code);
                    }
                }
            });
        }
    }
}

#[test]
#[ignore = "requires pinned Go reference"]
fn bridge_store_operations_and_mutation_transport() {
    for req in [
        r#"{"operation":"writeExpertiseFileTransport","args":["<project>/.luuna/atlas/expertise/core.jsonl",[[null]]],"numbers":[{"path":["args",1,0,0],"value":"NaN"}]}"#,
        r#"{"operation":"appendRecordTransport","args":["<project>/.luuna/atlas/expertise/core.jsonl",{"type":"convention","content":"API body"}]}"#,
        r#"{"operation":"writeExpertiseFileTransport","args":["<project>/.luuna/atlas/expertise/core.jsonl",[{"type":"convention","content":"One"}]]}"#,
        r#"{"operation":"getExpertisePath","args":["core","."]}"#,
        r#"{"operation":"appendRecord","args":["<project>/.luuna/atlas/expertise/core.jsonl",{"type":"convention","content":"API body"}]}"#,
        r#"{"operation":"appendRecordTransport","args":["missing/subdir.jsonl",{"type":"convention","content":"API body"}]}"#,
        r#"{"operation":"writeExpertiseFile","args":["<project>/.luuna/atlas/expertise/core.jsonl",[{"type":"convention","content":"One"},{"type":"convention","content":"Two"}]]}"#,
        r#"{"operation":"writeExpertiseFileTransport","args":["<project>/.luuna/atlas/expertise/core.jsonl",[{"type":"convention","content":"One"},{"type":"unknown","content":"Two"}]]}"#,
        r#"{"operation":"writeExpertiseFileTransport","args":["<project>/.luuna/atlas/expertise/core.jsonl",[[1,2]]] }"#,
        r#"{"operation":"readExpertiseFile","args":["missing.jsonl"]}"#,
        r#"{"operation":"recordExpertise","args":["core",{"type":"convention","content":"API body"}] }"#,
        r#"{"operation":"recordExpertiseTransport","args":["absent",{"type":"convention","content":"API body"}] }"#,
        r#"{"operation":"queryDomain","args":["core"]}"#,
        r#"{"operation":"searchExpertise","args":["body"]}"#,
        r#"{"operation":"editRecord","args":["core","mx-absent",{"content":"new"}]}"#,
        r#"{"operation":"appendOutcome","args":["core","mx-absent",{"status":"success","recorded_at":"2026-01-01T00:00:00Z"}]}"#,
    ] {
        bridge_compare(req)
    }
}

#[test]
#[ignore = "requires pinned Go reference"]
fn bridge_decoder_token_diagnostics() {
    let cases = luuna_atlas::json::parse(include_str!(
        "fixtures/s5-atlas-lib-5/bridge-json-errors.json"
    ))
    .unwrap();
    for case in cases.as_array().unwrap() {
        bridge_compare(
            case.as_object()
                .unwrap()
                .get("input")
                .unwrap()
                .as_str()
                .unwrap(),
        );
    }
}

#[test]
#[ignore = "requires pinned Go reference"]
fn bridge_legacy_migration_notice() {
    for operation in [
        "readConfig",
        "queryDomain",
        "searchExpertise",
        "getExpertisePath",
    ] {
        let request = match operation {
            "readConfig" => r#"{"operation":"readConfig"}"#.to_owned(),
            "getExpertisePath" => r#"{"operation":"getExpertisePath","args":["core"]}"#.to_owned(),
            "queryDomain" => r#"{"operation":"queryDomain","args":["core"]}"#.to_owned(),
            _ => r#"{"operation":"searchExpertise","args":["test"]}"#.to_owned(),
        };
        bridge_prepared(&request, |p| {
            std::fs::rename(p.atlas_dir(), p.dir.join(".mulch")).unwrap()
        });
    }
}
#[test]
#[ignore = "requires pinned Go reference"]
fn init_refuses_unreadable_existing_attributes() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let t = Twin::new(true);
        let original = b"KEEP existing rules\n";
        for p in [&t.go, &t.rust] {
            std::fs::write(p.dir.join(".gitattributes"), original).unwrap();
            std::fs::set_permissions(
                p.dir.join(".gitattributes"),
                std::fs::Permissions::from_mode(0o200),
            )
            .unwrap();
            assert!(std::fs::read(p.dir.join(".gitattributes")).is_err());
        }
        let go = t.go.go(&["init"]);
        assert_eq!(go.exit_code, 0);
        let args = vec!["init".into(), "--json".into()];
        let ctx = Context {
            root: t.rust.dir.clone(),
            cwd: t.rust.dir.clone(),
            env: Env::fixed(t.rust.run_env(), args.clone(), false),
            globals: GlobalOptions::default(),
        };
        let actual = tooling::run(&ctx, &args, b"");
        assert_eq!(actual.exit_code, 1);
        assert!(actual.stderr.contains("Refusing"));
        assert!(!t.rust.atlas_dir().exists());
        assert_eq!(
            std::fs::metadata(t.rust.dir.join(".gitattributes"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o200
        );
        for p in [&t.go, &t.rust] {
            std::fs::set_permissions(
                p.dir.join(".gitattributes"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        assert_eq!(
            std::fs::read(t.rust.dir.join(".gitattributes")).unwrap(),
            original
        );
        assert!(!std::fs::read(t.go.dir.join(".gitattributes"))
            .unwrap()
            .windows(4)
            .any(|s| s == b"KEEP"));
    }
}

#[test]
#[ignore = "requires pinned Go reference"]
fn provider_and_repair_symlink_modes() {
    #[cfg(unix)]
    {
        use std::os::unix::{fs::symlink, fs::PermissionsExt};
        for which in 0..3 {
            let args = match which {
                0 => vec!["onboard", "--json"],
                1 => vec!["setup", "claude", "--json"],
                _ => vec!["doctor", "--fix", "--json"],
            };
            compare_prepared(&args, |p| {
                let (link,target,body)=match which{0=>("CLAUDE.md","instructions.md",b"# Keep instructions\n".to_vec()),1=>(".claude/settings.json","../saved-settings.json",br#"{"other":{"keep":true}}"#.to_vec()),_=>(".luuna/atlas/expertise/core.jsonl","../../../saved-records.jsonl",br#"{"id":"mx-aaaa","type":"pattern","name":"Keep","description":"Keep body","classification":"foundational","recorded_at":"2026-01-01T00:00:00Z","files":["missing.rs"]}
"#.to_vec())};
                let link = p.dir.join(link);
                std::fs::create_dir_all(link.parent().unwrap()).unwrap();
                let target = link.parent().unwrap().join(target);
                std::fs::write(&target, body).unwrap();
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
                if link.exists() {
                    std::fs::remove_file(&link).unwrap();
                }
                symlink(
                    if which == 0 {
                        "instructions.md"
                    } else if which == 1 {
                        "../saved-settings.json"
                    } else {
                        "../../../saved-records.jsonl"
                    },
                    &link,
                )
                .unwrap();
            });
        }
    }
}

#[test]
#[ignore = "requires pinned Go reference"]
fn audit_raw_yaml_keys_and_empty_tracker_values() {
    compare_prepared(&["audit", "--json"], |p| {
        p.append(&p.config_path(),"\naudit:\n  ignore_domains: [!!binary /w==, !!binary gA==]\n  thresholds:\n    ? !!binary /w==\n    : kept\n    ? !!binary gA==\n    : other\n")
    });
    compare_prepared(&["audit", "--json"], |p| {
        p.write(".luuna/atlas/expertise/core.jsonl",r#"{"id":"mx-aaaa","type":"convention","content":"Rule","classification":"foundational","recorded_at":"2026-01-01T00:00:00Z","evidence":{"missions":"ms-1234"}}
"#);
        p.write(
            ".luuna/missions/issues.jsonl",
            r#"{"id":"ms-1234","status":"","title":""}
"#,
        );
    });
}

#[test]
#[ignore = "requires pinned Go reference"]
fn bridge_explicit_file_replacement() {
    bridge_prepared(
        r#"{"operation":"writeExpertiseFile","args":["<project>/.luuna/atlas/expertise/core.jsonl",[{"id":"mx-new","type":"convention","content":"Explicit replacement"}]]}"#,
        |p| {
            p.write(
                ".luuna/atlas/expertise/core.jsonl",
                r#"{"id":"mx-old","type":"convention","content":"first","content":"second"}
"#,
            )
        },
    );
}
