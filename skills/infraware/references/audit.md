# Inspect stored data

Use this read-only flow when the user asks what is stored. Show the first page with:

```sh
cargo run -- list
```

For a selected page, use `cargo run -- list <LIMIT> <OFFSET>`. Add `--detailed` only when the user requests full details: `cargo run -- list <LIMIT> <OFFSET> --detailed`. For a whole inventory, paginate until complete; do not confuse one page with all records. Do not assume hardcoded defaults or maximums.

When the user knows an ID, inspect it with `cargo run -- get <ID> --entity rules`; substitute `tasks` for a task record. Do not modify or renew anything during an audit. If you find a problem, describe it and await separate approval to update or review it.
