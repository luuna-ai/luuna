//! The client runs only against a loopback mock, with private temporary HOME,
//! config and cwd. The Git case creates its own checkout and isolated config.
mod support;
use serde_json::{json, Value};
use std::{
    path::Path,
    process::{Command, Output, Stdio},
};
const TOKEN: &str = "synthetic-intent-token-at-least-32-bytes";
const NAMES: [&str; 8] = [
    "show",
    "init",
    "proposals",
    "questions",
    "sprouts",
    "accept",
    "reject",
    "reject-sprout",
];
fn run(cwd: &Path, args: &[&str], url: &str, extra: &[(&str, &str)]) -> Output {
    let home = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_luuna-tui"));
    command
        .env_clear()
        .current_dir(cwd)
        .args(args)
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .env("NO_COLOR", "1")
        .env("LUUNA_URL", url)
        .env("LUUNA_API_TOKEN", TOKEN)
        .stdin(Stdio::null());
    for name in ["PATH", "SystemRoot", "WINDIR", "PATHEXT"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    for (k, v) in extra {
        command.env(k, v);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command.output().unwrap()
}
#[test]
fn local_group_help_is_complete_and_unknown_verbs_are_usage_errors() {
    let cwd = tempfile::tempdir().unwrap();
    let mock = support::serve(|_| support::json(json!({})));
    for (args, code) in [
        (vec!["intent"], 2),
        (vec!["intent", "--help"], 0),
        (vec!["intent", "-h"], 0),
        (vec!["help", "intent"], 0),
    ] {
        let out = run(cwd.path(), &args, &mock.url(), &[("LUUNA_API_TOKEN", "")]);
        assert_eq!(out.status.code(), Some(code));
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        for name in NAMES {
            assert!(text.contains(&format!("intent-{name}")), "{text}");
        }
    }
    let out = run(cwd.path(), &["intent", "bogus"], &mock.url(), &[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Unknown command intent bogus"));
    let out = run(cwd.path(), &["help"], &mock.url(), &[]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("INTENT (ROOTS)"));
    for name in NAMES {
        assert_eq!(
            text.split_whitespace()
                .filter(|word| *word == format!("intent-{name}"))
                .count(),
            1,
            "{text}"
        );
    }
    assert!(mock.take().is_empty());
    assert_eq!(std::fs::read_dir(cwd.path()).unwrap().count(), 0);
}
#[test]
fn grouped_and_flat_commands_match_every_catalog_route() {
    let catalog: Value =
        serde_json::from_str(include_str!("../../../schemas/capability-catalog.json")).unwrap();
    let commands: Value = serde_json::from_str(include_str!("../commands.json")).unwrap();
    let entries = catalog["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["id"].as_str().unwrap().starts_with("intent."))
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 8);
    let cwd = tempfile::tempdir().unwrap();
    let mock = support::serve(|_| {
        support::json(json!({"settings":{"project":"walkthrough","tier":1},"nodes":[],"edges":[]}))
    });
    for entry in entries {
        let name = entry["tui"].as_str().unwrap();
        let spec = commands["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap();
        let args = spec["example"]["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect::<Vec<_>>();
        for mut argv in [
            vec![name],
            vec!["intent", name.strip_prefix("intent-").unwrap()],
        ] {
            argv.extend(&args);
            argv.push("--json");
            let out = run(cwd.path(), &argv, &mock.url(), &[]);
            assert!(
                out.status.success(),
                "{argv:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let reqs = mock.take();
            assert_eq!(reqs.len(), 1);
            let req = &reqs[0];
            assert_eq!(req.method, entry["method"].as_str().unwrap());
            let route = entry["route"]
                .as_str()
                .unwrap()
                .replace("{id}", "walkthrough")
                .replace("{proposal_id}", "p-1")
                .replace("{sprout_id}", "r-1");
            assert_eq!(req.path, route);
            assert_eq!(
                req.header("authorization"),
                Some(format!("Bearer {TOKEN}").as_str())
            );
        }
    }
    assert_eq!(std::fs::read_dir(cwd.path()).unwrap().count(), 0);
}
#[test]
fn human_commands_default_to_developer_and_refuse_agent_identity_before_sending() {
    let cwd = tempfile::tempdir().unwrap();
    let mock = support::serve(|_| support::json(json!({})));
    for verb in ["init", "accept", "reject", "reject-sprout"] {
        let mut args = vec!["intent", verb, "walkthrough"];
        if verb != "init" {
            args.push(if verb == "reject-sprout" {
                "r-1"
            } else {
                "p-1"
            });
        }
        args.push("--json");
        for (extra, by) in [(vec![], "developer"), (vec!["--by", "alice"], "alice")] {
            let mut argv = args.clone();
            argv.extend(extra);
            let out = run(cwd.path(), &argv, &mock.url(), &[]);
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(mock.take()[0].json()["by"], by);
        }
        for by in ["agent:run-1", "agent:x", "roots", "roots:rule", ""] {
            let mut argv = args.clone();
            argv.extend(["--by", by]);
            let out = run(cwd.path(), &argv, &mock.url(), &[]);
            assert!(!out.status.success());
            assert!(mock.take().is_empty());
        }
    }
    assert_eq!(std::fs::read_dir(cwd.path()).unwrap().count(), 0);
}
#[test]
fn checkout_and_environment_cannot_supply_the_human_name() {
    let cwd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let global = home.path().join("empty-gitconfig");
    std::fs::write(&global, "").unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Mallory Gitname"],
    ] {
        let mut c = Command::new("git");
        c.env_clear()
            .env("HOME", home.path())
            .env("GIT_CONFIG_GLOBAL", &global)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .arg("-C")
            .arg(cwd.path())
            .args(args);
        for name in ["PATH", "SystemRoot", "WINDIR", "PATHEXT"] {
            if let Some(value) = std::env::var_os(name) {
                c.env(name, value);
            }
        }
        assert!(c.output().unwrap().status.success());
    }
    let planted = cwd.path().join(".luuna/roots");
    std::fs::create_dir_all(planted.join("human")).unwrap();
    std::fs::write(planted.join("config.yaml"), "project: untouched\n").unwrap();
    fn files(p: &Path, base: &Path, out: &mut Vec<(std::path::PathBuf, Vec<u8>)>) {
        for e in std::fs::read_dir(p).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                files(&p, base, out)
            } else {
                out.push((
                    p.strip_prefix(base).unwrap().into(),
                    std::fs::read(&p).unwrap(),
                ))
            }
        }
    }
    let mut before = vec![];
    files(cwd.path(), cwd.path(), &mut before);
    before.sort();
    let mock = support::serve(|_| support::json(json!({})));
    let env = [
        ("ROOTS_USER", "mallory"),
        ("ROOTS_AGENT", "agent:intruder"),
        ("LUUNA_ROOTS_USER", "human:mallory"),
        ("LUUNA_ROOTS_AGENT", "agent:intruder"),
        ("USER", "mallory"),
        ("USERNAME", "mallory"),
        ("LOGNAME", "mallory"),
    ];
    for verb in ["init", "accept", "reject", "reject-sprout"] {
        let mut args = vec!["intent", verb, "walkthrough"];
        if verb != "init" {
            args.push("p-1");
        }
        args.push("--json");
        let out = run(cwd.path(), &args, &mock.url(), &env);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(mock.take()[0].json()["by"], "developer");
    }
    let mut after = vec![];
    files(cwd.path(), cwd.path(), &mut after);
    after.sort();
    assert_eq!(before, after);
}

#[test]
fn command_help_is_identical_for_flat_and_grouped_names_without_a_token() {
    let cwd = tempfile::tempdir().unwrap();
    let mock = support::serve(|_| support::json(json!({})));
    for verb in NAMES {
        let flat = format!("intent-{verb}");
        let expected = run(
            cwd.path(),
            &[&flat, "--help"],
            &mock.url(),
            &[("LUUNA_API_TOKEN", "")],
        );
        assert!(expected.status.success());
        for args in [vec!["help", "intent", verb], vec!["intent", verb, "--help"]] {
            let out = run(cwd.path(), &args, &mock.url(), &[("LUUNA_API_TOKEN", "")]);
            assert!(out.status.success());
            assert_eq!(out.stdout, expected.stdout);
            assert_eq!(out.stderr, expected.stderr);
        }
    }
    assert!(mock.take().is_empty());
    assert_eq!(std::fs::read_dir(cwd.path()).unwrap().count(), 0);
}
