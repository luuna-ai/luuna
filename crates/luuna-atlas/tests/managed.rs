//! Opt-in managed execution leaves the raw Go-compatible library unchanged.
use luuna_atlas::{
    json,
    managed::{self, YamlLimits},
    yaml,
};
#[test]
fn yaml_limits_are_scoped_and_raw_defaults_are_preserved() {
    let input = "a: &a {x: 1}\nb: *a\n";
    let limits = YamlLimits {
        max_nodes: 3,
        ..YamlLimits::default()
    };
    let refused = managed::with_yaml_limits(limits, || yaml::parse(input));
    assert!(refused.unwrap_err().to_string().contains("managed YAML"));
    let raw = yaml::parse(input).unwrap();
    assert!(json::to_compact_string(&raw)
        .unwrap()
        .contains("\"b\":{\"x\":1}"));
    let _ =
        std::panic::catch_unwind(|| managed::with_yaml_limits(limits, || panic!("scope unwind")));
    assert!(yaml::parse(input).is_ok());
}
#[test]
#[ignore = "hostile alias expansion: compile first; 4GiB/120s execution cap"]
fn hostile_managed_yaml_alias_expansion_is_bounded() {
    use std::io::Write;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/managed-hostile");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("aliases.yaml");
    let mut out = std::fs::File::create(&file).unwrap();
    writeln!(out, "n0: &n0 [payload, payload]").unwrap();
    for n in 1..30 {
        writeln!(out, "n{n}: &n{n} [*n{}, *n{}]", n - 1, n - 1).unwrap();
    }
    drop(out);
    let text = std::fs::read_to_string(file).unwrap();
    let result = std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(move || managed::with_yaml_limits(YamlLimits::default(), || yaml::parse(&text)))
        .unwrap()
        .join()
        .unwrap();
    let error = match result {
        Err(e) => e,
        Ok(_) => panic!("managed alias expansion must refuse"),
    };
    assert!(error.to_string().contains("managed YAML"));
}

#[cfg(unix)]
#[test]
fn managed_hook_completion_cleans_descendants_and_preserves_unrelated_process() {
    use luuna_atlas::{
        config::Config,
        hooks,
        options::{Context, Env, GlobalOptions},
    };
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let dir = tempfile::tempdir().unwrap();
    let mut unrelated = Command::new("sleep")
        .arg("10")
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        cwd: dir.path().to_owned(),
        env: Env::fixed([("PATH", "/usr/bin:/bin")], vec![], false).with_managed_children(),
        globals: GlobalOptions::default(),
    };
    let cfg: Config = match json::parse(
        r#"{"hooks":{"post-record":["(sleep 1; printf leaked > late) & printf done"]}}"#,
    )
    .unwrap()
    {
        json::Value::Object(ref m) => m.clone(),
        _ => unreachable!(),
    };
    let start = Instant::now();
    let out = hooks::run_hooks(
        &ctx,
        Some(&cfg),
        "post-record",
        &json::Value::Null,
        &mut String::new(),
    )
    .unwrap();
    assert_eq!(out.executions[0].stdout, "done");
    assert_eq!(out.executions[0].exit_code, 0);
    assert!(start.elapsed() < Duration::from_secs(2));
    std::thread::sleep(Duration::from_millis(1200));
    assert!(!dir.path().join("late").exists());
    assert!(unrelated.try_wait().unwrap().is_none());
    unrelated.kill().unwrap();
    unrelated.wait().unwrap();
}

#[cfg(unix)]
#[test]
#[ignore = "hostile hook output: compile first; 4GiB/120s execution cap"]
fn hostile_managed_hook_output_is_bounded_and_stops_descendants() {
    use luuna_atlas::{
        config::Config,
        hooks,
        options::{Context, Env, GlobalOptions},
    };
    let dir =
        tempfile::tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"))
            .unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        cwd: dir.path().to_owned(),
        env: Env::fixed([("PATH", "/usr/bin:/bin")], vec![], false).with_managed_children(),
        globals: GlobalOptions::default(),
    };
    let cfg: Config = match json::parse(
        r#"{"hooks":{"pre-record":["(sleep 1; printf leaked > late) & yes payload"]}}"#,
    )
    .unwrap()
    {
        json::Value::Object(ref m) => m.clone(),
        _ => unreachable!(),
    };
    let out = hooks::run_hooks(
        &ctx,
        Some(&cfg),
        "pre-record",
        &json::Value::Null,
        &mut String::new(),
    )
    .unwrap();
    assert!(out.blocked);
    assert!(out.executions[0].stderr.contains("output limit"));
    assert!(out.executions[0].stdout.len() <= 1024 * 1024);
    std::thread::sleep(std::time::Duration::from_millis(1200));
    assert!(!dir.path().join("late").exists());
}
