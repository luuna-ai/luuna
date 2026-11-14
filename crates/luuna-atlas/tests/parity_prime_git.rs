//! Isolated Go comparisons for prime, Git commands and hooks.
mod support;
use luuna_atlas::{
    options::{Context, Env, GlobalOptions},
    prime, CommandOutput,
};
use std::{
    ffi::OsString,
    io::Write,
    process::{Command, Stdio},
};
use support::project::{assert_same_output, Project, Step, Twin};
const ROWS:&str=concat!(
    "{\"id\":\"mx-aaaa\",\"type\":\"convention\",\"content\":\"Prefer exact checks.\",\"classification\":\"tactical\",\"recorded_at\":\"2026-01-01T00:00:00.000Z\",\"tags\":[\"rust\"],\"outcomes\":[{\"status\":\"success\"}]}\n",
    "{\"id\":\"mx-bbbb\",\"type\":\"failure\",\"description\":\"Flaky check\",\"resolution\":\"Use deterministic checks.\",\"classification\":\"foundational\",\"recorded_at\":\"2026-01-02T00:00:00.000Z\",\"outcomes\":[{\"status\":\"failure\"}]}\n",
    "{\"id\":\"mx-cccc\",\"type\":\"pattern\",\"name\":\"Atomic write\",\"description\":\"Use locks for exact checks.\",\"files\":[\"src/main.rs\"],\"classification\":\"observational\",\"recorded_at\":\"2026-01-03T00:00:00.000Z\"}\n"
);
fn base(seed: bool) -> Twin {
    let t = Twin::new(true);
    t.steps(&[
        Step::Init,
        Step::Add("core".into()),
        Step::Add("other".into()),
    ]);
    if seed {
        t.prepare(|p| p.write(".luuna/atlas/expertise/core.jsonl", ROWS));
    }
    t
}
fn vars(p: &Project, extra: &[(&str, String)]) -> Vec<(OsString, OsString)> {
    let mut vars = p.run_env();
    let bin = p.dir.parent().unwrap().join("prime-test-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    if !gh.exists() {
        std::fs::write(&gh, "#!/bin/sh\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    if let Some((_, path)) = vars.iter_mut().find(|(key, _)| key == "PATH") {
        let old = path.to_string_lossy().into_owned();
        *path = format!("{}:{old}", bin.display()).into();
    }
    vars.retain(|(key, _)| key != "LUUNA_ATLAS_LOG_LEVEL");
    vars.push(("LUUNA_ATLAS_LOG_LEVEL".into(), "silent".into()));
    vars.retain(|(key, _)| !extra.iter().any(|(name, _)| key == *name));
    vars.extend(
        extra
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v))),
    );
    vars
}
fn go(
    p: &Project,
    args: &[String],
    input: Option<&[u8]>,
    extra: &[(&str, String)],
) -> CommandOutput {
    let mut c = Command::new(&support::reference::reference().binary);
    c.args(args)
        .current_dir(&p.dir)
        .env_clear()
        .envs(vars(p, extra))
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    if let Some(input) = input {
        child.stdin.take().unwrap().write_all(input).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    CommandOutput {
        exit_code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}
fn parsed(args: &[String]) -> (Vec<String>, Vec<(String, String)>) {
    let mut pos = Vec::new();
    let mut flags = Vec::new();
    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if let Some(flag) = arg.strip_prefix("--") {
            if let Some((k, v)) = flag.split_once('=') {
                flags.push((k.into(), v.into()));
            } else if let Some(k) = flag.strip_prefix("no-") {
                flags.push((k.into(), "false".into()));
            } else if [
                "json",
                "quiet",
                "verbose",
                "force",
                "dry-run",
                "all",
                "yes",
                "apply",
                "analyze",
                "auto",
                "hard",
                "aggressive",
                "explain",
                "check-anchors",
                "stdin",
                "compact",
                "manifest",
                "records-only",
                "context",
                "limit",
                "validate",
                "archived",
                "sort-by-score",
                "full",
                "allow-unknown-types",
                "allow-domain-mismatch",
            ]
            .contains(&flag)
            {
                flags.push((flag.into(), "true".into()));
            } else if ["files", "domain", "exclude-domain"].contains(&flag) {
                while args.get(i + 1).is_some_and(|v| !v.starts_with('-')) {
                    i += 1;
                    flags.push((flag.into(), args[i].clone()));
                }
            } else {
                i += 1;
                flags.push((flag.into(), args.get(i).cloned().unwrap_or_default()));
            }
        } else if arg == "-q" {
            flags.push(("quiet".into(), "true".into()));
        } else {
            pos.push(arg.clone());
        }
        i += 1;
    }
    (pos, flags)
}

fn native(p: &Project, args: &[String], input: Option<&[u8]>) -> CommandOutput {
    let (pos, flags) = parsed(args);
    let mut globals = GlobalOptions::prescan(args);
    for (k, v) in &flags {
        globals.set(k, std::slice::from_ref(v));
    }
    let env = Env::fixed(vars(p, &[]), args.to_vec(), false);
    let ctx = Context {
        root: luuna_atlas::git::resolve_store_root(&p.dir, &env),
        cwd: p.dir.clone(),
        env,
        globals,
    };
    prime::run(
        &ctx,
        &args[0],
        &pos,
        &prime::Options {
            flags,
            input: input.unwrap_or_default().to_vec(),
        },
    )
}
fn compare(t: &Twin, args: &[&str], human: bool, expected: i32, input: Option<&[u8]>) {
    let mut full: Vec<String> = args.iter().map(|s| (*s).into()).collect();
    if !human {
        full.extend(["--json".into(), "--quiet".into()]);
    }
    let g = go(&t.go, &full, input, &[]);
    assert_eq!(
        g.exit_code,
        expected,
        "Go fixture {}: {}",
        full.join(" "),
        g.stderr
    );
    let r = native(&t.rust, &full, input);
    assert_same_output(&full.join(" "), &t.go, &g, &t.rust, &r);
    t.assert_same_tree(&full.join(" "));
}
macro_rules! case {($name:ident,$seed:expr,$human:expr,$expected:expr,$($args:expr),+ $(,)?)=>{
    #[test] #[ignore="needs the pinned Go reference; run -- --ignored"] fn $name(){let t=base($seed);for args in [$($args),+]{compare(&t,args,$human,$expected,None);}}
};}

case!(
    prime_json,
    true,
    false,
    0,
    &["prime", "--all"][..],
    &["prime", "core"],
    &["prime", "--manifest"],
    &["prime", "--dry-run"]
);
case!(
    prime_human,
    true,
    true,
    0,
    &["prime", "--all"][..],
    &["prime", "core", "--format", "plain"],
    &["prime", "--all", "--format", "xml"],
    &["prime", "--manifest"]
);
case!(
    prime_scopes,
    true,
    false,
    0,
    &["prime", "--files", "src/main.rs"][..],
    &["prime", "--domain", "core", "--exclude-domain", "other"],
    &["prime", "--files", "no-match"]
);
case!(
    prime_budget,
    true,
    true,
    0,
    &["prime", "--all", "--budget", "1"][..],
    &["prime", "--all", "--dry-run", "--budget", "20"],
    &["prime", "--all", "--no-limit"],
    &["prime", "--all", "--full"]
);
case!(
    prime_errors,
    true,
    false,
    1,
    &["prime", "--manifest", "--full"][..],
    &["prime", "--dry-run", "--manifest"],
    &["prime", "--manifest", "core"],
    &["prime", "--budget", "0"],
    &["prime", "missing"],
    &["prime", "--exclude-domain", "missing"]
);
case!(
    prime_export,
    true,
    true,
    0,
    &["prime", "--all", "--export", "primed.txt"][..],
    &["prime", "--dry-run", "--export", "preview.json"]
);
case!(
    prime_empty,
    false,
    true,
    0,
    &["prime", "--all"][..],
    &["prime", "--manifest"],
    &["prime", "--dry-run"]
);
case!(
    git_learn_diff_empty,
    true,
    false,
    0,
    &["learn", "--since", "HEAD"][..],
    &["diff", "--since", "HEAD"],
    &["diff", "--since", "missing-ref"]
);
case!(
    git_sync_validation,
    true,
    false,
    0,
    &["sync"][..],
    &["sync", "--no-validate"]
);

#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn hook_guard_shell_matrix() {
    use luuna_atlas::json::{Object, Value};
    let t = base(true);
    for command in [
        "echo x > .luuna/atlas/expertise/a.jsonl",
        "cat x | tee .luuna/atlas/expertise/a.jsonl",
        "rm .luuna/atlas/expertise/a.jsonl",
        "sed -i x .luuna/atlas/expertise/a.jsonl",
        "cp x .luuna/atlas/expertise/a.jsonl",
        "cat .luuna/atlas/expertise/a.jsonl",
        "luuna-atlas record core --type convention x",
        "git add .luuna/atlas/expertise/a.jsonl",
        "echo x 2> '.luuna/atlas/expertise/a.jsonl'",
        "echo x 12> .luuna/atlas/expertise/a.jsonl",
        "echo x &>> \".luuna/atlas/expertise/a.jsonl\"",
        "echo x <> .luuna/atlas/expertise/a.jsonl",
        "echo x < .luuna/atlas/expertise/a.jsonl",
        "A=1 sudo env command nohup time xargs /bin/rm .luuna/atlas/expertise/a.jsonl",
        "cp .luuna/atlas/expertise/a.jsonl unrelated",
        "ln x .luuna\\atlas\\expertise\\a.jsonl",
        "perl -p -i -e x .luuna/atlas/expertise/a.jsonl",
        "sed --in-place x .luuna/atlas/expertise/a.jsonl",
        "dd if=x of=.luuna/atlas/expertise/a.jsonl",
        "echo 1; truncate .luuna/atlas/expertise/a.jsonl",
        "echo 1 && touch .luuna/atlas/expertise/a.jsonl",
        "echo 1 || unlink .luuna/atlas/expertise/a.jsonl",
        "mulch record x --description y > .luuna/atlas/expertise/a.jsonl",
        "ml record x > .luuna/atlas/expertise/a.jsonl",
        "echo x > '.luuna/atlas/expertise/a.jsonl",
        "echo x > \".luuna/atlas/expertise/a.jsonl",
        "echo x >\u{b}.luuna/atlas/expertise/a.jsonl",
        "echo x >\u{a0}.luuna/atlas/expertise/a.jsonl",
        "echo x > unrelated ; cat .luuna/atlas/expertise/a.jsonl",
    ] {
        let mut input = Object::new();
        input.insert("tool_name", "Bash");
        let mut tool = Object::new();
        tool.insert("command", command);
        input.insert("tool_input", tool);
        let raw = luuna_atlas::json::to_compact_string(&Value::Object(input)).unwrap();
        compare(&t, &["hook"], false, 0, Some(raw.as_bytes()));
    }
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn hook_file_guard_and_fail_open() {
    let t = base(true);
    for raw in [
        "not-json",
        "[]",
        "{}",
        r#"{"hook_event_name":"PostToolUse","tool_name":"Write","tool_input":{"file_path":".luuna/atlas/expertise/a.jsonl"}}"#,
        r#"{"tool_name":"Write","tool_input":{"file_path":".luuna/atlas/expertise/a.jsonl"}}"#,
        r#"{"tool_name":"Edit","tool_input":{"file_path":"./.luuna/atlas/expertise//a.jsonl"}}"#,
        r#"{"tool_name":"MultiEdit","tool_input":{"file_path":".luuna\\atlas\\expertise\\a.jsonl"}}"#,
        r#"{"tool_name":"NotebookEdit","tool_input":{"notebook_path":".luuna/atlas/expertise/a.jsonl"}}"#,
        r#"{"tool_name":"Write","tool_input":{"file_path":".luuna/atlas/expertise/.jsonl"}}"#,
        r#"{"tool_name":"Unknown","tool_input":{"file_path":".luuna/atlas/expertise/a.jsonl"}}"#,
        r#"{"tool_name":"Read","tool_input":{}}"#,
    ] {
        compare(&t, &["hook"], false, 0, Some(raw.as_bytes()));
    }
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn hook_injection_deduplicates_and_ignores_state() {
    let t = base(true);
    t.prepare(|p| p.write("src/main.rs", "source\n"));
    let raw = r#"{"hook_event_name":"PreToolUse","tool_name":"Read","tool_input":{"file_path":"src/main.rs"},"session_id":"session/one"}"#;
    compare(&t, &["hook"], false, 0, Some(raw.as_bytes()));
    compare(&t, &["hook"], false, 0, Some(raw.as_bytes()));
    assert_eq!(
        std::fs::read_to_string(t.rust.atlas_dir().join("state/.gitignore")).unwrap(),
        "*\n"
    );
    assert_eq!(
        std::fs::read_to_string(t.rust.atlas_dir().join("state/usage.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert_eq!(
        std::fs::read_to_string(t.rust.atlas_dir().join("state/sessions/session_one.txt")).unwrap(),
        "mx-cccc\n"
    );
}

fn append_config(t: &Twin, text: &str) {
    t.prepare(|p| {
        let file = p.atlas_dir().join("config.yaml");
        let mut s = std::fs::read_to_string(&file).unwrap();
        s.push_str(text);
        std::fs::write(file, s).unwrap();
    });
}
fn commit_all(t: &Twin) {
    t.prepare(|p| {
        p.git(&["add", "."]);
        p.git(&["commit", "-qm", "fixture base"]);
    });
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn prime_all_formats_and_variadic_scopes() {
    for format in ["markdown", "compact", "xml", "plain"] {
        let t = base(true);
        for args in [
            vec!["prime", "--all", "--format", format],
            vec!["prime", "--all", "--records-only", "--format", format],
            vec!["prime", "--manifest", "--format", format],
            vec![
                "prime",
                "--files",
                "src/main.rs",
                "other.rs",
                "--domain",
                "core",
                "other",
                "--format",
                format,
            ],
        ] {
            compare(&t, &args, true, 0, None);
        }
    }
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn prime_contract_and_reminder() {
    for format in ["markdown", "compact", "xml", "plain"] {
        let t = base(true);
        append_config(&t,"custom_types:\n  hypothesis:\n    required: [statement]\n    optional: [explanation]\n    dedup_key: statement\n    summary: '{statement}'\ndisabled_types: [guide]\nprime:\n  session_close:\n    style: minimal\n");
        t.prepare(|p| {
            let file = p.atlas_dir().join("config.yaml");
            let mut cfg = std::fs::read_to_string(&file).unwrap();
            cfg = cfg.replace(
                "core: {}",
                "core: {allowed_types: [convention, failure], required_fields: [owner]}",
            );
            std::fs::write(file, cfg).unwrap();
        });
        compare(&t, &["prime", "--all", "--format", format], true, 0, None);
        compare(
            &t,
            &["prime", "--manifest", "--format", format],
            true,
            0,
            None,
        );
    }
    for style in ["none", "directive", "conditional", "unrecognised"] {
        let t = base(true);
        append_config(
            &t,
            &format!("prime: {{session_close: {{style: {style}}}}}\n"),
        );
        compare(&t, &["prime", "--all"], true, 0, None);
    }
    let t = base(true);
    append_config(
        &t,
        "prime: {session_close: {custom: 'Custom reminder <&>'}}\n",
    );
    compare(&t, &["prime", "--all", "--format", "xml"], true, 0, None);
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn prime_active_branch_matchers() {
    for branch in [
        "a-abcd",
        "a-abcd_",
        "a-abcdef123",
        "UPPER-abcd",
        "thing/foo-bar-abcd",
        "gh-12#34",
        "égh-7",
        "_gh-7",
        "GH-7",
        "gh-007",
        "AB-123",
        "AB12-34",
        "A1-5",
        "BD-7-bd-7",
        "bead-123",
        "ébead-9",
        "x_bd-1",
    ] {
        let t = base(false);
        t.prepare(|p|{
            let mut out=String::from("{\"id\":\"mx-broad\",\"type\":\"convention\",\"content\":\"broad\",\"classification\":\"foundational\"}\n");
            let mut index=0;
            for(tracker,ids)in [("missions",vec!["a-abcd","bar-abcd"]),("gh",vec!["#12","#34","#7","#007"]),("linear",vec!["AB-123","AB12-34"]),("bead",vec!["bd-7","bead-123","bead-9","bd-1"])]{
                for id in ids{out.push_str(&format!("{{\"id\":\"mx-{index:04}\",\"type\":\"pattern\",\"name\":\"{tracker}-{id}\",\"description\":\"scoped\",\"classification\":\"foundational\",\"files\":[\"unrelated-{index}\"],\"evidence\":{{\"{tracker}\":\"{id}\"}}}}\n"));index+=1;}
            }
            p.write(".luuna/atlas/expertise/core.jsonl",&out);
            p.git(&["add","."]);p.git(&["commit","-qm","base"]);p.git(&["checkout","-qb",branch]);
        });
        compare(
            &t,
            &["prime", "--format", "plain", "--no-limit"],
            true,
            0,
            None,
        );
    }
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn prime_active_tracker_files_and_gh_stub() {
    for legacy in [false, true] {
        let t = base(false);
        t.prepare(|p|{
            p.write(".luuna/atlas/expertise/core.jsonl",concat!(
                "{\"id\":\"mx-aaaa\",\"type\":\"pattern\",\"name\":\"active\",\"description\":\"chosen\",\"files\":[\"no-file\"],\"evidence\":{\"seeds\":\"test-abcd\"}}\n",
                "{\"id\":\"mx-bbbb\",\"type\":\"pattern\",\"name\":\"inactive\",\"description\":\"other\",\"files\":[\"no-other\"],\"evidence\":{\"missions\":\"test-dcba\"}}\n",
                "{\"id\":\"mx-cccc\",\"type\":\"pattern\",\"name\":\"PR\",\"description\":\"stub\",\"files\":[\"no-pr\"],\"evidence\":{\"gh\":\"#42\"}}\n"
            ));
            p.write(if legacy{".seeds/issues.jsonl"}else{".luuna/missions/issues.jsonl"},"\u{a0}{\"id\":\"test-abcd\",\"status\":\"in_progress\"}\u{a0}\nnot-json\n");
        });
        commit_all(&t);
        t.prepare(|p| {
            let _ = vars(p, &[]);
            let gh = p.dir.parent().unwrap().join("prime-test-bin/gh");
            std::fs::write(gh, "#!/bin/sh\nprintf '42\\n'\n").unwrap();
        });
        compare(
            &t,
            &["prime", "--format", "plain", "--no-limit"],
            true,
            0,
            None,
        );
    }
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn git_learn_diff_sync_changed_records() {
    for human in [false, true] {
        let t = base(true);
        t.prepare(|p| p.write("src/main.rs", "base\n"));
        commit_all(&t);
        t.prepare(|p| {
            p.write("src/main.rs", "changed\n");
            p.git(&["add", "src/main.rs"]);
            p.write("unmatched.txt", "untracked\n");
            p.write(
                ".luuna/atlas/expertise/core.jsonl",
                &ROWS.replace("Prefer exact checks.", "Updated check."),
            );
        });
        for args in [
            &["learn", "--since", "HEAD"][..],
            &["diff", "--since", "HEAD"],
            &["sync", "--message", "fixture sync"],
            &["sync"],
            &["diff", "--since", "HEAD~1"],
        ] {
            compare(&t, args, human, 0, None);
        }
    }
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn git_sync_strict_validation() {
    for variant in ["unknown", "domain", "required", "malformed"] {
        let t = base(true);
        match variant {
            "unknown" => t.prepare(|p| {
                p.write(
                    ".luuna/atlas/expertise/core.jsonl",
                    "{\"id\":\"mx-aaaa\",\"type\":\"mystery\"}\n",
                )
            }),
            "domain" => t.prepare(|p| {
                let file = p.atlas_dir().join("config.yaml");
                let s = std::fs::read_to_string(&file)
                    .unwrap()
                    .replace("core: {}", "core: {allowed_types: [failure]}");
                std::fs::write(file, s).unwrap();
            }),
            "required" => t.prepare(|p| {
                let file = p.atlas_dir().join("config.yaml");
                let s = std::fs::read_to_string(&file)
                    .unwrap()
                    .replace("core: {}", "core: {required_fields: [owner]}");
                std::fs::write(file, s).unwrap();
            }),
            _ => t.prepare(|p| p.write(".luuna/atlas/expertise/core.jsonl", "{malformed}\n")),
        }
        compare(
            &t,
            &["sync", "--allow-unknown-types", "--allow-domain-mismatch"],
            false,
            1,
            None,
        );
        compare(&t, &["sync"], true, 1, None);
        compare(&t, &["sync", "--no-validate"], false, 0, None);
    }
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn git_worktree_uses_main_store_and_skips_commit() {
    let mut t = base(true);
    commit_all(&t);
    t.prepare(|p| {
        let linked = p.dir.parent().unwrap().join("linked");
        p.git(&["worktree", "add", "-b", "worker", linked.to_str().unwrap()]);
        p.write(
            ".luuna/atlas/expertise/core.jsonl",
            &ROWS.replace("Prefer exact checks.", "Main store changed."),
        );
    });
    let go_root = t.go.dir.clone();
    let rust_root = t.rust.dir.clone();
    t.go.dir = go_root.parent().unwrap().join("linked");
    t.rust.dir = rust_root.parent().unwrap().join("linked");
    for args in [&["sync"][..], &["prime", "--all"]] {
        compare(&t, args, false, 0, None);
    }
    t.go.dir = go_root;
    t.rust.dir = rust_root;
    t.assert_same_tree("shared main store untouched by worktree");
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn json_without_quiet_and_direct_quiet_output() {
    for command in ["prime", "learn", "diff", "sync"] {
        let t = base(true);
        let args = vec![command.into(), "--json".into()];
        let g = go(&t.go, &args, None, &[]);
        let r = native(&t.rust, &args, None);
        assert_eq!(g.exit_code, 0);
        assert_same_output(command, &t.go, &g, &t.rust, &r);
        t.assert_same_tree(command);
    }
    for command in ["prime", "learn", "diff"] {
        let t = base(true);
        compare(&t, &[command, "--quiet"], true, 0, None);
    }
}

#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn git_commands_without_repository_or_store() {
    let t = Twin::new(false);
    t.steps(&[Step::Init, Step::Add("core".into())]);
    for human in [false, true] {
        for command in ["learn", "diff", "sync"] {
            compare(&t, &[command], human, 1, None);
        }
        compare(&t, &["prime", "--context"], human, 1, None);
    }
    let t = Twin::new(true);
    compare(&t, &["learn"], false, 0, None);
    compare(&t, &["learn"], true, 0, None);
    compare(&t, &["sync"], false, 1, None);
    compare(&t, &["sync", "--no-validate"], false, 0, None);
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn prime_array_rows_unknowns_and_number_edges() {
    for human in [false, true] {
        let t = base(true);
        t.prepare(|p|p.write(".luuna/atlas/expertise/core.jsonl",&format!("[]\n[1,2]\n{ROWS}{{\"id\":\"mx-unknown\",\"type\":\"unknown-kind\",\"classification\":\"foundational\"}}\n")));
        for args in [
            &["prime", "--all", "--allow-unknown-types"][..],
            &["prime", "--manifest", "--allow-unknown-types"],
        ] {
            compare(&t, args, human, 0, None);
        }
    }
    for budget in [
        "0001",
        "9223372036854775807",
        "9223372036854775808",
        "-1",
        "+1",
        "1.5",
        "１",
    ] {
        let t = base(true);
        compare(
            &t,
            &["prime", "--all", "--budget", budget],
            false,
            if matches!(budget, "0001" | "9223372036854775807") {
                0
            } else {
                1
            },
            None,
        );
    }
}
#[cfg(unix)]
fn prime_hook(t: &Twin, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    t.prepare(|p| {
        let path = p.atlas_dir().join("prime-hook.sh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    });
    append_config(t, "hooks:\n  pre-prime: [.luuna/atlas/prime-hook.sh]\n");
}
#[cfg(unix)]
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn prime_hook_mutations_arrays_and_dry_run() {
    for human in [false, true] {
        let t = base(true);
        prime_hook(
            &t,
            r#"printf '%s\n' '{"domains":[{"domain":"core","records":[[],[1,2],{"id":"mx-hook","type":"convention","content":"hook supplied","classification":"foundational"}]}]}'"#,
        );
        compare(&t, &["prime", "--all"], human, 0, None);
        let t = base(true);
        prime_hook(&t, "printf blocked >&2; exit 7");
        compare(&t, &["prime", "--all"], human, 1, None);
        compare(&t, &["prime", "--all", "--dry-run"], human, 0, None);
        compare(&t, &["prime", "--manifest"], human, 0, None);
    }
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn prime_manifest_default_mode_and_empty_context() {
    let t = base(true);
    append_config(&t, "prime: {default_mode: manifest}\n");
    for human in [false, true] {
        for args in [
            &["prime"][..],
            &["prime", "--records-only"],
            &["prime", "core"],
            &["prime", "--full"],
        ] {
            compare(&t, args, human, 0, None);
        }
    }
    let t = base(true);
    commit_all(&t);
    for human in [false, true] {
        compare(&t, &["prime", "--context"], human, 0, None);
    }
}
#[cfg(unix)]
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn hook_session_expiry_and_raw_cache() {
    for symlink in [false, true] {
        let t = base(true);
        t.prepare(|p| {
            p.write("src/main.rs", "source\n");
            p.write(".luuna/atlas/state/sessions/old.txt", "old-id\n");
            let old = p.atlas_dir().join("state/sessions/old.txt");
            let time = std::time::SystemTime::now() - std::time::Duration::from_secs(9 * 24 * 3600);
            std::fs::File::open(old)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(time))
                .unwrap();
            if symlink {
                let target = p.atlas_dir().join("state/old-directory");
                std::fs::create_dir_all(&target).unwrap();
                std::fs::File::open(&target)
                    .unwrap()
                    .set_times(std::fs::FileTimes::new().set_modified(time))
                    .unwrap();
                std::os::unix::fs::symlink(
                    "../old-directory",
                    p.atlas_dir().join("state/sessions/old-link"),
                )
                .unwrap();
            }
        });
        let raw =
            r#"{"tool_name":"Read","tool_input":{"file_path":"src/main.rs"},"session_id":"new"}"#;
        compare(&t, &["hook"], false, 0, Some(raw.as_bytes()));
        assert!(!t.rust.atlas_dir().join("state/sessions/old.txt").exists());
    }
    let t = base(true);
    t.prepare(|p| {
        p.write("src/main.rs", "source\n");
        p.write(".luuna/atlas/state/sessions/one.txt", "mx-cccc\n");
        let path = p.atlas_dir().join("state/sessions/one.txt");
        let mut data = std::fs::read(&path).unwrap();
        data.extend_from_slice(b"\xff\n");
        std::fs::write(path, data).unwrap();
    });
    let raw = r#"{"tool_name":"Read","tool_input":{"file_path":"src/main.rs"},"session_id":"one"}"#;
    compare(&t, &["hook"], false, 0, Some(raw.as_bytes()));
    assert!(!t.rust.atlas_dir().join("state/usage.jsonl").exists());
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn hook_and_prime_pinned_failure_budgets() {
    let t = base(false);
    t.prepare(|p|{
        p.write("src/main.rs","source\n");let mut file=std::io::BufWriter::new(std::fs::File::create(p.atlas_dir().join("expertise/core.jsonl")).unwrap());
        for index in 0..40{
            let typ=if index<30{"failure"}else{"pattern"};
            writeln!(file,"{{\"id\":\"mx-{index:04}\",\"type\":\"{typ}\",\"description\":\"Repeated failure needs an exact preventive resolution in every relevant session.\",\"resolution\":\"Always preserve this failure even after the budget is consumed.\",\"name\":\"pattern-{index}\",\"classification\":\"foundational\",\"files\":[\"src/main.rs\"]}}").unwrap();
        }
    });
    compare(
        &t,
        &["prime", "--files", "src/main.rs", "--budget", "1"],
        true,
        0,
        None,
    );
    let raw =
        r#"{"tool_name":"Read","tool_input":{"file_path":"src/main.rs"},"session_id":"pinned"}"#;
    compare(&t, &["hook"], false, 0, Some(raw.as_bytes()));
    assert_eq!(
        std::fs::read_to_string(t.rust.atlas_dir().join("state/sessions/pinned.txt"))
            .unwrap()
            .lines()
            .count(),
        30
    );
}
