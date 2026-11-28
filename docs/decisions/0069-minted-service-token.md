# ADR 0069: Luuna mints the local service token, so nobody types one

Date: 2026-11-26. Status: accepted. Builds on [ADR 0050](0050-first-run-terminal-setup.md) and [ADR 0065](0065-desktop-local-start-from-the-installed-release.md). Part of the repo-local, zero-setup direction (ADRs 0067 and 0068).

## Context

The runtime refuses to start without `LUUNA_API_TOKEN`. `lu init` generated one, but only on Unix (from `/dev/urandom`); on Windows it stopped and asked for platform setup. Without `lu init`, the workbench and `lu` commands asked for the token in a hidden prompt, and the desktop said "No service token is saved; run lu init first". Agent platforms and local daemons we studied (Docker, Jupyter, language servers, Codex and Claude Code's local services) generate a per-machine secret and hand it to their own clients through an owner-only file; the user never sees it.

## Decision

1. **Cross-platform minting.** `setup::mint_token` reads 32 bytes from the operating system's random source (`getrandom`) and returns them as 64 hex characters, on every platform. `lu init` uses it too.
2. **Minted on first launch.** When neither the environment nor the private settings hold a token, the URL is the default `http://127.0.0.1:4242`, and nothing answers its unauthenticated health check, the terminal and `luuna-tui desktop-connection` mint a token and save it in the private `environment.json` (owner-only `0600`, in an owner-only `0700` folder that must be outside every repository), together with the default data folder. Later launches reuse it.
3. **Ready on launch.** Opening the workbench or running a command that is not help then starts the installed runtime with the saved settings when it is not already running, the same way `lu init` does. A failure is reported and the workbench still opens.
4. **Never over somebody else's runtime.** If a runtime already answers on the default port, Luuna cannot know its token, so it mints nothing and the hidden prompt stays. An explicit `LUUNA_API_TOKEN` or a non-default `LUUNA_URL` always wins. Help, `catalog` and `--help` never mint or start anything.
5. **Safe with several launches and with init.** Minting holds an exclusive lock file in the private folder and re-reads the settings under it, so concurrent first launches adopt one token. An empty `LUUNA_API_TOKEN` counts as unset everywhere, so the minted token reaches the runtime and the relaunched client. When `lu init` changes the settings and a runtime started earlier with the saved token answers, init stops it so the next start applies the new settings; a runtime that rejects the saved token is left alone.
6. **The token never reaches a repository or an agent.** It lives only in the private settings, and the existing rules still withhold it from sandboxed and component processes.

## Consequences

A clean install opens the workbench or the desktop without typing a token, on macOS, Linux and Windows. A provider key is still needed for agent work; the credential chain is a separate change. Someone who runs `luuna-runtime` by hand still supplies their own token.

## Evidence

`crates/luuna-tui/tests/setup.rs`: minted tokens are 64 hex characters and differ; a token is minted and saved with `0600`/`0700` when nothing answers and reused on the next launch; nothing is minted or saved over an explicit token, another URL or a runtime that already answers; `doctor --help` creates no private folder; three concurrent first launches end with one saved token; init stops a runtime that accepts the saved token. By hand: a fresh home with `LUUNA_API_TOKEN` exported empty minted a token, started the runtime and passed doctor, and `POST /api/v1/shutdown` stopped it. The whole `luuna-tui` test suite passes.
