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

Bare `infraware` and `infraware --help` print the same compact help text. `infraware --version` prints `infraware ` followed by the version from Cargo's `CARGO_PKG_VERSION`, then a newline. The informational forms do not read or create `.infraware` files.

The accepted command grammar is:

```
Usage: infraware <COMMAND>

Commands:
  ingest <TASK_TEXT> <RULE_TEXT>
  query <TASK_TEXT>
  list [LIMIT] [OFFSET]
  get <ID> [--entity rules|tasks]
  patch <ID> --text <TEXT> [--entity rules|tasks]
  remove <ID>
  status
  revise <ID> --entity rules|tasks
```

`-h`, `-V`, `help`, and command-specific help such as `query --help` are unsupported and return a usage error.

The eight verbs use `<project>/.infraware/vec-index.sqlite`.

- `ingest <TASK_TEXT> <RULE_TEXT>` stores one task and its paired historical rule. It embeds both raw texts independently, reuses nearby canonical task and rule rows, and returns `task_id`, `rule_id`, `created_at`, and `duplicate` after commit.
- The v2 store has no user metadata or metadata filters. `list [LIMIT] [OFFSET]` always returns rules and their related tasks. `get <ID> [--entity rules|tasks]` and `patch <ID> --text <TEXT> [--entity rules|tasks]` retain their entity selectors. `list` defaults to a limit of 50 and offset of 0, accepts a maximum limit of 500, and paginates rules only. `total` is the number of rules before pagination.

  Compact YAML is the default:

  ```yaml
  limit: 50
  offset: 0
  rules:
    - text: Require review before deploy
      id: 3
      tasks:
        - text: Open a review request
          id: 8
  total: 1
  ```

  JSON uses the same grouped fields:

  ```json
  {"limit":50,"offset":0,"rules":[{"text":"Require review before deploy","id":3,"tasks":[{"text":"Open a review request","id":8}]}],"total":1}
  ```

  Rules are ordered by `created_at`, then ID. Tasks nested under each rule use the same ordering. A rule with no relation has `tasks: []`, and a task linked to several rules appears under each rule. `--detailed` adds `created_at` and non-null `updated_at` to every rule and task; null `updated_at` fields are omitted.

  Pagination applies to rules, not nested tasks. For example, `infraware list 10 20` returns rules 21 through 30 in order and includes every task related to each returned rule.

  `list --entity ...` is no longer accepted and exits with status 2. This is a breaking change. Replace flat `records` parsing with the grouped `rules` array, and read tasks from each rule's `tasks` array. Task pages are no longer available through `list`; callers that need them must process the nested tasks from rule pages.

  ```sh
  infraware list 10 20
  infraware get 3 --entity tasks
  infraware patch 3 --text "new task" --entity tasks
  ```
- Only ingest creates a missing store, and it creates a fresh schema with informational `schema_version=1`. The fresh schema includes nullable `updated_at` columns on tasks and rules. Other verbs require an existing usable store created with this schema. A store missing either column is unsupported, and the application does not migrate or repair it. If a non-ingest command cannot find the canonical store, it exits with `error: canonical storage is not initialized; run \`infraware ingest <TASK_TEXT> <RULE_TEXT>\` first`. Non-ingest commands do not auto-initialize, migrate, or repair storage.
- `status` reads tasks and rules without changing the store. It reports compact JSON with `all_up_to_date`, `window_days`, `stale_tasks`, and `stale_rules`. Each stale entry contains only `id`, `text`, and `last_revised_at`, an epoch-millisecond timestamp. `last_revised_at` uses `updated_at` when present and `created_at` otherwise. A record is stale only when that timestamp is older than the configured window. Status does not create an index or settings file.
- `revise <ID> --entity rules|tasks` records an explicit revision touch on one existing selected-entity row. It updates only `updated_at`, including when it is `NULL`, and returns `{"id":N,"revised":true}` after the write succeeds. Usage errors, including a missing or unsupported `--entity`, `--text`, duplicate flags, and unknown flags, exit 2 with empty stdout and one usage diagnostic. Missing records and operational failures exit 1 with empty stdout and one `error:` diagnostic. `revise` affects the freshness reported by `status`, but does not change text, embeddings, vector relations, or entity identity.
- `query <TASK_TEXT>` searches historical task embeddings, follows task/rule pair relations, and returns nearest tasks first. Query auto-ingest is disabled by default, so a query does not write to the index unless `query_auto_ingest: true` is set.
- Failed SQLite writes roll back. Usage and validation failures exit 2 with empty stdout. Store and operational failures exit 1 with empty stdout and one `error:` line on stderr.

### Repository distance settings

Infraware optionally reads `root/.infraware/settings.yaml`, where `root` is the repository root passed to the command. The supported schema is:

```yaml
distances:
  task_dedup: 0.15
  rule_dedup: 0.15
  query_max: 0.15
query_auto_ingest: false
staleness_threshold_days: 14
```

The three distance keys are independent. Each defaults to `0.15` when it is missing or set to `null`. `staleness_threshold_days` is an optional top-level setting. It defaults to `14` days when it is missing or set to `null`. `query_auto_ingest` is an optional top-level boolean that defaults to `false`; set it to `true` to enable query write-back. A missing file, empty or whitespace-only file, null YAML document, missing `distances` section, or null section also uses the distance defaults. Unknown keys are ignored at every supported level.

Configured distance values must be finite numbers in the inclusive range `[0, 2]`. A configured staleness window must be a finite, non-negative number of days. Malformed YAML, wrong-shaped roots or sections, invalid leaf types, non-finite values, and values outside those ranges produce an operational configuration error.

`ingest` loads the settings for task and rule deduplication, `patch` loads them for task collision checks, and `query` loads `query_max` for filtering and the response envelope plus `query_auto_ingest` for optional write-back. `status` loads `staleness_threshold_days`; `list`, `get`, and `remove` do not load this file. Without settings, distance behavior remains the existing `0.15` behavior and status uses a 14-day window. Query write-back used to happen for every non-empty result. It now requires `query_auto_ingest: true`. Settings do not change the CLI verbs, stored vectors, or no-network behavior.
