//! Go-backed action parity. Each project and child environment is isolated.
mod support;
use luuna_atlas::{
    actions::{bulk, delete, edit, record, search, show},
    options::{Context, Env, GlobalOptions},
    CommandOutput,
};
use std::{
    ffi::OsString,
    io::Write,
    process::{Command, Stdio},
};
use support::project::{assert_same_output, Project, Step, Twin};
const ROWS: &str = concat!(
    "{\"id\":\"mx-aaaa\",\"type\":\"convention\",\"content\":\"Prefer exact checks.\",\"recorded_at\":\"2026-01-01T00:00:00.000Z\",\"tags\":[\"rust\"],\"classification\":\"tactical\",\"outcomes\":[{\"status\":\"success\"}]}\n",
    "{\"id\":\"mx-bbbb\",\"type\":\"failure\",\"description\":\"Flaky check\",\"resolution\":\"Use deterministic checks.\",\"recorded_at\":\"2026-01-01T00:00:00.000Z\",\"outcomes\":[{\"status\":\"failure\"}]}\n",
    "{\"id\":\"mx-cccc\",\"type\":\"pattern\",\"name\":\"Atomic write\",\"description\":\"Use locks for exact checks.\",\"files\":[\"src/main.rs\"],\"recorded_at\":\"2026-01-01T00:00:00.000Z\"}\n"
);
fn base(seed: bool) -> Twin {
    let t = Twin::new(true);
    t.steps(&[Step::Init, Step::Add("core".into())]);
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
fn native(
    p: &Project,
    args: &[String],
    input: Option<&[u8]>,
    extra: &[(&str, String)],
) -> CommandOutput {
    let (pos, flags) = parsed(args);
    let mut globals = GlobalOptions::prescan(args);
    for (k, v) in &flags {
        globals.set(k, std::slice::from_ref(v));
    }
    let ctx = Context {
        root: p.dir.clone(),
        cwd: p.dir.clone(),
        env: Env::fixed(vars(p, extra), args.to_vec(), false),
        globals,
    };
    let get = |name: &str| {
        flags
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    match args[0].as_str() {
        "record" => {
            let opts = record::RecordOptions {
                content: pos.get(1).cloned(),
                flags: flags.clone(),
            };
            if let Some(file) = get("batch") {
                bulk::run_record_bulk(&ctx, &pos[0], bulk::BulkInput::Batch(file.into()), &opts)
            } else if get("stdin").is_some_and(|s| s != "false") {
                bulk::run_record_bulk(
                    &ctx,
                    &pos[0],
                    bulk::BulkInput::Stdin(input.unwrap_or_default().to_vec()),
                    &opts,
                )
            } else {
                record::run_record(&ctx, &pos[0], &opts)
            }
        }
        "edit" => edit::run_edit(&ctx, &pos[0], &pos[1], &edit::EditOptions { flags }),
        "delete" => delete::run_delete(
            &ctx,
            &pos[0],
            pos.get(1).map(String::as_str),
            &delete::DeleteOptions {
                records: get("records"),
                all_except: get("all-except"),
                dry_run: get("dry-run").is_some_and(|s| s != "false"),
            },
        ),
        "search" => search::run_search(
            &ctx,
            pos.first().map(String::as_str),
            &search::SearchOptions { flags },
        ),
        "show" => show::run_show(&ctx, &pos),
        other => panic!("test runner has no command {other}"),
    }
}
fn compare(t: &Twin, args: &[&str]) {
    compare_mode(t, args, None, &[], false);
}
fn compare_mode(
    t: &Twin,
    args: &[&str],
    input: Option<&[u8]>,
    extra: &[(&str, String)],
    human: bool,
) {
    let mut args: Vec<String> = args.iter().map(|s| (*s).into()).collect();
    if !human {
        args.extend(["--json".into(), "--quiet".into()]);
    }
    let g = go(&t.go, &args, input, extra);
    let r = native(&t.rust, &args, input, extra);
    assert_same_output(&args.join(" "), &t.go, &g, &t.rust, &r);
    t.assert_same_tree(&args.join(" "));
}
macro_rules! case {
    ($name:ident,$seed:expr,$($args:expr),+ $(,)?)=>{
        #[test]
        #[ignore="needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
        fn $name(){let t=base($seed);$(compare(&t,$args);)+}
    };
}
case!(
    actions_record_runtime_convention_shape,
    false,
    &[
        "record",
        "core",
        "--type",
        "convention",
        "--content",
        "Use exact checks.",
        "--tags",
        "runtime,fixture"
    ]
);
case!(
    actions_record_builtin_types_and_shorthands,
    false,
    &[
        "record",
        "core",
        "--type",
        "pattern",
        "--description",
        "Use atomic writes"
    ],
    &[
        "record",
        "core",
        "--type",
        "failure",
        "--description",
        "A check failed",
        "--resolution",
        "Fix it"
    ],
    &[
        "record",
        "core",
        "--type",
        "decision",
        "--rationale",
        "Keep safe state"
    ],
    &[
        "record",
        "core",
        "--type",
        "reference",
        "--description",
        "Read the contract"
    ],
    &[
        "record",
        "core",
        "--type",
        "guide",
        "--description",
        "Run the tests"
    ]
);
case!(
    actions_record_named_upsert_merges_outcomes,
    false,
    &[
        "record",
        "core",
        "--type",
        "pattern",
        "--name",
        "safe",
        "--description",
        "Before",
        "--outcome-status",
        "success"
    ],
    &[
        "record",
        "core",
        "--type",
        "pattern",
        "--name",
        "safe",
        "--description",
        "After",
        "--outcome-status",
        "failure"
    ]
);
case!(
    actions_record_exact_duplicate_refused_with_force,
    false,
    &[
        "record",
        "core",
        "--type",
        "convention",
        "--content",
        "Use exact checks."
    ],
    &[
        "record",
        "core",
        "--type",
        "convention",
        "--content",
        "Use exact checks.",
        "--force"
    ]
);
case!(
    actions_record_named_force_same_id,
    false,
    &[
        "record",
        "core",
        "--type",
        "pattern",
        "--name",
        "same",
        "--description",
        "One"
    ],
    &[
        "record",
        "core",
        "--type",
        "pattern",
        "--name",
        "same",
        "--description",
        "Two",
        "--force"
    ]
);
case!(
    actions_record_missing_type_still_creates_domain,
    false,
    &["record", "fresh"]
);
case!(
    actions_record_validation_errors,
    false,
    &["record", "core", "--type", "missing", "--content", "x"],
    &["record", "core", "--type", "convention"],
    &[
        "record",
        "core",
        "--type",
        "convention",
        "--content",
        "x",
        "--outcome-status",
        "success",
        "--outcome-duration",
        "-1"
    ]
);
case!(
    actions_record_dry_run,
    false,
    &[
        "record",
        "core",
        "--type",
        "convention",
        "--content",
        "Dry check",
        "--dry-run"
    ]
);
case!(
    actions_record_strict_property_order,
    false,
    &[
        "record",
        "core",
        "--type",
        "pattern",
        "--name",
        "ordered",
        "--description",
        "Order matters",
        "--files",
        "b.rs,a.rs",
        "--tags",
        "b,a",
        "--classification",
        "tactical",
        "--relates-to",
        "mx-aaaa,mx-bbbb",
        "--outcome-status",
        "success",
        "--outcome-duration",
        "2.5"
    ]
);
case!(
    actions_record_unicode_duplicate,
    false,
    &[
        "record",
        "core",
        "--type",
        "convention",
        "--content",
        "Kelvin İ exact"
    ],
    &[
        "record",
        "core",
        "--type",
        "convention",
        "--content",
        "kelvin i exact"
    ]
);
case!(
    actions_edit_fields_outcome_classification,
    true,
    &[
        "edit",
        "core",
        "mx-cccc",
        "--description",
        "Changed",
        "--files",
        "b.rs,a.rs",
        "--classification",
        "tactical",
        "--outcome-status",
        "success",
        "--outcome-agent",
        "fixture"
    ]
);
case!(
    actions_edit_ignored_flag_and_no_flags_rewrite,
    true,
    &["edit", "core", "mx-aaaa", "--name", "ignored"],
    &["edit", "core", "mx-aaaa"]
);
case!(
    actions_edit_schema_failure,
    true,
    &["edit", "core", "mx-aaaa", "--content", ""]
);
case!(
    actions_edit_domain_and_id_errors,
    true,
    &["edit", "missing", "mx-aaaa", "--content", "x"],
    &["edit", "core", "unknown", "--content", "x"]
);
case!(
    actions_edit_runtime_content_shape,
    true,
    &[
        "edit",
        "core",
        "mx-aaaa",
        "--content",
        "Updated runtime content"
    ]
);
case!(actions_delete_single, true, &["delete", "core", "mx-bbbb"]);
case!(
    actions_delete_dry_run_single_shape,
    true,
    &["delete", "core", "mx-aaaa", "--dry-run"]
);
case!(
    actions_delete_records_and_all_except,
    true,
    &["delete", "core", "--records", "mx-bbbb"],
    &["delete", "core", "--all-except", "mx-aaaa"]
);
case!(
    actions_delete_unknown_id_is_atomic,
    true,
    &["delete", "core", "--records", "mx-aaaa,absent"]
);
case!(
    actions_delete_mode_errors,
    true,
    &["delete", "core"],
    &["delete", "core", "mx-aaaa", "--records", "mx-bbbb"],
    &["delete", "core", "--all-except", ""]
);
case!(
    actions_search_runtime_type_convention,
    true,
    &["search", "--type", "convention"]
);
case!(
    actions_search_bm25_boost_and_no_boost,
    true,
    &["search", "exact"],
    &["search", "exact", "--no-boost"]
);
case!(
    actions_search_filters_and_sort,
    true,
    &[
        "search",
        "--domain",
        "core",
        "--tag",
        "RUST",
        "--classification",
        "tactical",
        "--outcome-status",
        "success",
        "--sort-by-score"
    ],
    &["search", "--file", "MAIN.RS"]
);
case!(
    actions_search_errors,
    true,
    &["search"],
    &["search", "--domain", "missing"]
);
case!(
    actions_search_unicode_tokenizer,
    false,
    &[
        "record",
        "core",
        "--type",
        "convention",
        "--content",
        "École 東京 １２３ checks"
    ],
    &["search", "東京"]
);
case!(
    actions_show_json_multiple_ids,
    true,
    &["show", "mx-aaaa", "mx-cccc"]
);
case!(
    actions_show_ambiguous_and_missing_fail_fast,
    true,
    &["show", "mx-"],
    &["show", "absent", "mx-aaaa"]
);

fn append_config(t: &Twin, text: &str) {
    t.prepare(|p| {
        let mut config = std::fs::read_to_string(p.config_path()).unwrap();
        config.push_str(text);
        std::fs::write(p.config_path(), config).unwrap();
    });
}
fn custom(t: &Twin) {
    append_config(t, include_str!("fixtures/s5-atlas-lib-2/custom-types.yaml"));
}
macro_rules! extra_test {
    ($name:ident,$body:block)=>{
        #[test]
        #[ignore="needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
        fn $name() $body
    };
}
extra_test!(actions_record_near_duplicate_and_quality_levels, {
    for level in ["off", "warn", "error"] {
        let t = base(false);
        append_config(&t, &format!("quality:\n  level: {level}\n"));
        compare(
            &t,
            &[
                "record",
                "core",
                "--type",
                "convention",
                "--content",
                "Use exact checks for stable behavior",
            ],
        );
        compare(
            &t,
            &[
                "record",
                "core",
                "--type",
                "convention",
                "--content",
                "Use exact checks for stable behaviors",
            ],
        );
        compare(
            &t,
            &[
                "record",
                "core",
                "--type",
                "convention",
                "--content",
                "Use exact checks for stable behaviors",
                "--force",
            ],
        );
    }
});
extra_test!(actions_record_domain_rules_and_mismatch_flag, {
    let t = base(false);
    t.prepare(|p|p.write(".luuna/atlas/config.yaml","version: '1'\ndomains:\n  core:\n    allowed_types: [failure]\n    required_fields: [tags]\n"));
    compare(
        &t,
        &[
            "record",
            "core",
            "--type",
            "convention",
            "--content",
            "Prefer stable tests because noise hides faults.",
        ],
    );
    compare(
        &t,
        &[
            "record",
            "core",
            "--type",
            "failure",
            "--description",
            "Failure",
            "--resolution",
            "Fix",
        ],
    );
    compare(
        &t,
        &[
            "record",
            "core",
            "--type",
            "convention",
            "--content",
            "Prefer stable checks.",
            "--allow-domain-mismatch",
        ],
    );
});
extra_test!(actions_record_custom_types, {
    let t = base(false);
    custom(&t);
    compare(
        &t,
        &[
            "record",
            "core",
            "--type",
            "runbook",
            "--title",
            "Restart",
            "--steps",
            "Run checks",
            "--owner-team",
            "team",
        ],
    );
    compare(
        &t,
        &[
            "record",
            "core",
            "--type",
            "snippet",
            "--body",
            "A snippet",
            "--language",
            "rust",
        ],
    );
});
extra_test!(actions_record_auto_commit_and_dir_anchor_inference, {
    let t = base(false);
    t.prepare(|p| {
        p.write("src/lib.rs", "base\n");
        p.git(&["add", "."]);
        p.git(&["commit", "-qm", "base"]);
        p.write("src/lib.rs", "changed\n");
    });
    compare(
        &t,
        &[
            "record",
            "core",
            "--type",
            "pattern",
            "--name",
            "scope",
            "--description",
            "Prefer scoped checks.",
        ],
    );
});
extra_test!(actions_record_stdin_bulk, {
    let t = base(false);
    let input = include_bytes!("fixtures/s5-atlas-lib-2/bulk-mixed.json");
    compare_mode(&t, &["record", "core", "--stdin"], Some(input), &[], false);
    compare_mode(
        &t,
        &["record", "core", "--stdin", "--dry-run"],
        Some(input),
        &[],
        false,
    );
    compare_mode(
        &t,
        &["record", "fresh", "--stdin"],
        Some(b"not-json"),
        &[],
        false,
    );
});
extra_test!(actions_record_batch_file, {
    let t = base(false);
    t.prepare(|p| {
        p.write(
            "batch.json",
            include_str!("fixtures/s5-atlas-lib-2/bulk-mixed.json"),
        )
    });
    compare_mode(
        &t,
        &["record", "core", "--stdin", "--batch", "batch.json"],
        Some(b"ignored"),
        &[],
        false,
    );
    compare(&t, &["record", "core", "--batch", "absent.json"]);
});
extra_test!(actions_record_human_output, {
    let t = base(false);
    compare_mode(
        &t,
        &[
            "record",
            "fresh",
            "Prefer exact checks because failures need evidence.",
            "--type",
            "convention",
        ],
        None,
        &[],
        true,
    );
    compare_mode(
        &t,
        &["record", "core", "--type", "failure"],
        None,
        &[],
        true,
    );
    compare_mode(
        &t,
        &[
            "record",
            "core",
            "--type",
            "pattern",
            "--description",
            "Preview",
            "--dry-run",
        ],
        None,
        &[],
        true,
    );
});
extra_test!(actions_record_colour_forced, {
    let t = base(false);
    compare_mode(
        &t,
        &[
            "record",
            "core",
            "--type",
            "convention",
            "--content",
            "Prefer evidence because changes need checks.",
        ],
        None,
        &[("FORCE_COLOR", "1".into())],
        true,
    );
});
extra_test!(actions_delete_human_output, {
    let t = base(true);
    compare_mode(
        &t,
        &["delete", "core", "mx-bbbb", "--dry-run"],
        None,
        &[],
        true,
    );
    compare_mode(
        &t,
        &["delete", "core", "--records", "mx-bbbb,mx-cccc"],
        None,
        &[],
        true,
    );
});
extra_test!(actions_search_human_formats, {
    for format in ["markdown", "compact", "xml", "plain"] {
        let t = base(true);
        compare_mode(
            &t,
            &["search", "exact", "--format", format],
            None,
            &[],
            true,
        );
    }
});
extra_test!(actions_search_archived, {
    let t = base(true);
    t.prepare(|p|p.write(".luuna/atlas/archive/core.jsonl","{\"archived_at\":\"2026-01-01T00:00:00.000Z\",\"type\":\"convention\",\"id\":\"mx-dddd\",\"content\":\"Old exact checks\"}\n"));
    compare(&t, &["search", "exact", "--archived"]);
    compare_mode(&t, &["search", "exact", "--archived"], None, &[], true);
});
extra_test!(actions_show_human_markdown, {
    let t = base(true);
    compare_mode(
        &t,
        &["show", "mx-aaaa", "mx-bbbb", "mx-cccc"],
        None,
        &[],
        true,
    );
});
extra_test!(actions_show_custom_type_fallback, {
    let t = base(true);
    t.prepare(|p| {
        p.write(
            ".luuna/atlas/expertise/core.jsonl",
            "{\"id\":\"mx-unknown\",\"type\":\"unregistered\",\"extra\":\"literal\"}\n",
        )
    });
    compare_mode(
        &t,
        &["show", "mx-unknown", "--allow-unknown-types"],
        None,
        &[],
        true,
    );
});
extra_test!(actions_quiet_does_not_silence_search_show, {
    let t = base(true);
    compare_mode(&t, &["search", "exact", "--quiet"], None, &[], true);
    compare_mode(&t, &["show", "mx-aaaa", "--quiet"], None, &[], true);
});
extra_test!(actions_search_format_ids_is_a_parser_error_in_go, {
    let t = base(true);
    let args = vec![
        "search".into(),
        "exact".into(),
        "--format".into(),
        "ids".into(),
    ];
    let out = go(&t.go, &args, None, &[]);
    assert_ne!(out.exit_code, 0);
    assert!(out.stderr.contains("ids"));
    let out = native(&t.rust, &args, None, &[]);
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.contains("mx-aaaa"));
});
#[cfg(unix)]
fn script(t: &Twin, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    t.prepare(|p| {
        let path = p.atlas_dir().join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    });
}
#[cfg(unix)]
fn hook_config(t: &Twin, event: &str, body: &str) {
    script(t, "hook.sh", body);
    append_config(t, &format!("hooks:\n  {event}: [.luuna/atlas/hook.sh]\n"));
}
#[cfg(unix)]
extra_test!(actions_record_hooks_mutate_block_warn, {
    let t = base(false);
    hook_config(
        &t,
        "pre-record",
        r#"printf '%s\n' '{"domain":"core","record":{"type":"convention","content":"Mutated by hook","classification":"tactical","recorded_at":"2026-01-01T00:00:00.000Z"}}'"#,
    );
    compare_mode(
        &t,
        &[
            "record",
            "core",
            "--type",
            "convention",
            "--content",
            "Before",
        ],
        None,
        &[("LUUNA_ATLAS_LOG_LEVEL", "silent".into())],
        false,
    );
    script(&t, "hook.sh", "printf 'blocked' >&2; exit 7");
    compare_mode(
        &t,
        &[
            "record",
            "core",
            "--type",
            "convention",
            "--content",
            "Different",
        ],
        None,
        &[("LUUNA_ATLAS_LOG_LEVEL", "silent".into())],
        false,
    );
    let t = base(false);
    hook_config(&t, "post-record", "printf 'warned' >&2; exit 3");
    compare_mode(
        &t,
        &[
            "record",
            "core",
            "--type",
            "convention",
            "--content",
            "After warning",
        ],
        None,
        &[("LUUNA_ATLAS_LOG_LEVEL", "silent".into())],
        false,
    );
});
#[cfg(unix)]
extra_test!(actions_record_hook_timeout, {
    let t = base(false);
    hook_config(&t, "pre-record", "sleep 1");
    append_config(&t, "hook_settings:\n  timeout_ms: 10\n");
    compare_mode(
        &t,
        &[
            "record",
            "core",
            "--type",
            "convention",
            "--content",
            "Timeout",
        ],
        None,
        &[("LUUNA_ATLAS_LOG_LEVEL", "silent".into())],
        false,
    );
});
#[cfg(unix)]
extra_test!(actions_record_hook_finds_sh_on_the_given_path, {
    use std::os::unix::fs::PermissionsExt;
    let t = base(false);
    append_config(&t, "hooks:\n  pre-record: [ignored-command]\n");
    t.prepare(|p| {
        let bin = p.dir.join("stub");
        std::fs::create_dir(&bin).unwrap();
        let path = bin.join("sh");
        std::fs::write(
            &path,
            "#!/bin/sh\nprintf called > .luuna/atlas/stub-called\n",
        )
        .unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    });
    let extra = |p: &Project| vec![("PATH", p.dir.join("stub").to_string_lossy().into_owned())];
    let args: Vec<String> = [
        "record",
        "core",
        "--type",
        "convention",
        "--content",
        "Stub shell",
        "--json",
        "--quiet",
    ]
    .iter()
    .map(|s| (*s).into())
    .collect();
    let g = go(&t.go, &args, None, &extra(&t.go));
    let r = native(&t.rust, &args, None, &extra(&t.rust));
    assert_same_output("stub shell", &t.go, &g, &t.rust, &r);
    t.assert_same_tree("stub shell");
    assert_eq!(
        std::fs::read(t.rust.atlas_dir().join("stub-called")).unwrap(),
        b"called"
    );
});

use luuna_atlas::{
    actions::api,
    json::{self, Object, Value},
};
fn value(text: &str) -> Value {
    json::parse(text).unwrap()
}
fn native_api(p: &Project, operation: &str, args: &[Value]) -> luuna_atlas::Result<Value> {
    let nil = Value::Null;
    let arg = |n: usize| args.get(n).unwrap_or(&nil);
    let text = |n| arg(n).as_str().unwrap_or_default();
    let obj = |n| arg(n).as_object().cloned().unwrap_or_default();
    let field = |n: usize, key: &str| {
        arg(n)
            .as_object()
            .and_then(|o| o.get(key))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    match operation {
        "recordExpertise" => api::record_expertise(
            &p.dir,
            text(0),
            obj(1),
            &api::RecordOptions {
                force: arg(2)
                    .as_object()
                    .and_then(|o| o.get("force"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
            None,
        )
        .map(|v| v.to_value()),
        "searchExpertise" => api::search_expertise(
            &p.dir,
            text(0),
            &api::SearchOptions {
                domain: field(1, "domain"),
                type_name: field(1, "type"),
                tag: field(1, "tag"),
                classification: field(1, "classification"),
                file: field(1, "file"),
            },
            None,
        )
        .map(|v| Value::Array(v.iter().map(api::SearchResult::to_value).collect())),
        "queryDomain" => api::query_domain(
            &p.dir,
            text(0),
            &api::QueryOptions {
                type_name: field(1, "type"),
                classification: field(1, "classification"),
                file: field(1, "file"),
            },
            None,
        )
        .map(|v| Value::Array(v.into_iter().map(Value::Object).collect())),
        "editRecord" => {
            api::edit_record(&p.dir, text(0), text(1), &obj(2), None).map(Value::Object)
        }
        "appendOutcome" => {
            api::append_outcome(&p.dir, text(0), text(1), &obj(2), None).map(|v| v.to_value())
        }
        _ => panic!("unknown test API operation"),
    }
}
fn go_api(p: &Project, operation: &str, args: &[Value]) -> CommandOutput {
    let mut args = args.to_vec();
    let option_index = match operation {
        "recordExpertise" => 2,
        "searchExpertise" | "queryDomain" => 1,
        "editRecord" | "appendOutcome" => 3,
        _ => panic!("unknown API"),
    };
    while args.len() <= option_index {
        args.push(Value::Object(Object::new()));
    }
    args[option_index]
        .as_object_mut()
        .unwrap()
        .insert("cwd", p.dir.to_string_lossy().into_owned());
    let mut request = Object::new();
    request.insert("operation", operation);
    request.insert("args", Value::Array(args));
    let input = json::to_compact_string(&Value::Object(request)).unwrap();
    go(
        p,
        &[],
        Some(input.as_bytes()),
        &[("LUUNA_ATLAS_API_BRIDGE", "1".into())],
    )
}
fn compare_api(t: &Twin, operation: &str, args: &[Value]) {
    let g = go_api(&t.go, operation, args);
    let response = json::parse(&g.stdout).unwrap();
    let response = response.as_object().unwrap();
    let r = native_api(&t.rust, operation, args);
    match r {
        Ok(result) => {
            assert_eq!(g.exit_code, 0, "{}", g.stdout);
            let expected = json::to_compact_string(response.get("result").unwrap()).unwrap();
            let actual = json::to_compact_string(&result).unwrap();
            assert_eq!(
                t.go.normalizer().normalize(&expected),
                t.rust.normalizer().normalize(&actual),
                "{operation}"
            );
        }
        Err(error) => {
            assert_ne!(g.exit_code, 0);
            assert_eq!(
                t.go.normalizer()
                    .normalize(response.get("error").unwrap().as_str().unwrap()),
                t.rust.normalizer().normalize(&error.to_string())
            );
        }
    }
    t.assert_same_tree(operation);
}
extra_test!(actions_api_record_expertise, {
    let t = base(false);
    let first = value(
        r#"{"type":"pattern","name":"api","description":"First","classification":"tactical","recorded_at":"2026-01-01T00:00:00.000Z"}"#,
    );
    let second = value(
        r#"{"type":"pattern","name":"api","description":"Second","classification":"tactical","recorded_at":"2026-01-01T00:00:00.000Z"}"#,
    );
    compare_api(&t, "recordExpertise", &[Value::from("core"), first.clone()]);
    compare_api(
        &t,
        "recordExpertise",
        &[Value::from("core"), second.clone()],
    );
    compare_api(
        &t,
        "recordExpertise",
        &[Value::from("core"), first, value(r#"{"force":true}"#)],
    );
    let c = value(
        r#"{"type":"convention","content":"API duplicate","classification":"tactical","recorded_at":"2026-01-01T00:00:00.000Z"}"#,
    );
    compare_api(&t, "recordExpertise", &[Value::from("core"), c.clone()]);
    compare_api(&t, "recordExpertise", &[Value::from("core"), c]);
});
extra_test!(actions_api_search_expertise, {
    let t = base(true);
    compare_api(&t, "searchExpertise", &[Value::from("exact"), value("{}")]);
    compare_api(
        &t,
        "searchExpertise",
        &[
            Value::from(""),
            value(r#"{"domain":"core","type":"convention","tag":"RUST"}"#),
        ],
    );
    compare_api(
        &t,
        "searchExpertise",
        &[Value::from(""), value(r#"{"domain":"missing"}"#)],
    );
});
extra_test!(actions_api_query_domain, {
    let t = base(true);
    compare_api(
        &t,
        "queryDomain",
        &[
            Value::from("core"),
            value(r#"{"type":"pattern","file":"MAIN.RS"}"#),
        ],
    );
    compare_api(&t, "queryDomain", &[Value::from("missing")]);
});
extra_test!(actions_api_edit_record, {
    let t = base(true);
    compare_api(
        &t,
        "editRecord",
        &[
            Value::from("core"),
            Value::from("mx-aaaa"),
            value(
                r#"{"content":"API changed","name":"ignored","tags":["api"],"outcomes":[{"status":"success"}]}"#,
            ),
        ],
    );
    compare_api(
        &t,
        "editRecord",
        &[Value::from("core"), Value::from("absent"), value("{}")],
    );
    compare_api(
        &t,
        "editRecord",
        &[
            Value::from("core"),
            Value::from("mx-cccc"),
            value(r#"{"name":null,"files":"not-an-array","extra":"ignored"}"#),
        ],
    );
});
extra_test!(actions_api_append_outcome, {
    let t = base(true);
    compare_api(
        &t,
        "appendOutcome",
        &[
            Value::from("core"),
            Value::from("mx-aaaa"),
            value(r#"{"status":"success","agent":"fixture"}"#),
        ],
    );
    compare_api(
        &t,
        "appendOutcome",
        &[
            Value::from("core"),
            Value::from("mx-aaaa"),
            value(
                r#"{"status":"failure","recorded_at":"2026-01-01T00:00:00.000Z","duration":2.5}"#,
            ),
        ],
    );
});
fn bad_and_good(bad: &str) -> String {
    format!("{bad}\n{{\"id\":\"mx-eeee\",\"type\":\"pattern\",\"name\":\"good\",\"description\":\"Keep good record\",\"classification\":\"tactical\",\"recorded_at\":\"2026-01-01T00:00:00.000Z\"}}\n")
}
fn a8_rewrites(bad: &str, custom_type: bool, needle: &str) {
    // Each operation rewrites a different, good record; the bad record is retained.
    for site in [
        "upsert",
        "bulk",
        "edit",
        "delete",
        "api-replace",
        "api-edit",
        "api-outcome",
    ] {
        if custom_type && site.starts_with("api-") {
            continue;
        } // Go's thin API has a built-in-only registry.
        let t = base(false);
        if custom_type {
            custom(&t);
        }
        let body = bad_and_good(bad);
        t.prepare(|p| p.write(".luuna/atlas/expertise/core.jsonl", &body));
        let before = std::fs::read(t.rust.expertise("core")).unwrap();
        if needle == "negative zero" {
            assert!(go_number_markers(&t.go).contains("-0"));
        }

        let mode = std::fs::metadata(t.rust.expertise("core"))
            .unwrap()
            .permissions();
        let (g, r) = match site {
            "api-replace" | "api-edit" | "api-outcome" => {
                let (operation, args) = match site {
                    "api-replace" => (
                        "recordExpertise",
                        vec![
                            Value::from("core"),
                            value(
                                r#"{"type":"pattern","name":"good","description":"Updated","classification":"tactical","recorded_at":"2026-01-01T00:00:00.000Z"}"#,
                            ),
                        ],
                    ),
                    "api-edit" => (
                        "editRecord",
                        vec![
                            Value::from("core"),
                            Value::from("mx-eeee"),
                            value(r#"{"description":"Updated"}"#),
                        ],
                    ),
                    _ => (
                        "appendOutcome",
                        vec![
                            Value::from("core"),
                            Value::from("mx-eeee"),
                            value(r#"{"status":"success"}"#),
                        ],
                    ),
                };
                (
                    go_api(&t.go, operation, &args),
                    native_api(&t.rust, operation, &args)
                        .map(|_| ())
                        .map_err(|e| e.to_string()),
                )
            }
            _ => {
                let a: Vec<String> = match site {
                    "upsert" => vec![
                        "record",
                        "core",
                        "--type",
                        "pattern",
                        "--name",
                        "good",
                        "--description",
                        "Updated",
                        "--json",
                        "--quiet",
                    ],
                    "bulk" => vec!["record", "core", "--stdin", "--json", "--quiet"],
                    "edit" => vec![
                        "edit",
                        "core",
                        "mx-eeee",
                        "--description",
                        "Updated",
                        "--json",
                        "--quiet",
                    ],
                    _ => vec!["delete", "core", "mx-eeee", "--json", "--quiet"],
                }
                .into_iter()
                .map(String::from)
                .collect();
                let input = if site == "bulk" {
                    Some(
                        br#"{"type":"convention","content":"Fresh","classification":"tactical"}"#
                            .as_slice(),
                    )
                } else {
                    None
                };
                let g = go(&t.go, &a, input, &[]);
                let r = native(&t.rust, &a, input, &[]);
                let r = if r.exit_code == 0 {
                    Ok(())
                } else {
                    Err(r.stderr)
                };
                (g, r)
            }
        };
        assert_eq!(g.exit_code, 0, "{site}: {}", g.stderr);
        if needle == "negative zero" {
            assert_eq!(
                go_number_markers(&t.go),
                "[]",
                "{site}: Go did not lose the typed-reader marker"
            );
        } else {
            assert!(
                !String::from_utf8_lossy(&std::fs::read(t.go.expertise("core")).unwrap())
                    .contains("discard-marker"),
                "{site}: Go did not drop the proven value"
            );
        }
        let error = r.expect_err("native rewrite must refuse data loss");
        assert!(error.contains("Refusing to rewrite"), "{site}: {error}");
        assert!(error.contains(needle), "{site}: {error}");
        assert_eq!(std::fs::read(t.rust.expertise("core")).unwrap(), before);
        assert_eq!(
            std::fs::metadata(t.rust.expertise("core"))
                .unwrap()
                .permissions(),
            mode
        );
        assert!(t.go.leftovers().is_empty());
        assert!(t.rust.leftovers().is_empty());
    }
}
extra_test!(actions_a8_duplicate_key_line_refused, {
    a8_rewrites(
        r#"{"id":"mx-dddd","type":"pattern","name":"bad","description":"discard-marker","description":"Kept","classification":"tactical","recorded_at":"2026-01-01T00:00:00.000Z"}"#,
        false,
        "repeats",
    );
});
extra_test!(actions_a8_legacy_outcome_extra_field_refused, {
    a8_rewrites(
        r#"{"id":"mx-dddd","type":"pattern","name":"bad","description":"Kept","classification":"tactical","recorded_at":"2026-01-01T00:00:00.000Z","outcome":{"status":"success","notes":"discard-marker"}}"#,
        false,
        "legacy",
    );
});
extra_test!(actions_a8_alias_conflict_refused, {
    a8_rewrites(
        r#"{"id":"mx-dddd","type":"runbook","title":"Kept","heading":"discard-marker","steps":"s","classification":"tactical","recorded_at":"2026-01-01T00:00:00.000Z"}"#,
        true,
        "heading",
    );
});
extra_test!(actions_a8_deleting_the_bad_record_is_allowed, {
    let t = base(false);
    t.prepare(|p|p.write(".luuna/atlas/expertise/core.jsonl",&bad_and_good(r#"{"id":"mx-dddd","type":"pattern","name":"bad","description":"discard-marker","description":"Kept","classification":"tactical","recorded_at":"2026-01-01T00:00:00.000Z"}"#)));
    compare(&t, &["delete", "core", "mx-dddd"]);
});
extra_test!(actions_a8_comments_and_blank_lines_dropped_as_go, {
    let t = base(true);
    t.prepare(|p| {
        p.write(
            ".luuna/atlas/expertise/core.jsonl",
            &format!("# leading\n\n{ROWS}\n# trailing\n"),
        )
    });
    compare(&t, &["edit", "core", "mx-aaaa"]);
});
extra_test!(actions_a8_number_forms_read_back_same, {
    let t = base(false);
    let row = r#"{"id":"mx-aaaa","type":"convention","content":"Exact checks","classification":"tactical","recorded_at":"2026-01-01T00:00:00.000Z","outcomes":[{"status":"success","duration":1e2},{"status":"success","duration":9007199254740993}]}"#;
    t.prepare(|p| p.write(".luuna/atlas/expertise/core.jsonl", &format!("{row}\n")));
    let before = t.go.go(&["show", "mx-aaaa"]);
    compare(&t, &["edit", "core", "mx-aaaa"]);
    let after = t.go.go(&["show", "mx-aaaa"]);
    assert_same_output("number readback", &t.go, &before, &t.go, &after);
});
extra_test!(actions_a8_invalid_utf8_copied, {
    let t = base(false);
    let mut row = br#"{"id":"mx-aaaa","type":"convention","content":""#.to_vec();
    row.push(0xff);
    row.extend_from_slice(
        br#"","classification":"tactical","recorded_at":"2026-01-01T00:00:00.000Z"}"#,
    );
    row.push(b'\n');
    t.prepare(|p| std::fs::write(p.expertise("core"), &row).unwrap());
    let before = t.go.go(&["show", "mx-aaaa"]);
    compare(&t, &["edit", "core", "mx-aaaa"]);
    let after = t.go.go(&["show", "mx-aaaa"]);
    assert_same_output("UTF-8 readback", &t.go, &before, &t.go, &after);
});
extra_test!(actions_a8_array_row_refused_as_go, {
    let t = base(true);
    t.prepare(|p| p.write(".luuna/atlas/expertise/core.jsonl", &format!("[]\n{ROWS}")));
    let before = std::fs::read(t.rust.expertise("core")).unwrap();
    compare(&t, &["edit", "core", "mx-aaaa"]);
    assert_eq!(std::fs::read(t.rust.expertise("core")).unwrap(), before);
});
extra_test!(actions_a8_empty_legacy_alias_is_copied, {
    for old in ["null", r#""""#] {
        let t = base(false);
        custom(&t);
        let row = format!(
            r#"{{"id":"mx-aaaa","type":"runbook","title":"A","heading":{old},"steps":"s","classification":"tactical","recorded_at":"2026-01-01T00:00:00.000Z"}}"#
        ) + "\n";
        t.prepare(|p| p.write(".luuna/atlas/expertise/core.jsonl", &row));
        let before = t.go.go(&["show", "mx-aaaa"]);
        let g = t.go.go(&["edit", "core", "mx-aaaa"]);
        assert_eq!(g.exit_code, 0, "{}", g.stderr);
        let after = t.go.go(&["show", "mx-aaaa"]);
        assert_same_output("empty alias readback", &t.go, &before, &t.go, &after);
        let a: Vec<String> = ["edit", "core", "mx-aaaa", "--json", "--quiet"]
            .into_iter()
            .map(String::from)
            .collect();
        let r = native(&t.rust, &a, None, &[]);
        assert_same_output("empty alias edit", &t.go, &g, &t.rust, &r);
        t.assert_same_tree("empty alias copied");
    }
});

fn go_number_markers(p: &Project) -> String {
    let mut request = Object::new();
    request.insert("operation", "readExpertiseFile");
    request.insert(
        "args",
        Value::Array(vec![
            Value::from(p.expertise("core").to_string_lossy().into_owned()),
            Value::Object(Object::new()),
        ]),
    );
    let input = json::to_compact_string(&Value::Object(request)).unwrap();
    let out = go(
        p,
        &[],
        Some(input.as_bytes()),
        &[("LUUNA_ATLAS_API_BRIDGE", "1".into())],
    );
    assert_eq!(out.exit_code, 0, "{}", out.stdout);
    let out = json::parse(&out.stdout).unwrap();
    json::to_compact_string(out.as_object().unwrap().get("numbers").unwrap()).unwrap()
}
extra_test!(actions_a8_negative_zero_refused, {
    a8_rewrites(
        r#"{"id":"mx-dddd","type":"pattern","name":"bad","description":"Kept","classification":"tactical","recorded_at":"2026-01-01T00:00:00.000Z","outcomes":[{"status":"success","duration":-0}]}"#,
        false,
        "negative zero",
    );
});
extra_test!(actions_a8_explicit_outcome_replacement_is_allowed, {
    let t = base(false);
    t.prepare(|p|p.write(".luuna/atlas/expertise/core.jsonl","{\"id\":\"mx-aaaa\",\"type\":\"convention\",\"content\":\"Exact state\",\"classification\":\"tactical\",\"recorded_at\":\"2026-01-01T00:00:00.000Z\",\"outcomes\":[{\"status\":\"success\",\"duration\":-0}]}\n"));
    compare_api(
        &t,
        "editRecord",
        &[
            Value::from("core"),
            Value::from("mx-aaaa"),
            value(r#"{"outcomes":[]}"#),
        ],
    );
});
#[cfg(unix)]
extra_test!(actions_record_hook_diagnostic_line, {
    let t = base(false);
    hook_config(&t, "pre-record", "exit 7");
    let args: Vec<String> = [
        "record",
        "core",
        "--type",
        "convention",
        "--content",
        "Blocked diagnostic",
        "--json",
        "--quiet",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    let extra = [("LUUNA_ATLAS_LOG_LEVEL", "warn".into())];
    let mut g = go(&t.go, &args, None, &extra);
    let mut r = native(&t.rust, &args, None, &extra);
    for (out, native_side) in [(&mut g, false), (&mut r, true)] {
        let (line, rest) = out.stderr.split_once('\n').expect("diagnostic line");
        let mut diagnostic = json::parse(line).expect("valid diagnostic JSON");
        let o = diagnostic.as_object_mut().unwrap();
        assert_eq!(o.get("level").and_then(Value::as_f64), Some(40.0));
        assert_eq!(o.get("event").and_then(Value::as_str), Some("pre-record"));
        assert_eq!(o.get("blocking").and_then(Value::as_bool), Some(true));
        let pid = o.get("pid").and_then(Value::as_f64).unwrap();
        assert!(pid > 0.0);
        if native_side {
            assert_eq!(pid, f64::from(std::process::id()));
        }
        let time = o.get("time").and_then(Value::as_f64).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as f64;
        assert!((time - now).abs() < 10_000.0);
        o.insert("pid", 0i64);
        o.insert("time", 0i64);
        out.stderr = format!("{}\n{rest}", json::to_compact_string(&diagnostic).unwrap());
    }
    assert_same_output("diagnostic", &t.go, &g, &t.rust, &r);
    t.assert_same_tree("diagnostic");
});
#[cfg(unix)]
extra_test!(actions_cf2_symlinked_expertise_file, {
    use std::os::unix::fs::symlink;
    let t = base(false);
    t.prepare(|p| {
        let path = p.expertise("core");
        std::fs::remove_file(&path).unwrap();
        let target = p.dir.join("linked.jsonl");
        std::fs::write(&target, ROWS).unwrap();
        symlink(target, path).unwrap();
    });
    compare(
        &t,
        &[
            "record",
            "core",
            "--type",
            "convention",
            "--content",
            "Prefer link-safe evidence because state matters.",
        ],
    );
    assert!(std::fs::symlink_metadata(t.rust.expertise("core"))
        .unwrap()
        .file_type()
        .is_symlink());
    let target = std::fs::read(t.rust.dir.join("linked.jsonl")).unwrap();
    compare(
        &t,
        &[
            "edit",
            "core",
            "mx-aaaa",
            "--content",
            "Changed through atomic rewrite",
        ],
    );
    assert!(!std::fs::symlink_metadata(t.rust.expertise("core"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        std::fs::read(t.rust.dir.join("linked.jsonl")).unwrap(),
        target
    );
});
extra_test!(actions_hostile_deep_stdin_record, {
    let t = base(false);
    let text = format!("{}{{}}{}", "[".repeat(9999), "]".repeat(9999));
    compare_mode(
        &t,
        &["record", "core", "--stdin"],
        Some(text.as_bytes()),
        &[],
        false,
    );
    assert!(std::fs::metadata(t.rust.expertise("core")).unwrap().len() <= 64 * 1024);
});
extra_test!(actions_hostile_large_stdin, {
    let t = base(false);
    let mut input = vec![b' '; 1024 * 1024];
    input[1024 * 1024 - 1] = b'!';
    compare_mode(&t, &["record", "core", "--stdin"], Some(&input), &[], false);
    assert!(std::fs::metadata(t.rust.expertise("core")).unwrap().len() <= 64 * 1024);
});
extra_test!(actions_hostile_deep_stored_line_edit, {
    let t = base(false);
    let row=format!("{{\"id\":\"mx-aaaa\",\"type\":\"convention\",\"content\":\"Deep field\",\"classification\":\"tactical\",\"recorded_at\":\"2026-01-01T00:00:00.000Z\",\"extra\":{}0{}}}\n","[".repeat(9999),"]".repeat(9999));
    assert!(row.len() <= 64 * 1024);
    t.prepare(|p| p.write(".luuna/atlas/expertise/core.jsonl", &row));
    let args = vec![
        Value::from("core"),
        Value::from("mx-aaaa"),
        value(r#"{"content":"Deep edit"}"#),
    ];
    let g = go_api(&t.go, "editRecord", &args);
    assert_eq!(g.exit_code, 0);
    assert_eq!(g.stdout, "\n");
    let result = native_api(&t.rust, "editRecord", &args).unwrap();
    let mut response = Object::new();
    response.insert("success", true);
    response.insert("result", result);
    response.insert("numbers", Value::Array(Vec::new()));
    let r = CommandOutput::emit(&response);
    assert_same_output("deep API record", &t.go, &g, &t.rust, &r);
    t.assert_same_tree("deep API record");
    assert!(std::fs::metadata(t.rust.expertise("core")).unwrap().len() <= 64 * 1024);
});

extra_test!(actions_record_nonprinting_quotes_match_go, {
    let t = base(false);
    for typ in [
        "missing\u{00a0}",
        "missing\u{00ad}",
        "missing\u{200b}",
        "missing\u{0378}",
    ] {
        compare(
            &t,
            &["record", "core", "--type", typ, "--content", "Quoted type"],
        );
    }
});
extra_test!(actions_a8_upsert_keeps_negative_zero_outcomes, {
    let t = base(false);
    t.prepare(|p|p.write(".luuna/atlas/expertise/core.jsonl","{\"id\":\"mx-aaaa\",\"type\":\"pattern\",\"name\":\"same\",\"description\":\"Before\",\"classification\":\"tactical\",\"recorded_at\":\"2026-01-01T00:00:00.000Z\",\"outcomes\":[{\"status\":\"success\",\"duration\":-0}]}\n"));
    let before = std::fs::read(t.rust.expertise("core")).unwrap();
    assert!(go_number_markers(&t.go).contains("-0"));
    let args: Vec<String> = [
        "record",
        "core",
        "--type",
        "pattern",
        "--name",
        "same",
        "--description",
        "After",
        "--json",
        "--quiet",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    let g = go(&t.go, &args, None, &[]);
    assert_eq!(g.exit_code, 0, "{}", g.stderr);
    assert_eq!(go_number_markers(&t.go), "[]");
    let r = native(&t.rust, &args, None, &[]);
    assert_eq!(r.exit_code, 1);
    assert!(r.stderr.contains("negative zero"), "{}", r.stderr);
    assert_eq!(std::fs::read(t.rust.expertise("core")).unwrap(), before);
    assert!(t.rust.leftovers().is_empty());
});
extra_test!(actions_a8_explicit_api_replacement_drops_old_outcomes, {
    let t = base(false);
    t.prepare(|p|p.write(".luuna/atlas/expertise/core.jsonl","{\"id\":\"mx-aaaa\",\"type\":\"pattern\",\"name\":\"same\",\"description\":\"Before\",\"classification\":\"tactical\",\"recorded_at\":\"2026-01-01T00:00:00.000Z\",\"outcomes\":[{\"status\":\"success\",\"duration\":-0}]}\n"));
    compare_api(
        &t,
        "recordExpertise",
        &[
            Value::from("core"),
            value(
                r#"{"type":"pattern","name":"same","description":"Replacement","classification":"tactical","recorded_at":"2026-01-01T00:00:00.000Z"}"#,
            ),
        ],
    );
});
