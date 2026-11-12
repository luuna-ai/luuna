//! Self-tests of the comparison harness: the normaliser and the reference guard. They need
//! host git only, never the reference clone or Go; every repository they use is a
//! temporary one created here.

mod support;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use support::normalize::{Normalizer, GENERATED_TIMESTAMP, PROJECT};
use support::project::{Project, Sandbox};
use support::reference::{
    binary_path, build_root, check_clean, check_head, check_variables, go_build_command,
    go_build_env, null_device, reference_git, workspace_root, GO_VAR, REFERENCE_COMMIT,
    REFERENCE_VAR,
};

fn go_format(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time)
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// The first line of a refusal: the problem, without the how-to-run hint.
fn problem(message: &str) -> &str {
    message.lines().next().unwrap_or("")
}

/// A temporary repository with one commit holding `tracked.txt` and a `.gitignore`.
fn committed_repo(sandbox: &Sandbox) -> Project {
    let repo = sandbox.project("clone", true);
    repo.write("tracked.txt", "tracked\n");
    repo.write(".gitignore", "ignored.txt\n");
    repo.git(&["add", "tracked.txt", ".gitignore"]);
    repo.git(&["commit", "-q", "-m", "fixture"]);
    repo
}

fn env_value<'a>(env: &'a [(OsString, OsString)], key: &str) -> Option<&'a OsString> {
    env.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

#[test]
fn harness_normalizes_project_paths() {
    let sandbox = Sandbox::without_go();
    let project = sandbox.project("p", false);
    let normalizer = project.normalizer();
    let dir = project.dir.display().to_string();
    let json = format!("{{\n  \"path\": \"{dir}/.luuna/atlas\"\n}}\n");
    assert_eq!(
        normalizer.normalize(&json),
        format!("{{\n  \"path\": \"{PROJECT}/.luuna/atlas\"\n}}\n")
    );
    assert_eq!(
        normalizer.normalize(&format!("Initialized .luuna/atlas/ in {dir}")),
        format!("Initialized .luuna/atlas/ in {PROJECT}")
    );
    // Another directory is left alone.
    let other = sandbox.root().join("elsewhere").display().to_string();
    assert_eq!(normalizer.normalize(&other), other);

    // A raw path through a symbolic link and its canonical form both become <PROJECT>.
    #[cfg(unix)]
    {
        let link = sandbox.root().join("link-to-p");
        std::os::unix::fs::symlink(project.dir.parent().unwrap(), &link).unwrap();
        let raw = link.join("project");
        let normalizer = Normalizer::new(&raw, sandbox.started());
        let text = format!("raw {} canonical {}", raw.display(), project.dir.display());
        assert_eq!(
            normalizer.normalize(&text),
            format!("raw {PROJECT} canonical {PROJECT}")
        );
    }
}

#[test]
fn harness_normalizes_generated_timestamps() {
    let started = SystemTime::now();
    let normalizer = Normalizer::new(Path::new("/nonexistent-project"), started);
    let now = go_format(started + Duration::from_millis(1500));
    let recent = go_format(started - Duration::from_secs(30));

    // As a whole JSON value.
    assert_eq!(
        normalizer.normalize(&format!("{{\"recorded_at\":\"{now}\"}}")),
        format!("{{\"recorded_at\":\"{GENERATED_TIMESTAMP}\"}}")
    );
    // Within the 60 s window before the test started.
    assert_eq!(normalizer.normalize(&recent), GENERATED_TIMESTAMP);
    // Embedded in a longer string, twice.
    assert_eq!(
        normalizer.normalize(&format!("from {now} to {recent}.")),
        format!("from {GENERATED_TIMESTAMP} to {GENERATED_TIMESTAMP}.")
    );
    // Only the exact Go form counts: no milliseconds, microseconds, or an offset do not.
    let seconds_only = format!("{}Z", &now[..19]);
    let micros = format!("{}123Z", &now[..23]);
    let offset = format!("{}+00:00", &now[..23]);
    for text in [&seconds_only, &micros, &offset] {
        assert_eq!(
            &normalizer.normalize(text),
            text,
            "{text} must stay as it is"
        );
    }
}

#[test]
fn harness_leaves_old_timestamps() {
    let started = SystemTime::now();
    let normalizer = Normalizer::new(Path::new("/nonexistent-project"), started);
    let fixed = "2025-01-02T03:04:05.000Z";
    assert_eq!(normalizer.normalize(fixed), fixed);
    let before_window = go_format(started - Duration::from_secs(61));
    assert_eq!(normalizer.normalize(&before_window), before_window);
    let line = format!("{{\"recorded_at\":\"{fixed}\",\"archived_at\":\"{before_window}\"}}");
    assert_eq!(normalizer.normalize(&line), line);
    // An impossible date in the right shape is not a timestamp.
    let impossible = "2025-13-45T25:61:61.000Z";
    assert_eq!(normalizer.normalize(impossible), impossible);
}

