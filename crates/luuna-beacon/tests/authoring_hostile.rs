//! Hostile authoring inputs; every case requires the explicit Linux limit.
mod support;
use luuna_beacon::BeaconError;
use std::{
    fs,
    io::{BufWriter, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use support::{
    authoring::*,
    case::{write_file, Step},
    go,
};

fn bomb() -> String {
    let mut text = String::from("a0: &a0 [x, x, x, x, x, x, x, x, x, x]\n");
    for level in 1..12 {
        text.push_str(&format!("a{level}: &a{level} ["));
        for item in 0..10 {
            if item != 0 {
                text.push_str(", ");
            }
            text.push_str(&format!("*a{}", level - 1));
        }
        text.push_str("]\n");
    }
    text
}
fn pair() -> Pair {
    let mut pair = Pair::new();
    pair.step(Step::Init);
    pair
}
fn target_dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("authoring-hostile-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap()
}
fn flow(path: &Path, depth: usize) {
    let mut out = BufWriter::new(fs::File::create(path).unwrap());
    out.write_all(b"project: luuna-beacon\nversion: \"1\"\ndeep: ")
        .unwrap();
    for _ in 0..depth {
        out.write_all(b"[").unwrap();
    }
    for _ in 0..depth {
        out.write_all(b"]").unwrap();
    }
    out.write_all(b"\n").unwrap();
    out.flush().unwrap();
}
fn block(path: &Path, depth: usize) {
    assert!(depth <= 10_001);
    let spaces = [b' '; 20_004];
    let mut out = BufWriter::new(fs::File::create(path).unwrap());
    out.write_all(b"project: luuna-beacon\nversion: \"1\"\ndeep:\n")
        .unwrap();
    for level in 0..depth {
        out.write_all(&spaces[..2 * (level + 1)]).unwrap();
        out.write_all(b"k:\n").unwrap();
    }
    out.write_all(&spaces[..2 * (depth + 1)]).unwrap();
    out.write_all(b"0\n").unwrap();
    out.flush().unwrap();
}
#[cfg(unix)]
fn linked_config(root: &Path, source: &Path) {
    let path = root.join(CONFIG);
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(source, path).unwrap();
}
#[cfg(not(unix))]
fn linked_config(_root: &Path, _source: &Path) {
    panic!("hostile cases require Linux prlimit")
}

#[test]
#[ignore = "hostile input: run only under prlimit --as=4294967296 timeout 120 with LUUNA_BEACON_REFERENCE and LUUNA_BEACON_GO; see crates/luuna-beacon/notes/s3-beacon-lib-2.md"]
fn config_yaml_alias_bomb_is_refused_like_go() {
    assert_memory_limited();
    let mut pair = pair();
    let text = format!("project: luuna-beacon\nversion: \"1\"\n{}", bomb());
    pair.raw(write_file(CONFIG, text.as_bytes()));
    let before = tree_bytes(&pair.lib_root);
    pair.step_os_error(config_show(), "yaml: ");
    assert_tree_unchanged(&before, &pair.lib_root, "alias bomb");
}
#[test]
#[ignore = "hostile input: run only under prlimit --as=4294967296 timeout 120 with LUUNA_BEACON_REFERENCE and LUUNA_BEACON_GO; see crates/luuna-beacon/notes/s3-beacon-lib-2.md"]
fn config_set_value_alias_bomb_is_refused_like_go() {
    assert_memory_limited();
    let mut pair = pair();
    let before = tree_bytes(&pair.lib_root);
    pair.step_os_error(config_set("targets.default.extra", &bomb()), "yaml: ");
    assert_tree_unchanged(&before, &pair.lib_root, "value alias bomb");
}
#[test]
#[ignore = "hostile input: run only under prlimit --as=4294967296 timeout 120 with LUUNA_BEACON_REFERENCE and LUUNA_BEACON_GO; see crates/luuna-beacon/notes/s3-beacon-lib-2.md"]
fn import_frontmatter_alias_bomb_is_refused_like_go() {
    assert_memory_limited();
    let mut pair = pair();
    let source = format!("---\n{}---\n## S\nbody\n", bomb());
    pair.raw(write_file("source.md", source.as_bytes()));
    let before = tree_bytes(&pair.lib_root);
    pair.step_os_error(
        import("source.md", "bomb", &[], false),
        "invalid frontmatter: yaml: ",
    );
    assert_tree_unchanged(&before, &pair.lib_root, "frontmatter alias bomb");
}
#[test]
#[ignore = "hostile input: run only under prlimit --as=4294967296 timeout 120 with LUUNA_BEACON_REFERENCE and LUUNA_BEACON_GO; see crates/luuna-beacon/notes/s3-beacon-lib-2.md"]
fn many_documents_and_nested_anchors_stay_bounded() {
    assert_memory_limited();
    let mut pair = pair();
    for root in [&pair.go_root, &pair.lib_root] {
        let mut out = BufWriter::new(fs::File::create(root.join(CONFIG)).unwrap());
        out.write_all(b"project: luuna-beacon\nversion: \"1\"\na0: &a0 [1, 2]\n")
            .unwrap();
        for level in 1..32 {
            writeln!(out, "a{level}: &a{level} [*a{}]", level - 1).unwrap();
        }
        for _ in 0..2_000 {
            out.write_all(b"---\nlater: &later [one, two]\ncopy: *later\n")
                .unwrap();
        }
        out.flush().unwrap();
    }
    pair.step(config_show());
    let shown = pair.beacon().config_show().unwrap();
    assert!(shown.text.len() > 60_000);
    assert!(!shown.raw.is_null());
}
#[test]
#[ignore = "hostile input: run only under prlimit --as=4294967296 timeout 120 with LUUNA_BEACON_REFERENCE and LUUNA_BEACON_GO; see crates/luuna-beacon/notes/s3-beacon-lib-2.md"]
fn nesting_at_go_limit_is_accepted_on_a_two_mib_stack() {
    assert_memory_limited();
    let pair = pair();
    let holder = target_dir();
    let source = holder.path().join("limit.yaml");
    flow(&source, 10_000);
    linked_config(&pair.go_root, &source);
    linked_config(&pair.lib_root, &source);
    let (code, expected, stderr) =
        run_go_text(&pair.go_root, &["config", "show", "--path", "deep"]);
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&stderr));
    let root = pair.lib_root.clone();
    let actual = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            let beacon = fixed_beacon(&root);
            let value = beacon.config_get("deep").unwrap();
            let copy = value.clone();
            assert_eq!(value, copy);
            value.to_yaml()
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(actual.as_bytes(), expected);
}
#[test]
#[ignore = "hostile input: run only under prlimit --as=4294967296 timeout 120 with LUUNA_BEACON_REFERENCE and LUUNA_BEACON_GO; see crates/luuna-beacon/notes/s3-beacon-lib-2.md"]
fn nesting_above_go_limit_is_refused_like_go() {
    assert_memory_limited();
    let holder = target_dir();
    for is_block in [false, true] {
        let pair = pair();
        let source = holder
            .path()
            .join(if is_block { "block.yaml" } else { "flow.yaml" });
        if is_block {
            block(&source, 10_000);
        } else {
            flow(&source, 10_001);
        }
        linked_config(&pair.go_root, &source);
        linked_config(&pair.lib_root, &source);
        let (code, _, stderr) = run_go_text(&pair.go_root, &["config", "show", "--path", "deep"]);
        assert_ne!(code, 0, "Go accepted above-limit input");
        let expected = String::from_utf8(stderr).unwrap();
        assert!(
            expected.contains("exceeded max depth of 10000"),
            "{expected}"
        );
        let error = pair.beacon().config_get("deep").unwrap_err();
        assert_eq!(
            error.to_string(),
            expected
                .trim_end()
                .strip_prefix("Error: ")
                .expect("Go CLI error wrapper")
        );
    }
}
#[test]
#[ignore = "hostile input: run only under prlimit --as=4294967296 timeout 120 with LUUNA_BEACON_REFERENCE and LUUNA_BEACON_GO; see crates/luuna-beacon/notes/s3-beacon-lib-2.md"]
fn deep_json_frontmatter_renders_without_stack_overflow() {
    assert_memory_limited();
    let (_holder, root) = api_root();
    let beacon = fixed_beacon(&root);
    beacon.init().unwrap();
    let store = root.join(PROMPTS);
    let mut out = BufWriter::new(fs::File::create(&store).unwrap());
    out.write_all(br#"{"id":"deep-1","name":"deep","version":1,"sections":[{"name":"body","body":"text"}],"frontmatter":"#).unwrap();
    for _ in 1..9_999 {
        out.write_all(br#"{"a":"#).unwrap();
    }
    out.write_all(b"{}").unwrap();
    for _ in 1..9_999 {
        out.write_all(b"}").unwrap();
    }
    out.write_all(b"}\n").unwrap();
    out.flush().unwrap();
    drop(out);
    let before = fs::read(&store).unwrap();
    let text = beacon.render_text("deep").unwrap();
    assert!(text.starts_with("---\n"));
    assert!(text.ends_with("text\n"));
    assert!(text.contains("name: deep\n"));
    drop(text);
    assert_eq!(fs::read(&store).unwrap(), before);
}
#[test]
#[ignore = "hostile input: run only under prlimit --as=4294967296 timeout 120 with LUUNA_BEACON_REFERENCE and LUUNA_BEACON_GO; see crates/luuna-beacon/notes/s3-beacon-lib-2.md"]
fn very_large_markdown_import_matches_go() {
    assert_memory_limited();
    let holder = target_dir();
    let source = holder.path().join("large.md");
    let mut out = BufWriter::new(fs::File::create(&source).unwrap());
    let heading = b"## Large\n";
    out.write_all(heading).unwrap();
    let chunk = [b'x'; 65_536];
    let mut remaining = 24 * 1024 * 1024 - heading.len();
    while remaining != 0 {
        let n = remaining.min(chunk.len());
        out.write_all(&chunk[..n]).unwrap();
        remaining -= n;
    }
    out.flush().unwrap();
    drop(out);
    assert_eq!(fs::metadata(&source).unwrap().len(), 24 * 1024 * 1024);
    let mut pair = pair();
    pair.step(import(source.to_str().unwrap(), "large", &[], false));
    assert!(fs::metadata(pair.lib_root.join(PROMPTS)).unwrap().len() > 23 * 1024 * 1024);
}
fn cycle_records() -> &'static [u8] {
    concat!(
        "{\"id\":\"a\",\"name\":\"a\",\"version\":1,\"extends\":\"b\"}\n",
        "{\"id\":\"b\",\"name\":\"b\",\"version\":1,\"extends\":\"a\"}\n"
    )
    .as_bytes()
}
#[test]
#[ignore = "hostile input: run only under prlimit --as=4294967296 timeout 120 with LUUNA_BEACON_REFERENCE and LUUNA_BEACON_GO; see crates/luuna-beacon/notes/s3-beacon-lib-2.md"]
fn tree_ancestor_cycle_is_bounded_in_the_library() {
    assert_memory_limited();
    let (_holder, root) = api_root();
    let beacon = fixed_beacon(&root);
    beacon.init().unwrap();
    fs::write(root.join(PROMPTS), cycle_records()).unwrap();
    let error = beacon.tree("a").unwrap_err();
    assert!(
        matches!(error, BeaconError::CircularInheritance(_)),
        "{error}"
    );
}
fn limited_go(
    root: &Path,
    outputs: &Path,
    name: &str,
) -> (std::process::ExitStatus, Vec<u8>, Vec<u8>) {
    // The shared helper validates the temporary project root and warms the oracle.
    let (code, _, _) = run_go_text(root, &["--version"]);
    assert_eq!(code, 0);
    let build = go::go_build();
    let oracle = build.dir.join("oracle");
    let prlimit: PathBuf = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("prlimit"))
        .find(|path| path.is_file())
        .expect("prlimit on PATH");
    let stdout = outputs.join(format!("{name}-stdout.txt"));
    let stderr = outputs.join(format!("{name}-stderr.txt"));
    let mut child = Command::new(prlimit)
        .args(["--as=1073741824", "--"])
        .arg(&build.binary)
        .args(["tree", "a", "--json"])
        .current_dir(root)
        .env_clear()
        .env("PATH", oracle.join("path"))
        .env("HOME", oracle.join("home"))
        .env("TMPDIR", oracle.join("tmp"))
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("TZ", "UTC")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(fs::File::create(&stdout).unwrap())
        .stderr(fs::File::create(&stderr).unwrap())
        .spawn()
        .unwrap();
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() >= Duration::from_secs(30) {
            child.kill().unwrap();
            break child.wait().unwrap();
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let read = |path: &Path| {
        let mut bytes = Vec::new();
        fs::File::open(path)
            .unwrap()
            .take(65_536)
            .read_to_end(&mut bytes)
            .unwrap();
        bytes
    };
    (status, read(&stdout), read(&stderr))
}
#[test]
#[ignore = "hostile input: run only under prlimit --as=4294967296 timeout 120 with LUUNA_BEACON_REFERENCE and LUUNA_BEACON_GO; see crates/luuna-beacon/notes/s3-beacon-lib-2.md"]
fn tree_ancestor_cycle_exhausts_go_under_a_one_gib_limit() {
    assert_memory_limited();
    let pair = pair();
    let holder = target_dir();
    fs::write(
        pair.go_root.join(PROMPTS),
        b"{\"id\":\"a\",\"name\":\"a\",\"version\":1}\n",
    )
    .unwrap();
    let (normal, stdout, stderr) = limited_go(&pair.go_root, holder.path(), "normal");
    assert!(
        normal.success(),
        "Go control failed: {}",
        String::from_utf8_lossy(&stderr)
    );
    let doc: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(doc["command"], "tree");
    fs::write(pair.go_root.join(PROMPTS), cycle_records()).unwrap();
    let (failed, stdout, stderr) = limited_go(&pair.go_root, holder.path(), "cycle");
    assert!(!failed.success());
    assert!(stdout.iter().all(u8::is_ascii_whitespace));
    let first = String::from_utf8_lossy(&stderr)
        .lines()
        .next()
        .unwrap_or("")
        .to_owned();
    eprintln!("Go cycle exit {failed}; first stderr line: {first}");
}
