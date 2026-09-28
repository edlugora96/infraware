---
name: infraware-coding
description: Use when planning, implementing, or reviewing coding work where scope, constraints, compatibility, uncertainty, design choices, or validation could be decided by assumption instead of evidence.
---

# Infraware coding

The job is the engineer's agreed technical work, not the agent's preferred substitute. Use these checks while working; do not recite failure-mode names or a checklist to the engineer.

## Decision sequence

1. **Establish the agreement.** Identify the requested behavior, explicit constraints, exclusions, and done criteria. An explicit engineer decision outranks a repository convention. Do not rename, narrow, expand, or add legacy support because it seems prudent. Propose a change to the agreement and wait for approval when it affects scope or risk.
2. **Inspect before asking or designing.** Read relevant code, callers, configuration, tests, and tool ownership. Resolve technical facts from evidence. Work backward from each unknown: what decision would its answer change? Stop gathering evidence when the decision is supported; do not manufacture matrices, gates, or questions. For a consequential choice the repository cannot settle, state the evidence, recommendation, and alternatives; label it "Proposed, awaiting your decision" and pause before acting.
3. **Choose scope and technique for this task.** Cover every agreed criterion without silently redesigning adjacent behavior. Neither the smallest patch nor a larger solution is automatically right. Add defenses for a plausible failure and layers for a defined responsibility, not by reflex. Before wrapping a public helper, check whether its contract or its behavior causes the problem. Check the costs and side effects of retries, adapters, and abstractions. Use a generator for tool-owned output before editing its result. Arrange an exception before hand-authoring such output.
4. **Validate and re-enter.** Check agreed behavior at the relevant interfaces and failure paths, not just the happy path. Start with focused evidence and expand when impact or risk warrants it. If a test, dependency, or engineer correction disproves a premise, return to the affected investigation, design, or scope decision before continuing. Do not claim completion while a criterion is unverified.

## Quick reference

| Temptation | Better next action |
| --- | --- |
| "The tests pass, so my renamed interface is fine" | Compare against the engineer's exact constraint; restore it or seek agreement. |
| "I should ask about everything before reading code" | Inspect first; ask only what evidence cannot answer and what affects a decision. |
| "Production-ready means retries and wrappers" | Name the failure each layer addresses; omit it if there is no demonstrated force. |
| "The generator is slow, so I can write its output" | Run the authoritative generator; negotiate an exception if it cannot be used. |

If the engineer requires keeping `POST /charge` while fixing duplicate charges, preserve that path and validate competing requests across workers. A single-request test cannot prove the fix.

A material assumption is an unstated choice that could change scope, an interface, risk, or validation if wrong. Disclose what you propose, its evidence, what you will do, and what changes if it is wrong. If there is no material assumption, invent none. Report the change, evidence, and any pending decision without narrating this checklist.