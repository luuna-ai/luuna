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
    // The product parser is plan5. Go's common format choices override the
    // advertised query ids choice before its handler; keep this parser case
    // distinct from the typed handler's still-present ids output branch.
    if flags
        .iter()
        .any(|(key, value)| key == "format" && value == "ids")
    {
        return CommandOutput::plain_failure("error: option '--format <format>' argument 'ids' is invalid. Allowed choices are markdown, compact, xml, plain.");
    }
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
        let yaml = if extra.starts_with("domains:") {
            format!("version: '1'\n{extra}")
        } else {
            format!("version: '1'\ndomains: {{core: {{}}, other: {{}}}}\n{extra}")
        };
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
        "governance: {!!binary /w==: one, !!binary gA==: two}\n",
        "classification_defaults: {!!binary /w==: one, shelf_life: {!!binary gA==: two}}\n",
        "domains: [!!binary /w==]\n",
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
        let yaml = if extra.starts_with("domains:") {
            format!("version: '1'\n{extra}")
        } else {
            format!("version: '1'\ndomains: {{core: {{}}, other: {{}}}}\n{extra}")
        };
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

#[test]
#[ignore = "hostile YAML: compile first, run under prlimit4GiB/timeout120"]
fn hostile_yaml_graph_depth_limits() {
    for flow in [true, false] {
        for depth in [9999, 10000, 10001] {
            let t = base(false);
            t.prepare(|p| {
                let mut file = std::io::BufWriter::new(
                    std::fs::File::create(p.atlas_dir().join("config.yaml")).unwrap(),
                );
                file.write_all(if flow {
                    b"extra: &anchor "
                } else {
                    b"extra: &anchor\n  "
                })
                .unwrap();
                for _ in 0..depth {
                    file.write_all(if flow { b"[" } else { b"- " }).unwrap();
                }
                file.write_all(b"0").unwrap();
                if flow {
                    for _ in 0..depth {
                        file.write_all(b"]").unwrap();
                    }
                }
                file.write_all(b"\n").unwrap();
                file.flush().unwrap();
            });
            let args = vec![
                "config".into(),
                "show".into(),
                "--path".into(),
                "governance.max_entries".into(),
            ];
            let g = go(&t.go, &args, None, &[]);
            let expected = if depth > if flow { 10000 } else { 9999 } {
                1
            } else {
                0
            };
            assert_eq!(
                g.exit_code, expected,
                "flow={flow}, depth={depth}: {}",
                g.stderr
            );
            let r = match luuna_atlas::config::read_config(&t.rust.atlas_dir()) {
                Ok(cfg) => CommandOutput::json(
                    luuna_atlas::records::obj(cfg.get("governance"))
                        .get("max_entries")
                        .unwrap(),
                ),
                Err(error) => CommandOutput::plain_failure(&error.to_string()),
            };
            assert_same_output("graph depth", &t.go, &g, &t.rust, &r);
            t.assert_same_tree("graph depth");
        }
    }
}

