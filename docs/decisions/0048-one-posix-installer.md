# ADR 0048: One POSIX installer for luuna and lu

Date: 2026-11-12. Status: accepted under the owner's signed migration charter (step 7).

## Context

[ADR 0044](0044-one-repository-and-agent-shipping.md) consolidates Luuna into one repository. The suite hub that grew beside it has its own POSIX installer, with install, upgrade, rollback and uninstall and a test harness, built for four separate components and for a hub whose rules kept `luuna` and `lu` out of the names it owned. [ADR 0041](0041-no-repository-ci.md) leaves checks to run locally and be recorded as evidence. [ADR 0036](0036-orbit-and-project-sandbox.md) defines `lu` as the short name of `luuna`, and the terminal client is one executable, `luuna-tui`, which finds `luuna-sbx` beside itself. [ADR 0024](0024-portable-component-distribution.md) defines the portable component layout, whose trees contain symbolic links.

Today the public install paths are a Windows developer install, an unsigned Windows installer and an unverified Homebrew source-build draft. No release has been published, and releasing is an irreversible action that this change does not take.

## Decision

1. **Installer and harness folded in.** The hub's installer, its manifest library and its test harness (including the install, upgrade, rollback and uninstall cases) move into this repository under `packaging/installer/`, as `install.sh`, `install-manifest.sh`, `tests/` and `releases/`. The hub's dispatcher, ADW wrapper, Windows scripts and workflow are not carried. The user-facing description is [INSTALL.md](../INSTALL.md).
2. **Names, link table and ownership.** The archive carries `luuna-tui`, `luuna-runtime` and `luuna-sbx`. The installer links exactly three public names from a fixed table: `luuna` and `lu` to `luuna-tui`, and `luuna-runtime` to `luuna-runtime`. `luuna-sbx` sits beside `luuna-tui` and is not linked. A name is owned only if it is a symbolic link into the install home; anything else at a public name is an ownership conflict (exit 6) and is never replaced. This inverts the hub's rule against owning `luuna` and `lu`. The checkout launchers `bin/luuna`, `bin/lu` and their `.cmd` twins are for a Windows checkout and are neither shipped nor linked.
3. **Manifest format, one-repository allow-list and `v<version>` tags.** Manifests are tab-separated, with a component column, one row per component, os and arch, a SHA-256 per archive and an exact file name. URLs are allowed only as `https://github.com/luuna-ai/luuna/releases/download/v<version>/<file>`: one repository, and the `v<version>` tag form that the charter approves. See [FORMAT.md](../../packaging/installer/releases/FORMAT.md).
4. **Archive layout.** The archive may hold only `bin/` and the row's executables, as regular files, with no link, device or duplicate. No Bun and no component tree is allowed until the adapter is removed by the later wiring plans, because those trees contain symbolic links and the work would be discarded.
5. **Trust root and pinning.** Integrity rests on the per-archive SHA-256 in the manifest; there is no signature. `--manifest` trusts the file given. `--version` requires the manifest's SHA-256 to match `packaging/installer/releases/INDEX.tsv`, which is committed header-only. No manifest or checksum is committed before a release exists, and `packaging/installer/tests/check-manifests.sh` keeps released files append-only. Downloads use `https` only and are never executed.
6. **Install home, bin and environment names.** `LUUNA_INSTALL_HOME` (default `${XDG_DATA_HOME:-$HOME/.local/share}/luuna-install`), `LUUNA_INSTALL_BIN` (default `$HOME/.local/bin`), and `LUUNA_INSTALL_UNAME_S` and `LUUNA_INSTALL_UNAME_M` for tests. The names, marker and state files differ from the hub's, so the installer never adopts or removes a hub installation, and the home cannot collide with the runtime data folder `.luuna`. Project stores are never touched.
7. **Platforms and Windows.** macOS and Linux on x86_64 and arm64. Everything else, Windows included, exits 3. Windows keeps [CLI-INSTALL.md](../CLI-INSTALL.md) and [PACKAGING.md](../PACKAGING.md) and is a later part.
8. **Tests run in sandboxes, under sh, dash and bash.** Every test file runs in an `env -i` sandbox under a fresh temporary directory, with no network and no real home, with stub executables and generated archives. Each runs under `sh`, `dash` and `bash`. The tests are not part of `npm test`; CONTRIBUTING.md lists the commands.

## Consequences

This change adds the installer machinery and documents that say no release exists. It supersedes no record.

Later parts add, each when its prerequisites exist: release packaging (a packager that builds the archives, checksums and manifest outside the tree, after the wiring plans); the first tag and release (an irreversible action under the charter); the pinned manifest in `packaging/installer/releases/` and public install documentation, after that release; Windows public install; and the Homebrew formula moving from a source build to release archives. Until then `--version` always fails and the installer has nothing to install from GitHub.

The hub's rule against `luuna` and `lu` as installer-owned names is retired with the hub's installer (decision D8 of the charter). Two hub behaviours are carried and documented rather than fixed: a stale `.lock` blocks the next run until removed by hand, and POSIX `mv` has no no-replace rename, so a foreign entry created between check and rename can be replaced. Unverified: macOS, arm64, real downloads and real binaries.
