# Installing Luuna on macOS and Linux

This page describes `packaging/installer/install.sh`, the POSIX installer for Luuna. The decision behind it is [ADR 0048](decisions/0048-one-posix-installer.md).

No release of Luuna has been published yet. The installer therefore has nothing to install from GitHub today: `--version` reads `packaging/installer/releases/INDEX.tsv`, which has no entries, and the manifest it would fetch archives from does not exist. Its tests exercise it with `--manifest` and `--asset-dir`, using generated stub archives and local files only (`packaging/installer/tests/`). Nothing on this page is a promise that a download exists.

Supported platforms: macOS and Linux (x86_64, arm64).

Windows: not supported by this installer. Windows users should follow [CLI-INSTALL.md](CLI-INSTALL.md) (developer install from a checkout) and [PACKAGING.md](PACKAGING.md) (the unsigned Windows installer).

## What it does

The installer installs one release of Luuna for the current user, without sudo. A release archive carries three executables in its `bin/` folder: `luuna-tui`, `luuna-runtime` and `luuna-sbx`. The installer:

- links `luuna` and `lu` to the installed `luuna-tui` (both names reach the same client);
- links `luuna-runtime` to the installed `luuna-runtime`;
- leaves `luuna-sbx` beside `luuna-tui` in the same version folder, where the client finds it; it is not linked and need not be on `PATH`.

It never edits shell profiles or `PATH`. Add the bin directory to `PATH` yourself.

An existing `luuna`, `lu` or `luuna-runtime` that the installer did not install (a regular file, or a link made by Homebrew or by a checkout) is an ownership conflict: the installer exits 6 and never replaces or removes it. After an install, if `luuna` or `lu` on `PATH` resolves to a different path than the installed link, the installer prints a note naming that path and changes nothing.

The checkout launchers `bin/luuna`, `bin/lu`, `bin/luuna.cmd` and `bin/lu.cmd` are for a Windows checkout (Git Bash and cmd). They are not part of the archive and the installer does not install or link them.

The archive carries no Bun and no component trees. Until the native ports replace the component adapter, an installed `luuna` is not a complete product; the release-packaging part decides the payload (ADR 0048).

## Commands

```sh
sh packaging/installer/install.sh --manifest PATH [--asset-dir DIR] [--dry-run]
sh packaging/installer/install.sh --version V [--asset-dir DIR] [--dry-run]
sh packaging/installer/install.sh --rollback [--dry-run]
sh packaging/installer/install.sh --uninstall [--dry-run]
sh packaging/installer/install.sh --help
```

- `--manifest PATH` installs the release a manifest file describes ([FORMAT.md](../packaging/installer/releases/FORMAT.md)). You trust the file you give.
- `--version V` installs `packaging/installer/releases/V/manifest.tsv` from the checkout and requires its SHA-256 to equal the line for `V` in `packaging/installer/releases/INDEX.tsv`. No release is published, so this currently always fails.
- `--asset-dir DIR` reads the archives from `DIR` (offline). Without it, archives are fetched from the manifest's `https` URLs with `curl` or `wget`.
- `--rollback` swaps the current and previous installed versions.
- `--uninstall` removes the links the receipt names and the install home. Project stores and every `.luuna` folder are never touched.
- `--dry-run` validates and prints the plan without writing anything.

## Install home, bin and environment

| Variable | Default |
| --- | --- |
| `LUUNA_INSTALL_HOME` | `${XDG_DATA_HOME:-$HOME/.local/share}/luuna-install` |
| `LUUNA_INSTALL_BIN` | `$HOME/.local/bin` |

`LUUNA_INSTALL_UNAME_S` and `LUUNA_INSTALL_UNAME_M` override `uname -s` and `uname -m`. They exist for tests; they select which checksummed manifest row is used, not what is trusted.

Both roots must be absolute, not `/`, free of control characters and of `.`, `..` and `//` segments, not `HOME` or an ancestor of it, not a symbolic link, not nested in each other, and not inside the repository that holds the script. The installer writes only under these two directories.

## Layout and state files