#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn yaml_a8_add_refuses_dropped_config_data() {
    for (yaml, lost) in [
        ("[held-root-payload]\n", "held-root-payload"),
        ("version: '1'\ndomains: [core, 31]\n", "31"),
        (
            "version: '1'\ndomains: {}\ngovernance: [held-governance]\n",
            "held-governance",
        ),
        (
            "version: '1'\ndomains: {}\nclassification_defaults: held-classification\n",
            "held-classification",
        ),
        (
            "version: '1'\ndomains: {}\nclassification_defaults: {shelf_life: held-shelf}\n",
            "held-shelf",
        ),
        (
            "version: '1'\ndomains: {}\n---\nheld-document: 17\n",
            "held-document",
        ),
    ] {
        let t = base(false);
        t.prepare(|p| p.write(".luuna/atlas/config.yaml", yaml));
        let before = t.rust.snapshot();
        let args = vec![
            "add".into(),
            "third".into(),
            "--json".into(),
            "--quiet".into(),
        ];
        let g = go(&t.go, &args, None, &[]);
        assert_eq!(g.exit_code, 0, "{yaml}: {}", g.stderr);
        assert!(
            !std::fs::read_to_string(t.go.atlas_dir().join("config.yaml"))
                .unwrap()
                .contains(lost),
            "Go must actually lose {lost}"
        );
        let r = luuna_atlas::commands::run_add(&t.rust.dir, "third");
        assert_eq!(r.exit_code, 1);
        assert!(r.stderr.contains("Refusing"), "{}", r.stderr);
        assert_eq!(
            before,
            t.rust.snapshot(),
            "refusal leaves every file/mode unchanged"
        );
    }
}
fn go_config_number_markers(p: &Project) -> String {
    let input = format!(
        "{{\"operation\":\"readConfig\",\"args\":[{}]}}",
        luuna_atlas::json::to_compact_string(&luuna_atlas::json::Value::from(
            p.dir.to_string_lossy().into_owned()
        ))
        .unwrap()
    );
    let g = go(
        p,
        &[],
        Some(input.as_bytes()),
        &[("LUUNA_ATLAS_API_BRIDGE", "1".into())],
    );
    assert_eq!(g.exit_code, 0);
    let value = luuna_atlas::json::parse(&g.stdout).unwrap();
    luuna_atlas::json::to_compact_string(value.as_object().unwrap().get("numbers").unwrap())
        .unwrap()
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn yaml_a8_retained_negative_zero_refused() {
    let t = base(false);
    t.prepare(|p| {
        p.write(
            ".luuna/atlas/config.yaml",
            "version: '1'\ndomains: {}\nsearch: {boost_factor: -0.0}\n",
        )
    });
    let before = t.rust.snapshot();
    let markers = go_config_number_markers(&t.go);
    assert!(markers.contains("-0"));
    let args = vec![
        "add".into(),
        "third".into(),
        "--json".into(),
        "--quiet".into(),
    ];
    let g = go(&t.go, &args, None, &[]);
    assert_eq!(g.exit_code, 0);
    assert_eq!(go_config_number_markers(&t.go), "[]");
    let r = luuna_atlas::commands::run_add(&t.rust.dir, "third");
    assert_eq!(r.exit_code, 1);
    assert!(r.stderr.contains("Refusing"), "{}", r.stderr);
    assert_eq!(before, t.rust.snapshot());
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn lifecycle_config_explicit_replacement_controls() {
    for yaml in [
        "version: '1'\ndomains: {}\nsearch: {boost_factor: -0.0}\n",
        "version: '1'\ndomains: {}\nprime: held-parent\n",
    ] {
        let t = base(false);
        t.prepare(|p| p.write(".luuna/atlas/config.yaml", yaml));
        if yaml.contains("search:") {
            compare(
                &t,
                &["config", "set", "search.boost_factor", "0"],
                false,
                0,
                None,
            );
        } else {
            compare(
                &t,
                &["config", "set", "prime", "{default_mode: full}"],
                false,
                0,
                None,
            );
        }
    }
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn lifecycle_config_implicit_parent_loss_refused() {
    let t = base(false);
    t.prepare(|p| {
        p.write(
            ".luuna/atlas/config.yaml",
            "version: '1'\ndomains: {}\nprime: held-parent\n",
        )
    });
    let before = t.rust.snapshot();
    let args = vec![
        "config".into(),
        "set".into(),
        "prime.default_mode".into(),
        "full".into(),
    ];
    let g = go(&t.go, &args, None, &[]);
    assert_eq!(g.exit_code, 0);
    assert!(
        !std::fs::read_to_string(t.go.atlas_dir().join("config.yaml"))
            .unwrap()
            .contains("held-parent")
    );
    let r = native(&t.rust, &args, None);
    assert_eq!(r.exit_code, 1);
    assert!(r.stderr.contains("Refusing"), "{}", r.stderr);
    assert_eq!(before, t.rust.snapshot());
}

#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn lifecycle_config_explicit_default_subtree_replacement() {
    let t = base(false);
    t.prepare(|p| {
        p.write(
            ".luuna/atlas/config.yaml",
            "version: '1'\ndomains: {}\nclassification_defaults: {shelf_life: held-shelf}\n",
        )
    });
    compare(
        &t,
        &[
            "config",
            "set",
            "classification_defaults",
            "{shelf_life: {tactical: 2, observational: 3}}",
        ],
        false,
        0,
        None,
    );
}

#[test]
#[cfg(unix)]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn lifecycle_config_symlink_write_contracts() {
    for atomic in [false, true] {
        let t = base(false);
        t.prepare(|p| {
            let file = p.atlas_dir().join("config.yaml");
            std::fs::rename(&file, p.atlas_dir().join("config-source.yaml")).unwrap();
            std::os::unix::fs::symlink("config-source.yaml", file).unwrap();
        });
        if atomic {
            compare(
                &t,
                &["config", "set", "governance.max_entries", "120"],
                false,
                0,
                None,
            );
            assert!(!t.rust.atlas_dir().join("config.yaml").is_symlink());
        } else {
            let args = vec![
                "add".into(),
                "third".into(),
                "--json".into(),
                "--quiet".into(),
            ];
            let g = go(&t.go, &args, None, &[]);
            assert_eq!(g.exit_code, 0);
            let r = luuna_atlas::commands::run_add(&t.rust.dir, "third");
            assert_same_output("truncate follows link", &t.go, &g, &t.rust, &r);
            t.assert_same_tree("truncate follows link");
            assert!(t.rust.atlas_dir().join("config.yaml").is_symlink());
        }
    }
}
#[test]
#[cfg(unix)]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn lifecycle_config_refusal_keeps_link_and_target() {
    let t = base(false);
    t.prepare(|p| {
        let file = p.atlas_dir().join("config.yaml");
        std::fs::write(&file, "version: '1'\ndomains: {}\n---\nheld-document: 17\n").unwrap();
        std::fs::rename(&file, p.atlas_dir().join("config-source.yaml")).unwrap();
        std::os::unix::fs::symlink("config-source.yaml", file).unwrap();
    });
    let before = t.rust.snapshot();
    let args = vec![
        "config".into(),
        "set".into(),
        "governance.max_entries".into(),
        "120".into(),
    ];
    let g = go(&t.go, &args, None, &[]);
    assert_eq!(g.exit_code, 0);
    assert!(
        !std::fs::read_to_string(t.go.atlas_dir().join("config.yaml"))
            .unwrap()
            .contains("held-document")
    );
    let r = native(&t.rust, &args, None);
    assert_eq!(r.exit_code, 1);
    assert!(r.stderr.contains("Refusing"), "{}", r.stderr);
    assert_eq!(before, t.rust.snapshot());
}

