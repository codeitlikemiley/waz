# waz 🔮

**Warp-style command prediction for any terminal.**

A Rust-powered command prediction engine that provides ghost-text autosuggestions in any terminal emulator (Ghostty, Alacritty, Kitty, iTerm2, WezTerm, etc.) by integrating at the shell level.

Inspired by [Warp's multi-tier prediction approach](https://x.com/BHolmesDev/status/2025588198571757892).

## How It Works

Waz uses a **multi-tier prediction system** (same approach as Warp terminal):

| Tier | Strategy | Confidence | Description |
|------|----------|------------|-------------|
| 1 | **Sequence** | High | Predicts based on command patterns. If you always run `git push` after `git commit`, it learns that. |
| 2 | **CWD History** | Medium | Falls back to your most recently used commands in the current directory. |
| 3 | **LLM** | Low | Uses an LLM to predict based on shell context. Supports multiple providers with key rotation. |

**Proactive prediction**: Ghost text appears even on an **empty prompt** — right after a command finishes, waz suggests what you'll probably run next.

## Quick Start

### Build & Install

```bash
cargo build --release
cp target/release/waz ~/.local/bin/   # or /usr/local/bin/
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
waz session-id                             # Generate a new session ID
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
┌─────────────────────────────────────────────┐
│          Shell Integration Layer            │
│  ┌─────────┐  ┌──────────┐  ┌────────────┐ │
│  │   Zsh   │  │   Bash   │  │    Fish    │ │
│  │  (ZLE)  │  │(readline)│  │  (events)  │ │
│  └────┬────┘  └────┬─────┘  └─────┬──────┘ │
└───────┼────────────┼───────────────┼────────┘
        └────────────┼───────────────┘
                     │
         ┌───────────▼───────────┐
         │    waz binary (Rust)  │
         │                       │
         │  ┌─ Tier 1: Sequence ─┤
         │  │  (bigram analysis) │
         │  ├─ Tier 2: CWD ─────┤
         │  │  (history search)  │
         │  ├─ Tier 3: LLM ─────┤
         │  │  Multi-provider    │
         │  │  Key rotation      │
         │  └────────────────────┤
         │                       │
         │     SQLite History    │
         └───────────────────────┘
```

## Data Storage

- **History DB**: `~/Library/Application Support/waz/history.db` (macOS) / `~/.local/share/waz/history.db` (Linux)
- **Rotation state**: `~/Library/Application Support/waz/rotation.json`
- **Config**: `~/.config/waz/config.toml`

## License

MIT