#[test]
fn harness_normalizes_crlf() {
    let normalizer = Normalizer::new(Path::new("/nonexistent-project"), SystemTime::now());
    assert_eq!(normalizer.normalize("a\r\nb\r\n"), "a\nb\n");
    // A lone CR is not a line ending and is kept.
    assert_eq!(normalizer.normalize("a\rb\n"), "a\rb\n");
    // Nothing else changes: trailing spaces, tabs and blank lines stay.
    assert_eq!(normalizer.normalize(" x \t\n\n"), " x \t\n\n");
}

#[test]
fn harness_guard_rejects_missing_path() {
    let sandbox = Sandbox::without_go();
    let existing_dir: OsString = sandbox.root().as_os_str().to_owned();
    let existing_file = sandbox.root().join("go");
    std::fs::write(&existing_file, "").unwrap();
    let existing_file: OsString = existing_file.into_os_string();
    let missing: OsString = sandbox.root().join("missing").into_os_string();

    let cases: Vec<(Option<OsString>, Option<OsString>, &str)> = vec![
        (None, Some(existing_file.clone()), REFERENCE_VAR),
        (
            Some(OsString::new()),
            Some(existing_file.clone()),
            REFERENCE_VAR,
        ),
        (
            Some(missing.clone()),
            Some(existing_file.clone()),
            REFERENCE_VAR,
        ),
        (Some(existing_dir.clone()), None, GO_VAR),
        (Some(existing_dir.clone()), Some(missing.clone()), GO_VAR),
        (
            Some(existing_dir.clone()),
            Some(existing_dir.clone()),
            GO_VAR,
        ),
    ];
    for (reference, go, culprit) in cases {
        let label = format!("{reference:?} {go:?}");
        let message = check_variables(reference, go).expect_err(&label);
        let problem = problem(&message);
        assert!(problem.contains(culprit), "{label}: {problem}");
        let innocent = if culprit == REFERENCE_VAR {
            GO_VAR
        } else {
            REFERENCE_VAR
        };
        assert!(!problem.contains(innocent), "{label}: {problem}");
    }
    // Both missing: both are named.
    let message = check_variables(None, None).unwrap_err();
    assert!(problem(&message).contains(REFERENCE_VAR));
    assert!(problem(&message).contains(GO_VAR));
    // Both present: accepted.
    let (reference, go) =
        check_variables(Some(existing_dir.clone()), Some(existing_file.clone())).unwrap();
    assert_eq!(reference, PathBuf::from(existing_dir));
    assert_eq!(go, PathBuf::from(existing_file));
}

#[test]
fn harness_guard_rejects_wrong_head() {
    let sandbox = Sandbox::without_go();
    let repo = committed_repo(&sandbox);
    let head = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();
    assert_ne!(head, REFERENCE_COMMIT);

    let message = check_head(&repo.dir, REFERENCE_COMMIT).unwrap_err();
    assert!(problem(&message).contains(REFERENCE_VAR), "{message}");
    assert!(problem(&message).contains(REFERENCE_COMMIT), "{message}");
    assert!(problem(&message).contains(&head), "{message}");
    check_head(&repo.dir, &head).expect("the actual HEAD is accepted");

    // Not a repository at all.
    let plain = sandbox.project("plain", false);
    let message = check_head(&plain.dir, REFERENCE_COMMIT).unwrap_err();
    assert!(problem(&message).contains(REFERENCE_VAR), "{message}");
}

#[test]
fn harness_guard_rejects_dirty_clone() {
    let sandbox = Sandbox::without_go();
    let repo = committed_repo(&sandbox);
    check_clean(&repo.dir).expect("a fresh commit is clean");

    // An untracked file.
    repo.write("untracked.txt", "x\n");
    let message = check_clean(&repo.dir).unwrap_err();
    assert!(problem(&message).contains(REFERENCE_VAR), "{message}");
    assert!(message.contains("untracked.txt"), "{message}");
    std::fs::remove_file(repo.dir.join("untracked.txt")).unwrap();
    check_clean(&repo.dir).unwrap();

    // An ignored file also counts (--ignored).
    repo.write("ignored.txt", "x\n");
    let message = check_clean(&repo.dir).unwrap_err();
    assert!(message.contains("ignored.txt"), "{message}");
    std::fs::remove_file(repo.dir.join("ignored.txt")).unwrap();

    // A modified tracked file.
    repo.write("tracked.txt", "changed\n");
    let message = check_clean(&repo.dir).unwrap_err();
    assert!(message.contains("tracked.txt"), "{message}");
}

