---
name: tmp-schema
description: >
  Generate a complete waz Token Model Protocol (TMP) schema from a CLI's help
  text. Use when creating or regenerating ~/.config/waz/schemas/<tool>.json
  for the command palette, waz tmp, or waz resolve.
---

# TMP schema generation

You convert CLI `--help` into a **complete** JSON array of command entries for waz.

Read `references/tmp-contract.md` before emitting JSON.

## Goal

Cover **every subcommand and nested command** present in the help harvest, not a popular subset. Prefer completeness over brevity. If help is truncated, still emit every command named in the harvest and mark missing flags only when the help for that command was not provided.

## Output

Return **only** a JSON array (no markdown fences, no commentary). Each element:

```json
{
  "command": "tool sub",
  "description": "one line",
  "group": "tool",
  "tokens": []
}
```

## Rules

1. `command` is **only** the binary plus subcommand words (`git commit`, `docker compose up`). Never put `[args]`, `<file>`, or flags in `command`.
2. Include a base entry `command: "<tool>"` for flags that apply with no subcommand.
3. One JSON object per invocable command path. Nested CLIs (`docker compose logs`) are separate entries, not flattened into `docker`.
4. Every flag and positional in that command's help becomes a token.
5. Token `default` is always a string or JSON null (`"false"`, `"0"`, never a JSON boolean/number).
6. Token `flag` is `"--verbose"`, `"-n"`, or JSON null for positionals. Never `false`.
7. `token_type`: `Boolean` for switches, `Enum` when help lists choices (`values: [...]`), `File` for paths, `Number` for ints/floats, else `String`.
8. Do not invent subcommands that are not in the harvest. Do not skip ones that are.
9. Prefer built-in resolvers over empty enums when the value is dynamic:
   - git files → `{"resolver":"git:status_files"}`
   - git branches → `{"resolver":"git:branches"}`
   - git remotes → `{"resolver":"git:remotes"}`
   - cargo bins/examples/packages/features/profiles/tests/benches → `cargo:<name>`
   - npm scripts → `{"resolver":"npm:scripts"}`
   Otherwise use `"data_source": {"command": "<safe list cmd>", "parse": "lines"}`.
10. Boolean flags default to `"false"` and `required: false`.
11. Required positionals: `required: true`, `flag: null`.
12. `group` is the tool name (the binary).
13. Repeatable positionals/flags (`git add` paths): `"repeat": true` so multiple `--set path=` values append.

## Completeness checklist

- [ ] Base command present
- [ ] Every harvested `=== tool … --help ===` heading has a matching `command`
- [ ] Nested commands included
- [ ] Help/version meta commands omitted
- [ ] Flags are tokens, never commands
