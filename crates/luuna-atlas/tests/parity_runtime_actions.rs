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
    "{\"id\":\"mx-a\",\"type\":\"convention\",\"content\":\"Prefer exact checks.\",\"recorded_at\":\"2026-01-01T00:00:00.000Z\",\"tags\":[\"rust\"],\"classification\":\"tactical\",\"outcomes\":[{\"status\":\"success\"}]}\n",
    "{\"id\":\"mx-b\",\"type\":\"failure\",\"description\":\"Flaky check\",\"resolution\":\"Use deterministic checks.\",\"recorded_at\":\"2026-01-01T00:00:00.000Z\",\"outcomes\":[{\"status\":\"failure\"}]}\n",
    "{\"id\":\"mx-c\",\"type\":\"pattern\",\"name\":\"Atomic write\",\"description\":\"Use locks for exact checks.\",\"files\":[\"src/main.rs\"],\"recorded_at\":\"2026-01-01T00:00:00.000Z\"}\n"
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
        "mx-a,mx-b",
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
        "mx-c",
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
    &["edit", "core", "mx-a", "--name", "ignored"],
    &["edit", "core", "mx-a"]
);
case!(
    actions_edit_schema_failure,
    true,
    &["edit", "core", "mx-a", "--content", ""]
);
case!(
    actions_edit_domain_and_id_errors,
    true,
    &["edit", "missing", "mx-a", "--content", "x"],
    &["edit", "core", "unknown", "--content", "x"]
);
case!(
    actions_edit_runtime_content_shape,
    true,
    &[
        "edit",
        "core",
        "mx-a",
        "--content",
        "Updated runtime content"
    ]
);
case!(actions_delete_single, true, &["delete", "core", "mx-b"]);
case!(
    actions_delete_dry_run_single_shape,
    true,
    &["delete", "core", "mx-a", "--dry-run"]
);
case!(
    actions_delete_records_and_all_except,
    true,
    &["delete", "core", "--records", "mx-b"],
    &["delete", "core", "--all-except", "mx-a"]
);
case!(
    actions_delete_unknown_id_is_atomic,
    true,
    &["delete", "core", "--records", "mx-a,absent"]
);
case!(
    actions_delete_mode_errors,
    true,
    &["delete", "core"],
    &["delete", "core", "mx-a", "--records", "mx-b"],
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
    &["show", "mx-a", "mx-c"]
);
case!(
    actions_show_ambiguous_and_missing_fail_fast,
    true,
    &["show", "mx-"],
    &["show", "absent", "mx-a"]
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
        &["delete", "core", "mx-b", "--dry-run"],
        None,
        &[],
        true,
    );
    compare_mode(
        &t,
        &["delete", "core", "--records", "mx-b,mx-c"],
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
    t.prepare(|p|p.write(".luuna/atlas/archive/core.jsonl","{\"archived_at\":\"2026-01-01T00:00:00.000Z\",\"type\":\"convention\",\"id\":\"mx-old\",\"content\":\"Old exact checks\"}\n"));
    compare(&t, &["search", "exact", "--archived"]);
    compare_mode(&t, &["search", "exact", "--archived"], None, &[], true);
});
extra_test!(actions_show_human_markdown, {
    let t = base(true);
    compare_mode(&t, &["show", "mx-a", "mx-b", "mx-c"], None, &[], true);
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
    compare_mode(&t, &["show", "mx-a", "--quiet"], None, &[], true);
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
    assert!(out.stdout.contains("mx-a"));
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
