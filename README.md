# infraware

## Private Distribution

Repository: `git@github.com:edlugora96/infraware.git`

### Authentication

**SSH:**
```
# add your SSH key to GitHub, then
git clone git@github.com:edlugora96/infraware.git
```

**HTTPS with PAT:**
Create a fine-grained PAT (classic PAT with `repo` scope also works), then use credential helper:
```
git config --global credential.helper store  # or osxkeychain / manager
git clone https://github.com/edlugora96/infraware.git
# username: your GitHub username, password: PAT
```
Or inline (not recommended for persistence): `https://<PAT>@github.com/edlugora96/infraware.git`

### Install

```sh
npm install -g github:edlugora96/infraware
# with tag:
npm install -g github:edlugora96/infraware#v0.1.0
```

Requires Node >=18 and valid GitHub auth for the private repo.

### npx (if supported)

npm >=7 supports GitHub URL shorthand:

```sh
npx github:edlugora96/infraware --help
npx github:edlugora96/infraware#v0.1.0 --help
```

If your npm version does not support `npx github:` URLs, use the global install above.

### Tags & Versioning

- Version `0.1.0` (Cargo.toml authoritative, package.json in sync)
- Git tags: `v0.1.0`
- Updates: reinstall via npm with new tag; no auto-update or `infraware update` command exists.

### Troubleshooting

- **Auth failures:** verify SSH key `ssh -T git@github.com` or PAT scope/credential helper; for HTTPS ensure PAT not expired.
- **npx GitHub URL not supported:** upgrade npm (`npm i -g npm`) or use `npm install -g`.
- **Unsupported platform:** launcher supports `darwin-arm64`, `darwin-x64`, `linux-x64`, `win32-x64` only; others exit with error (deferred).

### Updating

No internal updater. To update: `npm install -g github:edlugora96/infraware#<new-tag>`

### CLI (SQLite semantic-search verbs)

The six verbs are `ingest`, `query`, `list`, `get`, `patch`, and `remove`. They use `<project>/.infraware/vec-index.sqlite`.

- `ingest <TASK_TEXT> <RULE_TEXT>` stores one task and its paired historical rule. It embeds both raw texts independently, reuses nearby canonical task and rule rows, and returns `task_id`, `rule_id`, `created_at`, and `duplicate` after commit.
- The v2 store has no user metadata or metadata filters. `list [LIMIT] [OFFSET] [--entity rules|tasks|all]`, `get <ID> [--entity rules|tasks]`, and `patch <ID> --text <TEXT> [--entity rules|tasks]` select the entity explicitly. The selector defaults to `rules` for all three commands; `all` is valid only for `list`. `list` returns `total`, `limit`, `offset`, and `records`, while each record and `get` response contains only `id`, `text`, and epoch-millisecond `created_at`.
- List IDs are scoped to the selected entity. `list --entity all` returns one combined page with rules first, ordered by ascending rule ID, followed by tasks ordered by ascending task ID. An ID lookup never falls through to the other entity. Callers that relied on the old task default must add `--entity tasks`.

  ```sh
  infraware list 10 20 --entity tasks
  infraware list --entity all
  infraware get 3 --entity tasks
  infraware patch 3 --text "new task" --entity tasks
  ```
- Only ingest creates a missing store, and it creates a fresh schema with informational `schema_version=1`. Other verbs require an existing compatible store and never migrate or repair it.
- `query <TASK_TEXT>` searches historical task embeddings, follows task/rule pair relations, and returns nearest tasks first.
- Failed SQLite writes roll back. Usage and validation failures exit 2 with empty stdout. Store and operational failures exit 1 with empty stdout and one `error:` line on stderr.

### Repository distance settings

Infraware optionally reads `root/.infraware/settings.yaml`, where `root` is the repository root passed to the command. The supported schema is:

```yaml
distances:
  task_dedup: 0.15
  rule_dedup: 0.15
  query_max: 0.15
```

The three keys are independent. Each defaults to `0.15` when it is missing or set to `null`. A missing file, empty or whitespace-only file, null YAML document, missing `distances` section, or null section also uses those defaults. Unknown keys are ignored at every supported level.

Configured values must be finite numbers in the inclusive range `[0, 2]`. Malformed YAML, wrong-shaped roots or sections, invalid leaf types, non-finite values, and values outside that range produce an operational configuration error.

`ingest` loads the settings for task and rule deduplication, `patch` loads them for task collision checks, and `query` loads `query_max` for both filtering and the response envelope. `list`, `get`, and `remove` do not load this file. Without settings, threshold behavior remains the existing `0.15` behavior. Settings do not change the CLI verbs, SQLite storage schema, stored vectors, or no-network behavior.
