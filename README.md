# waz

Warp-style **command prediction** and a **command palette** for any terminal (Ghostty, Alacritty, Kitty, iTerm2, WezTerm, …). Ghost-text suggestions, a TUI, and optional AI — not the Warp terminal fork.

Inspired by [Warp’s multi-tier prediction approach](https://x.com/BHolmesDev/status/2025588198571757892).

| Tier | What it uses |
|------|----------------|
| 0 | Output hint (`waz hint`) |
| 1 | Command sequences from history (stemmed) |
| 2 | Workflows (`mkdir` → `cd`, `git commit` → `git push` when a remote exists, …) |
| 3 | Directory history |
| 4 | LLM (optional) |

Ghost text can appear on an **empty** prompt: the *next* command, not the one you just ran. Failed commands skip sequence/workflow.

Full guides: **[docs/](docs/README.md)** · [usage](docs/usage.md) · [AI](docs/ai.md) · [plugins](docs/plugins.md) · [TMP](docs/tmp.md)

## Install

```bash
make install
# or: cargo install waz
```

`make install` puts the binary in `~/.cargo/bin` and `~/.local/bin`.

From a clone:

```bash
cargo build --release
cp target/release/waz ~/.local/bin/
```

## Getting started

Add **one** line to your shell (quotes required for Zsh/Bash):

**Zsh** (`~/.zshrc`): `eval "$(waz init zsh)"`  
**Bash** (`~/.bashrc`): `eval "$(waz init bash)"`  
**Fish**: `waz init fish | source`  
**PowerShell** (`$PROFILE`): `Invoke-Expression (& waz init powershell | Out-String)`

`waz init pwsh` and `waz init ps1` are the same as `powershell`.

Optional — Ghostty Cmd+I for the palette:

```
keybind = super+i=text:\x1b[119;97;122~
```

Import existing history:

```bash
waz import
waz import --shell zsh
waz import --shell powershell
```

## Basic usage

| Shell | Suggestion | Palette |
|-------|------------|---------|
| Zsh | Dim ghost text; **→** accept, **Alt+F** next word | **Ctrl+T** |
| Bash / Fish / PowerShell | **Ctrl+Space** fill | **Ctrl+T** |

```bash
waz tui                    # palette: / TMP · ! shell · other text = AI
waz predict --cwd .
waz stats
waz doctor --cwd .
```

TUI keys, `run` / `runnables`, and the rest of the CLI: **[docs/usage.md](docs/usage.md)**.

AI, OAuth, local models, OpenAI/Gemini/Anthropic-compatible APIs: **[docs/ai.md](docs/ai.md)**.  
Codex / Claude Code / Gemini CLI plugin: **[docs/plugins.md](docs/plugins.md)**.  
Schemas and `waz generate`: **[docs/tmp.md](docs/tmp.md)**.

## License

MIT
