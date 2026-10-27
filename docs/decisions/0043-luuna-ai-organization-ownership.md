# ADR 0043: Luuna repositories under the luuna-ai organization

Date: 2026-10-26. Status: proposed; requires maintainer review.

## Context

The active Luuna, component, app and documentation repositories are being
consolidated under the `luuna-ai` GitHub organization. This decision concerns
repository ownership and canonical source URLs; it does not authorize changes
to repository visibility, credential settings, releases or deployment
configuration.

## Decision

1. Use `luuna-ai/luuna-missions`, `luuna-ai/luuna-atlas` and
   `luuna-ai/luuna-beacon` as canonical component source repositories.
2. Keep component integration pinned by exact commit. Update source URLs and
   ownership metadata without silently changing an integration pin or CLI
   contract.
3. Treat `luuna-ai/luuna-ai` as the independent desktop application repository;
   it is not a core runtime submodule.
4. Update documentation import/setup scripts to fetch repositories owned by
   `luuna-ai` and record exact source commits.
5. Keep personal repositories intact until organization-only clone, build,
   installer and release paths have been verified and maintainers approve
   retirement.
6. Do not change visibility, credentials, releases, deployments, or repository
   access settings as part of this source change.

## Verification

The existing core component commits recorded in `components/manifest.json` are
present in the corresponding organization repositories. Current `main` uses
newer commits; the consumer manifest and gitlinks must agree before any pin is
changed.

Organization ownership does not by itself establish that the repositories are
ready to replace any private source or that personal copies may be deleted.
Those conditions require separate evidence.
