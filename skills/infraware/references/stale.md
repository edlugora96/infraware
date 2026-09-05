# Review stale records

Use this flow when the user agrees to review stale records. Start with:

```sh
cargo run -- status --detailed
```

Inspect the stale rules and tasks, then decide each record with the user. For a still-valid record whose content is unchanged, run `cargo run -- revise <ID> --entity rules` (substitute `tasks` for a task record). Revising marks it valid without changing its content. Do not patch unchanged text to reaffirm it, and do not bulk-revise stale entries the user has not decided on.

For a changed record, run:

```sh
cargo run -- patch <ID> --text "<TEXT>" --entity rules
```

Substitute `tasks` for a task record. Run commands only for records the user decided. Rerun `cargo run -- status --detailed` and report what was resolved and what remains. Limit the maintenance summary to confirmed outcomes and unresolved records. Do not infer release readiness. Do not make automatic further edits. If more context is needed, use the [audit reference](audit.md); do not add another scenario document.
