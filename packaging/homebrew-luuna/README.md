# Luuna Homebrew packaging

This directory contains the source-side packaging implementation for the
public `luuna-ai/homebrew-luuna` tap, which currently contains a draft formula.
The formula has not passed Homebrew audit and source-build installation checks
on supported macOS and Linux hosts. Treat it as unverified; do not present it as
a supported install path yet.

It builds the **Luuna command-line suite** (runtime, terminal client and the
native Missions, Atlas and Beacon commands) from one immutable commit of this
repository. It does not
install the Luuna workbench desktop app or the separate Luuna AI desktop app.
The intended install command after platform validation is
`brew install luuna-ai/luuna/luuna`.

## Contents

- `Formula/luuna.rb` — pinned source-build formula for Apple Silicon/Intel macOS
  and Linux x86_64/aarch64.
- `scripts/luuna-doctor` — checks every installed command and reports the
  platform and separate provider/Pi prerequisites without exposing secrets.
- `DESKTOP-RELEASE-MATRIX.md` — exact artifacts needed before adding casks.

The source is pinned by one Git commit. Missions, Atlas and Beacon are native
Rust in the same build (ADR 0066), so the formula fetches no other repository
and needs no Bun or Go. Cargo.lock governs build dependencies. Building
requires Rust 1.90.0 or newer; the formula checks it.

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
