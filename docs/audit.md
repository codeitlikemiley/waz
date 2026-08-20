# waz audit — 2026-08-20

Full pass over **waz 0.1.7** (`640167f`): code quality, cleanup, performance, and the Token Model Protocol. Goal: improve without breaking existing schemas, the TUI, `waz resolve`, or shell integrations.

This is a findings document. Implementation of the recommended stack (tests, P0 fixes, resolver cache, embed schemas, TUI tty fallback, predict `--fast` config skip, delete duplicated command graphs, `docs/tmp.md`) landed after 0.1.7. Additive token fields (`repeat`, `visible_if`, …) remain future work.

---

## Snapshot

| Area | Size | Notes |
|------|------|--------|
| Rust sources | ~12k lines across 20 files | One binary crate, no library target |
| Largest modules | `generate.rs` 1660, `cargo_schema.rs` 899, `tui/mod.rs` 736, `main.rs` 735, `predict.rs` 725 | Several kitchen-sink files |
| Tests | 81 passing | Strong on predict/hint/db; thin on TUI token assembly and schema loading |
| Shells | zsh, bash, fish, PowerShell | Zsh has ghost text; others are Ctrl+Space fill |
| TMP | JSON schemas + named resolvers + TUI forms + `waz resolve` | Curated JSON is source of truth; leftover Rust command builders still exist |

**Verdict:** The product shape is right (local history first, TMP for grounded commands, LLM last). The highest-leverage work is (1) making TMP a real protocol instead of “JSON we happen to parse,” (2) deleting duplicated cargo/git command graphs, (3) cutting per-keystroke and per-open cost, (4) fixing a few protocol bugs that already look like missing features.

---

## Compatibility rule for TMP

Existing schemas must keep working. That means:

- **Additive only.** New JSON fields need `#[serde(default)]` and `skip_serializing_if`. Old files without the field load as today.
- **Resolvers are an open namespace.** Unknown `data_source.resolver` values already warn and skip; adding a resolver never invalidates old schemas.
- **Do not change `build_command` join order** (flags, then positionals) without a schema flag. Current schemas depend on it.
- **Do not require `meta.version` bumps** for new optional token fields.
- **Introduce `meta.protocol` later, default `"tmp/1"`.** Use it only when a breaking change is unavoidable (repeatable tokens that change argv shape, exclusive groups that hide flags, etc.). Until then, stay on implicit v1.

If a change would make `schemas/curated/git.json` or a user-generated `brew.json` fail to deserialize or emit a different argv, it is a break.

---

## P0 — bugs and correctness

### 1. `git:status_files` is declared but not implemented

`schemas/curated/git.json` uses `{ "resolver": "git:status_files" }` on `git add`. `resolve_builtin` in `src/generate.rs` has `git:branches` and `git:remotes` only. Unknown resolvers print a warning and leave `values` empty, so **TMP git-add cannot list dirty files**.

This is a protocol hole, not a schema mistake. Fix: implement `git:status_files` (and probably `git:status_files:unstaged` / `:staged` as optional suffixes) using `git status --porcelain`. No schema change required.

### 2. Curated schemas may vanish after `cargo install`

`curated_schemas_dir()` prefers `env!("CARGO_MANIFEST_DIR")/schemas/curated` at **runtime**. That path is wherever the crate was compiled (registry checkout or this repo). Copying the binary, `cargo install` after a registry GC, or a user without the source tree can make `waz generate --init` / first TUI open fail to install curated schemas.

`include_str!` is already used for `waz.json` in config mode. All nine curated files should be embedded the same way (`include_dir!` / `rust-embed`). Runtime copy to `~/.config/waz/schemas/` can stay so users can edit them.

### 3. TUI hard-requires `/dev/tty`

`src/tui/mod.rs` opens `/dev/tty` for the backend. That is correct for zsh widgets. It **breaks Windows PowerShell** (and any environment without `/dev/tty`) even though `waz init powershell` exists.

Compatible fix: try `/dev/tty`, then `CONIN$`/`CONOUT$` on Windows, then stderr/stdout. Crossterm already has `use-dev-tty`. Do not change the zsh widget contract.

### 4. SQLite has no WAL and no busy timeout

`record` runs from a background shell job while `predict` runs on the next prompt. Default SQLite locking will surface as silent failed records (`expect` in `main.rs` can also kill the helper).

