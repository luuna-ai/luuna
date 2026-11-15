# ADR 0049: opt-in OpenAI OAuth for isolated Pi workers

Date: 2026-11-13. Status: accepted for the owner's local runtime setup.

## Decision

An explicitly selected OpenAI OAuth grant may be shared with Luuna's supervised
Pi workers. This is opt-in, using `tools/pi/oauth-launch.mjs` as the Node launcher
and `LUUNA_PI_AUTH_FILE` for credential-presence diagnostics and redaction.
No ordinary Pi settings, extensions, skills, project context or session history
are imported. The ordinary API-key launcher remains the default.

The adapter requires Pi 1.3.0 and absolute `--luuna-auth-file` and
`--luuna-pi-root` arguments. It adapts the pinned SDK's ModelRuntime factory
because the CLI does not expose a credential-store parameter. The normal CLI
and RPC implementation still own tools, streams and session lifecycle.

Only the `openai` OAuth entry is routed to the selected source AuthStorage.
Pi's own cross-process credential-store lock governs refresh and preserves
other provider entries. No grant is copied into per-run directories. Workers
cannot log the user out. The grant must include `chatgpt.tokens.use.direct`;
OpenAI API-key environment fallbacks are removed in this launcher.

The runtime reads only this explicitly configured grant for diagnostic presence
and access/refresh-token redaction. A configured grant is not proof of model
entitlement. Only a completed real inference establishes access for that request.

## Evidence and limits

The macOS local runtime completed a tool-free OpenAI/gpt-6.1-sol request with
the developer's existing OAuth grant on November 13, 2026. Synthetic tests cover
provider isolation, refresh writes, logout refusal, missing grants and token
redaction. This does not establish a passing project baseline, writer/reviewer
acceptance, distribution support, or real expired-token recovery.
