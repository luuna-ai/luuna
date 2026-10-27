# Local agent prompt: clone and set up the Luuna system

Paste the prompt below into a local coding agent running on the machine where
you want to develop or use Luuna. Review platform prerequisites and the plan
before allowing any install commands.

```text
Set up the Luuna system from the current public repositories in the luuna-ai
GitHub organization. First inspect each repository's current README, AGENTS.md,
AI-START-HERE.md (where present), setup scripts, lockfiles, and platform notes.
Do not use personal forks, saved snapshots, moving dependency branches, or
unverified installers.

Repositories to inspect:
- https://github.com/luuna-ai/luuna
- https://github.com/luuna-ai/luuna-atlas
- https://github.com/luuna-ai/luuna-beacon
- https://github.com/luuna-ai/luuna-missions
- https://github.com/luuna-ai/luuna-ai
- https://github.com/luuna-ai/luuna-documentation

Treat `luuna` plus its pinned Atlas, Beacon, and Missions components as the core
CLI/runtime system. Treat `luuna-ai` as a separate desktop app, and the Luuna
workbench desktop target as a separate target too. Do not describe a CLI-only
setup as the complete desktop system. Do not run `brew tap`/`brew install` for
Luuna unless the official tap and formula have actually been published and
verified in the organization. If they are unavailable, say so and use only the
repositories' documented development-checkout setup.

Before changing the machine, report the OS/version, CPU architecture, available
disk space, and detected toolchains. Identify prerequisites from current repo
instructions (including Rust, Node/npm, Bun, Go, Git, and platform-native GUI
libraries as applicable). Give a concise plan with the exact commands and
expected disk/build impact. Do not install packages or change system settings
until the user approves that plan.

After approval, create one clearly named workspace directory and clone the
organization repositories there. For `luuna`, initialize recursive
submodules and verify every component checkout matches `components/manifest.json`
and the recorded public organization repository/commit. Keep each checkout on
the intended documented branch/revision; do not silently upgrade pins or edit
dependency manifests to work around incomplete installs. Keep the application
repository and the desktop app repository separate.

Use the supported OS-specific dependency installation method from each repo's
instructions. Prefer existing system package managers. Do not use `curl | sh`,
unreviewed remote scripts, `sudo` unless a documented prerequisite truly
requires it and the user separately approves, or destructive cleanup. Use
locked/frozen dependency installs. Never put credentials in command history,
files, logs, generated prompts, or test output. Never request or create API
keys, GitHub tokens, signing credentials, or production secrets. If a task
requires one, stop that task and state the exact blocker.

Build and test the CLI/runtime and each component using the repository's
documented checks. Build either desktop app only when its documented native
dependencies are available. Detect macOS/Linux/Windows and architecture before
choosing commands; do not claim support based on another OS's build. Do not
publish releases, deploy, sign/notarize, change repository settings, push
branches, create pull requests, or modify/delete existing user data.

Verify command availability and report the actual executable paths and help
results for supported aliases: `lu` and `luuna` (terminal client),
`luuna-runtime`, Missions `luuna-missions`/`lms`/`lm`, Atlas
`luuna-atlas`/`lat`/`la`, and Beacon `luuna-beacon`/`lbc`/`lb`. Run the
repository's runtime/component health or smoke checks with temporary test data
where possible, and clean up only those temporary files created by the checks.
Do not claim model-backed functionality until Pi and a provider have been
configured by the user. Explain how to configure them without asking the user
to paste secrets into the agent.

Finish with a concise report: OS/architecture; repositories and exact commits
cloned; prerequisites found/installed; commands built and their paths; tests
run with pass/fail/skip details; health-check results; desktop targets that
remain unbuilt; and remaining manual steps. Report every failure accurately.
Do not improvise fixes outside the repositories' documented procedures.
```
