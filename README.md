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

### Install the skill

Install the Infraware skill with:

```sh
npx skills add git@github:edlugora96/infraware.git --skill infraware
npx skills add git@github:edlugora96/infraware.git --skill infraware-coding
npx skills add git@github:edlugora96/infraware.git --skill infraware-audit
```

The repository is private, so you need GitHub access and a configured SSH key.

### Tags & Versioning

- Version `0.1.0` (Cargo.toml authoritative, package.json in sync)
- Git tags: `v0.1.0`
- Updates: reinstall via npm with new tag; no auto-update or `infraware update` command exists.

### Troubleshooting

- **Auth failures:** verify SSH key `ssh -T git@github.com` or PAT scope/credential helper; for HTTPS ensure PAT not expired.
- **Unsupported platform:** launcher supports `darwin-arm64`, `darwin-x64`, `linux-x64`, `win32-x64` only; others exit with error (deferred).

### Updating

No internal updater. To update: `npm install -g github:edlugora96/infraware#<new-tag>`

### CLI (SQLite semantic-search verbs)

Bare `infraware` and `infraware --help` print the same compact help text. `infraware --version` prints `infraware ` followed by the version from Cargo's `CARGO_PKG_VERSION`, then a newline. The informational forms do not read or create `.infraware` files.

The accepted command grammar is:

```
Usage: infraware <COMMAND>

Commands:
  init
  ingest <TASK_TEXT> <RULE_TEXT>
  query <TASK_TEXT>
  list [LIMIT] [OFFSET]
  get <ID> --entity rules|tasks
  patch <ID> --text <TEXT> [--entity rules|tasks]
  remove <ID> --entity rules|tasks
  status
  revise <ID> --entity rules|tasks
```

`remove` deliberately requires the entity selector; the former bare `remove <ID>` form is unsupported and returns a usage error. `-h`, `-V`, `help`, and command-specific help such as `query --help` are unsupported and return a usage error.

The CLI uses the canonical store at `<project>/.infraware/vec-index.sqlite`. Run `infraware init` to create that store explicitly before using read or mutation commands. `init` is idempotent: it succeeds without changing an existing compatible store. If the canonical path exists but is not a usable compatible store, `init` fails safely and does not migrate, repair, replace, or alias it.

- `ingest <TASK_TEXT> <RULE_TEXT>` stores one task and its paired historical rule. It embeds both raw texts independently, reuses nearby canonical task and rule rows, and returns `task_id`, `rule_id`, `created_at`, and `duplicate` after commit.
- The v2 store has no user metadata or metadata filters. `list [LIMIT] [OFFSET]` always returns rules and their related tasks. `get <ID> --entity rules|tasks` requires an entity selector because task and rule IDs are scoped separately. `patch <ID> --text <TEXT> [--entity rules|tasks]` retains its entity selector. `list` defaults to a limit of 50 and offset of 0, accepts a maximum limit of 500, and paginates rules only. `total` is the number of rules before pagination.

  `get` returns `id`, `text`, `created_at`, and numeric `related_count` in compact YAML and JSON. In detailed mode, a rule includes `tasks` and a task includes `rules`; each related item has `id`, `text`, and `created_at`. Arrays are present as `[]` when no valid related records exist. Related items are ordered by `created_at` ascending, then `id` ascending. Detailed output emits at most 500 related rows, while `related_count` reports the full valid count.

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

  `list --entity ...` is no longer accepted and exits with status 2. This is a breaking change. Replace flat `records` parsing with the grouped `rules` array, and read tasks from each rule's `tasks` array. Task pages are no longer available through `list`; callers that need them must process the nested tasks from rule pages. `get` now requires `--entity rules|tasks` and adds numeric `related_count` to compact output. Consumers using detailed `get` must read the selected entity's `tasks` or `rules` array, including the stable empty array. This is an intentional output compatibility break, not a database change. Roll back the code if needed. Stored rows and relation writes are unchanged; consumers should migrate their parsers to accept the new field and arrays.

  ```sh
  infraware list 10 20
  infraware get 3 --entity tasks
  infraware patch 3 --text "new task" --entity tasks
  ```
- `init` creates a missing canonical store with informational `schema_version=1`. The fresh schema includes nullable `updated_at` columns on tasks and rules. Running `init` again against a compatible store succeeds without changing it. If the canonical path is unusable or incompatible, `init` fails safely; it does not migrate, repair, replace, or use an alias path. Read and mutation commands require an existing usable canonical store and report an operational error when it is absent. `ingest` keeps its existing lazy initialization behavior, so it may create the missing canonical store when it writes the first pair.
- `status` reads tasks and rules without changing the store. Tasks are never stale, so status evaluates rule freshness only and derives `all_up_to_date` from stale rules. JSON reports `all_up_to_date`, `window_days`, and `stale_rules`; compact and detailed YAML report the same rule-only staleness and omit task staleness output. Each stale rule contains only `id`, `text`, and `last_revised_at`, an epoch-millisecond timestamp in JSON. `last_revised_at` uses `updated_at` when present and `created_at` otherwise. A rule is stale only when that timestamp is older than the configured window. This is a breaking output-contract change. Status does not create an index or settings file.
- `revise <ID> --entity rules|tasks` records an explicit revision touch on one existing selected-entity row. It updates only `updated_at`, including when it is `NULL`, and returns `{"id":N,"revised":true}` after the write succeeds. Rule revisions affect the freshness reported by `status`; task revisions update stored timestamps but do not affect staleness. Usage errors, including a missing or unsupported `--entity`, `--text`, duplicate flags, and unknown flags, exit 2 with empty stdout and one usage diagnostic. Missing records and operational failures exit 1 with empty stdout and one `error:` diagnostic. `revise` does not change text, embeddings, vector relations, or entity identity.
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