#[test]
fn harness_go_env_points_inside_target() {
    let workspace = workspace_root();
    assert!(
        workspace.join("Cargo.lock").is_file(),
        "{}",
        workspace.display()
    );
    let root = build_root(&workspace);
    let expected_root = workspace
        .join("target")
        .join("atlas-reference")
        .join(REFERENCE_COMMIT);
    assert_eq!(root, expected_root);
    assert_eq!(
        binary_path(&root),
        expected_root.join("bin").join("luuna-atlas")
    );

    let env = go_build_env(&root, Some("/usr/bin".into()));
    let mut keys: Vec<String> = env
        .iter()
        .map(|(k, _)| k.to_string_lossy().into_owned())
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "CGO_ENABLED",
            "GIT_CONFIG_GLOBAL",
            "GIT_CONFIG_NOSYSTEM",
            "GIT_OPTIONAL_LOCKS",
            "GOCACHE",
            "GOENV",
            "GOFLAGS",
            "GOMODCACHE",
            "GOPATH",
            "GOPROXY",
            "GOSUMDB",
            "GOTMPDIR",
            "GOTOOLCHAIN",
            "GOWORK",
            "HOME",
            "PATH",
        ],
        "the Go environment holds these variables and nothing inherited but PATH"
    );
    for key in ["GOCACHE", "GOTMPDIR", "GOPATH", "GOMODCACHE", "HOME"] {
        let value = PathBuf::from(env_value(&env, key).unwrap());
        assert!(
            value.starts_with(&expected_root),
            "{key}={}",
            value.display()
        );
    }
    for (key, value) in [
        ("GOFLAGS", "-mod=vendor"),
        ("GOTOOLCHAIN", "local"),
        ("GOPROXY", "off"),
        ("GOSUMDB", "off"),
        ("GOWORK", "off"),
        ("GOENV", "off"),
        ("CGO_ENABLED", "0"),
        ("PATH", "/usr/bin"),
        // `go build` stamps VCS information by running git in the reference clone with
        // this environment: no optional locks (index.lock, index refresh) and no system
        // or global git config.
        ("GIT_OPTIONAL_LOCKS", "0"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_CONFIG_GLOBAL", null_device()),
    ] {
        assert_eq!(
            env_value(&env, key).map(|v| v.to_str().unwrap()),
            Some(value),
            "{key}"
        );
    }

    // The build command: the clone is only read (-C), the output goes under target/.
    let clone = Path::new("/reference-clone");
    let output = binary_path(&root).with_file_name("luuna-atlas.tmp.42");
    let cmd = go_build_command(Path::new("/opt/go/bin/go"), clone, &root, &output);
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        [
            "-C".to_owned(),
            clone.display().to_string(),
            "build".into(),
            "-trimpath".into(),
            "-o".into(),
            output.display().to_string(),
            "./cmd/luuna-atlas".into(),
        ]
    );
    assert_eq!(cmd.get_current_dir(), Some(root.as_path()));
    for (key, value) in cmd.get_envs() {
        let value = value.expect("the build sets variables, it does not remove them");
        if ["GOCACHE", "GOTMPDIR", "GOPATH", "GOMODCACHE", "HOME"].contains(&key.to_str().unwrap())
        {
            assert!(Path::new(value).starts_with(&expected_root));
        }
    }

    // Every git call against the reference clone disables optional locks.
    let git = reference_git(clone);
    let optional_locks = git
        .get_envs()
        .find(|(k, _)| *k == "GIT_OPTIONAL_LOCKS")
        .and_then(|(_, v)| v);
    assert_eq!(optional_locks, Some(std::ffi::OsStr::new("0")));
    let git_args: Vec<String> = git
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(git_args, ["-C".to_owned(), clone.display().to_string()]);
}

#[test]
fn harness_reference_git_leaves_index_untouched() {
    let sandbox = Sandbox::without_go();
    let repo = committed_repo(&sandbox);
    let index = repo.dir.join(".git").join("index");
    let tracked = repo.dir.join("tracked.txt");
    let make_stale = || {
        // Same content, older timestamp: a plain `git status` would refresh the index.
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&tracked)
            .unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000))
            .unwrap();
    };
    make_stale();
    let before = std::fs::read(&index).unwrap();
    check_clean(&repo.dir).unwrap();
    check_head(&repo.dir, repo.git(&["rev-parse", "HEAD"]).trim()).unwrap();
    assert_eq!(
        std::fs::read(&index).unwrap(),
        before,
        "the guard's git calls must not rewrite the index"
    );

    // Control: the same status without GIT_OPTIONAL_LOCKS=0 does rewrite it, so the
    // assertion above is meaningful.
    let mut plain = reference_git(&repo.dir);
    plain.env_remove("GIT_OPTIONAL_LOCKS");
    let status = plain
        .args(["status", "--porcelain", "--ignored"])
        .output()
        .unwrap();
    assert!(status.status.success());
    assert_ne!(
        std::fs::read(&index).unwrap(),
        before,
        "control: git refreshed the index"
    );
}
