# Install the Luuna command line tools on Windows

Missions, Atlas and Beacon are native in the Luuna terminal client ([ADR 0066](decisions/0066-components-only-native.md)), so installing the command line tools needs only this checkout with the terminal client built:

```powershell
.\scripts\install-cli.ps1
```

The script creates launchers under `%LOCALAPPDATA%\Luuna\bin` and adds that directory to the current user's `PATH`. Open a new terminal after the script finishes.

| Command | Runs |
| --- | --- |
| `luuna` or `lu` | Luuna terminal workbench |
| `luuna atlas ...` or `lat ...` | Atlas expertise and project memory CLI |
| `luuna beacon ...` or `lbc ...` | Beacon prompt and instruction CLI |
| `luuna missions ...` or `lms ...` | Missions tickets and work plans CLI |
| `luuna-atlas`, `luuna-beacon`, `luuna-missions` | The same component CLIs under their long names |

The component commands work from the current project directory and do not start the Luuna runtime. `luuna` still opens the workbench for its other commands. A project that uses Luuna's integrated workflows must also have its component workspace enabled; the installed commands do not bypass runtime approval or scope policy.

The installer needs no Bun and no component checkouts. It does not publish packages, change Git configuration, or install global npm packages.
