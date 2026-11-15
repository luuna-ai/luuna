#[cfg(unix)]
#[tokio::test]
async fn managed_sync_preserves_raw_combined_git_diagnostics() {
    use std::{collections::BTreeMap, os::unix::fs::PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let strings = |args: &[&str]| args.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    luuna_components::run_beacon_native(
        root.path(),
        &strings(&["init"]),
        BTreeMap::new(),
        Vec::new(),
    )
    .await
    .unwrap();
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let git = bin.join("git");
    std::fs::write(
        &git,
        "#!/bin/sh\nprintf A\nprintf B >&2\nprintf C\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
    let env = BTreeMap::from([("PATH".into(), bin.to_string_lossy().into_owned())]);
    let args = strings(&["sync", "--status", "--json"]);
    let raw = luuna_beacon::cli::run(root.path(), &args, &env);
    assert!(String::from_utf8_lossy(&raw.stdout).contains("git status: ABC"));
    let managed = luuna_components::run_beacon_native(root.path(), &args, env, Vec::new())
        .await
        .unwrap();
    assert_eq!(managed, raw);
}
