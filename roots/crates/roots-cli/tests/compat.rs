//! Black-box compatibility checks against the language-independent fixtures
//! captured from the TypeScript/Bun reference at `ce22500`.
//!
//! The committed corpus was captured on Windows.  Keep this test Windows-only
//! until a separate Unix fixture baseline exists; silently normalizing path
//! separators would hide a public CLI change.

#![cfg(windows)]

use roots_core::serde_json::{self, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

struct TempProject {
    root: PathBuf,
}

impl TempProject {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "luuna-roots-rust-compat-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create isolated fixture project");
        Self { root }
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        // This is a test-generated, uniquely named directory.  It never
        // points at user/project data.
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Default)]
struct Normalizers {
    root: String,
    project: String,
    sprout_id: Option<String>,
    sprout_hex: Option<String>,
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

fn fixture(path: &str) -> String {
    fs::read_to_string(workspace_root().join("tests").join("compat").join(path))
        .unwrap_or_else(|error| panic!("read fixture {path}: {error}"))
}

fn json_fixture(path: &str) -> String {
    // The JSON corpus stores insertion-ordered records. Reconstruct the
    // reference CLI's `JSON.stringify(value, null, 2)` layout, then compare
    // bytes against the candidate; this is not a loose semantic assertion.
    let value: Value = serde_json::from_str(&fixture(path))
        .unwrap_or_else(|error| panic!("parse JSON fixture {path}: {error}"));
    format!(
        "{}\n",
        serde_json::to_string_pretty(&value).expect("pretty fixture JSON")
    )
}

fn run(project: &Path, args: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_luuna-roots"))
        .current_dir(project)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("run {:?}: {error}", args));
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8(output.stdout).expect("stdout utf-8"),
        String::from_utf8(output.stderr).expect("stderr utf-8"),
    )
}

fn replace_general(value: &str, normalizers: &Normalizers) -> String {
    let mut out = value.replace(&normalizers.root, "<ROOT>");
    if let Some(id) = &normalizers.sprout_id {
        out = out.replace(id, "<SPROUT_ID>");
    }
    if let Some(hex) = &normalizers.sprout_hex {
        out = out.replace(hex, "<SPROUT_HEX>");
    }
    // Root replacement comes first, so this cannot alter the temporary
    // directory path when its basename happens to be the project name.
    out.replace(&normalizers.project, "<PROJECT>")
}

fn normalize_value(value: &mut Value, key: Option<&str>, normalizers: &Normalizers) {
    match value {
        Value::Object(object) => {
            for (name, child) in object {
                normalize_value(child, Some(name), normalizers);
            }
        }
        Value::Array(values) => {
            for child in values {
                normalize_value(child, key, normalizers);
            }
        }
        Value::String(text) => {
            if matches!(key, Some("createdAt") | Some("updatedAt")) {
                *text = "<CREATED_AT>".to_owned();
            } else if key == Some("expiresAt") {
                *text = "<EXPIRES_AT>".to_owned();
            } else {
                *text = replace_general(text, normalizers);
            }
        }
        _ => {}
    }
}

fn normalize_json(text: &str, normalizers: &Normalizers) -> String {
    let mut value: Value = serde_json::from_str(text).expect("candidate JSON output");
    normalize_value(&mut value, None, normalizers);
    format!(
        "{}\n",
        serde_json::to_string_pretty(&value).expect("pretty JSON output")
    )
}

fn assert_json_case(
    project: &Path,
    args: &[&str],
    expected_fixture: &str,
    expected_exit: i32,
    normalizers: &Normalizers,
) -> String {
    let (exit, stdout, stderr) = run(project, args);
    assert_eq!(exit, expected_exit, "exit for {:?}; stderr={stderr}", args);
    assert!(stderr.is_empty(), "stderr for {:?}: {stderr}", args);
    let actual = normalize_json(&stdout, normalizers);
    assert_eq!(
        actual,
        json_fixture(expected_fixture),
        "JSON output for {:?}",
        args
    );
    stdout
}

