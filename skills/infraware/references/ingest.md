# Add approved rules

Use this flow when the user agrees to add exact rule text. For each accepted `RULE_TEXT`, generate 5–10 different, concrete task descriptions where that rule applies. A task description must describe real work where the rule applies, never the user's request to store the rule or the rule itself. Do not paraphrase one scenario to fill the quota.

Use the same approved rule text for every generated task context. Run one command for each task-rule pair:

```sh
cargo run -- ingest "<TASK_TEXT>" "<RULE_TEXT>"
```

Do not require the tasks to have already occurred. Inspect each result and report stored pairs accurately. If a command fails, report completed and remaining pairs, then stop; do not claim the whole batch succeeded.
