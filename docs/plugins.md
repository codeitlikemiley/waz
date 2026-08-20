# Plugins and MCP

Waz ships an [Agent Plugins](https://agent-plugins.org/) v1.0.0 package so **coding agents assemble TMP commands instead of guessing flags**.

Package layout (bundled, copied by `waz plugin install`):

```text
plugins/waz/
├── plugin.json
├── mcp.json
└── skills/
    ├── tmp-use/SKILL.md       # agents: list → show → build, never invent argv
    └── tmp-schema/SKILL.md    # complete TMP schema generation
```

waz is a **skills + MCP** client. Generate uses the `tmp-schema` skill. External apps use **stdio MCP** (`waz mcp`).

## Install into agent apps

Gemini CLI, Codex, Claude Code, and Cursor do **not** load `plugin.json` by themselves. They already speak MCP. Write that config for them:

```bash
waz plugin install
waz plugin connect all       # gemini + claude + codex + cursor
waz plugin connect gemini    # ~/.gemini/settings.json
waz plugin connect claude    # ~/.claude.json
waz plugin connect codex     # ~/.codex/config.toml
waz plugin connect cursor    # ~/.cursor/mcp.json
waz plugin list
waz plugin doctor
```

Restart the agent after connect. Then ask it to list TMP cargo commands in this repo.

Manual equivalent (stdio):

```toml
# Codex ~/.codex/config.toml
[mcp_servers.waz]
command = "waz"
args = ["mcp"]
```

```bash
# Claude Code
claude mcp add --transport stdio waz -- waz mcp
```

Gemini CLI uses `mcpServers.waz` in `~/.gemini/settings.json` (`command` + `args: ["mcp"]`). `connect gemini` writes that.

Any Agent Plugins client can also load the plugin directory (`plugin.json` + `skills/` + `mcp.json`). On macOS that is often `~/Library/Application Support/waz/plugins/waz`.

## What the MCP server exposes

| Tool | Role |
|------|------|
| `waz_tmp_list` | Commands loaded for a cwd |
| `waz_tmp_show` | One command + resolved tokens (`file` / `line` optional) |
| `waz_tmp_build` | Fill tokens → argv (`file` / `line` optional; prefills cargo bin from context) |
| `waz_resolve` | Natural language → schema command + `argv` from assemble. Does not pin cargo just because `Cargo.toml` exists. |
| `waz_generate` | Generate a schema (`wait` defaults **true**; pass `false` only if you will poll) |
| `waz_generate_jobs` | Job list (reaps dead PIDs) |
| `waz_generate_status` | One job; optional `wait` |
| `waz_generate_cancel` | Cancel a background job |
| `waz_plugin_list` | Loaded plugins |

`list` / `show` / `build` are **local** (no LLM). `resolve` / `generate` use **waz’s pinned provider** — see [ai.md](ai.md). The agent app can be Gemini while generate uses Ollama.

## Typical split

```bash
waz plugin connect gemini          # Gemini CLI is the agent UI
export WAZ_OLLAMA=1
waz config use ollama              # waz generate/resolve use local
```
