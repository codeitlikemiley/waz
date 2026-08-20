# Usage

Install and shell hooks are in the [root README](../README.md). This page is day-to-day use.

## Predictions

Once `waz init` is in your shell:

| Shell | Suggestion | Palette |
|-------|------------|---------|
| **Zsh** | Ghost text (empty prompt and while typing). Right arrow accepts; Alt+F accepts a word. | Ctrl+T or Cmd+I (Ghostty) |
| **Bash / Fish / PowerShell** | Ctrl+Space fills the prediction | Ctrl+T |

PowerShell has no Zsh-style ghost text. Unknown natural-language commands open the TUI, same idea as Zsh/Bash.

```bash
waz predict --cwd .
waz predict --prefix "git" --format json
waz hint --output "Run 'npm start'"
waz import                         # bootstrap from existing history
waz import --shell powershell
waz stats
waz clear                          # this directory
waz clear --all
```

`waz predict` `tier` may be `output_hint`, `sequence`, `workflow`, `cwd_history`, or `llm`.

## Command palette (TUI)

| Trigger | Where |
|---------|-------|
| **Cmd+I** | Ghostty (`keybind = super+i=text:\x1b[119;97;122~`) |
| **Ctrl+T** | Any terminal with shell integration |
| `waz tui` | Manual |
| `waz tui --file <path> [--line <n>]` | Seed TMP with file/line context |

Type a prefix to pick a mode:

| Mode | Prefix | What it does |
|------|--------|----------------|
| **TMP** | `/` | Schema command palette with token forms |
| **Shell** | `!` | Direct shell input |
| **AI** | any other text | Natural language → numbered commands |

| Key | Action |
|-----|--------|
| Esc | Peel one layer (placeholder → selection → conversation → empty → quit) |
| ↑ / ↓ | List |
| Tab / Shift+Tab | Tokens |
| Enter | Run |
| 1–9 | Quick-select an AI suggestion |

TMP filters by score (subcommand names first). Cargo commands appear only with `Cargo.toml`, npm/bun with `package.json`. Git is always available when `git` is on `PATH`.

Seed from an editor:

```bash
waz tui --file src/main.rs --line 12
waz run
waz run src/main.rs:1
waz runnables
```

`waz run` / `waz runnables` prefer `cargo runner` when installed, otherwise local Cargo / script heuristics.

AI mode uses TMP schemas when it can (tool name, keywords, or project files) and tags those results `[TMP]`. Placeholders like `<db_name>` open an inline form.

## CLI cheat sheet

```bash
waz doctor --cwd .
waz ask "how to uninstall a package with homebrew"
waz ask --provider grok --json "list rust tests"
waz resolve "run the backend" --tool cargo
waz session-id
waz record -- "git push"
```

TMP / schema commands: [tmp.md](tmp.md). LLM setup: [ai.md](ai.md). Agents: [plugins.md](plugins.md).
