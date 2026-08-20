---
name: tmp-use
description: >
  Build real shell commands from waz Token Model Protocol (TMP) schemas instead
  of guessing flags. Use whenever the user wants to run, fill, or generate a
  cargo/git/npm/docker/kubectl (or any schema-backed) command. Prefer waz MCP
  tools over inventing argv.
---

# Use waz TMP instead of guessing

Waz stores **verified command shapes** (binary + tokens + live values) in TMP schemas. Agents MUST assemble commands from those schemas.

## Workflow

1. `waz_tmp_list` with the user's cwd and a short query (`cargo`, `git`, `docker`).
2. Pick the exact `command` string from the list (e.g. `cargo run`, `git commit`).
3. `waz_tmp_show` for that command to see tokens and resolved values.
4. `waz_tmp_build` with `--set name=value` pairs (or `waz_resolve` for natural language).
5. Return the `argv` string. Do not invent flags that were not in the schema.

If the tool is missing from the list, call `waz_generate` and **wait until it finishes** (`wait` defaults to true). Then retry list/show. Do not call `waz_tmp_list` immediately after a background generate (`wait=false`) — poll `waz_generate_status` with `wait=true` (or `waz generate --wait --job <id>`) until `done` or `error`. Do not hand-write a schema.

## Rules

- Never guess `--flag` names for a tool that has a TMP schema.
- Never put positionals into the command name; they are tokens.
- Prefer resolver-backed values (bins, branches, scripts) from `waz_tmp_show`.
- If `waz_tmp_build` errors, fix tokens and retry — do not fall back to a guessed command.
