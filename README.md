<div align="center">

<img src="assets/waz-logo.svg" alt="Waz" width="128" />

# Waz

[Simplified Chinese](./README.zh-CN.md) · [Japanese](./README.ja.md)
<sub><i>Currently based on <a href="https://github.com/warpdotdev/warp">Warp</a>; evolving independently going forward.</i></sub>

</div>

Waz is an open, local-first terminal with first-class AI and agent support. Plug in any AI provider, bring in any CLI agent, manage SSH hosts inside the terminal — with keys, history and agent state staying on your machine by default.

## What Waz adds over upstream Warp

- **No mandatory cloud** — no account, login, Drive sync or cloud agent history required.
- **BYOP AI providers** — any OpenAI-compatible endpoint, plus native OpenAI / Anthropic / Gemini / DeepSeek / Ollama protocols. Keys stay local.
- **Third-party CLI agents** — DeepSeek-TUI / Codex CLI / Claude Code / Google Antigravity (`agy`) wired into Blocks and the notification center.
- **Built-in SSH host manager** — manage hosts, configs and sessions inside the terminal, with tmux integration.
- **Editable system prompts** — minijinja templates rendered on the client.
- **Rendering fixes** — tuned Markdown pipeline; CJK soft-wrap caret and bold subpixel fixes.
- **Localized UI** — English / Simplified Chinese / Japanese out of the box, community-extensible.
- **Privacy defaults** — Cloud Agent / Computer Use / Referral / telemetry off by default.
- **Token Model Protocol (TMP)** — Mapped JSON schemas that power supercharged path/flag tabbing autocomplete in the terminal Form Panel, and translate dynamically into structured MCP-aligned tools for AI agents with strict validation.

## Token Model Protocol (TMP) & AI Agent Integration

Waz includes native support for the **Token Model Protocol (TMP)**. TMP defines structured JSON schemas for command-line utilities, enabling two primary benefits:
1. **Supercharged Terminal Autocomplete**: Tab completion suggests paths, flags, and dynamic resolver values (e.g. from `git status`) inside the terminal's Form Panel.
2. **Expose Custom Tools to AI Agents**: Instead of appending verbose markdown usage headers directly to system prompts, Waz compiles TMP definitions into native, structured **Model Context Protocol (MCP)** tool schemas. The AI Agent invokes commands using key-value JSON parameters, which Waz validates, escapes, and compiles into safe shell instructions on the local Rust runtime.

### Defining and Generating Schemas

TMP schemas can be checked into git under your repository's workspace root:
- `.waz/schemas/*.json` — Recommended for team-wide schemas checked into source control.
- `.warp/tmp/*.json` — Legacy fallback directory for local scripts.

#### Schema Example (`cargo build` structure)
Schemas define subcommands, descriptions, required parameters, and dynamic autocomplete resolvers:
```json
{
  "meta": {
    "tool": "cargo",
    "description": "Rust compilation manager"
  },
  "commands": [
    {
      "command": "cargo build",
      "description": "Compile the current package or workspace projects.",
      "group": "cargo",
      "verified": false,
      "tokens": [
        {
          "name": "package",
          "description": "Package to build",
          "required": false,
          "token_type": "Enum",
          "flag": "--package",
          "data_source": {
            "resolver": "cargo:packages"
          }
        },
        {
          "name": "release",
          "description": "Build release binary",
          "required": false,
          "token_type": "Boolean",
          "flag": "--release"
        }
      ]
    }
  ]
}
```

### Autocomplete & Dynamic Resolvers
TMP schemas support dynamic autocomplete values (Resolvers) for arguments:
- **Built-in Resolvers**: `git:status_files` (resolves modified/untracked files via porcelain git status), `git:branches`, `git:remotes`, `cargo:packages`, and `npm:scripts`.
- **Command-line Resolvers**: Execute arbitrary shell command resolvers (`data_source.command`) to fetch suggestion lists dynamically.

### Security & Trust Boundary
Because custom schemas can define arbitrary shell commands to resolve suggestions (e.g., `data_source.command`), Waz enforces a strict security boundary:
- **Trusted Workspaces**: Only workspaces in the Trusted Workspace Registry (`~/.config/zap/trusted_workspaces.json`) can execute custom command resolvers.
- **Untrusted Workspaces**: Arbitrary command resolvers are blocked. Built-in git resolvers are executed under a heavily sandboxed environment (`GIT_CONFIG_NOSYSTEM=1`, `-c core.hooksPath=/dev/null`, `-c protocol.file.allow=never`) with absolute binary paths to prevent RCE, malicious git hook execution, and PATH hijacking.
- **Input Sanitization**: All parameters parsed from the AI Agent are scanned for shell injection metacharacters and unbalanced quotes, and escaped (`'\''` on Unix) to prevent command injection.

## Migrating from OpenWarp or Warp

If you used the project before it was renamed to Waz (formerly **OpenWarp**),
or are coming from upstream **Warp**, see
[docs/migrate-from-warp.md](docs/migrate-from-warp.md) to bring your settings
across.

## Roadmap

See [docs/roadmap.md](docs/roadmap.md).

## Acknowledgements

- [Warp](https://github.com/warpdotdev/warp) — the upstream terminal Waz is built on.
- [DeepSeek-TUI](https://github.com/Hmbown/DeepSeek-TUI) — first-class CLI agent partner.
