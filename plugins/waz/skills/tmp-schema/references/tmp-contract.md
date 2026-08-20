# TMP JSON contract (waz)

Schema file on disk:

```json
{
  "meta": {
    "tool": "docker",
    "version": 1,
    "generated_by": "ai",
    "coverage": "partial",
    "requires_binary": "docker"
  },
  "commands": [ /* array you generate */ ]
}
```

You generate **only** the `commands` array.

## CommandEntry

| Field | Type | Notes |
|-------|------|--------|
| command | string | `git commit`, not `git commit -m <msg>` |
| description | string | Short |
| group | string | Usually the tool name |
| tokens | array | Flags and positionals |

## TokenDef

| Field | Type | Notes |
|-------|------|--------|
| name | string | Identifier, snake or kebab ok |
| description | string | |
| required | bool | |
| token_type | String, Boolean, Enum, File, Number | |
| default | string or null | `"false"` not `false` |
| values | string[] or null | Enum choices |
| flag | string or null | `--bin` / `-m` / null if positional |
| data_source | object or null | `{ "resolver": "cargo:bins" }` or `{ "command": "…", "parse": "lines" }` |

## Built-in resolvers

`cargo:bins`, `cargo:examples`, `cargo:packages`, `cargo:features`, `cargo:profiles`, `cargo:tests`, `cargo:benches`, `git:branches`, `git:remotes`, `git:status_files`, `npm:scripts`, `waz:models:<provider>`, `waz:context:<field>`.
