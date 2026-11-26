# Luuna components

Missions, Atlas and Beacon are native Rust libraries in this repository. The runtime calls them in process, and the terminal client runs them as `lu missions`, `lu atlas` and `lu beacon` (also under the names `luuna-missions`/`lms`, `luuna-atlas`/`lat` and `luuna-beacon`/`lbc`). They need no Bun, Go or other checkout. See [ADR 0053](../docs/decisions/0053-native-component-runtime.md) and [ADR 0066](../docs/decisions/0066-components-only-native.md).

| Component | Responsibility | Crate | Origin | Data format |
| --- | --- | --- | --- | --- |
| Luuna Missions | Tickets and structured plans | [`crates/luuna-missions`](../crates/luuna-missions) | Seeds 0.6.0 (MIT) | `.luuna/missions/` |
| Luuna Atlas | Expertise and project memory | [`crates/luuna-atlas`](../crates/luuna-atlas) | Mulch 0.11.0 (MIT) | `.luuna/atlas/` |
| Luuna Beacon | Composable prompts and instructions | [`crates/luuna-beacon`](../crates/luuna-beacon) | Canopy 0.2.6 (MIT) | `.luuna/beacon/` |

The former `luuna-ai/luuna-missions`, `luuna-atlas` and `luuna-beacon` repositories were attached here as exact-commit submodules until the ports landed. Those submodules, `components/manifest.json`, the portable Bun layout and its staging script were removed in ADR 0066. Upstream license notices and provenance stay with each crate and in [THIRD-PARTY-NOTICES.md](../THIRD-PARTY-NOTICES.md).

Developer helpers: `npm run components:setup` builds the native terminal binary and checks each component answers `--help`. `npm run components:run -- luuna-missions -- --help` runs a component from your current directory. `npm run test:components` runs real isolated ticket, knowledge and prompt operations. These helpers grant no Luuna approval.
