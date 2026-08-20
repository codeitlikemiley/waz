# Architecture

```
┌─────────────────────────────────────────────────────┐
│              Shell Integration Layer                │
│  Zsh (ZLE ghost text)  Bash  Fish  PowerShell       │
│  Cmd+I / Ctrl+T → TUI   waz hint → Tier 0           │
└──────────────────────┬──────────────────────────────┘
                       │
         ┌─────────────▼─────────────┐
         │       waz (Rust)          │
         │  Predict: hint → sequence │
         │           → workflow → cwd│
         │           → LLM           │
         │  TUI: / TMP  ! shell  AI  │
         │  TMP schemas + resolvers  │
         │  LLM: 5 wire protocols    │
         │  MCP stdio + Agent Plugins│
         │  SQLite history           │
         └───────────────────────────┘
```

Prediction tiers are described in the [root README](../README.md). TMP protocol: [tmp.md](tmp.md). LLM: [ai.md](ai.md). Plugins: [plugins.md](plugins.md).

## Data storage

| What | Typical path |
|------|----------------|
| History DB | macOS `~/Library/Application Support/waz/history.db` · Linux `~/.local/share/waz/history.db` |
| Config | `waz config` path (often Application Support on macOS, `~/.config/waz` on Linux) |
| OAuth | `auth.json` next to config.toml (mode 0600) |
| Schemas | `…/waz/schemas/*.json` · versions under `schemas/versions/<tool>/` |
| Plugins | `…/waz/plugins/waz/` |
| Generate jobs | data dir `waz/jobs/` |
| Rotation | `rotation.json` in the data dir |
| Curated JSON | repo `schemas/curated/*.json` (copied on first load, not overwritten) |

Override schemas with `WAZ_SCHEMAS_DIR` (tests and agents). Plugins with `WAZ_PLUGINS_DIR`.
