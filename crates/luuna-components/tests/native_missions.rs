use luuna_components::{call_missions_native, run_missions_native, ComponentError};
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::Path};
fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).into()).collect()
}
async fn call(root: &Path, values: &[&str]) -> Value {
    call_missions_native(root, &args(values)).await.unwrap()
}
#[tokio::test]
async fn tickets_are_native_and_preserve_raw_command_contract() {
    let temp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let root = temp.path();
    call(root, &["init"]).await;
    let created = call(
        root,
        &[
            "create",
            "--title=Native ticket",
            "--description=literal $(unused)",
        ],
    )
    .await;
    let id = created["id"].as_str().unwrap();
    assert_eq!(
        call(root, &["show", id]).await["issue"]["description"],
        "literal $(unused)"
    );
    call(root, &["update", id, "--status=in_progress"]).await;
    assert_eq!(
        call(root, &["list", "--all"]).await["issues"][0]["status"],
        "in_progress"
    );
    let raw = run_missions_native(
        root,
        &args(&["close", id, "--json"]),
        BTreeMap::new(),
        Vec::new(),
    )
    .await
    .unwrap();
    assert_eq!(raw.exit_code, 0);
    assert_eq!(call(root, &["show", id]).await["issue"]["status"], "closed");
    let template = run_missions_native(
        root,
        &args(&["plan", "templates", "--json"]),
        BTreeMap::new(),
        Vec::new(),
    )
    .await
    .unwrap();
    assert_eq!(template.exit_code, 0);
    let prime = run_missions_native(
        root,
        &args(&["prime", "--compact"]),
        BTreeMap::new(),
        Vec::new(),
    )
    .await
    .unwrap();
    assert_eq!(prime.exit_code, 0);
    assert!(!prime.stdout.is_empty());
}
#[tokio::test]
async fn managed_refusals_leave_store_untouched() {
    let temp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let root = temp.path();
    call(root, &["init"]).await;
    let file = root.join(".luuna/missions/issues.jsonl");
    for bytes in [
        b"{bad json}\n".to_vec(),
        format!("{}0{}\n", "[".repeat(150), "]".repeat(150)).into_bytes(),
    ] {
        fs::write(&file, &bytes).unwrap();
        assert!(call_missions_native(root, &args(&["create", "--title=x"]))
            .await
            .is_err());
        assert_eq!(fs::read(&file).unwrap(), bytes);
    }
    fs::write(&file, b"").unwrap();
    assert!(matches!(
        call_missions_native(root, &args(&["sync"])).await,
        Err(ComponentError::Protocol)
    ));
    assert!(fs::read(&file).unwrap().is_empty());
    let f = fs::File::create(&file).unwrap();
    f.set_len(64 * 1024 * 1024 + 1).unwrap();
    assert!(matches!(
        call_missions_native(root, &args(&["list"])).await,
        Err(ComponentError::OutputLimit)
    ));
    assert_eq!(f.metadata().unwrap().len(), 64 * 1024 * 1024 + 1);
}
#[cfg(unix)]
#[tokio::test]
async fn managed_links_are_refused() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    call(temp.path(), &["init"]).await;
    let file = temp.path().join(".luuna/missions/issues.jsonl");
    let outside = temp.path().join("outside");
    fs::write(&outside, b"original").unwrap();
    fs::remove_file(&file).unwrap();
    symlink(&outside, &file).unwrap();
    assert!(
        call_missions_native(temp.path(), &args(&["create", "--title=x"]))
            .await
            .is_err()
    );
    assert_eq!(fs::read(outside).unwrap(), b"original");
}
