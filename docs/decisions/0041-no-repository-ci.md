# ADR 0041: no repository CI/CD

Date: 2026-10-26. Status: accepted at the owner's request; point 3 superseded by [ADR 0044](0044-one-repository-and-agent-shipping.md) (how changes to this repository ship; points 1, 2, 4 and 5 remain in force). Revises the M0 work package P00 ("repository/CI bootstrap") and the M0 exit criterion "Mac/Windows CI builds a skeleton binary" of [docs/10-roadmap.md](../10-roadmap.md), and the "Linux is recommended for CI" note of [docs/11-acceptance.md](../11-acceptance.md). Every other requirement is unchanged.

## Context

The repository carried one GitHub Actions workflow (`.github/workflows/ci.yml`, "Luuna foundation": Rust on three platforms, frontend contracts, the native terminal audit, component pins and the native desktop check). Hosted runs had not started since October 21, 2026: every job was refused before its first step, which [MAIN-INTEGRATION-2026-10-21.md](../MAIN-INTEGRATION-2026-10-21.md) and [RELEASE-EVIDENCE.md](../RELEASE-EVIDENCE.md) already record as unavailable hosted evidence. Pull requests therefore showed red checks that said nothing about their changes, and agents working the repository kept treating those checks as blockers to be investigated. The owner decided that the repository has no CI/CD at all.

## Decision

1. **No hosted workflow.** `.github/workflows/ci.yml` is removed and no workflow, status check, deployment pipeline or release automation is to be added to this repository. A red or missing hosted check is not a signal about a change and is never a reason to hold, investigate or re-run anything.
2. **Checks run locally and are recorded as evidence.** The commands the workflow ran are the repository's local checks and are listed in [CONTRIBUTING.md](../../CONTRIBUTING.md). A change's evidence is the commands actually run, on the host they ran on, recorded in the pull request, the handoff or the relevant evidence document, with what was not run named. This is the discipline [FOUNDATION-STATUS.md](../FOUNDATION-STATUS.md) and [RELEASE-EVIDENCE.md](../RELEASE-EVIDENCE.md) already follow; nothing about evidence standards is loosened.
3. **Merging stays the developer's decision**, as before (AGENTS.md). Nothing is merged automatically and nothing waits on a hosted check. (Superseded by [ADR 0044](0044-one-repository-and-agent-shipping.md): changes to this repository are merged by agents through the ship gate under the owner's signed migration charter, and nothing waits on a hosted check.)
4. **The product is unchanged.** Luuna's runtime still observes CI on the pull requests *it publishes to a developer's own repositories* ([ADR 0018](0018-publication-and-doctor.md)): that is a feature about other projects' checks, not this repository's. The validation recipes a project manifest calls "required checks" are likewise unrelated.
5. **Historical records stay as written.** Documents that describe hosted runs that did happen (the component extraction, the M3 matrix, the terminal verification) remain accurate history; they are not rewritten.

## Consequences

Cross-platform evidence must come from a person or agent running the checks on that platform, which is how the recorded Windows and macOS evidence was produced in any case. Future sessions should not create `.github/workflows`, should not describe a change as blocked on CI, and should not expect `git push` to run anything. The component repositories (`luuna-missions`, `luuna-atlas`, `luuna-beacon`) are independent and keep their own CI; this decision is about this repository only. [ADR 0044](0044-one-repository-and-agent-shipping.md) supersedes ADR 0012's separate-repository model; until later migration changes bring the components into this repository, the component repositories keep their own CI.
