use luuna_components::{ComponentError, Components};

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// The complete native component workflow runs without installed source packages.
#[tokio::test]
async fn actual_cli_workflows_and_large_private_arguments() {
    let runner = Components::discover().expect("native components are compiled in");
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().canonicalize().unwrap();
    for name in ["luuna-missions", "luuna-atlas", "luuna-beacon"] {
        runner
            .call(name, &workspace, &strings(&["init"]))
            .await
            .unwrap_or_else(|error| panic!("{name} init failed: {error}"));
    }
    let title = "Real Missions ticket";
    let description = "private content beyond the Windows command line limit ".repeat(1000);
    let created = runner
        .call(
            "missions",
            &workspace,
            &[
                "create".into(),
                "--title".into(),
                title.into(),
                "--description".into(),
                description.clone(),
            ],
        )
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();
    let shown = runner
        .call("missions", &workspace, &strings(&["show", id]))
        .await
        .unwrap();
    assert!(shown.to_string().contains(title));
    assert!(shown.to_string().contains(&description));
    let empty = runner
        .call(
            "atlas",
            &workspace,
            &strings(&["search", "--type", "convention"]),
        )
        .await
        .unwrap();
    assert_eq!(empty["total"], 0);
    let envelope = serde_json::json!({"id":"mem-fixture","scope":"project",
        "content":"Verified actual CLI record","provenance":"Synthetic fixture"})
    .to_string();
    let memory = runner
        .call(
            "atlas",
            &workspace,
            &strings(&[
                "record",
                "luuna-project",
                &envelope,
                "--type",
                "convention",
                "--tags",
                "mem-fixture",
            ]),
        )
        .await
        .unwrap();
    let record_id = memory["record"]["id"].as_str().unwrap();
    let memory = runner
        .call("atlas", &workspace, &strings(&["show", record_id]))
        .await
        .unwrap();
    assert_eq!(memory["records"][0]["record"]["content"], envelope);
    let listed = runner
        .call(
            "atlas",
            &workspace,
            &strings(&["search", "--type", "convention"]),
        )
        .await
        .unwrap();
    assert_eq!(listed["total"], 1);
    assert_eq!(listed["domains"][0]["domain"], "luuna-project");
    assert_eq!(listed["domains"][0]["matches"][0]["content"], envelope);
    runner
        .call(
            "atlas",
            &workspace,
            &strings(&["delete", "luuna-project", record_id]),
        )
        .await
        .unwrap();
    let search = runner
        .call(
            "atlas",
            &workspace,
            &strings(&["search", "--type", "convention"]),
        )
        .await
        .unwrap();
    assert_eq!(search["total"], 0);
    runner
        .call(
            "beacon",
            &workspace,
            &strings(&[
                "create",
                "--name",
                "integration",
                "--section",
                "purpose=Original version",
            ]),
        )
        .await
        .unwrap();
    let first = runner
        .call("beacon", &workspace, &strings(&["render", "integration@1"]))
        .await
        .unwrap();
    runner
        .call(
            "beacon",
            &workspace,
            &strings(&["update", "integration", "--section", "purpose=New version"]),
        )
        .await
        .unwrap();
    let old = runner
        .call("beacon", &workspace, &strings(&["render", "integration@1"]))
        .await;
    let current = runner
        .call("beacon", &workspace, &strings(&["render", "integration"]))
        .await
        .unwrap();
    assert_eq!(first["version"], 1);
    assert!(matches!(old, Err(ComponentError::Refused)));
    assert_eq!(current["version"], 2);
    assert_eq!(current["sections"][0]["body"], "New version");
    runner
        .call(
            "beacon",
            &workspace,
            &strings(&[
                "schema",
                "create",
                "--name",
                "integration-schema",
                "--required",
                "purpose",
            ]),
        )
        .await
        .unwrap();
    runner
        .call(
            "beacon",
            &workspace,
            &strings(&["update", "integration", "--schema", "integration-schema"]),
        )
        .await
        .unwrap();
    let validated = runner
        .call("beacon", &workspace, &strings(&["validate", "integration"]))
        .await
        .unwrap();
    assert_eq!(validated["valid"], true);
    let schema = runner
        .call(
            "beacon",
            &workspace,
            &strings(&["schema", "show", "integration-schema"]),
        )
        .await
        .unwrap();
    assert_eq!(schema["schema"]["requiredSections"][0], "purpose");
    runner
        .call(
            "beacon",
            &workspace,
            &strings(&[
                "schema",
                "rule",
                "add",
                "integration-schema",
                "--section",
                "purpose",
                "--pattern",
                "^required unmatched text$",
                "--message",
                "synthetic schema rule",
            ]),
        )
        .await
        .unwrap();
    let invalid_schema = runner
        .call("beacon", &workspace, &strings(&["validate", "integration"]))
        .await;
    assert!(matches!(invalid_schema, Err(ComponentError::Refused)));
    // Render itself does not apply schemas; the runtime must validate before use.
    let unvalidated = runner
        .call("beacon", &workspace, &strings(&["render", "integration"]))
        .await
        .unwrap();
    assert_eq!(unvalidated["sections"][0]["body"], "New version");
    let invalid = runner
        .call("not-a-component", &workspace, &strings(&["list"]))
        .await;
    assert!(matches!(invalid, Err(ComponentError::Identity)));
    // A component-configured shell command is not authorized by a memory write.
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(workspace.join(".luuna/atlas/config.yaml"))
        .unwrap()
        .write_all(b"\nhooks:\n  pre-record: [echo synthetic-hook]\n")
        .unwrap();
    let guarded = runner
        .call(
            "atlas",
            &workspace,
            &strings(&[
                "record",
                "integration",
                "Should refuse configured hooks",
                "--type",
                "convention",
            ]),
        )
        .await;
    assert!(matches!(guarded, Err(ComponentError::Hooks)));
}
