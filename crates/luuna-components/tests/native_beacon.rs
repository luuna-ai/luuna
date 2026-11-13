use luuna_components::{call_beacon_native, ComponentError};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::Path};
fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).into()).collect()
}
fn fixture() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("native-beacon-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap()
}
async fn call(root: &Path, values: &[&str]) -> Value {
    call_beacon_native(root, &args(values)).await.unwrap()
}
fn digest(path: &Path) -> String {
    let mut f = fs::File::open(path).unwrap();
    let mut digest = Sha256::new();
    let mut bytes = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut bytes).unwrap();
        if n == 0 {
            break;
        }
        digest.update(&bytes[..n]);
    }
    format!("{:x}", digest.finalize())
}

#[test]
fn native_workflows_need_no_toolchain_or_checkout() {
    if let Some(root) = std::env::var_os("LUUNA_NATIVE_BEACON_CHILD") {
        let root = std::path::PathBuf::from(root);
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStrExt;
                    let file = root.join(".gitattributes");
                    let path = std::ffi::CString::new(file.as_os_str().as_bytes()).unwrap();
                    // The parent deadline also bounds the initialization-only input.
                    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
                    assert!(matches!(
                        call_beacon_native(&root, &args(&["init"])).await,
                        Err(ComponentError::Workspace)
                    ));
                    fs::remove_file(file).unwrap();
                }
                call(&root, &["init"]).await;
                let body = "literal shell-looking $(unused) ${unused} and private long content "
                    .repeat(1000);
                let section = format!("purpose={body}");
                let created = call(
                    &root,
                    &[
                        "create",
                        "--name",
                        "p",
                        "--section",
                        &section,
                        "--fm",
                        "request=synthetic",
                    ],
                )
                .await;
                assert_eq!(created["version"], 1);
                let list = call(&root, &["list"]).await;
                assert_eq!(list["count"], 1);
                assert_eq!(list["prompts"][0]["frontmatter"]["request"], "synthetic");
                let rendered = call(&root, &["render", "p@1"]).await;
                assert!(
                    rendered["sections"][0]["body"] == body,
                    "native body must be literal and complete"
                );
                assert_eq!(rendered["resolvedFrom"], json!(["p"]));
                call(
                    &root,
                    &["schema", "create", "--name", "s", "--required", "purpose"],
                )
                .await;
                call(
                    &root,
                    &[
                        "update",
                        "p",
                        "--section",
                        "purpose=new body",
                        "--schema",
                        "s",
                    ],
                )
                .await;
                assert!(matches!(
                    call_beacon_native(&root, &args(&["render", "p@1"])).await,
                    Err(ComponentError::Refused)
                ));
                assert_eq!(call(&root, &["render", "p"]).await["version"], 2);
                assert_eq!(
                    call(&root, &["schema", "show", "s"]).await["schema"]["requiredSections"],
                    json!(["purpose"])
                );
                let validation = call(&root, &["validate", "p"]).await;
                assert_eq!(validation["valid"], true);
                assert_eq!(validation["warnings"], json!([]));
                call(
                    &root,
                    &[
                        "schema",
                        "rule",
                        "add",
                        "s",
                        "--section",
                        "purpose",
                        "--pattern",
                        "impossible-pattern",
                        "--message",
                        "synthetic rule",
                    ],
                )
                .await;
                assert!(matches!(
                    call_beacon_native(&root, &args(&["validate", "p"])).await,
                    Err(ComponentError::Refused)
                ));
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStrExt;
                    let file = root.join(".luuna/beacon/prompts.jsonl");
                    fs::remove_file(&file).unwrap();
                    let path = std::ffi::CString::new(file.as_os_str().as_bytes()).unwrap();
                    // Safe pathname lifetime and mode; the parent kills this child
                    // if opening an unexpected FIFO ever blocks the native reader.
                    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
                    assert!(matches!(
                        call_beacon_native(&root, &args(&["list"])).await,
                        Err(ComponentError::Workspace)
                    ));
                }
            });
        return;
    }
    let dir = fixture();
    let root = dir.path().canonicalize().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "native_workflows_need_no_toolchain_or_checkout",
            "--nocapture",
        ])
        .env_clear()
        .env("PATH", "")
        .env("HOME", &root)
        .env("LUUNA_NATIVE_BEACON_CHILD", &root)
        .current_dir(&root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if start.elapsed() > std::time::Duration::from_secs(15) {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("native workflow child exceeded its hard deadline")
        }
        std::thread::sleep(std::time::Duration::from_millis(10))
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "native child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!root.join("components").exists());
    assert!(!root.join("unused").exists());
}

#[tokio::test]
async fn unsupported_arguments_create_nothing() {
    let dir = fixture();
    let root = dir.path().canonicalize().unwrap();
    for values in [
        vec!["sync"],
        vec!["create", "--unknown", "value"],
        vec!["schema", "rule", "add"],
    ] {
        assert!(matches!(
            call_beacon_native(&root, &args(&values)).await,
            Err(ComponentError::Protocol)
        ));
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0)
    }
}

