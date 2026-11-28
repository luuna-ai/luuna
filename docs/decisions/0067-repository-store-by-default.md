# ADR 0067: Repository store by default

Date: 2026-11-26. Status: accepted under the owner's migration charter. This record supersedes decision 3 of [ADR 0061](0061-repository-store-mode.md), and narrows decision 4 of that ADR as described below. It builds on [ADR 0045](0045-per-project-store-mode.md) and [ADR 0062](0062-choosing-the-store-mode.md). It adds no route, client method, table or stored format.

## Context

The owner wants Luuna's project state to live in the repository Luuna works on, and wants Luuna to run as soon as it is launched. Research into other coding agents and agent platforms found that they share one pattern (see the design doc "Repo-local, zero-setup Luuna"). Project knowledge is committed under a dot folder in the repository. Secrets, transcripts and trust decisions stay with the user. Seeds and Mulch, the originals of Missions and Atlas, keep their JSONL records committed under `.seeds/` and `.mulch/`, with `merge=union`.

Until now a new project used the runtime store under the data directory. Repository mode needed a store file written by hand or chosen from the product, and it needed stores the developer had initialised in the checkout first.

## Decision

1. **New projects default to their checkout.** When a project is registered, the service records repository mode for its first repository. This happens only when the checkout passes the checks of ADR 0061 decision 2, none of its `.luuna/{missions,atlas,beacon,roots}` stores holds records yet, and it has no `.seeds`, `.mulch` or `.canopy` store from the upstream tools. Registration holds the component lock from before the project is stored until the mode is recorded, so no component request can reach the new project in the runtime store first. In any other case the project keeps the runtime store, and the reason goes to the service log. Records are never adopted, moved or merged. A store mode file that already exists is kept. A removed project that is registered again comes back with the mode it had. The change is recorded as a `project.store_mode.changed` event with `"reason": "default"`.
2. **The default is a setting.** `LUUNA_DEFAULT_STORE_MODE` is `repository` (the default) or `runtime`. Any other value stops startup. An in-process runtime built with `AppState::open` keeps the runtime default until `with_default_store` changes it, so embedding code and existing tests keep their behaviour.
3. **The runtime may initialise a checkout store.** Enabling components creates any `.luuna/<component>` store that does not exist yet, using that component's own `init`. A developer running `lu missions init`, `lu atlas init` or `lu beacon init` gets the same files. Anything already at that path must be usable as it is: a folder without `config.yaml`, a link or a special file is refused exactly as before. A checkout that still has a `.seeds`, `.mulch` or `.canopy` store is refused before anything is written, because a component's `init` would move that store and rewrite ignore files. Roots setup is unchanged.
4. **What is written outside `.luuna/<component>`.** A component `init` in a checkout also appends that component's union-merge lines to the checkout's `.gitattributes`, as the upstream tools do. Nothing else changes from ADR 0061 decision 4: the runtime writes no other file, changes no permissions, and never stages or commits anything. The developer reviews `.luuna/` and `.gitattributes` and commits them, or ignores them.

## Consequences

A project registered in a git checkout keeps its tickets, memory and prompts in `<checkout>/.luuna/` from its first use, with no setup step. Teammates who clone the repository get the committed records. A checkout whose stores already hold records, such as a clone of a repository someone else set up, keeps the runtime store for now. Using those records needs the explicit migration step from the design doc, which is later work, because ADR 0003 allows only one writable ticket authority.

Both mode stays refused. The owner chose that Luuna commits its own `.luuna/` changes; [ADR 0068](0068-commit-repository-store-changes.md) does that, and narrows decision 4's "never stages or commits".

## Evidence

`crates/luuna-runtime/tests/store_repository_git.rs` covers this with real temporary git checkouts:

- a missing store is initialised in the checkout, `.gitattributes` gains only the union lines, HEAD is unchanged, nothing is staged, and no runtime store is created;
- a store folder that exists but is unusable is refused, left alone, and does not freeze the project;
- a checkout with an upstream `.canopy` store is refused, with nothing moved or rewritten;
- a new project registered over the API defaults to its checkout, records one mode event, and enables, records memory, creates a ticket and lists prompts there;
- a checkout whose stores already hold records, or that has an upstream `.seeds` store, keeps the runtime store, with no mode file and no event;
- the runtime default records no mode.

`crates/luuna-runtime/tests/data_dir.rs` covers reading the setting. `crates/luuna-components` now allows `init` on a checkout store root, and each component's preflight still refuses links.
