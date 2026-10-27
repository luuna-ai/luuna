# Desktop cask release requirements

The Luuna CLI formula is separate from desktop applications. Do not add a cask
until each application has immutable, checksummed, tested artifacts.

| Application | macOS Apple Silicon | macOS Intel | Linux | Current blocker |
|---|---|---|---|---|
| Luuna workbench (`luuna` repository) | `.dmg` or `.app.tar.gz`, SHA-256, launch/upgrade/uninstall smoke | same | `.AppImage` or `.tar.gz`, SHA-256, launch/upgrade/uninstall smoke | no release assets; native Mac/Linux package evidence absent; core ADR 0041 explicitly says no repository CI/CD |
| Luuna AI (`luuna-ai` repository) | signed/notarized `.dmg` or `.app.tar.gz`, SHA-256 | signed/notarized `.dmg` or `.app.tar.gz`, SHA-256 | not advertised as supported | no release assets or bundling workflow; README currently only documents Windows and macOS |

For each supported macOS architecture, publish a universal artifact or an
architecture-specific artifact. Record Tauri/WebView prerequisites, Pi
dependency behavior, signing/notarization identity and verification steps.
Release automation must be separately authorized and must not add secrets to
the repository. Until those artifacts exist, a source-built CLI formula must
not be described as the whole desktop system.