Set `PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; PRAGMA synchronous=NORMAL` on open. Additive, no schema migration.

### 5. `which_exists` is Unix-only

`Command::new("which")` decides whether a schema with `requires_binary` loads. On Windows this is `where`. Schemas like `git` / `waz` can disappear on PowerShell. Use `which::which` or a PATH walk.

---

## P1 — TMP protocol (improve without breaking)

The current model is already a protocol, just unnamed:

```
SchemaFile { meta, commands[] }
CommandEntry { command, description, group, verified, tokens[] }
TokenDef { name, description, required, token_type, default, values, flag, data_source }
DataSource { command?, resolver?, parse }
TokenType = String | Boolean | Enum | File | Number
```

**Keep this JSON as v1.** Improvements below are additive or implementation-only.

### A. Write the spec down

There is no `docs/tmp.md`. The README describes UX, not the wire format. A one-page spec should lock:

- Field meanings and defaults
- How `flag: null` vs `flag: "--x"` serializes
- Boolean `true`/`yes` → emit flag, else omit
- Positionals always after flags (current `build_command`)
- `data_source` fills `values` at **select time**, not at file write time
- Share strips `values` when `data_source` is set

This is the cheapest way to stop accidental breaks.

### B. Named resolver registry, cached per session

Today every cargo token calls `CargoContext::detect` again (`cargo_resolve_bins`, `_examples`, `_packages`, …). Opening `cargo build` in TMP can re-parse `Cargo.toml` and walk `src/bin` **seven times**.

Compatible change: resolve `CargoContext` once per `(cwd, tui session)` and dispatch resolvers through a `HashMap<&str, fn>`. Same JSON, much faster TUI select.

Missing resolvers worth adding (all optional, unknown names already safe):

| Resolver | Used by | Status |
|----------|---------|--------|
| `git:branches` | git.json | implemented |
| `git:remotes` | git.json | implemented |
| `git:status_files` | git.json | **missing** |
| `cargo:*` | cargo.json | implemented, uncached |
| `npm:scripts` | npm/bun | implemented |
| `waz:models:<provider>` | waz.json | implemented, shells out to `curl` |
| `waz:context:*` | script schemas | implemented |

Natural extensions that do not break v1: `git:stash`, `docker:containers`, `kubectl:namespaces` as new names.

### C. Additive token fields (all defaulted)

Safe to add because serde ignores unknown fields on **read of old files**, and new fields default on **old code reading new files** only if we keep skip-serialize-when-default. Old waz binaries ignoring new fields is fine.

| Field | Default | Why |
|-------|---------|-----|
| `repeat` | `false` | `git add` should accept multiple paths without a new command |
| `exclusive_with` | `[]` | `--release` vs `--profile` |
| `visible_if` | `null` | `--no-edit` only when `amend=true` |
| `multi` | `false` | `cargo -F feat1,feat2` vs single enum |
| `placeholder` | `null` | TUI hint text without changing argv |
| `env` | `null` | “this token may come from `$EDITOR`” |

Do **not** add these until `build_command` has tests for current argv shape (see tests gap below).

### D. Stop mutating `token_type` during resolve

```rust
token.values = Some(vals);
token.token_type = TokenType::Enum;
```

A `File` token with a resolver becomes `Enum` in memory. That is reasonable for the TUI, but it is a silent type change and would leak if anyone saved the loaded schema back. Keep the declared type; treat populated `values` as a completion overlay.

### E. `data_source.command` is an arbitrary shell

`run_data_source_command` runs `sh -c cmd` in the project cwd. Generated schemas can put anything there. Compatible hardenings:

- Timeout (2–3s) and output cap
- Skip when cwd is not a trusted project (optional config)
- Prefer named resolvers in the generator prompt so new schemas do not need shell

Do not remove `command`; imported brew-style schemas use it.

### F. Generator prompt vs real schema

The generate prompt still asks for a **JSON array of commands**, then the saver wraps `SchemaFile`. Legacy `Vec<CommandEntry>` load remains. That dual format is a compatibility gift and a maintenance cost. Keep the reader; stop emitting the array-only form. Add a `$schema` comment in meta later (`meta.protocol = "tmp/1"`).

### G. Duplicate cargo/git graphs

Canonical TMP commands live in `schemas/curated/*.json`. `src/tui/cargo_schema.rs` still contains a full `build_cargo_commands` (~400 lines of TokenDef literals). `export_builtin_schema` in `generate.rs` rebuilds git commands in Rust that **diverge** from `git.json` (git add is `File` in export, `Enum` + `git:status_files` in JSON).

