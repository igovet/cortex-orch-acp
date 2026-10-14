---
name: requirements-intake
description: Clarify materially incomplete requests of any kind before planning, using existing project evidence first and bounded structured questions only for remaining decisions.
---

# Requirements intake

Use before planning a new request whose goal lacks information that would
materially change the result. Apply to every request kind, not just design.
Do not interrogate the user when the request and project evidence suffice.

## Check sufficiency first

Read the request, retained answers, project instructions and relevant product
docs. Use bounded code-index orientation for existing behavior and constraints;
do not turn intake into a coordinator implementation investigation. If locating
facts needs substantial investigation, propose that bounded investigation rather
than asking the user to reconstruct the repository.

Separate established facts, decisions the user must make, and safe implementation
details delegated to a specialist. Check goal, affected users, scope and non-goals,
observable acceptance, constraints, risks, and verification. Missing information
matters only if different answers would change scope, behavior, risk or acceptance.
Never ask what docs, code or a retained answer already establish.

Read linked package references through Cortex read_skill using question-banks
or agreed-spec (also accepted in references/<id>.md form), not project_read on
the target project.

## Ask only material gaps

1. Select only relevant prompts from the [question banks](references/question-banks.md).
   For feature/product ask outcome and users; for bug/incident ask expected versus
   actual behavior and reproduction; for refactor ask invariants and permitted
   changes; for UI/UX ask audience, platforms, screens and visual direction; for
   documentation ask audience and authoritative facts; for infra/ops ask target,
   authority and rollback; for investigation ask the decision and evidence needed.
2. Group related questions into one request through the advertised coordinator
   question tool. Read its live schema; do not use a native engine question tool
   or prose-only questions. Use the runtime-selected user question language.
   Offer meaningful, distinct options grounded in the project, explain tradeoffs,
   and preserve custom free text. Do not present invented requirements as defaults.
3. Ask at most two rounds, normally one, with at most five material questions per
   round (or fewer if the live schema requires it). End the turn immediately after
   asking. On resumption, incorporate real answers and ask only remaining gaps.
4. If a critical decision is still unresolved after the bound, do not guess or
   implement it. Offer a bounded discovery-only scope with no product commitment,
   or report the concrete missing decision and stop the implementation proposal.
   A user explicitly delegating a choice is a decision; label the resulting
   specialist proposal rather than inventing functionality.

## Carry the agreement forward

Record the agreed specification in plan objectives: goal, users, scope/non-goals,
acceptance examples, constraints, risks, verification expectations and unresolved
items. Preserve exact source references for consequential decisions. Use the
[spec outline](references/agreed-spec.md) as a completeness check, not a mandate
to create a document for every small fix.

For durable product or design intake, propose a writing assignment to record the
agreed facts in the target project's documentation before dependent work. Read-only
designers return specifications in reports; writers persist them. Do not modify
lifecycle, permissions or question contracts, promise a tool capability not
observed in this session, or treat answered intake as runtime acceptance.
