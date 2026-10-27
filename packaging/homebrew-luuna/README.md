# Luuna Homebrew packaging

This directory contains the source-side packaging implementation for the
public `luuna-ai/homebrew-luuna` tap, which currently contains a draft formula.
The formula has not passed Homebrew audit and source-build installation checks
on supported macOS and Linux hosts. Treat it as unverified; do not present it as
a supported install path yet.

It builds the **Luuna command-line suite** (runtime, terminal client, Missions,
Atlas, and Beacon) from immutable organization repository commits. It does not
install the Luuna workbench desktop app or the separate Luuna AI desktop app.
The intended install command after platform validation is
`brew install luuna-ai/luuna/luuna`.

## Contents

- `Formula/luuna.rb` — pinned source-build formula for Apple Silicon/Intel macOS
  and Linux x86_64/aarch64.
- Luuna's existing `scripts/stage-components.mjs stage-homebrew` path — verifies
  component commit pins, installs locked Bun dependencies without lifecycle
  scripts, builds native Go CLIs, and creates the runtime-verified portable
  component layout. It shares the same digest code as regular packaging.
- `scripts/luuna-doctor` — checks every installed command and reports the
  platform and separate provider/Pi prerequisites without exposing secrets.
- `DESKTOP-RELEASE-MATRIX.md` — exact artifacts needed before adding casks.

The source is intentionally pinned by Git commit (including every component).
The bundled Bun runtime is pinned to v1.4.3 with SHA-256 values from its tagged
upstream release. Cargo.lock, each component Bun lockfile, and the Go module
checksums govern build dependencies. Building requires Rust 1.90.0 or newer,
Go 1.27.0 or newer, Node.js 24.19.0 or newer, and Git. The formula checks the
minimum Node.js, Go and Rust versions and prevents Go from downloading another toolchain.

## Intended commands after platform validation

```sh
brew tap luuna-ai/luuna
brew install luuna-ai/luuna/luuna
luuna-doctor
lu help
luuna-runtime --help
luuna-missions --help
luuna-atlas --help
luuna-beacon --help
```

Upgrade with `brew upgrade luuna`; remove the managed software with
`brew uninstall luuna`. Homebrew does not delete user data in `~/.luuna`.
Back up that directory before manual removal. The runtime requires a configured
Pi executable and provider credentials to perform model-backed work; install
and configure Pi separately. Installation itself does not write credentials.

## Local review

On a supported Mac or Linux host with Homebrew, run:

```sh
brew install --build-from-source --formula ./packaging/homebrew-luuna/Formula/luuna.rb
luuna-doctor
```

The formula has not yet been installed or audited on supported Homebrew hosts;
these commands are validation steps, not evidence of a successful install.
