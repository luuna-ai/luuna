# Rust compatibility fixtures

These fixtures freeze observable behavior from the TypeScript/Bun reference
implementation at Git commit `ce22500` (`feat: adapt Roots for Luuna suite`).
They are deliberately language-independent: a Rust test should invoke the
candidate binary with the manifest arguments, normalize only the explicitly
listed nondeterministic fields, and compare the result to these files.

## Capture host

- Windows `x86_64-pc-windows-msvc`
- Bun `1.4.3`
- Reference entrypoint: `bun run <checkout>/src/index.ts`

The Windows path separators in these snapshots are intentional. A future
cross-platform fixture may declare a separate platform baseline; it must not
silently rewrite this one.

## Normalization

The manifest permits only these substitutions:

| Placeholder | Source value |
| --- | --- |
| `<ROOT>` | Temporary project root used for the capture |
| `<PROJECT>` | Basename of that temporary root |
| `<HUMAN>` | Resolved human actor from Git configuration |
| `<CREATED_AT>` | UTC creation timestamp |
| `<EXPIRES_AT>` | UTC sprout-expiry timestamp |
| `<SPROUT_ID>` | Generated sprout ID (`s-` plus random hex) |
| `<SPROUT_HEX>` | Hex portion of `<SPROUT_ID>` |

No other output normalization is permitted. In particular, JSON member order,
human-facing legacy `roots` wording, whitespace, line endings within output,
and backslash paths are part of this baseline.

## Fixture flow

`init` runs in an empty temporary directory, followed by:

```text
sprout "Agent candidate" --as agent:fixture --json
list --json
show <SPROUT_ID> --json
view --sprouts
view --sprouts --json
prime --json
```

The fixture only writes agent-owned test content; no human intent file is
created or hand-edited.
