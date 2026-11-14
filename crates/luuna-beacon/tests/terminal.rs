//! Additive comparisons of the production terminal frontend, not a test dispatcher.
mod support;
use luuna_beacon::cli;
use std::{
    collections::BTreeMap,
    process::{Command, Stdio},
};
use support::{go, normalise::Normaliser};
#[test]
fn terminal_help_and_version_need_no_store_or_external_tools() {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec![],
        vec!["--help"],
        vec!["--version"],
        vec!["show", "--help"],
        vec!["--quiet", "--version"],
    ] {
        let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
        let out = cli::run(dir.path(), &args, &BTreeMap::new());
        assert_eq!(out.exit_code, 0);
        assert!(!out.stdout.is_empty());
        assert!(out.stderr.is_empty());
        assert!(!dir.path().join(".luuna").exists());
    }
}
#[test]
#[ignore = "requires the pinned Go reference; production terminal comparison"]
fn terminal_command_contract_matches_go() {
    let build = go::go_build();
    let cases: Vec<Vec<&str>> = vec![
        vec![],
        vec!["--help"],
        vec!["--version"],
        vec!["create", "--help"],
        vec!["what"],
        vec!["config", "schema"],
        vec!["show"],
        vec!["create"],
        vec!["schema"],
        vec!["skill"],
        vec!["init"],
        vec!["init"],
        vec!["list"],
        vec!["stats"],
        vec![
            "create",
            "--name",
            "base",
            "--section",
            "Role=hello <world>",
            "--fm",
            "purpose=compose",
            "--tag",
            "shared",
        ],
        vec![
            "create",
            "--name",
            "child",
            "--extends",
            "base",
            "--section",
            "Body",
            "--body",
            "body text",
        ],
        vec!["list"],
        vec!["list", "--extends", "base"],
        vec!["list", "--tag", "shared"],
        vec!["show", "base"],
        vec!["render", "child"],
        vec!["render", "child", "--format", "json"],
        vec!["tree", "base"],
        vec![
            "update",
            "base",
            "--description",
            "updated",
            "--section",
            "Role=new",
            "--add-section",
            "Other=second",
            "--tag",
            "new",
            "--untag",
            "shared",
            "--fm",
            "more=<&",
        ],
        vec!["history", "base"],
        vec!["history", "base", "--limit", "1tail"],
        vec!["history", "base", "--limit", "bad"],
        vec!["diff", "base", "1", "2"],
        vec!["pin", "base@1"],
        vec!["render", "base"],
        vec!["unpin", "base"],
        vec![
            "schema",
            "create",
            "--name",
            "strict",
            "--required",
            "Role,Missing",
            "--optional",
            ",Other,",
        ],
        vec!["schema", "list"],
        vec!["schema", "show", "strict"],
        vec![
            "schema",
            "rule",
            "add",
            "strict",
            "--section",
            "Role",
            "--pattern",
            "^yes$",
            "--message",
            "role mismatch",
        ],
        vec!["update", "base", "--schema", "strict"],
        vec!["validate", "base"],
        vec!["validate", "--all"],
        vec![
            "update",
            "base",
            "--section",
            "Role=yes",
            "--add-section",
            "Missing=ok",
        ],
        vec!["validate", "base"],
        vec!["emit", "--all", "--dry-run"],
        vec!["emit", "--all"],
        vec!["emit", "--check"],
        vec!["emit", "base"],
        vec!["config", "show"],
        vec!["config", "show", "--path", "project"],
        vec!["config", "set", "project", "terminal"],
        vec![
            "config",
            "set",
            "targets.extra",
            "{dir: extra, default: false}",
        ],
        vec!["config", "unset", "targets.extra"],
        vec!["config", "unset", "absent"],
        vec!["config", "set", "project", "luuna-beacon"],
        vec!["prime"],
        vec!["prime", "--compact"],
        vec!["prime", "--export"],
        vec!["onboard", "--stdout"],
        vec!["onboard", "--check"],
        vec!["onboard"],
        vec!["onboard"],
        vec!["onboard", "--check"],
        vec!["doctor"],
        vec!["import", "input.md", "--name", "imported"],
        vec!["show", "imported"],
        vec!["archive", "child"],
        vec!["list", "--status", "archived"],
        vec!["stats"],
        vec!["skill", "import", "skill", "--name", "demo"],
        vec!["skill", "list"],
        vec!["skill", "show", "demo"],
        vec!["skill", "read", "demo", "SKILL.md"],
        vec!["skill", "verify"],
        vec!["skill", "history", "demo"],
        vec!["skill", "export", "demo", "--out", "exported"],
        vec!["agent", "import", "agent.md", "--name", "helper"],
        vec!["agent", "read", "helper", "agent.md"],
        vec!["agent", "export", "helper", "--out", "helper.md"],
        vec!["bundle", "import", "bundle", "--name", "kit"],
        vec!["bundle", "list"],
        vec!["bundle", "verify"],
        vec!["completions", "bash"],
        vec!["completions", "zsh"],
        vec!["completions", "fish"],
        vec!["completions", "unknown"],
        vec!["upgrade"],
        vec!["sync", "--status"],
    ];
    for json in [false, true] {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        for root in [a.path(), b.path()] {
            std::fs::write(root.join("input.md"), "## Imported\nbody\n").unwrap();
            std::fs::create_dir(root.join("skill")).unwrap();
            std::fs::write(root.join("skill/SKILL.md"), "# Skill\n").unwrap();
            std::fs::write(root.join("agent.md"), "# Agent\n").unwrap();
            std::fs::create_dir_all(root.join("bundle/.claude-plugin")).unwrap();
            std::fs::write(root.join("bundle/.claude-plugin/plugin.json"), "{}\n").unwrap();
        }
        let mut ga = Normaliser::new("luuna-beacon", a.path());
        let mut nb = Normaliser::new("luuna-beacon", b.path());
        for args in &cases {
            let mut args = args.iter().map(|s| s.to_string()).collect::<Vec<_>>();
            if json {
                args.push("--json".into());
            }
            let go = Command::new(&build.binary)
                .args(&args)
                .current_dir(a.path())
                .env_clear()
                .stdin(Stdio::null())
                .output()
                .unwrap();
            let native = cli::run(b.path(), &args, &BTreeMap::new());
            assert_eq!(
                native.exit_code,
                go.status.code().unwrap_or(-1),
                "{args:?}: {}",
                String::from_utf8_lossy(&native.stderr)
            );
            let parse = |bytes: &[u8]| -> Option<Vec<serde_json::Value>> {
                serde_json::Deserializer::from_slice(bytes)
                    .into_iter()
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .ok()
                    .filter(|v| !v.is_empty())
            };
            match (parse(&go.stdout), parse(&native.stdout)) {
                (Some(mut left), Some(mut right)) => {
                    ga.documents(&mut left);
                    nb.documents(&mut right);
                    assert_eq!(left, right, "{args:?}");
                }
                _ => assert_eq!(
                    ga.text(&String::from_utf8_lossy(&go.stdout)),
                    nb.text(&String::from_utf8_lossy(&native.stdout)),
                    "stdout {args:?}"
                ),
            }
            assert_eq!(
                ga.text(&String::from_utf8_lossy(&go.stderr)),
                nb.text(&String::from_utf8_lossy(&native.stderr)),
                "stderr {args:?}"
            );
        }
    }
}
