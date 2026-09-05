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

- `ingest <FACT_TEXT> <QUERY_TEXT>` stores one fact and its paired historical query. It embeds both raw texts independently, reuses nearby canonical fact and query rows, and returns `fact_id`, `query_id`, `created_at`, and `duplicate` after commit.
- The v2 store has no user metadata or metadata filters. `list` and `get` return only IDs, text, and epoch-millisecond timestamps. `patch` accepts `--text` only.
- Only ingest creates a missing store, and it creates a fresh v2 schema. Other verbs require an existing compatible v2 store and never migrate or repair it.
- `query <TEXT>` searches historical query embeddings, follows fact/query pair relations, and returns nearest facts first.
- Failed SQLite writes roll back. Usage and validation failures exit 2 with empty stdout. Store and operational failures exit 1 with empty stdout and one `error:` line on stderr.
