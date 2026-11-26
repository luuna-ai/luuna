# ADR 0068: Commit repository store changes

Date: 2026-11-26. Status: accepted. The owner chose on 2026-11-26 that Luuna commits its own `.luuna/` changes. This record narrows [ADR 0061](0061-repository-store-mode.md) decision 4 and [ADR 0067](0067-repository-store-by-default.md) decision 4, both of which said the runtime never stages or commits in a checkout. It builds on [ADR 0003](0003-ticket-authority.md).

## Decision

1. **Each store change is a commit.** When a project uses repository mode, the service commits `.luuna/` to the checkout's current branch after each successful component write. The writes are Missions `create`, `update` and `block`, Atlas `add`, `record`, `edit` and `delete`, and Beacon `create`, `update` and `schema`. It also commits once at the end of enabling components. The subject is `luuna: <component> <command>`. Commits are made one at a time across the runtime.
2. **Only Luuna's paths are committed.** The commit is built in a temporary index under `.git/`, starting from HEAD plus `git add --all -- .luuna`. `.gitattributes` is included only when its changes against HEAD are added lines naming `.luuna/` paths, which is what a component init writes. Anything else the developer changed, staged or left untracked stays as it was. HEAD moves with `git update-ref HEAD <new> <old>`, so a commit the developer made in the meantime makes this one fail rather than be overwritten. Afterwards the committed paths, and only those, are reset in the developer's own index, so `git status` does not show a staged revert.
3. **When nothing is committed.** No commit is made when `.luuna` is ignored by the checkout's ignore rules, when a merge, rebase, cherry-pick, revert or bisect is in progress, or when nothing changed. A git failure is logged and never fails the component call. The change is already on disk, and the next commit carries it.
4. **The same hardening as every Luuna git command.** Hooks, the file system monitor, content filters (including submodules'), signing, automatic maintenance and inherited `GIT_*` variables do not apply. The checkout's configured identity is used. When it has none, the commit is recorded as `Luuna <luuna@luuna.invalid>`. Nothing is pushed.
5. **A setting.** `LUUNA_STORE_AUTO_COMMIT` is `1` (the default) or `0`. Any other value stops startup. An in-process runtime built with `AppState::open` does not commit until `with_store_auto_commit(true)`.

## Consequences

Tickets, memory and prompts travel with the branch, and a push or pull request carries them. Luuna's commits land on whichever branch is checked out, so they appear in that branch's history and its pull requests. Worker worktrees still never write tickets: under ADR 0003 the service remains the only Missions writer.

## Evidence

`crates/luuna-runtime/tests/store_repository_git.rs` covers this with real temporary git checkouts:

- enabling, recording memory and creating a ticket each commit, and every commit touches only `.luuna/` or `.gitattributes`;
- a developer's staged edit stays staged and uncommitted, and their untracked file stays untracked;
- an ignored store is never committed;
- a developer's own `.gitattributes` edit is never committed;
- nothing is committed while a merge is in progress.

`crates/luuna-runtime/tests/data_dir.rs` covers the setting, and `crates/luuna-git` unit-tests the `.gitattributes` rule.