Under `LUUNA_INSTALL_HOME`:

- `.luuna-install-home`, the marker (`luuna-install-home 1`); recursive removal needs it;
- `versions/<version>/bin/` with the three executables, and `versions/<version>/.luuna-install-version.tsv`;
- `current` and `previous`, regular files naming the active and the previous version;
- `receipt.tsv`, which names the home, the bin directory, the current and previous versions and the links the installer owns. The receipt write is the commit point of an install;
- `.lock/` while a run holds the lock, and `.stage.*`, `.tmp.*` and `.trash.*` while a run is in progress.

Under `LUUNA_INSTALL_BIN`: the links `luuna`, `lu` and `luuna-runtime`, each pointing into `LUUNA_INSTALL_HOME/versions/<version>/bin/`. A name is owned only if it is such a link. The installer refuses to run when `receipt.tsv` was written for different roots, or when `current` exists without a receipt. It never adopts an installation made by any other installer.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | ok |
| 2 | usage or invalid input |
| 3 | unsupported platform, or no archive for this platform |
| 4 | checksum mismatch |
| 5 | unsafe or malformed archive |
| 6 | ownership conflict |
| 7 | fetch failure |
| 8 | another install holds the lock |

## Upgrade, rollback and uninstall

Installing a different release stages and verifies everything first, then moves the links. The installer keeps exactly one previous version beside the current one and removes older version folders. Installing the version that is already current changes nothing.

`--rollback` swaps current and previous and relinks; it exits 2 when there is no previous version. Running it twice returns to where you started.

`--uninstall` removes only what the receipt names: the public links it owns, then the install home. Links it does not own are left. Project stores (`.luuna/`), `~/.luuna` and repositories are never written or removed.

## Trust model

Integrity rests on the SHA-256 of each archive, recorded in the release manifest. There is no signature. `--manifest` trusts the local file you pass. `--version` pins the manifest by the SHA-256 recorded in `packaging/installer/releases/INDEX.tsv` in your checkout, which is the defence against a swapped manifest. Fetches use `https` only (`curl --proto =https --proto-redir =https --tlsv1.2`, or `wget --https-only`). Each archive is checked against its checksum, and its listing is checked against an allow-list (`bin/` and the row's executables only; no links, devices, traversal or duplicates) before anything is extracted. The installer never runs a downloaded file and never evaluates data as code.

## Transaction and recovery

The commit phase (version folder, pointers, links, receipt) is journaled under the lock in a `.trash.*` folder. If the installer fails, or receives HUP, INT or TERM, before the receipt is written, it replays the journal and restores the previous state; foreign entries it meets in the bin directory during the undo are left alone.

If the undo itself fails, the installer says so and keeps the journal and the marker. Install and `--rollback` may then refuse. To recover by hand:

1. Inspect the retained `.trash.*` folder in `LUUNA_INSTALL_HOME`. It holds copies of `current`, `previous` and `receipt.tsv`, a record of the links, and possibly an `old-version` folder.
2. Move `old-version` back to `versions/<version>` if it is there.
3. Do not run `--uninstall` first: it deletes the journal.
4. Rerun the install or `--rollback`. A later locked run with valid pointers sweeps the journal.

### Behaviours carried as they are

Two behaviours of the installer this one was derived from are carried unchanged and are not fixed here.

- A stale lock blocks the next run. A crash can leave `LUUNA_INSTALL_HOME/.lock` behind; the next run exits 8 and names it. If no other install is running, remove that directory by hand and retry.
- There is no no-replace rename in POSIX `mv`. Every check-then-rename or check-then-unlink of an entry in the bin directory leaves a narrowed but open window: a foreign entry created in that instant can be replaced or removed. The installer is not a privilege sandbox.

## What is not covered

The Homebrew formula in `packaging/homebrew-luuna` is an unverified source-build draft, not a supported install path (see the Homebrew section of [README.md](../README.md)). Signing, notarization and Windows are separate work. The installer has been exercised only with stub executables and local archives; macOS, arm64, real downloads and real Luuna binaries are unverified.
