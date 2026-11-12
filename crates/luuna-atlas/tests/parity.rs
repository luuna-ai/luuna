//! Parity with the Go reference tool for `init`, `add` and `config show`.
//!
//! Each test creates twin temporary projects, runs the Go tool (with `--json --quiet`, as
//! the runtime adapter does) in one and the crate in the other, and compares after
//! normalisation: exit code, stdout, stderr, and the file tree minus `.git` (paths, file or
//! directory type, permission bits and bytes). A leftover `*.lock` or `*.tmp.*` file fails.
//!
//! Every test needs the reference clone and Go, so all are ignored by default. Run:
//! `LUUNA_ATLAS_REFERENCE=<clone> LUUNA_ATLAS_GO=<go> cargo test -p luuna-atlas --locked
//! --test parity -- --ignored`.

mod support;

use support::project::{Project, Sandbox, Step, Twin};
use support::reference::{
    binary_path, build_root, check_clean, check_head, reference, workspace_root, REFERENCE_COMMIT,
};

const GIT_ATTRIBUTES_LINE: &str = ".luuna/atlas/expertise/*.jsonl merge=union";

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn parity_reference_guard() {
    let reference = reference();
    let expected_root = build_root(&workspace_root());
    assert_eq!(reference.build_root, expected_root);
    assert_eq!(reference.binary, binary_path(&expected_root));
    assert!(
        reference.binary.starts_with(
            workspace_root()
                .join("target")
                .join("atlas-reference")
                .join(REFERENCE_COMMIT)
        ),
        "{}",
        reference.binary.display()
    );
    assert!(reference.binary.is_file(), "{}", reference.binary.display());
    // The build left the clone exactly as it was.
    check_head(&reference.clone, REFERENCE_COMMIT).unwrap();
    check_clean(&reference.clone).unwrap();
    // The binary is the reference tool.
    let sandbox = Sandbox::new();
    let project = sandbox.project("p", true);
    let output = project.go(&["--version"]);
    assert_eq!(output.exit_code, 0, "{output:?}");
    assert!(
        output.stdout.contains("\"version\": \"0.11.0\""),
        "{output:?}"
    );
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn parity_init_fresh_repo() {
    let twin = Twin::new(true);
    twin.steps(&[Step::Init]);
    // The comparison is only meaningful if the Go tool really created the store.
    assert!(twin.go.config_path().is_file());
    assert!(twin.go.atlas_dir().join("README.md").is_file());
    assert!(twin.go.atlas_dir().join("expertise").is_dir());
    assert_eq!(
        twin.go.read(&twin.go.dir.join(".gitattributes")),
        format!("{GIT_ATTRIBUTES_LINE}\n")
    );
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn parity_init_gitattributes_without_newline() {
    let twin = Twin::new(true);
    twin.prepare(|p: &Project| p.write(".gitattributes", "*.png binary"));
    twin.steps(&[Step::Init]);
    assert_eq!(
        twin.go.read(&twin.go.dir.join(".gitattributes")),
        format!("*.png binary\n{GIT_ATTRIBUTES_LINE}\n")
    );
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn parity_init_gitattributes_already_has_line() {
    let twin = Twin::new(true);
    let existing = format!("*.png binary\n{GIT_ATTRIBUTES_LINE}\n*.jpg binary");
    twin.prepare(|p: &Project| p.write(".gitattributes", &existing));
    twin.steps(&[Step::Init]);
    assert_eq!(twin.go.read(&twin.go.dir.join(".gitattributes")), existing);
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn parity_init_twice() {
    let twin = Twin::new(true);
    twin.steps(&[Step::Init, Step::Init]);
    // A second init after a user edit keeps the edit and fills in what is missing.
    twin.prepare(|p: &Project| {
        std::fs::remove_file(p.atlas_dir().join("README.md")).unwrap();
        p.write(".luuna/atlas/config.yaml", "version: '1'\ndomains: {}\n");
    });
    twin.steps(&[Step::Init, Step::ConfigShow]);
    assert_eq!(
        twin.go.read(&twin.go.config_path()),
        "version: '1'\ndomains: {}\n"
    );
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn parity_init_outside_git() {
    let twin = Twin::new(false);
    assert!(!twin.go.dir.join(".git").exists());
    twin.steps(&[Step::Init, Step::add("notes"), Step::ConfigShow]);
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn parity_add_domain() {
    let twin = Twin::new(true);
    twin.steps(&[
        Step::Init,
        Step::add("architecture"),
        Step::add("testing_2"),
        Step::add("API-v2"),
        Step::add("9lives"),
    ]);
    let config = twin.go.read(&twin.go.config_path());
    assert!(
        config.contains(
            "domains:\n  architecture: {}\n  testing_2: {}\n  API-v2: {}\n  9lives: {}\n"
        ),
        "{config}"
    );
    assert_eq!(
        std::fs::read(twin.go.expertise("architecture")).unwrap(),
        b""
    );
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn parity_add_invalid_domain_name() {
    let twin = Twin::new(true);
    twin.steps(&[
        Step::Init,
        Step::add("../evil"),
        Step::add("has space"),
        Step::add("_leading"),
        Step::add("dots.not.ok"),
        Step::add("slash/inside"),
        Step::add("ünïcode"),
    ]);
    let output = twin.go.go(&["add", "../evil"]);
    assert_eq!(output.exit_code, 1);
    assert!(output.stderr.contains("Invalid domain name"), "{output:?}");
    assert!(!twin.go.dir.join(".luuna").join("evil.jsonl").exists());
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn parity_add_existing_domain() {
    let twin = Twin::new(true);
    twin.steps(&[Step::Init, Step::add("arch"), Step::add("arch")]);
    // A domain present in config.yaml whose expertise file was deleted is still "existing".
    twin.prepare(|p: &Project| std::fs::remove_file(p.expertise("arch")).unwrap());
    twin.steps(&[Step::add("arch"), Step::ConfigShow]);
    // Adding before init fails the same way on both sides.
    let before_init = Twin::new(true);
    before_init.steps(&[Step::add("arch")]);
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn parity_config_show_initial() {
    let twin = Twin::new(true);
    twin.steps(&[Step::Init, Step::ConfigShow]);
    let shown = twin.go.go_ok(&["config", "show"]);
    assert!(
        shown.starts_with("{\n  \"version\": \"1\",\n  \"domains\": {},\n"),
        "{shown}"
    );
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn parity_config_show_after_add() {
    let twin = Twin::new(true);
    twin.steps(&[
        Step::Init,
        Step::add("zeta"),
        Step::add("alpha"),
        Step::ConfigShow,
    ]);
    // Defaults are merged in Go's key order around a user's partial settings.
    twin.prepare(|p: &Project| {
        p.write(
            ".luuna/atlas/config.yaml",
            "# a comment\ngovernance:\n  hard_limit: 300\n  extra: true\ndomains:\n  - one\n  - two\nversion: '1'\nclassification_defaults:\n  shelf_life:\n    observational: 60\n",
        );
    });
    twin.steps(&[Step::ConfigShow, Step::add("three"), Step::ConfigShow]);
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn parity_config_show_without_store() {
    let twin = Twin::new(true);
    twin.steps(&[Step::ConfigShow]);
    let output = twin.go.go(&["config", "show"]);
    assert_eq!(output.exit_code, 1);
    assert!(
        output.stderr.contains("No .luuna/atlas/ directory found"),
        "{output:?}"
    );
    // A store directory without config.yaml is the same failure.
    twin.prepare(|p: &Project| std::fs::create_dir_all(p.atlas_dir()).unwrap());
    twin.steps(&[Step::ConfigShow]);
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn parity_legacy_mulch_migration() {
    let twin = Twin::new(true);
    twin.prepare(|p: &Project| {
        p.write(".mulch/mulch.config.yaml", "version: \"1\"\ndomains:\n  - arch\n");
        p.write(
            ".mulch/expertise/arch.jsonl",
            "{\"type\":\"convention\",\"classification\":\"foundational\",\"recorded_at\":\"2025-01-02T03:04:05.000Z\",\"content\":\"Legacy rule\",\"id\":\"mx-aaaaaa\"}\n",
        );
        p.write(".gitattributes", ".mulch/expertise/*.jsonl merge=union\n*.png binary\n");
        p.write(".gitignore", "node_modules/\n  /.mulch/tmp\n.mulchy/\n");
    });
    // The first command migrates (the notice goes to stderr); later ones find the new store.
    twin.steps(&[
        Step::ConfigShow,
        Step::Init,
        Step::add("arch"),
        Step::add("ops"),
        Step::ConfigShow,
    ]);
    assert!(!twin.go.dir.join(".mulch").exists());
    assert_eq!(
        twin.go.read(&twin.go.dir.join(".gitignore")),
        "node_modules/\n  /.luuna/atlas/tmp\n.mulchy/\n"
    );

    // A legacy store that still has its config under the old name next to a new one.
    let both_configs = Twin::new(true);
    both_configs.prepare(|p: &Project| {
        p.write(
            ".mulch/mulch.config.yaml",
            "version: '1'\ndomains:\n  - old\n",
        );
        p.write(".mulch/config.yaml", "version: '1'\ndomains:\n  - new\n");
    });
    both_configs.steps(&[Step::Init, Step::ConfigShow]);
}

#[test]
#[ignore = "needs the luuna-atlas reference clone and Go; run the -- --ignored check"]
fn parity_legacy_mulch_and_atlas_both_present_refused() {
    let twin = Twin::new(true);
    twin.prepare(|p: &Project| {
        p.write(".mulch/mulch.config.yaml", "version: '1'\ndomains: {}\n");
        p.write(".luuna/atlas/config.yaml", "version: '1'\ndomains: {}\n");
    });
    twin.steps(&[Step::Init, Step::add("arch"), Step::ConfigShow]);
    let output = twin.go.go(&["init"]);
    assert_eq!(output.exit_code, 1);
    assert!(
        output
            .stderr
            .contains("Both .mulch/ and .luuna/atlas/ exist"),
        "{output:?}"
    );
    // Nothing moved.
    assert!(twin
        .go
        .dir
        .join(".mulch")
        .join("mulch.config.yaml")
        .is_file());

    // A symbolic link in place of .mulch is refused too.
    #[cfg(unix)]
    {
        let linked = Twin::new(true);
        linked.prepare(|p: &Project| {
            p.write("elsewhere/mulch.config.yaml", "version: '1'\ndomains: {}\n");
            std::os::unix::fs::symlink("elsewhere", p.dir.join(".mulch")).unwrap();
        });
        linked.steps(&[Step::Init]);
    }
}