fn assert_text_case(project: &Path, args: &[&str], expected_fixture: &str, expected_exit: i32) {
    let (exit, stdout, stderr) = run(project, args);
    assert_eq!(exit, expected_exit, "exit for {:?}; stderr={stderr}", args);
    assert!(stderr.is_empty(), "stderr for {:?}: {stderr}", args);
    assert_eq!(
        stdout,
        fixture(expected_fixture),
        "text output for {:?}",
        args
    );
}

#[test]
fn windows_fixture_flow_matches_the_reference_cli() {
    let project = TempProject::new("flow");
    let mut normalizers = Normalizers {
        root: project.root.display().to_string(),
        project: project
            .root
            .file_name()
            .and_then(|part| part.to_str())
            .expect("temporary project basename")
            .to_owned(),
        ..Normalizers::default()
    };

    assert_text_case(&project.root, &["--help"], "root-help.txt", 0);
    assert_json_case(
        &project.root,
        &["--help", "--json"],
        "root-help.json",
        0,
        &normalizers,
    );
    assert_text_case(&project.root, &["--version"], "version.txt", 0);
    assert_json_case(
        &project.root,
        &["--version", "--json"],
        "version.json",
        0,
        &normalizers,
    );
    assert_text_case(&project.root, &["init", "--help"], "help/init.txt", 0);
    assert_text_case(&project.root, &["propose", "--help"], "help/propose.txt", 0);
    assert_text_case(&project.root, &["view", "--help"], "help/view.txt", 0);
    assert_json_case(
        &project.root,
        &["plnt", "--json"],
        "errors/unknown-command.json",
        2,
        &normalizers,
    );
    assert_json_case(
        &project.root,
        &["list", "--json"],
        "errors/not-initialized.json",
        3,
        &normalizers,
    );
    assert_json_case(
        &project.root,
        &["init", "--json"],
        "init/result.json",
        0,
        &normalizers,
    );

    // Capture before comparing: sprout IDs are intentionally generated at
    // runtime, and the first response supplies the normalizers used for its
    // own reference fixture and all following flow assertions.
    let (exit, sprout, stderr) = run(
        &project.root,
        &[
            "sprout",
            "Agent candidate",
            "--as",
            "agent:fixture",
            "--json",
        ],
    );
    assert_eq!(exit, 0, "exit for sprout; stderr={stderr}");
    assert!(stderr.is_empty(), "stderr for sprout: {stderr}");
    let sprout_value: Value = serde_json::from_str(&sprout).expect("sprout JSON");
    let sprout_id = sprout_value["id"].as_str().expect("sprout id").to_owned();
    normalizers.sprout_hex = sprout_id.strip_prefix("s-").map(ToOwned::to_owned);
    normalizers.sprout_id = Some(sprout_id.clone());
    assert_eq!(
        normalize_json(&sprout, &normalizers),
        json_fixture("flow/sprout.json"),
        "JSON output for sprout"
    );

    assert_json_case(
        &project.root,
        &["list", "--json"],
        "flow/list.json",
        0,
        &normalizers,
    );
    assert_json_case(
        &project.root,
        &["show", &sprout_id, "--json"],
        "flow/show.json",
        0,
        &normalizers,
    );
    let (exit, stdout, stderr) = run(&project.root, &["view", "--sprouts"]);
    assert_eq!(exit, 0, "view markdown stderr={stderr}");
    assert!(stderr.is_empty(), "view markdown stderr={stderr}");
    assert_eq!(
        replace_general(&stdout, &normalizers),
        fixture("flow/view-sprouts.md"),
        "view markdown output"
    );
    assert_json_case(
        &project.root,
        &["view", "--sprouts", "--json"],
        "flow/view-sprouts.json",
        0,
        &normalizers,
    );
    assert_json_case(
        &project.root,
        &["prime", "--json"],
        "flow/prime.json",
        0,
        &normalizers,
    );
}

#[test]
fn lrt_is_the_same_native_entrypoint() {
    let project = TempProject::new("alias");
    let output = Command::new(env!("CARGO_BIN_EXE_lrt"))
        .current_dir(&project.root)
        .arg("--version")
        .output()
        .expect("run lrt");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf-8"),
        fixture("version.txt")
    );
    assert!(output.stderr.is_empty());
}
