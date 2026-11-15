use luuna_components::{call_atlas_native, run_atlas_native, ComponentError};
use std::collections::BTreeMap;
fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}
#[tokio::test]
async fn atlas_memory_workflow_is_native_and_lossless() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    call_atlas_native(root, &args(&["init"])).await.unwrap();
    let created = call_atlas_native(
        root,
        &args(&[
            "record",
            "memory",
            "Keep explicit approvals",
            "--type",
            "convention",
            "--classification",
            "foundational",
            "--force",
        ]),
    )
    .await
    .unwrap();
    let id = created["record"]["id"].as_str().unwrap();
    let shown = call_atlas_native(root, &args(&["show", id])).await.unwrap();
    assert_eq!(
        shown["records"][0]["record"]["content"],
        "Keep explicit approvals"
    );
    let found = call_atlas_native(root, &args(&["search", "--type", "convention"]))
        .await
        .unwrap();
    assert_eq!(found["total"], 1);
    call_atlas_native(
        root,
        &args(&["edit", "memory", id, "--content", "Keep reviewed approvals"]),
    )
    .await
    .unwrap();
    call_atlas_native(root, &args(&["delete", "memory", id]))
        .await
        .unwrap();
    assert_eq!(
        call_atlas_native(root, &args(&["search", "--type", "convention"]))
            .await
            .unwrap()["total"],
        0
    );
}
#[tokio::test]
async fn managed_hooks_refuse_before_execution_or_record_write() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    call_atlas_native(root, &args(&["init"])).await.unwrap();
    let path = root.join(".luuna/atlas/config.yaml");
    let mut cfg = std::fs::read_to_string(&path).unwrap();
    cfg += "\nhooks:\n  pre-record: ['touch must-not-run']\n";
    std::fs::write(&path, &cfg).unwrap();
    assert!(matches!(
        call_atlas_native(
            root,
            &args(&[
                "record",
                "memory",
                "private content",
                "--type",
                "convention"
            ])
        )
        .await,
        Err(ComponentError::Hooks)
    ));
    assert!(!root.join("must-not-run").exists());
    assert!(!root.join(".luuna/atlas/expertise/memory.jsonl").exists());
    assert_eq!(std::fs::read_to_string(path).unwrap(), cfg);
}
#[tokio::test]
async fn raw_terminal_contract_needs_neither_component_checkout_nor_bun() {
    let d = tempfile::tempdir().unwrap();
    let env = BTreeMap::from([
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("HOME".into(), d.path().to_string_lossy().into_owned()),
        ("LUUNA_BUN".into(), "does-not-exist".into()),
    ]);
    let out = run_atlas_native(
        d.path(),
        &args(&["init", "--json"]),
        env.clone(),
        Vec::new(),
    )
    .await
    .unwrap();
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.contains("\"created\": true"));
    let out = run_atlas_native(d.path(), &args(&["status", "--json"]), env, Vec::new())
        .await
        .unwrap();
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.contains("\"command\": \"status\""));
}

#[cfg(unix)]
#[tokio::test]
async fn terminal_hooks_do_not_receive_runtime_or_provider_bindings() {
    let d = tempfile::tempdir().unwrap();
    call_atlas_native(d.path(), &args(&["init"])).await.unwrap();
    let path = d.path().join(".luuna/atlas/config.yaml");
    let mut config = std::fs::read_to_string(&path).unwrap();
    config.push_str("\nhooks:\n  post-record: ['printf \"%s|%s|%s\" \"$LUUNA_API_TOKEN\" \"$OPENROUTER_API_KEY\" \"$LUUNA_PI_ARGS_JSON\" > observed']\n");
    std::fs::write(path, config).unwrap();
    let env = BTreeMap::from([
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("LUUNA_API_TOKEN".into(), "synthetic-runtime-secret".into()),
        (
            "OPENROUTER_API_KEY".into(),
            "synthetic-provider-secret".into(),
        ),
        (
            "LUUNA_PI_ARGS_JSON".into(),
            "synthetic-private-launcher".into(),
        ),
    ]);
    let out = run_atlas_native(
        d.path(),
        &args(&["record", "privacy", "Safe memory", "--type", "convention"]),
        env,
        Vec::new(),
    )
    .await
    .unwrap();
    assert_eq!(out.exit_code, 0, "{}", out.stderr);
    assert_eq!(
        std::fs::read_to_string(d.path().join("observed")).unwrap(),
        "||"
    );
}

#[tokio::test]
async fn managed_terminal_refuses_raw_ipc_before_file_access() {
    let d = tempfile::tempdir().unwrap();
    let out = run_atlas_native(
        d.path(),
        &args(&["init"]),
        BTreeMap::from([("LUUNA_ATLAS_API_BRIDGE".into(), "1".into())]),
        br#"{"op":"readExpertiseFile","path":"unadmitted.jsonl"}"#.to_vec(),
    )
    .await;
    assert!(matches!(out, Err(ComponentError::Protocol)));
    assert!(!d.path().join(".luuna").exists());
}
