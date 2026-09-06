---
name: codebase-rule-proposals
description: Walk a codebase and propose evidence-backed durable engineering rules.
disable-model-invocation: true
---

# Codebase rule proposals

## Boundaries

This skill does not:

- call the Infraware CLI;
- inspect status, settings, queries, or stored data;
- read or write `.infraware/` or SQLite data;
- ingest, patch, revise, or remove rules or tasks;
- mutate source or storage;
- make changes;
- produce HTML, Mermaid, or SVG;
- print an executable ingest command.

Keep proposal wording separate from Infraware ingestion concepts. Applicability examples, if included, must describe real engineering contexts, not approval or storage requests.

## Scope

Honor explicit user direction first. If the user gives no scope, start with hot spots indicated by git history. Hot-spot prioritization is a starting point, not an exhaustive review. If evidence or risk is scattered across boundaries, widen the review until the relevant path and its surrounding decisions are covered. State the scope reviewed and any meaningful exclusions.

## Context-first organic walk

Start with the code that gives the work context: entry points, neighboring types, callers, tests, configuration, and the documentation that explains the behavior. Follow the behavior organically rather than forcing the review through a fixed directory list. Move outward when a dependency, repeated decision, or failure path needs more context.

At each stop, look through these friction lenses:

- repeated manual decisions or checks;
- boundaries where data, ownership, or errors change;
- names or vocabulary that make the intended behavior unclear;
- tests, comments, or call sites that disagree with implementation;
- workarounds, duplication, and exception paths;
- changes that repeatedly need coordination or careful ordering.

Apply a deletion test to each possible rule. Ask whether removing the rule would make a concrete failure, inconsistency, or repeated cost more likely. Drop candidates that only restate taste, describe a one-off choice, or cannot guide a checkable future decision.

Use the vocabulary already used by the codebase when it is precise. Define a term once if it has multiple meanings. Do not turn a local name, implementation detail, or preferred style into a broader rule without evidence.

## Evidence classification and rule gate

Separate findings into four labels:

- **Facts:** directly observed code, tests, history, or documentation, with a path and line reference when possible.
- **Assumptions:** interpretations that still need confirmation.
- **Risks:** concrete likely failures, maintenance costs, or inconsistent behavior tied to the evidence.
- **Opinions:** reviewer preferences that are not supported by a durable codebase need.

A durable rule candidate requires at least two independent concrete evidence locations. Independent locations show the same decision or failure pattern in separate files, modules, tests, call paths, or history points. A second mention of the same statement is not enough. Put weaker single-site findings in **Observations (not rules)** instead.

Do not infer a rule from frequency alone. Explain why the evidence supports a reusable directive, and record uncertainty rather than filling gaps.

## Proposal schema

For every candidate, use plain Markdown and include these fields:

### [Short proposal title]

**RULE_TEXT:** One concise, checkable, durable directive. Write what an engineer should do or verify. Do not include a command, approval request, storage action, or implementation detail that will age quickly.

**Rationale:** Explain the recurring problem or decision the directive addresses. Distinguish facts from assumptions, risks, and opinions.

**Evidence:** List at least two independent concrete locations. Include file paths and line references, and state what each location demonstrates. If a line reference is unavailable, say what was inspected without inventing a number.

**Scope:** State where the rule applies and where it does not apply. Include relevant boundaries, exceptions, and the reviewed scope.

**Counter-case:** Give a concrete case where applying the rule would be wrong, unnecessary, or harmful. If no counter-case is known, say that it was not found during this review rather than claiming none exists.

**Confidence/strength:** Label the proposal, such as strong, moderate, or weak, and tie the label to the quality and independence of the evidence.

**Applicability examples:** Optionally give grounded examples of real engineering contexts in which an engineer would apply the rule. Do not turn examples into approval, ingestion, or storage requests.

Keep the proposed rule narrower than its evidence supports only when that makes it safer and still useful. Never broaden it to cover unobserved cases.

## Ranking and output

Rank proposals by the strength of independent evidence, the cost of the observed failure or friction, the breadth of the affected path, and the clarity of a checkable directive. Do not rank a proposal highly merely because it sounds general. Put uncertain or weak findings in **Observations (not rules)** with their evidence and the reason they did not pass the rule gate.

Include the reviewed scope, evidence classifications, proposals, and observations in the Markdown output. Keep it concise enough to inspect, but retain the paths and line references needed to verify each claim.

After explicit approval, state that the separate existing infraware workflow handles 5–10 concrete task contexts and any ingestion. Do not perform that handoff, approval, or ingestion in this skill.

## Detected rules

End every output with a `## Detected rules` section. State that the proposals are advisory, not approved, and not stored. List the proposed rule text and a short reason for each candidate, or write `None`. Stop there and wait for the user's approval.
