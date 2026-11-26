# ADR 0061: Repository store mode

Date: 2026-11-26. Status: accepted under the owner's migration charter, step 1, slice 4b. Builds on [ADR 0045](0045-per-project-store-mode.md) decisions 4 and 6 and on [ADR 0028](0028-luuna-project-component-layout.md). It adds no route, action, client method, table or stored format.

## Decision

1. **Repository mode resolves to the registered checkout.** When a project's `store.json` says `repository` and names a repository registered for the project, the resolver answers with that registration's checkout. The project's component stores are then `<checkout>/.luuna/missions`, `.luuna/atlas`, `.luuna/beacon` and `.luuna/roots`. The path always comes from the registration, never from the file, so after a rebind the store follows the new checkout. The old checkout's `.luuna` is left as it is, and nothing is moved or merged.
2. **The checkout is re-checked on every resolution.** These are synchronous metadata checks only, with no git process and no writes. The path must be absolute with no `..`, and every element must be a real directory, not a link or a Windows reparse point. `<checkout>/.git` must be a directory, so a linked worktree or a gitfile checkout is refused. Missions and Atlas would otherwise follow it to the main checkout's store. `<checkout>/.luuna`, when present, must be a real directory. The checkout and the data directory must not lie inside each other. A failed check answers 409 with a fixed text that names no path.
3. **The runtime never initialises a repository store.** A request that needs a component whose `<checkout>/.luuna/<component>/config.yaml` is missing is refused with 409. The message asks the developer to initialise that store in the checkout. Enabling components needs all three stores and refuses before the project is marked as initializing. Roots setup is refused the same way. Selecting `repository` in `store.json` is the developer's explicit choice, and it authorizes using the stores they initialised, as they are.
4. **Nothing outside `.luuna/<component>` is written.** Component calls take a `StoreRoot` (`crates/luuna-components`): `Managed` for the private workspace under the data directory, `Checkout` for a selected checkout. For a checkout, the runtime makes no 0700 workspace, no component home folder and no legacy-layout rename, changes no permissions, and refuses `init`. The runtime never stages or commits, and never edits `.gitignore`, `.git/info/exclude` or `.gitattributes`. It also never hides `.luuna` from a baseline's dirty count. A repository store must be ignored by the checkout's own rules or committed by the developer. An untracked store stays untracked, and baselines refuse it with the existing message. A committed store that the runtime changes shows up as unstaged edits for the developer to commit.
5. **Memory operations resolve the store before recording the operation.** Memory create, update, delete and promote now resolve every store they touch before the idempotency record is written. A refused store therefore leaves no pending record that a later retry could carry into another store. This closes the gap ADR 0045 decision 4 disclosed. Listing memory also resolves the project's store first, so a refused store answers 409 even when no memory exists yet.
6. **Both mode stays refused** until the next slice. Its 409 text now says that the runtime and repository stores are available.

## Consequences

The 409 texts for an unregistered repository and an unusable `store.json` are unchanged. Clients classify every 409 as refused, so no client changes. No product action writes `store.json` yet; choosing a mode from the product is slice 6.

## Evidence

`crates/luuna-runtime/tests/store_repository_git.rs` uses real temporary git checkouts and the native component libraries, on synthetic data. The developer's stores are initialised elsewhere and copied in. The tests cover:

- enabling components, recording and listing a memory, creating a ticket and listing prompts, all landing in the checkout's own `.luuna` stores;
- an ignored store, which leaves a clean `git status`;
- an untracked store, which stays `?? .luuna/`;
- a committed store, which changes only as unstaged `.luuna/` edits;
- HEAD unchanged, nothing staged, no file outside `.luuna/<component>` changed, no component home, no `.gitattributes`, the checkout's 0755 mode kept, and no managed workspace;
- a missing store refused and never created;
- a linked worktree, a data folder inside the checkout and a linked `.luuna` refused, with the fixed text on the components, memory, prompt and ticket routes;
- the store following a rebind.

`store_resolver.rs` and `store_repository.rs` keep the refusal cases that need no git.
