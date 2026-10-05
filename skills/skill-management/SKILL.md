---
name: skill-management
description: Activate and scope explicitly requested skill work, or propose a skill from repeated retained cases.
---

# Skill management

## Activation and ownership

Apply this workflow only to a user request delivered through the exact /skill-management <request> command or to a skill proposal the user approved through the runtime question (see Recurring opportunities). Do not infer authorization from ordinary prose, a recurring task, existing skill files, or a worker's recommendation. If the user invokes the command without a request, ask them to provide one.

The coordinator reads this skill, interprets the request, and proposes a bounded plan through Cortex's advertised planning tools. The runtime owns dispatch, scopes, permissions, user confirmations, independent review, verification, and completion. A skill instruction grants no tool authority; follow the current advertised schemas and use an existing worker role that fits the content.

Keep user intent separate from runtime authority. `/skill-management` authorizes
only the requested skill work, not additional paths or outside-project effects.

## Bounded assignment

Choose an existing writing role suited to the skill's content. Assign the requested
create, revise or import work and require `skill-authoring` before drafting or
adopting files. Workers own inspection, provenance, resource validation, edits,
tests and the guarded Claude link; the coordinator assesses their retained evidence.
Require exact changed paths, import identity/license where applicable, actual
checks and unresolved link or engine-loading evidence. Do not claim engine use
without observation.

## Recurring opportunities

When retained reports show the same procedure repeated in separate cases, you may propose one reusable project skill. Do not create it automatically, and never propose one merely because skill files or directories exist in the project. Propose only while planning or reconciling, at a point where the question does not interrupt running workers (for example after verified work, instead of synthesis), and at most once per task.

1. In your message, explain the repeated cases, what the skill would do, and its benefit, in the language selected by the runtime's per-turn directive.
2. Use the advertised proposal/question operation with retained case evidence.
   Follow its live schema and approval semantics; do not invent approval labels.
3. The runtime validates the evidence and current skill state, presents its
   evidence list and records the user's decision. An arbitrary first option never
   approves. A refusal or unclear answer grants no authority.
4. After approval, the next planning turn carries the /skill-management request the approval stands for. Follow this workflow exactly as for the explicit command.
5. After a decline or an unclear answer, leave the project unchanged for that skill and do not propose it again. The user can still run /skill-management <request>.

If the runtime refuses a proposal, do not work around it by asking in prose or planning the skill anyway.
