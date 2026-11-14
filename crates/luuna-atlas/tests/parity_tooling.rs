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
