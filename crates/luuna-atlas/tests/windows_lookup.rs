//! Native Windows executable discovery uses only the explicitly supplied environment.
#![cfg(windows)]
use luuna_atlas::options::Env;
use std::{ffi::OsString, fs, path::Path};
fn environment(path: OsString, extensions: &str) -> Env {
    Env::fixed(
        [("Path", path), ("Pathext", extensions.into())],
        vec![],
        false,
    )
}
fn file(dir: &Path, name: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    fs::write(&path, b"lookup fixture, never executed").unwrap();
    path
}
#[test]
fn windows_path_lists_quotes_case_and_suffix_priority() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first directory");
    let second = root.path().join("second directory");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    let exe = file(&second, "atlas-tool.exe");
    let cmd = file(&second, "atlas-tool.cmd");
    let path = format!("\"{}\";\"{}\"", first.display(), second.display());
    let env = environment(path.clone().into(), ".EXE;.CMD");
    assert_eq!(env.var("PATH"), Some(path.clone()));
    assert_eq!(env.var("PATHEXT"), Some(".EXE;.CMD".into()));
    assert_eq!(env.look_path("atlas-tool"), Some(exe.clone()));
    assert_eq!(
        environment(path.into(), "CMD;EXE").look_path("atlas-tool"),
        Some(cmd)
    );
    assert_eq!(
        environment(second.as_os_str().to_owned(), "").look_path("atlas-tool"),
        Some(exe.clone())
    );
    assert_eq!(
        environment(OsString::new(), ".EXE").look_path(exe.to_str().unwrap()),
        Some(exe)
    );
}
#[test]
fn windows_lookup_skips_directories_and_accepts_explicit_extensions() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("atlas-tool.exe")).unwrap();
    let cmd = file(root.path(), "atlas-tool.cmd");
    let env = environment(root.path().as_os_str().to_owned(), ".EXE;.CMD");
    assert_eq!(env.look_path("atlas-tool"), Some(cmd));
    let weird = file(root.path(), "weird.bat.exe");
    assert_eq!(env.look_path("weird.bat"), Some(weird));
    let explicit = file(root.path(), "custom.other");
    assert_eq!(env.look_path("custom.other"), Some(explicit));
}
#[test]
fn windows_lookup_retains_explicit_path_policy_and_refuses_relative_results() {
    let root = tempfile::tempdir().unwrap();
    let cargo = file(root.path(), "Cargo.toml");
    // Cargo runs this test with the package directory as cwd. That directory's
    // Cargo.toml must never shadow the independently supplied absolute PATH.
    assert!(Path::new("Cargo.toml").is_file());
    assert_eq!(
        environment(root.path().as_os_str().to_owned(), ";").look_path("Cargo.toml"),
        Some(cargo)
    );
    assert_eq!(environment(".".into(), ";").look_path("Cargo.toml"), None);
    assert_eq!(
        environment(OsString::new(), ";").look_path("Cargo.toml"),
        None
    );
}
