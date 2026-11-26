//! Migration step 6: `<group> <verb>` is the same command as its flat name. The client
//! runs only against a loopback mock, with a private temporary HOME, config and cwd.
mod support;
use serde_json::{json, Value};
use std::{
    path::Path,
    process::{Command, Output, Stdio},
};
const TOKEN: &str = "synthetic-groups-token-at-least-32-bytes";
/// Mirrors `help::COMMAND_GROUPS`; a change there must be made here on purpose.
const GROUPS: [(&str, &[(&str, &str)]); 5] = [
    (
        "ticket",
        &[
            ("list", "tickets"),
            ("create", "create-ticket"),
            ("show", "show-ticket"),
            ("edit", "edit-ticket"),
            ("source", "ticket-source"),
            ("reconcile", "reconcile-ticket"),
        ],
    ),
    (
        "plan",
        &[
            ("create", "plan"),
            ("list", "plans"),
            ("show", "show-plan"),
            ("approve", "approve"),
        ],
    ),
    (
        "run",
        &[
            ("list", "runs"),
            ("show", "run"),
            ("events", "events"),
            ("follow", "follow"),
            ("implement", "implement"),
            ("candidate", "candidate"),
            ("diff", "diff"),
            ("validate", "validate"),
            ("repair", "repair"),
            ("tasks", "tasks"),
            ("attempts", "attempts"),
            ("worktrees", "worktrees"),
            ("artifact", "artifact"),
            ("cancel", "cancel"),
        ],
    ),
    (
        "memory",
        &[
            ("list", "memory"),
            ("add", "remember"),
            ("correct", "correct-memory"),
            ("promote", "promote"),
            ("forget", "forget"),
            ("retention", "retention"),
            ("set-retention", "set-retention"),
            ("apply-retention", "apply-retention"),
            ("export", "export-user-memory"),
        ],
    ),
    (
        "prompt",
        &[
            ("list", "prompts"),
            ("save", "save-prompt"),
            ("render", "render-prompt"),
        ],
    ),
];
fn run(cwd: &Path, args: &[&str], url: &str, token: &str) -> Output {
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
        .env("LUUNA_API_TOKEN", token)
        .stdin(Stdio::null());
    for name in ["PATH", "SystemRoot", "WINDIR", "PATHEXT"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command.output().unwrap()
}
fn example(name: &str) -> Vec<String> {
    let commands: Value = serde_json::from_str(include_str!("../commands.json")).unwrap();
    let spec = commands["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("{name} is not a command"));
    spec["example"]["args"]
        .as_array()
        .map(|args| {
            args.iter()
                .map(|a| a.as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default()
}
fn strs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}
#[test]
fn every_group_verb_sends_exactly_what_its_flat_command_sends() {
    let cwd = tempfile::tempdir().unwrap();
    let mock = support::serve(|_| support::json(json!({})));
    for (group, verbs) in GROUPS {
        for (verb, flat) in verbs {
            // `follow` streams until the run ends; its page is compared below.
            if *verb == "follow" {
                continue;
            }
            let args = example(flat);
            let mut flat_argv = vec![flat.to_string()];
            flat_argv.extend(args.iter().cloned());
            flat_argv.push("--json".into());
            let mut grouped_argv = vec![group.to_string(), verb.to_string()];
            grouped_argv.extend(args.iter().cloned());
            grouped_argv.push("--json".into());
            let expected = run(cwd.path(), &strs(&flat_argv), &mock.url(), TOKEN);
            let sent = mock.take();
            let out = run(cwd.path(), &strs(&grouped_argv), &mock.url(), TOKEN);
            let grouped = mock.take();
            assert_eq!(
                out.status.code(),
                expected.status.code(),
                "{grouped_argv:?}"
            );
            assert_eq!(out.stdout, expected.stdout, "{grouped_argv:?}");
            assert_eq!(grouped.len(), sent.len(), "{grouped_argv:?}");
            for (a, b) in grouped.iter().zip(&sent) {
                assert_eq!(
                    (&a.method, &a.path, &a.query),
                    (&b.method, &b.path, &b.query),
                    "{grouped_argv:?}"
                );
            }
        }
    }
    assert_eq!(std::fs::read_dir(cwd.path()).unwrap().count(), 0);
}
#[test]
fn command_help_is_identical_for_flat_and_grouped_names_without_a_token() {
    let cwd = tempfile::tempdir().unwrap();
    let mock = support::serve(|_| support::json(json!({})));
    for (group, verbs) in GROUPS {
        for (verb, flat) in verbs {
            let expected = run(cwd.path(), &[flat, "--help"], &mock.url(), "");
            assert!(expected.status.success(), "{flat}");
            for args in [vec!["help", group, verb], vec![group, verb, "--help"]] {
                let out = run(cwd.path(), &args, &mock.url(), "");
                assert!(out.status.success(), "{args:?}");
                assert_eq!(out.stdout, expected.stdout, "{args:?}");
            }
        }
    }
    assert!(mock.take().is_empty());
}
#[test]
fn group_pages_are_local_and_unknown_verbs_are_usage_errors() {
    let cwd = tempfile::tempdir().unwrap();
    let mock = support::serve(|_| support::json(json!({})));
    for group in ["ticket", "prompt"] {
        let bare = run(cwd.path(), &[group], &mock.url(), "");
        assert_eq!(bare.status.code(), Some(2));
        let page = String::from_utf8(bare.stdout).unwrap();
        for (name, verbs) in GROUPS {
            if name == group {
                for (verb, flat) in verbs {
                    assert!(
                        page.contains(&format!("luuna-tui {group} {verb}")),
                        "{page}"
                    );
                    assert!(page.contains(&format!("same as {flat}\n")), "{page}");
                }
            }
        }
        for args in [vec![group, "--help"], vec!["help", group]] {
            let out = run(cwd.path(), &args, &mock.url(), "");
            assert_eq!(out.status.code(), Some(0), "{args:?}");
            assert_eq!(String::from_utf8(out.stdout).unwrap(), page);
        }
        let unknown = run(cwd.path(), &[group, "bogus"], &mock.url(), "");
        assert_eq!(unknown.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&unknown.stderr).contains(&format!("{group} bogus")));
        let unknown_help = run(cwd.path(), &["help", group, "bogus"], &mock.url(), "");
        assert_eq!(unknown_help.status.code(), Some(2));
    }
    assert!(mock.take().is_empty());
}
#[test]
fn plan_run_and_memory_still_reach_their_flat_commands() {
    let cwd = tempfile::tempdir().unwrap();
    let mock = support::serve(|_| support::json(json!({})));
    for flat in ["plan", "run", "memory"] {
        let mut argv = vec![flat.to_string()];
        argv.extend(example(flat));
        argv.push("--json".into());
        let argv = argv.iter().map(String::as_str).collect::<Vec<_>>();
        let first = run(cwd.path(), &argv, &mock.url(), TOKEN);
        let sent = mock.take();
        assert!(
            !sent.is_empty(),
            "{argv:?} sent nothing: {}",
            String::from_utf8_lossy(&first.stderr)
        );
        let help = run(cwd.path(), &[flat, "--help"], &mock.url(), "");
        let page = String::from_utf8(help.stdout).unwrap();
        assert!(page.starts_with(&format!("luuna-tui {flat} ")), "{page}");
    }
}
