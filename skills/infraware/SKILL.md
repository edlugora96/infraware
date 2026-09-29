---
name: infraware
description: 'Use before starting any task or taking any action to identify applicable standards, guidelines, and constraints.'
---
1. Start each session with `infraware status` or `infraware init` in case of a new session.
2. Before each concrete task or step, query a quoted description of that single task: `infraware query "<TASK_TEXT>"`. Repeat or refine the query for other steps or needed coverage. TASK Example:
    - Create a new Python service named `weather_api`.

3. Apply every applicable retrieved rule during the work. Respect higher-priority instructions and surface conflicts instead of guessing.
4. If broader context suggests a chat audit could be useful, end the final report with an **Infraware audit** section. Briefly explain why the engineer may want to use `infraware-audit`, without proposing any rules. Omit the section otherwise. The engineer decides whether to invoke the audit; do not invoke it automatically.

In the final report, offer relevant maintenance only if `infraware status` reports stale rules. Do not start maintenance unasked.

## Tasks and rules

- **TASKS** are concrete actions for the current work. They say what to do before or during a step.
- **RULES** are standards or constraints that guide decisions across the work. They say what must or must not happen.

## Other flows

Read the linked reference before executing a matching flow the user requests or agrees to:

- [Review stale records](references/stale.md) when the user agrees to review stale records.
- [Add approved rules](references/ingest.md) when the user agrees to add exact rules.
- [Inspect stored data](references/audit.md) when the user asks what is stored.
- [Update or remove a rule or task](references/update.md) when the user requests a record change.
- [Install and update](references/install-update.md) when the user wants to install or update infraware.