Cleanup that does not break TMP: make export copy curated JSON (or `include_str!`), delete the Rust command graphs, keep `CargoContext` as the **resolver implementation** only.

### H. Context matching is shallow

`requires_file` checks `cwd/file` only, not parents. In a workspace `crates/foo` with `Cargo.toml` only at root, cargo schema may not load. Compatible: walk up like `find_cargo_root` already does in `context.rs`. Same meta field, better behavior.

### I. npm and bun both require `package.json`

Both schemas load in a Node project → duplicate `install` / `run` entries. Compatible filter: `requires_binary` is already there (`bun` vs `npm`); prefer the lockfile (`bun.lockb` > `pnpm-lock.yaml` > `yarn.lock` > `package-lock.json`) as an optional `requires_lockfile` **or** as resolver logic, not a required field yet. Until then, `requires_binary` plus ranking in `filter_commands` is enough.

### J. `waz:models:gemini` is hardcoded in waz.json

The model token always hits the gemini resolver even if `--provider glm` is selected. Additive: a `depends_on: "provider"` resolver parameter, or resolve models in the TUI when the provider token changes. Old schema still works (just lists gemini models).

---

## P1 — performance

Ghost text on zsh is a **full process spawn per keystroke** (`waz predict --fast`). That path must stay under ~10–20ms.

| Hot spot | What happens | Fix (compatible) |
|----------|----------------|------------------|
| `Config::load()` | Disk TOML + env on every `PredictionEngine::new` | Process-lifetime cache, or skip config in `--fast` (LLM is already skipped) |
| Sequence query | Window function over **all** successful rows in cwd | Materialized `sequences(prev_key, next, cwd, count)` table updated on `record`; or at least `WHERE session_id IN (SELECT DISTINCT … LIMIT N)` |
| `get_session_commands` | Loads entire session for LLM | LLM already skipped in `--fast`; for record, store last command id in memory/file |
| `CargoContext::detect` | Re-run per token | Session cache (above) |
| `which_exists` | `which` subprocess per schema on TUI open | Cache PATH lookups |
| `waz:models` / share download | `curl` subprocess despite `ureq` in the crate | Use `ureq` everywhere |
| zsh `_waz_suggest` | Min 2 chars, still a process | Optional: unix socket or long-running helper later; not needed if predict stays tiny |
| Empty-prompt LLM | zsh empty buffer calls predict **without** `--fast` | Keep it; just do not let config load dominate. Document that empty prompt may hit network |
| TUI schema load | Parses every JSON in schemas dir, `eprintln` on init | Quiet init; lazy-parse per tool when `/` is pressed (already lazy until `/`) |

Do **not** add a daemon unless predict is still slow after WAL + config cache + sequence table. A daemon is a product change (lifecycle, crash, version skew).

---

## P1 — code quality and cleanup

### Module shape

`generate.rs` (1660 lines) mixes: schema I/O, curated install, data-source resolvers, help scraping, LLM generation, versioning, share/import, export of hardcoded builtins, curl. Split along existing seams:

- `schema::io` — load/save/init/share/import
- `schema::resolve` — named resolvers + shell data sources
- `schema::generate` — help scrape + LLM
- Keep `tui::app::{SchemaFile, TokenDef, …}` as the **protocol types** (or move them to `src/schema/types.rs` and re-export so JSON path does not change)

`App` in `tui/app.rs` is a god object (TMP + AI + placeholders + spinner). Splitting state later is internal; the JSON protocol does not move.

`main.rs` is a 700-line clap match. Fine for a CLI; extract `get_db_path`/`open_db` to kill the copy-paste `expect("Failed to open database")`.

### Dead / leftover code (safe deletes)

Clippy already flags several. Confirmed unused or superseded:

- `HistoryDb::count_by_cwd`
- `resolve::detect_best_tool` (wrapper; `detect_best_tool_with_context` is used)
- `tui/verify.rs`: unused `Alignment` import, `EditField::DataSource`, `current_cmd_mut`
- `export_builtin_schema`’s inlined git/cargo command graphs once curated JSON is the export path
- `ask()` legacy text path if only JSON callers remain (bash/fish still use `__WAZ_CMD__` — **do not delete** until those shells move to `--json`)