#[tokio::test]
async fn deep_records_are_refused_before_a_write() {
    let dir = fixture();
    let root = dir.path().canonicalize().unwrap();
    call(&root, &["init"]).await;
    let source=format!("{{\"id\":\"p\",\"name\":\"p\",\"version\":1,\"status\":\"active\",\"frontmatter\":{{\"k\":{}0{}}}}}\n","[".repeat(256),"]".repeat(256));
    let path = root.join(".luuna/beacon/prompts.jsonl");
    fs::write(&path, &source).unwrap();
    assert!(matches!(
        call_beacon_native(&root, &args(&["update", "p", "--section", "purpose=new"])).await,
        Err(ComponentError::Protocol)
    ));
    assert_eq!(fs::read_to_string(path).unwrap(), source)
}

#[tokio::test]
async fn oversized_store_is_refused_without_reading_or_rewriting_it() {
    let dir = fixture();
    let root = dir.path().canonicalize().unwrap();
    call(&root, &["init"]).await;
    let path = root.join(".luuna/beacon/prompts.jsonl");
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    let before = digest(&path);
    assert!(matches!(
        call_beacon_native(&root, &args(&["create", "--name", "p"])).await,
        Err(ComponentError::OutputLimit)
    ));
    assert_eq!(digest(&path), before);
    assert_eq!(fs::metadata(path).unwrap().len(), 64 * 1024 * 1024 + 1)
}

#[tokio::test]
async fn output_limit_never_truncates_or_changes_records() {
    let dir = fixture();
    let root = dir.path().canonicalize().unwrap();
    call(&root, &["init"]).await;
    let b = luuna_beacon::Beacon::open(&root).unwrap();
    for name in ["a", "b"] {
        b.create(&luuna_beacon::CreatePrompt {
            name: name.into(),
            sections: vec![luuna_beacon::Section {
                name: "purpose".into(),
                body: "x".repeat(2200000),
                required: None,
            }],
            ..Default::default()
        })
        .unwrap();
    }
    let path = root.join(".luuna/beacon/prompts.jsonl");
    let before = digest(&path);
    assert!(matches!(
        call_beacon_native(&root, &args(&["list"])).await,
        Err(ComponentError::OutputLimit)
    ));
    assert_eq!(digest(&path), before)
}

#[cfg(unix)]
#[tokio::test]
async fn linked_data_is_refused_before_touching_the_target() {
    let dir = fixture();
    let root = dir.path().canonicalize().unwrap();
    call(&root, &["init"]).await;
    let outside = root.join("outside.jsonl");
    fs::write(&outside, b"synthetic source bytes\n").unwrap();
    let path = root.join(".luuna/beacon/prompts.jsonl");
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&outside, &path).unwrap();
    assert!(matches!(
        call_beacon_native(&root, &args(&["create", "--name", "p"])).await,
        Err(ComponentError::Workspace)
    ));
    assert_eq!(fs::read(&outside).unwrap(), b"synthetic source bytes\n")
}

#[tokio::test]
async fn native_version_overflow_preserves_go_behavior_and_other_stores() {
    let dir = fixture();
    let root = dir.path().canonicalize().unwrap();
    call(&root, &["init"]).await;
    let path = root.join(".luuna/beacon/prompts.jsonl");
    let source="{\"id\":\"p\",\"name\":\"p\",\"version\":9223372036854775807,\"sections\":[],\"status\":\"active\"}\n";
    fs::write(&path, source).unwrap();
    let updated = call_beacon_native(&root, &args(&["update", "p", "--section", "purpose=new"]))
        .await
        .unwrap();
    assert_eq!(updated["version"].as_i64(), Some(i64::MIN));
    let bytes = fs::read_to_string(path).unwrap();
    assert!(bytes.starts_with(source));
    assert_eq!(bytes.lines().count(), 2);
    let other = fixture();
    let other = other.path().canonicalize().unwrap();
    assert_eq!(call(&other, &["init"]).await["success"], true);
    assert_eq!(call(&other, &["list"]).await["count"], 0);
}

#[tokio::test]
async fn many_small_records_are_bounded_before_model_allocation() {
    let dir = fixture();
    let root = dir.path().canonicalize().unwrap();
    call(&root, &["init"]).await;
    let path = root.join(".luuna/beacon/prompts.jsonl");
    let source = "null\n".repeat(100_001);
    fs::write(&path, &source).unwrap();
    assert!(matches!(
        call_beacon_native(&root, &args(&["create", "--name", "p"])).await,
        Err(ComponentError::OutputLimit)
    ));
    assert!(
        fs::read_to_string(path).unwrap() == source,
        "record count refusal changed source bytes"
    )
}
#[tokio::test]
async fn an_oversized_record_is_refused_before_any_update() {
    let dir = fixture();
    let root = dir.path().canonicalize().unwrap();
    call(&root, &["init"]).await;
    let path = root.join(".luuna/beacon/prompts.jsonl");
    let source=format!("{{\"id\":\"p\",\"name\":\"p\",\"version\":1,\"status\":\"active\",\"description\":\"{}\"}}\n","x".repeat(4*1024*1024));
    fs::write(&path, &source).unwrap();
    let before = digest(&path);
    assert!(matches!(
        call_beacon_native(&root, &args(&["update", "p", "--description", "changed"])).await,
        Err(ComponentError::OutputLimit)
    ));
    assert_eq!(digest(&path), before)
}