#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn lifecycle_query_array_rows_preserved() {
    use luuna_atlas::{
        json::{self, Value},
        registry::Registry,
        store,
    };
    let t = base(false);
    t.prepare(|p| {
        p.write(
            ".luuna/atlas/expertise/core.jsonl",
            &format!("[]\n[1,2]\n[{{\"nested\":true}}]\n{ROWS}"),
        )
    });
    let args = vec![
        "query".into(),
        "core".into(),
        "--json".into(),
        "--quiet".into(),
    ];
    let g = go(&t.go, &args, None, &[]);
    assert_eq!(g.exit_code, 0);
    let output = json::parse(&g.stdout).unwrap();
    let rows = output
        .as_object()
        .unwrap()
        .get("domains")
        .unwrap()
        .as_array()
        .unwrap()[0]
        .as_object()
        .unwrap()
        .get("records")
        .unwrap();
    let native = store::read_records(
        &t.rust.expertise("core"),
        &Registry::from_config(None).unwrap(),
        false,
    )
    .unwrap();
    let native = Value::Array(native.into_iter().map(Value::Object).collect());
    assert_eq!(
        json::to_compact_string(&native).unwrap(),
        json::to_compact_string(rows).unwrap()
    );
    t.assert_same_tree("raw array read");
}

#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn lifecycle_ready_unicode_duration_errors() {
    for since in ["1é", "é", "1🙂", "👩‍💻", "1 h"] {
        let t = base(true);
        compare(&t, &["ready", "--since", since], false, 1, None);
    }
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn lifecycle_read_quiet_is_ignored_and_ids_work() {
    let t = base(true);
    for args in [
        &["query", "core", "--quiet"][..],
        &["rank", "--quiet"],
        &["ready", "--quiet"],
        &["status", "--quiet"],
        &["validate", "--quiet"],
    ] {
        compare(&t, args, true, 0, None);
    }
    compare(&t, &["query", "core", "--format", "ids"], true, 1, None);
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn lifecycle_read_empty_domains() {
    let t = base(false);
    compare(&t, &["config", "unset", "domains.core"], false, 0, None);
    compare(&t, &["config", "unset", "domains.other"], false, 0, None);
    for args in [
        &["query", "--all"][..],
        &["rank"],
        &["ready"],
        &["status"],
        &["validate"],
    ] {
        compare(&t, args, false, 0, None);
        compare(&t, args, true, 0, None);
    }
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn lifecycle_query_rank_ready_raw_array_output() {
    let t = base(false);
    t.prepare(|p| {
        p.write(
            ".luuna/atlas/expertise/core.jsonl",
            &format!("[]\n[{{\"nested\":true}}]\n{ROWS}"),
        )
    });
    for args in [&["query", "core"][..], &["rank"], &["ready"]] {
        compare(&t, args, false, 0, None);
    }
    for format in ["compact", "plain", "xml"] {
        compare(&t, &["query", "core", "--format", format], true, 0, None);
    }
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn lifecycle_validate_errors_warnings_and_escape_hatches() {
    let t = base(true);
    t.prepare(|p|p.write(".luuna/atlas/expertise/core.jsonl",&format!("{ROWS}# comment is a validation error\n[]\n{{\"id\":\"mx-dddd\",\"type\":\"mystery\"}}\n{{\"id\":\"mx-eeee\",\"type\":\"convention\",\"outcome\":null}}\n")));
    for args in [&["validate"][..], &["validate", "--allow-unknown-types"]] {
        compare(&t, args, false, 1, None);
        compare(&t, args, true, 1, None);
    }
    let t = base(true);
    t.prepare(|p|p.write(".luuna/atlas/config.yaml","version: '1'\ndomains:\n  core: {allowed_types: [failure], required_fields: [tags]}\n  other: {}\n"));
    compare(&t, &["validate"], false, 1, None);
    compare(&t, &["validate"], true, 1, None);
    compare(&t, &["validate", "--allow-domain-mismatch"], false, 0, None);
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn lifecycle_validate_custom_aliases() {
    let t = base(false);
    t.prepare(|p|{p.write(".luuna/atlas/config.yaml","version: '1'\ndomains: {core: {}, other: {}}\ncustom_types:\n  hypothesis:\n    required: [statement]\n    dedup_key: statement\n    summary: '{statement}'\n    aliases: {statement: [claim]}\n");p.write(".luuna/atlas/expertise/core.jsonl","{\"id\":\"mx-dddd\",\"type\":\"hypothesis\",\"claim\":\"Original field\",\"classification\":\"tactical\",\"recorded_at\":\"2026-01-01T00:00:00.000Z\"}\n");});
    compare(&t, &["validate"], false, 0, None);
}

#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn lifecycle_read_binary_domain_errors() {
    for yaml in [
        "version: '1'\ndomains: {!!binary /w==: {}, !!binary 4oI=: {}}\n",
        "version: '1'\ndomains: [!!binary /w==]\n",
    ] {
        let t = base(false);
        t.prepare(|p| p.write(".luuna/atlas/config.yaml", yaml));
        for args in [
            &["query", "--all"][..],
            &["query", "missing"],
            &["rank"],
            &["ready"],
            &["ready", "--domain", "missing"],
            &["status"],
            &["validate"],
        ] {
            compare(&t, args, false, 1, None);
        }
    }
}
#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn lifecycle_config_binary_registry_errors() {
    for yaml in [
        "version: '1'\ndomains: {}\ncustom_types: {!!binary /w==: {required: [statement], summary: '{statement}'}}\n",
        "version: '1'\ndomains: {}\ndisabled_types: ['�']\ncustom_types:\n  note:\n    extends: !!binary /w==\n",
        "version: '1'\ndomains: {}\ncustom_types:\n  note:\n    required: [statement]\n    summary: '{statement}'\n    aliases: {!!binary /w==: [old]}\n",
    ]{let t=base(false);t.prepare(|p|p.write(".luuna/atlas/config.yaml",yaml));compare(&t,&["config","show"],false,1,None);}
}
#[test]
#[ignore = "hostile registry text: compile first, run under prlimit4GiB/timeout120"]
fn hostile_yaml_registry_deep_text() {
    let t = base(false);
    t.prepare(|p| {
        let mut file = std::io::BufWriter::new(
            std::fs::File::create(p.atlas_dir().join("config.yaml")).unwrap(),
        );
        file.write_all(b"version: '1'\ndomains: {}\ncustom_types:\n  note:\n    extends: &deep ")
            .unwrap();
        for _ in 0..9999 {
            file.write_all(b"[").unwrap();
        }
        file.write_all(b"convention").unwrap();
        for _ in 0..9999 {
            file.write_all(b"]").unwrap();
        }
        file.write_all(b"\n").unwrap();
        file.flush().unwrap();
    });
    compare(&t, &["config", "show"], false, 1, None);
}

#[test]
#[ignore = "needs the pinned Go reference; run -- --ignored"]
fn lifecycle_read_date_and_number_boundaries() {
    let t = base(false);
    t.prepare(|p|p.write(".luuna/atlas/expertise/core.jsonl","{\"id\":\"mx-aaaa\",\"type\":\"convention\",\"content\":\"Invalid date\",\"classification\":\"tactical\",\"recorded_at\":\"not-a-date\"}\n{\"id\":\"mx-bbbb\",\"type\":\"convention\",\"content\":\"Future date\",\"classification\":\"tactical\",\"recorded_at\":\"2099-01-01T00:00:00Z\"}\n"));
    for args in [&["status"][..],&["ready"],&["ready","--since","9999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999w"]]{compare(&t,args,false,0,None);}
    for yaml in ["version: '1'\ndomains: {core: {}, other: {}}\ngovernance: {max_entries: 18446744073709551615}\n","version: '1'\ndomains: {core: {}, other: {}}\ngovernance: {max_entries: 1e-300}\n","version: '1'\ndomains: {core: {}, other: {}}\nclassification_defaults: {shelf_life: {observational: .nan}}\n"]{t.prepare(|p|p.write(".luuna/atlas/config.yaml",yaml));compare(&t,&["status"],false,0,None);}
}
#[test]
#[ignore = "hostile raw array: compile first, run under prlimit4GiB/timeout120"]
fn hostile_raw_record_array_sidecar() {
    use luuna_atlas::{json::Value, registry::Registry, store};
    let t = base(false);
    t.prepare(|p| {
        let mut file = std::io::BufWriter::new(std::fs::File::create(p.expertise("core")).unwrap());
        for _ in 0..10000 {
            file.write_all(b"[").unwrap();
        }
        file.write_all(b"0").unwrap();
        for _ in 0..10000 {
            file.write_all(b"]").unwrap();
        }
        file.write_all(b"\n").unwrap();
        file.flush().unwrap();
    });
    let args = vec![
        "query".into(),
        "core".into(),
        "--json".into(),
        "--quiet".into(),
    ];
    let g = go(&t.go, &args, None, &[]);
    assert_eq!(g.exit_code, 0);
    assert_eq!(g.stdout, "\n");
    let rows = store::read_records(
        &t.rust.expertise("core"),
        &Registry::from_config(None).unwrap(),
        false,
    )
    .unwrap();
    let cloned = rows.clone();
    let result = CommandOutput::json(&Value::Array(
        cloned.into_iter().map(Value::Object).collect(),
    ));
    assert_eq!(result.stdout, "\n");
    t.assert_same_tree("deep array sidecar");
}
