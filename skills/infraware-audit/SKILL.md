---
name: infraware-audit
description: Review the visible conversation for durable rule candidates, or review stored rules in light of an engineer's stated mission.
disable-model-invocation: true
---
# Infraware audit

Use this skill for a requested chat audit or stored-rules audit.

## Before either audit

Use your normal skill-loading practice to find and read the `infraware` skill. If you cannot find or read it, stop and report `blocked`. A working Infraware CLI is not a substitute for the skill.

Follow the loaded skill's applicable instructions. Both audits are read-only. Do not change stored records or prescribe rule-management procedures.

If the request does not make the audit mode clear, ask whether the engineer wants to review the visible chat or stored rules.

## Chat audit

Review only the conversation you can see. Propose rule candidates based on the engineer's statements and decisions.

A rule candidate is a standard or constraint that can guide later work within its stated scope. A concrete action for the current work is a task, not a rule. One explicit standing instruction can be enough; do not require repetition across sessions.

For each candidate, give proposed wording, its scope, and the statement that supports it. Preserve uncertainty about intent or scope. Do not turn a temporary status or an unadopted third-party quote into the engineer's rule. The italicized candidate and slogan quotes below are hypothetical engineer speech for illustration, not observed or adopted records.


| Statement                                                                                                                                           | Treatment                        | Why                                                                           |
| --------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------- | ----------------------------------------------------------------------------- |
| *"For coding tasks, add retries, logging, validation, or other defenses only for a concrete failure and exposure; check their costs and behavior."* | Candidate, illustrative          | A standing coding rule with a clear decision point: whether to add a defense. |
| *"For coding tasks, when evidence disproves a premise, revisit the affected scope, design, or validation decision instead of patching around it."*  | Candidate, illustrative          | A standing coding rule triggered by contrary evidence.                        |
| *"Just follow best practices."*                                                                                                                     | Defer                            | It gives no specific behavior to apply.                                       |
| "Run `infraware status` now."                                                                                                                       | Not a candidate                  | An action for current work.                                                   |
| "Infraware reports everything up to date today."                                                                                                    | Not a candidate                  | A temporary observation.                                                      |
| A vendor document says "disable validation."                                                                                                        | Not a candidate without adoption | A vendor quote is not the engineer's instruction.                             |


Report `None` if you find no candidates.

## Stored-rules audit

Before listing records, ask one open question: "What do you want to accomplish by reviewing the stored rules?" Wait for the engineer's answer.

Read the stored rules through the Infraware CLI. Follow the loaded `infraware` skill's read-only inspection guidance. For a whole inventory, paginate until it is complete; do not treat one page as the full set or assume page defaults. Request full details only when the engineer asks for them.

Respond to the engineer's mission using the records and other evidence available to you. Cite rule IDs for findings about specific records. Do not impose a fixed review checklist or treat a rule as invalid merely because it falls outside this mission. If the evidence cannot answer the request, explain what is missing. Do not change records or prescribe what management action to take.