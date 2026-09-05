# Update or remove records

Use this flow when the user requests modification or removal of a rule or task. Identify the selected record and type, then verify it with `cargo run -- get <ID> --entity rules` or `cargo run -- get <ID> --entity tasks`. If the target is ambiguous, clarify rather than guess. Confirm that the requested replacement or deletion is authorized; explicit approval already given counts, so do not ask redundantly.

For an approved replacement, run:

```sh
cargo run -- patch <ID> --text "<TEXT>" --entity rules
```

Use `tasks` instead of `rules` for a task. For an approved rule or task removal, use `cargo run -- remove <ID> [--entity rules|tasks]`. The `--entity` selector is optional. For example, use `cargo run -- remove <ID> --entity rules` for a rule, or substitute `tasks` for a task. If omitted, do not infer a default. Remove only specifically approved records.

If an unchanged record only needs reaffirming, use [Review stale records](stale.md). Report command results accurately.
