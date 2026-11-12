# Installer tests

Tests for the POSIX installer in `packaging/installer/` (`install.sh`, `install-manifest.sh`). They are plain `sh` and need no build.

```sh
sh packaging/installer/tests/run.sh                                        # every test-*.sh, each in its own sandbox
sh packaging/installer/tests/run.sh packaging/installer/tests/test-install.sh
TEST_SHELL=dash dash packaging/installer/tests/run.sh                      # optional: catch bashisms
sh packaging/installer/tests/check-manifests.sh [--base origin/main]
```

`run.sh` takes test files as arguments. `TEST_SHELL` picks the shell that runs both the test files and, through `SH` in `lib.sh`, the installer (`sh`, `dash` or `bash`). Wrap a run in `prlimit --as=4294967296 timeout 120` when you feed it hostile archives on a shared machine.

## Files

- `run.sh` runs each test file in its own sandbox.
- `lib.sh` holds the case runner, assertions, snapshots, shims, barriers, the ustar writer and the fixture releases.
- `test-smoke.sh` checks the tools and the sandbox.
- `test-manifest.sh` checks manifest validation, platform selection, index hashes and append-only releases.
- `test-install.sh` checks installs, refusals, ownership of `luuna`, `lu` and `luuna-runtime`, root validation, locking and receipts.
- `test-upgrade-rollback.sh` checks upgrade, rollback and the undo after an injected failure.
- `test-uninstall.sh` checks that uninstall removes only what the receipt names.
- `test-docs.sh` checks that documents and release directories claim nothing that does not exist, and the installer's allow-lists.
- `check-manifests.sh` validates `packaging/installer/releases/` and, with `--base REF`, that released files are append-only.

## Tools

The tests need only POSIX `sh`, `tar`, `gzip`, `mktemp`, `sha256sum` or `shasum`, `grep`, `sed`, `awk`, `ln`, `mv`, `find`, `cksum`, `readlink`, `dd`, `tr` and `od`. They do not need network access, Bun, Rust, npm or a real `luuna`. `git` and `bash` are needed for the manifest-immutability cases and the `bash` run, which skip if `git` is missing. `mkfifo` is needed for the race case, which reports `# SKIP` without it.

## Isolation

`run.sh` gives each test file a fresh `mktemp -d` sandbox. It starts the test with `env -i` and exactly these variables: `HOME`, `XDG_DATA_HOME`, `LUUNA_INSTALL_HOME`, `LUUNA_INSTALL_BIN`, `PATH`, `TMPDIR`, `LC_ALL`, `COPYFILE_DISABLE`, `SANDBOX`, `REPO_ROOT`, `TESTS_DIR`, `REAL_HOME_GUARD` and `TEST_SHELL`. The first six point inside the sandbox, and the sandbox `PATH` is its own bin directory, the system directories and a folder holding links to `git` and `bash`. `run.sh` and `lib.sh` stop if `HOME` is the real home. No test touches the real home, the real `PATH` or the network. `REPO_ROOT` is three levels above the folder holding `run.sh`.

The installer's platform detection is pinned to `Linux/x86_64` through `LUUNA_INSTALL_UNAME_S` and `LUUNA_INSTALL_UNAME_M`, and fixtures are generated for that platform.

## Fixtures

`lib.sh` writes ustar archives byte for byte, so tests can build malicious entries with any host tar: traversal, absolute paths, symlinks, hardlinks, devices and duplicate entries. `make_fixture_release DIR VERSION [KIND]` writes `DIR/assets/luuna_<V>_linux_amd64.tar.gz` and `DIR/manifest.tsv`. The archive holds stub `bin/luuna-tui`, `bin/luuna-runtime` and `bin/luuna-sbx`; each is a tiny `#!/bin/sh` script that prints `stub:<exe>:<version>` and one `arg:[...]` line per argument. SHA256s are computed from the generated archives at test time. No real checksum is ever committed.

Every failure-path case asserts the exit code and that a snapshot of the case's `HOME` (and any victim directory) is byte-identical afterwards. The snapshot records type, mode, link target and checksum. The inside-repository refusal runs on a copy of the two installer scripts placed at `$CASE_DIR/repo/packaging/installer/`, never on the checkout. The only tests that run `git` do so in a temporary repository created inside a case directory.

## Fault injection and race barriers

Commit-phase failures are injected with one-shot `PATH` shims (`make_shim` in `lib.sh`): a shim for `ln`, `mv`, `cp` or `mkdir` fires once, when its last argument matches a glob, and otherwise runs the real tool, so the installer's own undo steps run unaffected. Only the command under test gets the shim directory on its `PATH`. Concurrency cases use `mkfifo` barriers (`start_paused` and `release_paused`) instead of sleeps. The race case proves the ordering at two pause points (the loser just before taking the lock, the winner at its first asset copy), not every interleaving.

## What the tests do not prove

- **Real binaries.** The tests prove the installer's logic with stub executables. They do not show that the real `luuna-tui`, `luuna-runtime` and `luuna-sbx` run on every platform.
- **macOS tar.** The archive checks run with the tar on the host. A Linux run does not exercise macOS's bsdtar; a `bash` run only approximates macOS's `/bin/sh`.
- **Real downloads.** The fetch path is exercised only up to a stub `curl` or `wget` that fails. TLS, redirects and real release URLs are not tested.
- **Gatekeeper, signing and notarization.** Not covered.
- **Windows.** Nothing here validates Windows; the installer does not support it.
