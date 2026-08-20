# TMP schemas

The `/` palette and `waz resolve` use **Token Model Protocol** schemas (implicit **tmp/1**). Curated JSON ships in `schemas/curated/*.json` and is copied to the user schemas dir on first load (**missing files only**). `waz schema upgrade` replaces installed curated files with the copies bundled in this waz (after a version backup).

User-facing generate and schema CLI is below. The file format follows.

## Generate (background by default)

```bash
waz generate docker              # returns a job id; shell stays usable
waz generate docker --wait       # block until the schema is written
waz generate --jobs
waz generate --jobs <id>         # one job (reaps a dead PID)
waz generate --wait --job <id>   # poll until done/error/cancelled
waz generate --cancel <id>
waz generate kubectl --force     # version the old file, then regenerate
waz generate brew --verify       # review TUI
waz generate brew --history
waz generate brew --rollback
waz generate brew --rollback 1
waz generate cargo --init        # install curated schemas
waz generate cargo --export      # dump a curated schema to cwd
```

Generate harvests nested `--help`, then calls your pinned LLM in batches using the `tmp-schema` [plugin skill](plugins.md). It does not drop the start of help to fit 12k.

If generation fails after `--force`, the previous version is restored.

## Schema CLI

```bash
waz schema list
waz schema upgrade              # all bundled (git, cargo, waz, npm, …)
waz schema upgrade git cargo waz
waz schema share cargo
waz schema import ./brew-schema.json
waz schema import https://example.com/schema.json
waz schema keywords psql postgres postgresql database db
```

Share strips resolved `values` when `data_source` is set. Import backups the existing file first.

Headless (agents / CI):

```bash
waz tmp list --cwd . --query cargo
waz tmp show "cargo run" --cwd .
waz tmp show "cargo run" --cwd . --file src/main.rs
waz tmp build "cargo run" --cwd . --file src/main.rs
waz tmp build "cargo run" --cwd . --set bin=waz --set release=true
```

Same argv serializer as the TUI (flags, then positionals). Missing required tokens exit 1 with `{"error":…}` on stderr. `--file` prefills cargo `bin`/`example` from project context.

## Token Model Protocol (tmp/1)

There is no required `meta.protocol` field yet. Existing JSON on disk stays valid.

Canonical documents live in `schemas/curated/*.json`. User copies are written to `~/.config/waz/schemas/` on first launch and are not overwritten.

## File shape

```
SchemaFile { meta, commands[] }
CommandEntry { command, description, group, verified, tokens[] }
TokenDef { name, description, required, token_type, default, values, flag, data_source }
DataSource { command?, resolver?, parse }
TokenType = String | Boolean | Enum | File | Number
```

Unknown JSON fields are ignored on load. New fields must be `#[serde(default)]` and omitted when empty.

## `meta`

| Field | Meaning |
|-------|---------|
| `tool` | Schema identity (`cargo`, `git`, …) |
| `version` | Per-tool document version (regeneration), not the protocol |
| `requires_file` | Load only if `<file>` exists at cwd or an ancestor (max 16 levels) |
| `requires_file_kind` | Load only if runtime context matches (`cargo_project`, `single_file_script`) |
| `requires_binary` | Load only if the binary is on `PATH` |
| `keywords` | Extra words that map NL queries onto this tool |

## Tokens and argv

`App::build_command` is the serializer:

1. Start with `command` (binary + subcommand only — never put `<args>` in `command`).
2. For each token with a non-empty value:
   - **Boolean** `true`/`yes` → emit `flag` if present; omit otherwise.
   - **Boolean** `false`/empty → omit.
   - **String / Enum / File / Number** with `flag` → emit `flag`, then value.
   - Same types with `flag: null` → positional, appended **after** all flags.
   - If `repeat` is true, whitespace-split the value and emit each piece (repeated flag+value, or N positionals). Default false — old JSON argv is unchanged.

`waz tmp build --set path=a --set path=b` on a `repeat` token appends (`git add a b`); the first `--set` replaces the default.

That flags-then-positionals order is part of tmp/1. Do not change it without a new protocol version.

## Data sources

`data_source` is resolved when a command is **selected** in the TUI (or when `waz resolve` builds a prompt), not when the file is saved.

- `resolver`: named builtin (`git:branches`, `git:status_files`, `cargo:bins`, `npm:scripts`, `waz:context:file_path`, `waz:models` / `waz:models:<provider>`, …). Unknown names warn and skip; the command stays.
- `depends_on`: optional sibling token name. `waz:models` with `depends_on: "provider"` uses that token’s value, else the pinned `llm.default`.
- `command`: `sh -c` (Unix) or `cmd /C` (Windows) in the project cwd (`parse`: `lines` or `words`). **3s timeout**, 64 KiB stdout cap; timeout skips values and keeps the command.
- Resolved values overlay `token.values`. The declared `token_type` is **not** rewritten.

`waz schema share` strips `values` when `data_source` is set so importers resolve locally.

## Built-in resolvers

| Name | Source |
|------|--------|
| `cargo:{bins,examples,packages,features,profiles,tests,benches}` | `Cargo.toml` + tree (cached per cwd in-process) |
| `git:branches` | `git branch --format=%(refname:short)` |
| `git:remotes` | `git remote` |
| `git:status_files` | `git status --porcelain` (optional `:staged` / `:unstaged`) |
| `npm:scripts` | `package.json` `scripts` |
| `waz:models` / `waz:models:<provider>` | provider model list (`waz:models` uses `depends_on` or pinned default) |
| `waz:context:<field>` | `RuntimeContext` from cargo-runner / local detect |

## Headless / agent API

The TUI is not scriptable. Agents and CI should use JSON commands instead:

```bash
waz doctor --cwd .
waz tmp list --cwd . --query cargo
waz tmp show "cargo run" --cwd .
waz tmp show "cargo run" --cwd . --file src/main.rs
waz tmp build "cargo run" --cwd . --file src/main.rs
waz tmp build "cargo run" --cwd . --set bin=waz --set release=true
waz predict --cwd . --prefix git --format json --fast
waz resolve "run the backend" --cwd . --json
```

`tmp build` uses the same `assemble_command` serializer as the TUI (flags, then positionals). Missing required tokens exit 1 with `{"error": "..."}` on stderr.

Set `WAZ_SCHEMAS_DIR` to use an isolated schema directory (agents should do this so tests do not depend on a developer’s edited `~/.config/waz/schemas`).

## Additive fields in tmp/1

| Field | Default | Meaning |
|-------|---------|---------|
| `repeat` | `false` | Split value on whitespace; emit each piece |
| `data_source.depends_on` | omitted | Re-resolve this token when the named sibling changes |
| `visible_if` | omitted | Hide/omit unless another token matches (`amend=true`) |

Still future: `exclusive_with`, `multi` (comma-join), `placeholder`, `env`.