### Error handling

Library-ish modules return `Option`/`String` errors. `main` uses `expect` on DB open and schema rewrite. Replace with `eprintln` + exit code so a locked DB does not panic the zsh hook.

`hint.rs` `chars().next().unwrap()` is after an emptiness check — fine. `dirs::home_dir().unwrap()` in config paths is the real production panic.

### Tests that would lock TMP

Missing, and they are what make “without breaking it” enforceable:

1. `build_command` golden tests for git commit (`-m` + optional `--amend`), cargo run (`--bin` + positional none), boolean omit when false
2. Schema load: curated files all deserialize as `SchemaFile`
3. Unknown resolver does not drop the command
4. Share round-trip: values stripped, data_source kept
5. `requires_file` / `requires_binary` filters
6. `git:status_files` porcelain parse (once implemented)

No TUI event-loop tests are required for protocol safety.

### Style / tooling

- No `[lints]` in `Cargo.toml`; clippy is noisy (`if_same_then_else` in `context.rs`, `ptr_arg` in import, dead code). Turn on a small deny set in CI: `clippy::perf`, `unused`, `dead_code`.
- `cargo fmt` is not CI-gated; a full fmt is a one-shot cleanup (do it in its own commit).
- `Makefile` `publish` re-bumps from crates.io; easy to desync from GitHub tags. Document “tag then `cargo publish`” as the real path (what 0.1.7 used).

---

## P2 — product / UX (not protocol breaks)

- **Zsh ghost text vs PowerShell/bash:** acceptable split; document it. A PSReadLine predictor plugin is a separate binary module — not TMP.
- **Output hints (tier 0):** still not captured by the shell. Do not bring back `tee`. Workflows already cover the common cases. Optional: opt-in `script`/`pty` later.
- **Filter scoring** is prefix/contains only; `/comit` misses `commit`. A tiny fuzzy rank (typo of 1) would help TMP search without schema changes.
- **Repeat last failed command** as a first-class empty-prompt suggestion is still missing (failed rows are excluded from CWD history).
- **README** says 8 curated schemas; there are **9** (`waz.json`).
- **Hardcoded tool aliases** in `detect_tool_from_query` (`postgres` → `psql`) duplicate `meta.keywords`. Prefer keywords in schema files so aliases ship with the schema.

---

## Recommended order of work

Do these in this order so TMP stays stable and each step is bisectable.

1. **Lock the protocol with tests** — deserialize all curated JSON; golden `build_command` cases. No user-visible change.
2. **Fix P0** — `git:status_files`, embed curated schemas, WAL + busy_timeout, TUI tty fallback, Windows `which`.
3. **Cache resolvers + CargoContext** — same JSON, faster TMP select and resolve prompts.
4. **Delete duplicate command graphs** — `export_builtin_schema` reads curated files; shrink `cargo_schema.rs` to detection only.
5. **Predict fast path** — cache `Config` in `--fast`; consider a sequences table if profiling still shows the window query.
6. **Write `docs/tmp.md`** — v1 spec as it actually behaves today, plus the additive fields we are willing to add.
7. **Additive token fields** (`repeat`, `visible_if`) only after (1) and (6), behind defaulted serde.

Skip a prediction daemon, a schema v2 rewrite, and removing `data_source.command` until the above is done.

---

## Out of scope / do not do

- Renaming JSON keys (`token_type` → `type`, `group` → `tool`) — breaks every schema on disk.
- Making all tokens positional-first to “match clap” — changes argv for cargo/git.
- Requiring `meta.keywords` or `meta.protocol` on load.
- Replacing SQLite with something else.
- Merging AI placeholder editing with TMP tokens into one UI model in the same change as protocol fields.

---

## Appendix — file map

| Path | Role |
|------|------|
| `src/tui/app.rs` | TMP types + TUI filter/select/`build_command` |
| `src/generate.rs` | Schema I/O, resolvers, AI generation, share/import |
| `src/resolve.rs` | NL → TMP-grounded command via LLM |
| `src/tui/cargo_schema.rs` | Cargo project inspection **and** leftover command graph |
| `schemas/curated/*.json` | Canonical TMP v1 documents |
| `src/predict.rs` / `db.rs` / `hint.rs` | Ghost-text engine (not TMP) |
| `src/tui/mod.rs` | Event loop, `/dev/tty` |
| `shell/waz.*` | Record + predict + TUI bindings |
