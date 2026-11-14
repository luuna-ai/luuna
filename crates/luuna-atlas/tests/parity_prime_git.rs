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
            } else if ["files","domain","exclude-domain"].contains(&flag) {
                while args.get(i+1).is_some_and(|v|!v.starts_with('-')) {
                    i+=1;flags.push((flag.into(),args[i].clone()));
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
    let env=Env::fixed(vars(p,&[]),args.to_vec(),false);
    let ctx = Context {
        root: luuna_atlas::git::resolve_store_root(&p.dir,&env),
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
