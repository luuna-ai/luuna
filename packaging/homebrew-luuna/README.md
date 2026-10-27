# Luuna Homebrew packaging (staged)

This directory is a tap-ready packaging draft, not a published tap. It builds
the **Luuna command-line suite** (runtime, terminal client, Missions, Atlas,
and Beacon) from immutable organization repository commits. It does not install
the Luuna desktop app or the separate Luuna AI app.

The supported command will be `brew install luuna-ai/luuna/luuna` after these
files are reviewed and copied to a dedicated `luuna-ai/homebrew-luuna` tap.
That repository does not exist yet, so do not advertise this as a usable live
Homebrew command.

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

## Commands after the tap is created

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

This Linux workspace has no Homebrew, Ruby, Rust, Bun, or usable Go toolchain,
so no Homebrew installation, native Mac build, or platform claim is made here.
