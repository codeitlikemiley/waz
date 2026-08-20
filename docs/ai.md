# AI providers

Waz uses an LLM for `ask`, `resolve`, `generate`, and the last prediction tier. Pin one with `waz config use <provider>` or `waz ask --provider grok`. `waz config` shows what is ready.

There are **two wires**:

1. **waz’s LLM** — this page. Gemini, OpenAI-compatible, Anthropic-compatible, local, or subscription OAuth.
2. **Agent apps** (Gemini CLI, Codex, Claude Code) — they talk to waz over MCP. See [plugins.md](plugins.md).

## Pin a provider (no hand-edited toml)

```bash
waz config                      # strategy, default, ready providers
waz config get llm.default
waz config use grok             # strategy=single + default=grok (must already be set up)
waz config set llm.strategy fallback
waz login grok --default        # login and pin
```

`waz config use` / `set llm.default` **refuses** a provider that is not logged in, keyed, or locally opted-in.

## Subscription OAuth

Same first-party clients as [Open Codex](https://opencodex.me/). No pay-as-you-go key required.

| Provider | Command | Subscription | Protocol | Default model |
|----------|---------|--------------|----------|---------------|
| **Grok (xAI)** | `waz login grok` | SuperGrok / X Premium+ | OpenAI-compatible → `api.x.ai/v1` | `grok-4.6` |
| **Claude** | `waz login anthropic` | Claude Pro / Max | Anthropic messages + OAuth beta | `claude-sonnet-4-5` |
| **ChatGPT / Codex** | `waz login chatgpt` | ChatGPT Plus / Pro / Codex | Codex responses API | `gpt-5.4-mini` |

Aliases: `xai` → grok, `claude` → anthropic, `codex` → chatgpt.

```bash
waz login grok                 # imports ~/.grok/auth.json if present
waz login grok --device        # SSH / VPS
waz login anthropic            # imports Claude Code Keychain / ~/.claude/.credentials.json
waz login chatgpt              # imports ~/.codex/auth.json
waz login --status
waz logout grok
```

Tokens live in `auth.json` next to `config.toml` (mode 0600), never in toml. Refresh is owned by waz after import.

If SuperGrok OAuth login works but inference returns HTTP 403, use a key from [console.x.ai](https://console.x.ai) (`XAI_API_KEY`).

## API keys and local

| Provider | Protocol | Default model | How to enable |
|----------|----------|---------------|---------------|
| **Gemini** | gemini | `gemini-3.1-flash-lite-preview` | `GEMINI_API_KEY` |
| **Grok** | openai | `grok-3-mini` | `XAI_API_KEY` (optional if OAuth) |
| **Claude** | anthropic | `claude-sonnet-4-5` | `ANTHROPIC_API_KEY` |
| **OpenAI** | openai | `gpt-4o-mini` | `OPENAI_API_KEY` |
| **OpenRouter** | openai | `openai/gpt-4o-mini` | `OPENROUTER_API_KEY` |
| **Groq** | openai | `llama-3.3-70b-versatile` | `GROQ_API_KEY` |
| **DeepSeek** | openai | `deepseek-chat` | `DEEPSEEK_API_KEY` |
| **GLM / Qwen / MiniMax** | openai | (defaults in code) | `GLM_API_KEY` / `DASHSCOPE_API_KEY` / `MINIMAX_API_KEY` |
| **Ollama** | ollama | `llama3.2` | `WAZ_OLLAMA=1` or `OLLAMA_HOST` |
| **LM Studio** | openai | `local` | `WAZ_LMSTUDIO=1` |
| **llama.cpp** | openai | `local` | `WAZ_LLAMACPP=1` |
| **vLLM** | openai | `local` | `WAZ_VLLM=1` |
| **Any OpenAI-compatible proxy** | openai | your model | `OPENAI_BASE_URL` |

Local servers are opt-in so a dead localhost does not stall `waz predict`.

```bash
# any ONE cloud key is enough with strategy=fallback
export GEMINI_API_KEY="..."
export ANTHROPIC_API_KEY="..."
export OPENAI_API_KEY="..."

export WAZ_OLLAMA=1
export WAZ_OLLAMA_MODEL=llama3.2
waz config use ollama
```

## Compatible endpoints (not the vendor cloud)

**OpenAI-compatible** (`/v1/chat/completions`):

```bash
export OPENAI_BASE_URL="http://127.0.0.1:4000/v1"
export OPENAI_API_KEY="optional-for-some-local"
export OPENAI_MODEL="your-model"
waz config use openai
```

Named extra host:

```bash
waz config set llm.providers.work.api openai
waz config set llm.providers.work.base_url https://api.example.com/v1
waz config set llm.providers.work.model my-model
```

`work` can be pinned only after waz sees a key or a localhost `base_url`.

**Gemini-compatible** (`generateContent`):

```bash
export GEMINI_API_KEY="..."
waz config set llm.providers.gemini.api gemini
waz config set llm.providers.gemini.base_url https://your-gemini-compat/v1beta
waz config set llm.providers.gemini.model gemini-2.5-flash
waz config use gemini
```

**Anthropic-compatible** (`/v1/messages` + `x-api-key`):

```bash
export ANTHROPIC_API_KEY="..."
waz config set llm.providers.anthropic.api anthropic
waz config set llm.providers.anthropic.base_url https://your-claude-compat
waz config use anthropic
```

Check the live path:

```bash
waz ask "ping"    # prints: using <provider> / <model>
```

## Not supported

Do not expect `waz login` or `config use` for these:

| Not in waz | Why |
|------------|-----|
| **Cursor** OAuth | Cursor HTTP/2 API, not a chat-completions backend |
| **Google Antigravity** / Cloud Code Assist OAuth | Not the Gemini API; Gemini here is API-key only |
| **Kiro** | AWS runtime + profile ARN |
| **Command Code** | Separate product API |
| **Kimi Coding Plan** OAuth | Not ported |
| **GitHub Copilot** OAuth | Unofficial extra-header bridge; not ported |
| **Nous Portal** OAuth | Not ported |
| **ChatGPT Plus as `api.openai.com`** | Use `waz login chatgpt`, not an OpenAI API key |
| **SuperGrok cookies / grok.com scrape** | Use `waz login grok` or `XAI_API_KEY` |

## config.toml (optional)

Prefer `waz config`. Env keys are **additive** to `keys` in toml (deduped).

```toml
[llm]
strategy = "single"          # fallback | round-robin | single
default = "gemini"
order = ["gemini", "grok", "anthropic"]
timeout_secs = 3

[[llm.providers]]
name = "gemini"
keys = ["account-1", "account-2"]
model = "gemini-3.1-flash-lite-preview"

[[llm.providers]]
name = "custom"
api = "openai"               # openai | anthropic | gemini | ollama
base_url = "https://your-api.example/v1"
keys = ["your-key"]
model = "your-model"
```

`fallback` tries `order` and skips failures. `round-robin` rotates. Multiple `keys` rotate inside a provider.
