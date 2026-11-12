# ADR 0044: one repository, native components, and agent shipping under a signed migration charter

Date: 2026-11-12. Status: accepted by the owner's signed migration charter. Supersedes [ADR 0012](0012-independent-component-repositories.md) in whole, and supersedes point 3 of [ADR 0041](0041-no-repository-ci.md) (point 3: how changes to this repository are merged); ADR 0041 points 1, 2, 4 and 5 remain in force.

## Context

[ADR 0012](0012-independent-component-repositories.md) made Missions, Atlas and Beacon independent public repositories, consumed by Luuna as exact-commit Git submodules under `components/`. [ADR 0041](0041-no-repository-ci.md) removed repository CI and left merging of changes to this repository as the developer's decision, with no automated gate of any kind.

The owner has now signed one migration charter. It consolidates Luuna into one repository, ports the maintained components to native Rust, and lets agents carry changes to this repository from branch to merge, so that the owner no longer approves individual plans or merges. This record writes the rules of that charter down for the repository, as step 0 of the migration. It is a statement of rules and direction. It changes no code, manifest, pin or submodule.

## Decision

1. **One repository.** ADR 0012 is superseded in whole. Luuna becomes one repository. Missions, Atlas and Beacon are ported to native Rust in this migration, with parity tests and the same `.luuna` data formats. Roots is imported into this repository and called natively. This record itself removes or changes no submodule, gitlink, `components/manifest.json` pin, adapter or command: until later migration changes replace them, the existing submodules, pins and component commands remain how Luuna consumes the components, and they are kept synchronized. Upstream attribution and license notices of derived components remain required.
2. **Stores, as direction.** The runtime store is authoritative by default and is located with the runtime data folder. Each project chooses a mode of runtime, repository, or both; in `both`, exactly one side is writable. SQLite is a rebuildable index. `.luuna` files stay human-readable. Implementation is later work.
3. **How changes to this repository ship, replacing ADR 0041 point 3.** The owner signs one migration charter instead of approving individual plans or merges. An independent plan auditor checks every plan against that charter. Agents push branches, open pull requests and merge them only through the ship gate. Agent reviews are the only gate on a merge until the CI pipeline of item 4 exists. Agents perform only the irreversible actions (tags, releases, archiving repositories) that both the charter and the audited plan list, and they do so through the ship gate. Agents may start other agents only when bound to an audited plan. This record was itself written and merged by agents under this model. The ship gate is migration tooling outside this repository and outside Luuna's runtime. It is described here only by the guarantees the charter's safeguards give: nothing ships except through it; it cannot force-push or rewrite history, delete a repository, change visibility, or publish to a package registry; and a step that needs one of those halts and asks.
4. **Checks.** ADR 0041 points 1 and 2 remain in force during the migration: no workflow is added, and checks run locally and are recorded as evidence. This record adds no workflow. One new CI pipeline for the consolidated repository follows the migration under its own record, which is where ADR 0041 point 1 is to be revised.
5. **The product is unchanged.** In a developer's own repositories, the developer controls merging; Luuna's runtime has no automatic merge operation in v1. Requirement R12, acceptance item A34, [ADR 0018](0018-publication-and-doctor.md) and the runtime's enforcement of a human merge policy stand unchanged. ADR 0041 point 4 stands. This record changes no runtime behaviour: it governs how this repository is developed, not what Luuna does for the projects it serves.

## Consequences

The current documents that stated ADR 0012's model or ADR 0041 point 3 are updated in the same change: AGENTS.md, CONTRIBUTING.md, SECURITY.md, README.md, AI-START-HERE.md, components/README.md, docs/CURRENT-ARCHITECTURE.md, docs/IMPLEMENTATION-ROADMAP.md, and status notes on ADRs 0012, 0041, 0014, 0024, 0028 and 0034. Dated evidence and historical reference documents stay as written, consistent with ADR 0041 point 5 and AGENTS.md. The process-role contracts in `agents/` are unchanged.

Proposed ADR 0043 rests on the separate component repositories. It is neither accepted nor changed by this record, and it must be revisited before it is accepted.

Later migration changes do the actual consolidation and removal: the native ports, the built-in Roots, the store modes and the new CI pipeline. Until each of them lands, the behaviour described by the earlier records stays in place.
