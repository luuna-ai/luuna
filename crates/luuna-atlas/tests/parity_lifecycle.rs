//! Isolated Go comparisons for YAML and lifecycle/config operations.
mod support;
use luuna_atlas::{
    lifecycle,
    options::{Context, Env, GlobalOptions},
    CommandOutput,
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
                "archived",
                "sort-by-score",
                "full",
                "allow-unknown-types",
                "allow-domain-mismatch",
            ]
            .contains(&flag)
            {
                flags.push((flag.into(), "true".into()));
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
    let ctx = Context {
        root: p.dir.clone(),
        cwd: p.dir.clone(),
        env: Env::fixed(vars(p, &[]), args.to_vec(), false),
        globals,
    };
    lifecycle::run(
        &ctx,
        &args[0],
        &pos,
        &lifecycle::Options {
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
    lifecycle_query_json,
    true,
    false,
    0,
    &["query", "core"][..],
    &["query", "--all"]
);
case!(
    lifecycle_query_filters,
    true,
    false,
    0,
    &[
        "query",
        "core",
        "--type",
        "convention",
        "--classification",
        "tactical",
        "--outcome-status",
        "success",
        "--sort-by-score"
    ][..],
    &["query", "core", "--file", "MAIN"]
);
case!(
    lifecycle_query_errors,
    true,
    false,
    1,
    &["query"][..],
    &["query", "missing"]
);
case!(
    lifecycle_query_human,
    true,
    true,
    0,
    &["query", "--all", "--format", "markdown"][..],
    &["query", "core", "--format", "plain"],
    &["query", "core", "--format", "compact"],
    &["query", "core", "--format", "xml"]
);
case!(
    lifecycle_rank_json,
    true,
    false,
    0,
    &["rank"][..],
    &[
        "rank",
        "core",
        "--limit",
        "2",
        "--min-score",
        "0",
        "--type",
        "convention"
    ]
);
case!(
    lifecycle_rank_human,
    true,
    true,
    0,
    &["rank"][..],
    &["rank", "--min-score", "100"]
);
case!(
    lifecycle_rank_errors,
    true,
    false,
    1,
    &["rank", "--limit", "0"][..],
    &["rank", "--min-score", "-1"],
    &["rank", "missing"]
);
case!(
    lifecycle_ready_json,
    true,
    false,
    0,
    &["ready"][..],
    &["ready", "--domain", "core", "--limit", "1"],
    &["ready", "--since", "1h"]
);
case!(
    lifecycle_ready_human,
    true,
    true,
    0,
    &["ready"][..],
    &["ready", "--since", "1h"]
);
case!(
    lifecycle_ready_errors,
    true,
    false,
    1,
    &["ready", "--limit", "0"][..],
    &["ready", "--since", "bad"],
    &["ready", "--domain", "missing"]
);
case!(lifecycle_status_json, true, false, 0, &["status"][..]);
case!(lifecycle_status_human, true, true, 0, &["status"][..]);
case!(lifecycle_validate_json, true, false, 0, &["validate"][..]);
case!(lifecycle_validate_human, true, true, 0, &["validate"][..]);
case!(
    lifecycle_config_schema,
    true,
    false,
    0,
    &["config", "schema"][..]
);
case!(
    lifecycle_config_show_path,
    true,
    false,
    0,
    &["config", "show", "--path", "domains.core"][..],
    &["config", "show", "--path", "search.boost_factor"],
    &["config", "show", "--path", "governance"]
);
case!(
    lifecycle_config_path_errors,
    true,
    false,
    1,
    &["config", "show", "--path", "..."][..],
    &["config", "set", "unknown.x", "1"],
    &["config", "set", "governance.max_entries", "0"],
    &["config", "unset", "governance.max_entries"]
);
case!(
    lifecycle_config_set_unset,
    true,
    false,
    0,
    &["config", "set", "governance.max_entries", "120"][..],
    &["config", "set", "search.boost_factor", "0.2"],
    &["config", "unset", "search"],
    &["config", "unset", "prime"]
);
case!(
    lifecycle_outcome_json,
    true,
    false,
    0,
    &[
        "outcome",
        "core",
        "mx-aaaa",
        "--status",
        "success",
        "--duration",
        "1.5",
        "--agent",
        "fixture",
        "--test-results",
        "passed"
    ][..]
);
case!(
    lifecycle_outcome_human,
    true,
    true,
    0,
    &["outcome", "core", "mx-aaaa", "--status", "partial"][..]
);
case!(
    lifecycle_outcome_errors,
    true,
    false,
    1,
    &["outcome", "core", "missing", "--status", "success"][..],
    &[
        "outcome",
        "core",
        "mx-aaaa",
        "--status",
        "success",
        "--duration",
        "-2"
    ]
);
case!(
    lifecycle_move_json,
    true,
    false,
    0,
    &["move", "core", "mx-aaaa", "other"][..]
);
case!(
    lifecycle_move_human,
    true,
    true,
    0,
    &["move", "core", "mx-aaaa", "other"][..]
);
case!(
    lifecycle_move_errors,
    true,
    false,
    1,
    &["move", "core", "mx-aaaa", "core"][..],
    &["move", "missing", "mx-aaaa", "other"]
);
case!(
    lifecycle_archive_restore,
    true,
    false,
    0,
    &["archive", "core", "mx-aaaa", "--reason", "keep history"][..],
    &["restore", "mx-aaaa"]
);
case!(
    lifecycle_archive_many_dry,
    true,
    false,
    0,
    &[
        "archive",
        "core",
        "--records",
        "mx-aaaa,mx-bbbb",
        "--reason",
        "probe",
        "--dry-run"
    ][..]
);
case!(
    lifecycle_archive_restore_human,
    true,
    true,
    0,
    &["archive", "core", "mx-aaaa", "--reason", "keep history"][..],
    &["restore", "mx-aaaa"]
);
case!(
    lifecycle_archive_errors,
    true,
    false,
    1,
    &["archive", "core", "mx-aaaa", "--reason", " "][..],
    &["archive", "core", "--reason", "probe"],
    &[
        "archive",
        "core",
        "mx-aaaa",
        "--records",
        "mx-bbbb",
        "--reason",
        "probe"
    ]
);
case!(
    lifecycle_restore_missing,
    true,
    false,
    1,
    &["restore", "missing"][..]
);
case!(
    lifecycle_delete_domain_json,
    true,
    false,
    0,
    &["delete-domain", "core", "--dry-run"][..],
    &["delete-domain", "core", "--yes"]
);
case!(
    lifecycle_delete_domain_human,
    true,
    true,
    0,
    &["delete-domain", "core", "--dry-run"][..],
    &["delete-domain", "core", "--yes"]
);
case!(
    lifecycle_stale_json,
    true,
    false,
    0,
    &["stale"][..],
    &["stale", "core"]
);
case!(lifecycle_stale_human, true, true, 0, &["stale"][..]);
case!(
    lifecycle_prune_json,
    true,
    false,
    0,
    &["prune", "--dry-run"][..]
);
case!(
    lifecycle_prune_human,
    true,
    true,
    0,
    &["prune", "--dry-run"][..]
);
case!(
    lifecycle_compact_json,
    true,
    false,
    0,
    &["compact", "core", "--analyze"][..]
);
case!(
    lifecycle_compact_human,
    true,
    true,
    0,
    &["compact", "core", "--analyze"][..]
);
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn yaml_full_reader_forms() {
    for extra in [
        "extra: &x {k: 1}\nother: *x\n",
        "extra: {before: 9, <<: &x {before: 1, inside: 2}, after: 3}\nother: {<<: *x, before: 8}\n",
        "extra: !foo bar\n",
        "extra: !!str 1\n",
        "extra: !!binary aGVsbG8=\n",
        "extra: !!binary /w==\n",
        "extra:\n  ? [a, b]\n  : 1\n",
        "extra: {[a, b]: 1, {x: 2}: 3}\n",
        "extra: [1,\n  2]\n",
        "---\nsecond: ignored\n",
    ] {
        let t = base(false);
        let yaml = format!("version: '1'\ndomains: {{core: {{}}, other: {{}}}}\n{extra}");
        t.prepare(|p| p.write(".luuna/atlas/config.yaml", &yaml));
        let args = vec!["config".into(), "show".into()];
        let g = go(&t.go, &args, None, &[]);
        assert_eq!(g.exit_code, 0, "{extra}: {}", g.stderr);
        let r = luuna_atlas::commands::run_config_show(&t.rust.dir);
        assert_same_output(extra, &t.go, &g, &t.rust, &r);
        t.assert_same_tree(extra);
    }
}

#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn yaml_full_roundtrip_forms() {
    for extra in [
        "extra: &x {k: 1}\nother: *x\n",
        "extra: {before: 9, <<: &x {before: 1, inside: 2}, after: 3}\nother: {<<: *x, before: 8}\n",
        "extra: !foo bar\n",
        "extra: !!str 1\n",
        "extra: !!binary aGVsbG8=\n",
        "extra: !!binary /w==\n",
        "extra: {!!binary /w==: one, !!binary gA==: two, '�': unicode}\n",
        "extra:\n  ? [a, b]\n  : 1\n",
        "extra: {[a, b]: 1, {x: 2}: 3}\n",
        "extra: [1,\n  2]\n",
    ] {
        let t = base(false);
        let yaml = format!("version: '1'\ndomains: {{core: {{}}, other: {{}}}}\n{extra}");
        t.prepare(|p| p.write(".luuna/atlas/config.yaml", &yaml));
        let args = vec!["config".into(), "show".into()];
        let before = go(&t.go, &args, None, &[]);
        assert_eq!(before.exit_code, 0, "{extra}: {}", before.stderr);
        let add = vec![
            "add".into(),
            "third".into(),
            "--json".into(),
            "--quiet".into(),
        ];
        let g = go(&t.go, &add, None, &[]);
        assert_eq!(g.exit_code, 0, "{extra}: {}", g.stderr);
        let r = luuna_atlas::commands::run_add(&t.rust.dir, "third");
        assert_same_output(extra, &t.go, &g, &t.rust, &r);
        t.assert_same_tree(extra);
        let after = go(&t.go, &args, None, &[]);
        let native = luuna_atlas::commands::run_config_show(&t.rust.dir);
        assert_same_output(extra, &t.go, &after, &t.rust, &native);
    }
}

#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn yaml_full_tag_merge_document_matrix() {
    for yaml in [
        "extra: [!!str true, !!str null, !!str 12, !!int '0x10', !!float '17', !!float 0x10, !!bool true, !!bool yes, !!null nope, !!null null]\n",
        "extra: [!!int '1.0', !!float '.inf', !!float '-.inf', !!float '.nan', !!timestamp 2001-01-02, !!timestamp nonsense, !thing 12, ! '12', !!binary '%%%']\n",
        "extra: &a {k: 1, old: first}\nother: &b {k: 2, later: second}\nmerged: {pre: 0, <<: [*a, *b], k: 3, post: 4}\n",
        "extra: {'<<': 12}\nother: {!!str <<: 13}\n",
        "%YAML 1.1\n---\nextra: 12\n",
        "%TAG !e! tag:example.invalid,2026:\n---\nextra: !e!thing 12\n",
        "extra: 1\n---\nlater: [broken\n",
        "extra: 1\n...\n%YAML 1.2\n---\nignored: true\n",
        "extra: !!binary 4oI=\n",
        "extra: &a {nested: &b [1, 2]}\ncopy: *a\nsequence: [*b, *b]\n",
        "extra: !!map {a: 1}\nother: !custom [1, 2]\n",
    ]{
        let t=base(false);t.prepare(|p|p.write(".luuna/atlas/config.yaml",yaml));
        let args=vec!["config".into(),"show".into()];let g=go(&t.go,&args,None,&[]);assert_eq!(g.exit_code,0,"{yaml}: {}",g.stderr);
        let r=luuna_atlas::commands::run_config_show(&t.rust.dir);assert_same_output(yaml,&t.go,&g,&t.rust,&r);t.assert_same_tree(yaml);
    }
}

#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn yaml_full_repeated_and_unknown_errors() {
    for yaml in [
        "extra: {'<<': 12, !!str <<: 13}\n",
        "extra: &a {k: 1, k: 2}\ncopy: *a\n",
        "extra: *missing\n",
        "extra: {[a, b]: 1, 'a,b': 2}\n",
    ] {
        let t = base(false);
        t.prepare(|p| p.write(".luuna/atlas/config.yaml", yaml));
        let args = vec!["config".into(), "show".into()];
        let g = go(&t.go, &args, None, &[]);
        assert_eq!(g.exit_code, 1, "{yaml}");
        let r = luuna_atlas::commands::run_config_show(&t.rust.dir);
        assert_same_output(yaml, &t.go, &g, &t.rust, &r);
        t.assert_same_tree(yaml);
    }
}
