# waz 🔮

**Warp-style command prediction and command palette for any terminal.**

A Rust-powered command prediction engine with ghost-text autosuggestions, an interactive **command palette TUI**, and **AI-powered command assistance** — works in any terminal emulator (Ghostty, Alacritty, Kitty, iTerm2, WezTerm, etc.).

Inspired by [Warp's multi-tier prediction approach](https://x.com/BHolmesDev/status/2025588198571757892).

## How It Works

Waz uses a **multi-tier prediction system** (same approach as Warp terminal):

| Tier | Strategy | Confidence | Description |
|------|----------|------------|-------------|
| 0 | **Output Hint** | Highest | Parses command output for suggested follow-up commands (like Warp's ghost text from `npm install` → `npm start`). |
| 1 | **Sequence** | High | Predicts based on command patterns. If you always run `git push` after `git commit`, it learns that. |
| 2 | **CWD History** | Medium | Falls back to your most recently used commands in the current directory. |
| 3 | **LLM** | Low | Uses an LLM to predict based on shell context. Supports multiple providers with key rotation. |

**Proactive prediction**: Ghost text appears even on an **empty prompt** — right after a command finishes, waz suggests what you'll probably run next.

## Quick Start

### Build & Install

```bash
make install
```

This builds the release binary, installs it to `~/.cargo/bin` and `~/.local/bin`, and reminds you to reload your shell.

Or manually:

```bash
cargo build --release
cp target/release/waz ~/.local/bin/
```

### Shell Integration

Add to your shell config (**quotes are required** for Zsh/Bash):

**Zsh** (`~/.zshrc`):
```bash
eval "$(waz init zsh)"
```

**Bash** (`~/.bashrc`):
```bash
eval "$(waz init bash)"
```

**Fish** (`~/.config/fish/config.fish`):
```fish
waz init fish | source
```

### Ghostty Keybinding (Recommended)

For Ghostty users, add this to your config to launch the TUI with **Cmd+I**:

```
keybind = super+i=text:\x1b[119;97;122~
```

This sends a custom escape sequence that the waz shell integration picks up.

### Import Existing History

Bootstrap predictions from your existing shell history:

```bash
waz import              # auto-detect all shells
waz import --shell zsh  # import from specific shell
```

Supports custom `$HISTFILE` locations (e.g., `~/.config/zsh/.zsh_history`).

## Usage

Once installed, waz works automatically:

### Zsh (Full Ghost Text)
- **Ghost text appears** as you type — dim gray suggestions
- **Ghost text appears on empty prompt** — proactive next-command prediction
- **Right arrow** → Accept full suggestion
- **Alt+F** → Accept next word

### Bash / Fish
- **Ctrl+Space** → Fill in the predicted command

### CLI Commands

```bash
waz predict --cwd .                        # Proactive prediction (no prefix)
waz predict --prefix "git" --format json   # Prediction with prefix
waz record -- "git push"                   # Manually record a command
waz stats                                  # Show database statistics
waz generate brew                          # Generate TMP schema for a CLI tool
waz generate brew --verify                 # Review & approve schema in TUI
waz generate brew --history                # Show schema version history
waz schema list                            # List all installed schemas
waz schema share cargo                     # Export shareable schema
waz schema import ./brew-schema.json       # Install shared schema
waz schema keywords psql postgres database # Set custom AI trigger keywords
waz resolve "run the backend" --tool cargo # AI + TMP grounded command
waz session-id                             # Generate a new session ID
```

---

## Command Palette (TUI)

A **Warp-like unified command palette** built with `ratatui`. Launch it with:

| Trigger | Where |
|---------|-------|
| **Cmd+I** | Ghostty (via custom escape sequence) |
| **Ctrl+T** | Any terminal |
| `waz tui` | Manual launch |
| `waz tui --file <path> [--line <n>]` | Seed TMP with file/line context |
| `waz run <path[:line]>` | Directly run the best command for a file context |

The TUI starts in an **Empty** state showing mode hints. Type a prefix to enter a mode:

### Three Modes

| Mode | Prefix | What it does |
|------|--------|------|
| **TMP** | `/` | Context-aware command palette with token forms |
| **Shell** | `!` | Direct shell command input |
| **AI** | *(any text)* | Natural language → AI suggests runnable commands |

### Navigation

| Key | Action |
|-----|--------|
| **Esc** | Go back one layer (see below) |
| **Backspace** (empty input) | Return to Empty mode |
| **Cmd+Backspace** / **Ctrl+U** | Clear entire input line |
| **↑ / ↓** | Navigate command list |
| **Tab** | Select command / next token field |
| **Shift+Tab** | Previous token field |
| **Enter** | Run selected command |
| **1-9** | Quick-select AI command by number |

### Layered Escape

Each Esc press peels back one layer — never dumps you out unexpectedly:

```
Placeholder editing → Command selection → AI conversation → Empty mode → Quit
```

---

### TMP Mode (`/`)

Context-aware command palette powered by **unified JSON schemas**. On first launch, curated schemas are auto-installed. Additional schemas can be AI-generated or imported.

Seed the palette with the current file when you launch from an editor:

```bash
waz tui --file example/power.rs
waz tui --file example/power.rs --line 12
```

When the file is a single-file script, waz detects either `#!/usr/bin/env rust-script` or `#!/usr/bin/env -S cargo +nightly -Zscript` and shows the matching script command with the current file prefilled.

If you already know the file and line and want to skip the interactive palette, use:

```bash
waz run src/main.rs:1
waz run examples/demo.rs:1
```

#### Built-in Curated Schemas (8)

| Schema | Commands | Dynamic Data Sources |
|--------|----------|---------------------|
| `cargo` | 12 | bins, features, packages, profiles, tests, benches from `Cargo.toml` |
| `cargo-script` | 1 | current script file path |
| `rust-script` | 1 | current script file path |
| `git` | 12 | branches, remotes from local repo |
| `npm` | 8 | scripts from `package.json` |
| `bun` | 8 | scripts from `package.json` |
| `npx` | 1 | — |
| `bunx` | 1 | — |

Schemas are **contextual** — cargo commands only appear when `Cargo.toml` exists, npm/bun when `package.json` exists. Git is always available.

#### AI-Generated Schemas

| Source | Commands Loaded |
|--------|----------------|
| `~/.config/waz/schemas/*.json` | **Any CLI tool** — AI-generated or imported schemas (see [Schema Generation](#schema-generation)) |

#### Smart Filtering

Type to filter — uses **score-based ranking** that prioritizes subcommand names:

```
/commit   → git commit (exact match, ranked first)
/git com  → git commit (full command match)
/build    → cargo build (subcommand match)
/install  → brew install (from generated schema)
```

Description-only matches are excluded to avoid false positives.

#### Token Form

When selecting a command with arguments:
- **Boolean tokens** → toggle with `Space`/`y`/`n`, `Tab` cycles
- **Enum tokens** → `Tab` cycles through values (packages, scripts, branches)
- **String tokens** → free-text input
- **Live preview** → shows the resolved command as you fill tokens

---

### AI Mode (natural language)

Just start typing a question — waz auto-detects natural language:

```
how to create a new database in psql
find large files over 100mb
run my rust app
```

The AI responds with an explanation and **numbered command suggestions**.

#### Smart TMP Integration

AI mode automatically uses **TMP schemas for grounded results** when it detects a relevant tool:

| Detection Method | Priority | Example |
|-----------------|----------|--------|
| **Query keywords** | Highest | "list psql tables" → uses psql schema |
| **Custom keywords** | High | "show database tables" → psql (if "database" is a keyword) |
| **File / project context** | Medium | Cargo.toml or a single-file script → uses the matching schema |
| **General AI** | Fallback | "what is rust?" → plain AI answer |

When TMP is used, results show a `[TMP]` tag and commands are grounded in real data (actual package names, branches, etc.).

```
🔮 waz:
  [TMP] Runs the 'waz' binary from the current workspace.

Commands:
▸ [1] cargo run --bin waz
       bin = waz (from Cargo.toml)
```

#### Selecting Commands

- Press **1-9** to quick-select by number
- Use **↑/↓** and **Enter** to select

#### Placeholder Editing

If the AI suggests a command with placeholders (e.g., `psql -U <username> -c "CREATE DATABASE <db_name>"`), waz detects them and shows an **inline editing form**:

```
⌨ Fill in placeholders:

→ psql -U postgres -c "CREATE DATABASE <db_name>"

  username: postgres█
  db_name:
```

- **Tab** / **Shift+Tab** → navigate between fields
- **Live preview** updates as you type
- **Enter** → run the resolved command
- **Esc** → back to command selection (pick a different command)

#### Continuing the Conversation

After getting AI results:
- Press **Esc** once → exit command selection, type a new question
- Press **Esc** twice → clear AI conversation, start fresh
- Just start typing → clears old response, asks new question

---

### Shell Mode (`!`)

Direct shell command input — type `!` followed by any command:

```
!docker compose up -d
!kubectl get pods
```

Press **Enter** to execute immediately.

---

## Schema System

Waz uses a **unified JSON schema system** for all CLI tool commands. Six curated schemas ship built-in, and you can generate schemas for any tool using AI, or import schemas shared by others.

### Curated Schemas

On first TUI launch (or manually with `--init`), curated schemas are auto-installed:

```bash
waz generate cargo --init    # Install all 6 curated schemas
```

### AI-Powered Generation

Generate schemas for **any CLI tool** using AI:

```bash
waz generate brew                              # Generate with default model
waz generate kubectl --model gemini-2.5-pro    # Use specific AI model
waz generate docker --force                    # Regenerate (versions old first)
```

How it works:
1. Runs `<tool> --help` and recursively `<tool> <subcommand> --help` (up to 20 subcommands)
2. Sends the help output to your configured LLM (Gemini by default)
3. AI extracts commands, flags, and argument types into a structured `SchemaFile`
4. Schema is saved to `~/.config/waz/schemas/<tool>.json` with metadata
5. Next TUI launch, the commands appear alongside curated ones

### Schema Verification TUI

Review and approve schemas before using them:

```bash
waz generate brew --verify
```

Opens a **two-pane TUI** for human-in-the-loop review:

| Key | Action |
|-----|--------|
| `j`/`k` or `↑`/`↓` | Navigate commands/tokens |
| `Space` | Toggle command verified ✅ |
| `Tab` | Switch between Commands / Tokens panes |
| `n`/`d`/`f` | Edit token name / description / flag |
| `r` | Toggle required / optional |
| `t` | Cycle token type (String → Boolean → Enum → File → Number) |
| `x` | Test data source live (runs resolver, shows results) |
| `a` / `Del` | Add / delete token |
| `Ctrl+V` | Verify all commands at once |
| `s` | Save changes to JSON |
| `q` | Quit |

### Schema Management

List, share, import, and configure schemas:

```bash
waz schema list                      # Show all installed schemas
waz schema share cargo               # Export portable .json to CWD
waz schema import ./brew-schema.json  # Install from file
waz schema import https://example.com/schema.json  # Install from URL
waz schema keywords psql             # Show current keywords for psql
waz schema keywords psql postgres postgresql database db  # Set keywords
```

### Schema Keywords

Keywords tell AI mode which schema to use when you mention certain words in your query:

```bash
waz schema keywords psql postgres postgresql database db tables
waz schema keywords cargo rust crate package
waz schema keywords brew homebrew formula
```

**How matching works:**
1. **Exact tool name** — "install with brew" → brew schema (always works)
2. **Custom keywords** — "show database tables" → psql schema (if "database" is a keyword)
3. **Built-in aliases** — "install with homebrew" → brew (hardcoded alias)
4. **CWD project files** — Cargo.toml exists → cargo schema

Keywords are stored in the schema's `meta.keywords` field and persist across regenerations.

`waz schema list` output:
```
Tool         Ver    Status     Cmds     Source Coverage
────────────────────────────────────────────────────────
bun          v1    ✅ verified 8/8      curated full
cargo        v1    ✅ verified 12/12    curated full
git          v1    ✅ verified 12/12    curated full
npm          v1    ✅ verified 8/8      curated full
```

**Share** strips runtime-resolved values (keeps data source definitions so importers resolve locally).  
**Import** auto-backups existing schemas before overwriting.

### Export Built-in Schemas

Export the battle-tested Rust-based cargo/git/npm schemas to JSON:

```bash
waz generate cargo --export   # From a Cargo project directory
waz generate git --export
waz generate npm --export
```

### Schema Versioning

Every `--force` regeneration auto-versions the old schema. Full version history:

```bash
waz generate brew --history
# 📋 Version history for 'brew' (3 versions):
# ─────────────────────────────────────────
#   v1   │ 2h ago          │ 15 commands
#   v2   │ 1h ago          │ 12 commands
#   v3   │ 5m ago          │ 14 commands ← latest
```

Rollback to any version:

```bash
waz generate brew --rollback       # Restore latest versioned backup
waz generate brew --rollback 1     # Restore specific version
```

On `--force`, a **colorized diff** is shown:
- 🟢 `+ brew search` — new command added
- 🔴 `- brew cleanup` — command removed
- 🟡 `~ brew install` — tokens changed

If generation fails, the previous version is **auto-restored**.

### Dynamic Data Sources

Schemas support two kinds of dynamic data sources:

**Shell commands** — run at TUI load time:
```json
{
  "name": "formula",
  "token_type": "Enum",
  "data_source": { "command": "brew list --formula", "parse": "lines" }
}
```

**Built-in resolvers** — use waz's optimized Rust parsers:
```json
{
  "name": "feature",
  "token_type": "Enum",
  "data_source": { "resolver": "cargo:features", "parse": "lines" }
}
```

Available resolvers: `cargo:bins`, `cargo:examples`, `cargo:packages`, `cargo:features`, `cargo:profiles`, `cargo:tests`, `cargo:benches`, `git:branches`, `git:remotes`, `npm:scripts`.

### SchemaFile Format

All schemas use a unified `SchemaFile` format with metadata:

```json
{
  "meta": {
    "tool": "cargo",
    "version": 1,
    "generated_by": "human",
    "verified": true,
    "coverage": "full",
    "requires_file": "Cargo.toml",
    "requires_binary": "cargo",
    "keywords": ["rust", "crate", "package"]
  },
  "commands": [ ... ]
}
```

### Storage Layout

```
~/.config/waz/schemas/
├── cargo.json                ← active curated schema
├── git.json
├── npm.json
├── bun.json
├── brew.json                 ← AI-generated
├── versions/
│   ├── brew/
│   │   ├── v1.json
│   │   └── v2.json
│   └── cargo/
│       └── v1.json
```

---

## AI Assistant (CLI)

Ask natural language questions directly from the command line:

```bash
# Just type a question as a command — waz intercepts it automatically
how to find large files

# Or use the ask command explicitly
waz ask "how to uninstall a package with homebrew"
waz ask --json "how to search in files"   # Structured JSON output
```

### Grounded Resolve (AI + TMP)

For **precise, non-hallucinated commands**, use `waz resolve` which combines AI with TMP schemas:

```bash
waz resolve "run the backend package" --tool cargo
# 🎯 Runs the waz binary from the workspace.
# cargo run --bin waz
#    bin = waz (from Cargo.toml)
#    confidence: high

waz resolve "list all tables" --tool psql
waz resolve "switch to main branch"   # auto-detects git
waz resolve "install react" --json    # structured JSON output
```

**How it works:**
1. Loads the specified (or auto-detected) TMP schema
2. Resolves data sources (`cargo:packages`, `git:branches`, etc.) for real values
3. Builds a schema-aware prompt with actual valid values
4. AI picks the best command and fills tokens using only real data

This prevents hallucination — the AI can only use values that actually exist in your project.

### History Management

```bash
waz clear            # Clear history for current directory
waz clear --all      # Clear ALL history across all directories
```

---

## LLM Providers

Tier 3 uses an LLM when local history can't produce a prediction. Waz supports **6 providers**:

| Provider | Default Model | Base URL | Free Tier |
|----------|---------------|----------|-----------|
| **Gemini** | `gemini-3.1-flash-lite-preview` | `generativelanguage.googleapis.com` | 50 req/day |
| **GLM (z.ai)** | `glm-4.7` | `api.z.ai` | Free for dev use |
| **Qwen (Alibaba)** | `qwen3.5-plus` | `dashscope-intl.aliyuncs.com` | 1M tokens free |
| **MiniMax** | `MiniMax-M2.5` | `api.minimax.io` | Free credits |
| **OpenAI** | `gpt-4o-mini` | `api.openai.com` | Paid only |
| **Ollama** | `llama3.2` | `localhost:11434` | Local, always free |

### Zero-Config Setup

Just export your API key — waz auto-detects it:

```bash
# Any ONE of these is enough to enable LLM predictions:
export GEMINI_API_KEY="your-key"       # Google Gemini
export GLM_API_KEY="your-key"          # z.ai GLM
export DASHSCOPE_API_KEY="your-key"    # Alibaba Qwen
export MINIMAX_API_KEY="your-key"      # MiniMax
export OPENAI_API_KEY="your-key"       # OpenAI
```

Multiple env vars? Waz creates a provider for each and uses **fallback** strategy by default.

---

## Multi-Provider Configuration

For advanced control, create `~/.config/waz/config.toml`:

### Single Provider (Simplest)

Use only Gemini, nothing else:

```toml
[llm]
strategy = "single"
default = "gemini"

[[llm.providers]]
name = "gemini"
keys = ["your-gemini-key"]
```

### Fallback Strategy (Default)

Try providers in order — if one fails (rate limit, timeout), try the next:

```toml
[llm]
strategy = "fallback"
order = ["gemini", "glm", "qwen", "minimax"]
timeout_secs = 3

[[llm.providers]]
name = "gemini"
keys = ["gemini-key-1"]

[[llm.providers]]
name = "glm"
keys = ["glm-key-1"]

[[llm.providers]]
name = "qwen"
keys = ["dashscope-key-1"]
model = "qwen3.5-plus"
```

> If Gemini hits its 50 req/day limit → automatically falls back to GLM → then Qwen.

### Round-Robin Strategy

Spread requests evenly across providers:

```toml
[llm]
strategy = "round-robin"
order = ["gemini", "glm", "qwen"]

[[llm.providers]]
name = "gemini"
keys = ["key-1"]

[[llm.providers]]
name = "glm"
keys = ["key-1"]

[[llm.providers]]
name = "qwen"
keys = ["key-1"]
```

> Request 1 → Gemini, Request 2 → GLM, Request 3 → Qwen, Request 4 → Gemini...

### Multiple Keys Per Provider

Rotate keys within a single provider (useful for multiple free-tier accounts):

```toml
[llm]
strategy = "single"
default = "gemini"

[[llm.providers]]
name = "gemini"
keys = ["account-1-key", "account-2-key", "account-3-key"]
model = "gemini-3.1-flash-lite-preview"
```

> Each request uses the next key: key1 → key2 → key3 → key1...

### Combo: Multiple Providers + Multiple Keys

Maximum free-tier usage — rotate keys AND providers:

```toml
[llm]
strategy = "fallback"
order = ["gemini", "glm", "qwen", "minimax"]

[[llm.providers]]
name = "gemini"
keys = ["gemini-acct-1", "gemini-acct-2"]
model = "gemini-3.1-flash-lite-preview"

[[llm.providers]]
name = "glm"
base_url = "https://api.z.ai/api/paas/v4"
keys = ["glm-key-1", "glm-key-2"]
model = "glm-4.7"

[[llm.providers]]
name = "qwen"
base_url = "https://dashscope-intl.aliyuncs.com/compatible-mode/v1"
keys = ["qwen-key-1"]
model = "qwen3.5-plus"

[[llm.providers]]
name = "minimax"
base_url = "https://api.minimax.io/v1"
keys = ["mm-key-1"]
model = "MiniMax-M2.5"
```

### How Env Vars Interact with Config

Env vars are **additive** — they add to the key pool, never override:

| Setup | Key Pool |
|-------|----------|
| Only `GEMINI_API_KEY="A"` | `["A"]` — 1 key |
| Only config `keys = ["A", "B"]` | `["A", "B"]` — 2 keys |
| Env `"C"` + config `["A", "B"]` | `["A", "B", "C"]` — 3 keys |
| Env `"A"` + config `["A", "B"]` | `["A", "B"]` — deduped |

### Custom Provider (Any OpenAI-Compatible API)

Any service with an OpenAI-compatible `/v1/chat/completions` endpoint works:

```toml
[[llm.providers]]
name = "custom"
base_url = "https://your-api-endpoint.com/v1"
keys = ["your-key"]
model = "your-model-name"
```

---

## Architecture

```
┌─────────────────────────────────────────────────────┐
│              Shell Integration Layer                │
│  ┌─────────┐  ┌──────────┐  ┌────────────┐         │
│  │   Zsh   │  │   Bash   │  │    Fish    │         │
│  │  (ZLE)  │  │(readline)│  │  (events)  │         │
│  └────┬────┘  └────┬─────┘  └─────┬──────┘         │
│       │            │              │                 │
│  Cmd+I / Ctrl+T launches unified TUI                │
│  Ghost-text autosuggestions via predictions          │
│  Output capture → hint suggestions (Tier 0)         │
└───────┼────────────┼───────────────┼────────────────┘
        └────────────┼───────────────┘
                     │
         ┌───────────▼───────────┐
         │    waz binary (Rust)  │
         │                       │
         │  ┌─ Prediction ───────┤
         │  │  Tier 0: Output   │
         │  │  Tier 1: Sequence  │
         │  │  Tier 2: CWD      │
         │  │  Tier 3: LLM      │
         │  ├─ Unified TUI ─────┤
         │  │  / → TMP mode     │
         │  │  ! → Shell mode   │
         │  │  text → AI mode   │
         │  │  Placeholder edit │
         │  │  Score filtering  │
         │  ├─ Schema System ───┤
         │  │  6 curated JSON   │
         │  │  AI-powered gen   │
         │  │  Verify TUI       │
         │  │  Share/import     │
         │  │  Version control  │
         │  │  Built-in resolvers│
         │  ├─ AI Assistant ────┤
         │  │  Structured JSON  │
         │  │  Command resolver │
         │  └────────────────────┤
         │                       │
         │     SQLite History    │
         └───────────────────────┘
```

## Data Storage

- **History DB**: `~/Library/Application Support/waz/history.db` (macOS) / `~/.local/share/waz/history.db` (Linux)
- **Rotation state**: `~/Library/Application Support/waz/rotation.json`
- **Config**: `~/.config/waz/config.toml`
- **Curated Schemas**: `schemas/curated/*.json` (bundled in repo, auto-copied on first run)
- **Active Schemas**: `~/Library/Application Support/waz/schemas/*.json` (installed schemas)
- **Schema Versions**: `~/Library/Application Support/waz/schemas/versions/<tool>/v1.json ...`

## License

MIT
