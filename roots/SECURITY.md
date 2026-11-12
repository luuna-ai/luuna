# Security Policy

## Supported versions

Roots is pre-release (0.x). Only the latest commit on `main` receives security
fixes.

## Reporting a vulnerability

**Do not open a public issue for security vulnerabilities.**

Report vulnerabilities privately through [GitHub Security Advisories](https://github.com/brodynelly/luuna-roots/security/advisories):

1. Open the repository's Security Advisories page.
2. Create a draft advisory.
3. Describe the vulnerability and reproduction steps.

### Response timeline

- **Acknowledgment:** within 48 hours.
- **Initial assessment:** within 7 days.
- **Fix or mitigation:** within 30 days for confirmed vulnerabilities.

## System and scope

Roots is a local Rust CLI. It reads and writes a project's
`.luuna/roots/` store, exposes an optional Claude Code hook, and can launch a
locally configured `agent.command`. It may read Mission and Atlas records but
does not own or write those stores.

Security-relevant assets are human-authored intent under
`.luuna/roots/human/`, agent-owned records/artifacts, JSONL/event integrity,
and the filesystem boundary around the project. The CLI is not a network
service and does not provide a sandbox.

## Threat model and security invariants

Treat command arguments, current working directory, Roots records, hook JSON,
and output from a configured agent as untrusted inputs. A trusted local human
chooses the project and configures `agent.command`; that command is allowed to
run with the user's local authority.

The following properties must hold:

- Agent-facing commands never write human-owned prose or accept their own
  proposals. Agent output is confined to the agent-owned Roots area.
- User-controlled paths, including `..` and symlink paths, cannot make a Roots
  write escape its intended Roots subtree or turn an agent write into a human
  write.
- Human-content-writing commands require an interactive terminal and a
  resolved human actor; a forged agent identity cannot become `human:...`.
- Persistent records are written atomically and guarded by advisory locks so a
  partial write or stale lock does not silently corrupt a store.
- `verify` identifies broken ownership, graph, ledger, path, and record
  invariants; it is detection, not a substitute for filesystem permissions.
- The Claude hook is a defense against accidental harness edits, not a
  security boundary or a grant of runtime authority.

## Reportable findings

Report defects with realistic local impact, including:

- A Roots-composed shell command that lets Roots content alter execution beyond
  the configured `agent.command`.
- Any agent-facing command or hook bypass that writes into
  `.luuna/roots/human/`, including through traversal, a symlink, a legacy
  `.roots/human/` spelling, or a shell-redirection path.
- A Roots write that escapes its intended Roots area, follows an unsafe link,
  loses data during an atomic-write/lock race, or corrupts an event/JSONL
  record without detection.
- An actor-resolution or TTY bypass that permits an agent to make a
  human-authorized mutation.
- A malformed/hostile hook payload that obtains an unsupported protected-tree
  write through a recognized tool shape.

## Out of scope and known limitations

- A person or process with arbitrary local shell/filesystem access can bypass
  Roots. The human/agent split prevents accidental mixing; it is not a
  sandbox.
- Denial of service through arbitrarily large local files or deliberate lock
  contention is not currently a supported security boundary.
- Running a command a trusted local user explicitly places in
  `agent.command` is expected behavior. Interpolation of Roots data into that
  shell command is not.
- The Claude PreToolUse hook deliberately fails open for malformed/unrecognized
  payloads and only classifies supported write shapes. Luuna's runtime, not the
  hook or a Roots record, is the authority for managed-run approvals.
- Platform CI is configured for Windows, macOS, and Linux, but a configuration
  file alone is not proof of a released build's security properties.
