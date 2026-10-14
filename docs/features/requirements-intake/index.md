# Requirements intake

Source: `skills/requirements-intake/` (`SKILL.md`, `references/question-banks.md`,
`references/agreed-spec.md`), the coordinator routing in `skills/orchestrator/SKILL.md`,
and the coordinator-only registration in `src/cortex_runtime/portable/roles.py`.

Requirements intake is situational coordinator guidance. It is not a runtime gate:
the runtime does not check whether a sufficiency review happened, how many questions
were asked, or what the answers were. The question tool, its schema and the question
lifecycle are described in [user input, questions and cancellation](../steering-and-questions/index.md).

## When it applies

Before planning, the coordinator checks any new request whose goal lacks information
that would materially change the result. This covers every request kind: feature,
bug, refactor, UI/UX, documentation, infrastructure and investigation. The orchestrator
skill routes here; substantial visual work also reads the
[design workflow](../design-workflow/index.md).

## The sufficiency check

The coordinator reads, in order:

1. The request, retained answers, project instructions and relevant product documents.
2. Bounded code-index orientation for existing behavior and constraints. If locating a
   fact needs substantial investigation, the coordinator proposes a bounded discovery
   item instead of asking the user to reconstruct the repository.
3. Then it separates established facts, decisions only the user can make, and safe
   implementation details that a specialist can decide.

It checks seven dimensions: goal, affected users, scope and non-goals, observable
acceptance, constraints, risks and verification. A gap matters only when different
answers would change scope, behavior, risk or acceptance.

**Questions are not asked when:**

- the request or project evidence already answers the dimension;
- a retained answer already answers it;
- the gap is a non-material implementation detail; the specialist decides it and the
  plan labels the choice as an assumption;
- the user explicitly delegated the choice. That delegation is a decision, and the
  resulting proposal is labeled as a specialist proposal rather than as user-specified
  functionality.

**Questions are asked when** a material decision remains open after the evidence has
been checked.

## How a question round runs

- Only the relevant prompts from the question bank for the request kind are used, and
  only for unanswered material points. The banks are prompts, not an interview script.
- Related questions are grouped into one request through the coordinator's advertised
  question tool, read from its live schema. A prose-only question or a native engine
  question tool is not used.
- Options are meaningful, distinct and grounded in the project. Tradeoffs are explained,
  custom free text is preserved, and no invented requirement is offered as a default.
- The question text and options use the runtime-selected user question language.
- Bounds: normally one round, at most two rounds, and at most five material questions per
  round (fewer if the live schema requires it). The coordinator ends its turn right after
  asking.
- On resumption, real answers are incorporated and only the remaining gaps are asked.
- A critical decision still open after the bound is not guessed and not implemented.
  The coordinator either offers a discovery-only scope with no product commitment, or
  reports the missing decision and stops the implementation proposal.

## Question banks

The bank for each request kind lists prompts for the seven dimensions above, and only the
unresolved ones are used. The full text is in
[`question-banks.md`](../../../skills/requirements-intake/references/question-banks.md).

| Request kind | Emphasis of its prompts |
| --- | --- |
| Feature / product | Outcome and users, included flows, concrete acceptance and failure behavior, fixed APIs and data, privacy or migration risks |
| Bug / incident | Expected versus observed behavior, affected environment, whether to fix, mitigate or diagnose only, shareable reproduction data, production-impact risk |
| Refactor | Maintainability outcome, consumers of the boundary, invariants that must hold, compatibility and performance limits |
| UI / UX design | Primary user job, audience including accessibility and localization, screens and states, platforms, brand and avoid list; detailed direction choices use the design intake bank |
| Documentation | Reader and prior knowledge, documents in scope, authoritative sources, examples and commands that must be checked |
| Infra / ops | Exact target, who can authorize the action, isolated versus production scope, rollback and health proof. Authorization is never inferred from a requested outcome |
| Investigation | The decision the findings should enable, evidence that would distinguish explanations, access and time limits, disruptive probes to avoid |

## From answers to plan objectives and project documents

Agreed facts enter the plan's objectives. The
[agreed-specification outline](../../../skills/requirements-intake/references/agreed-spec.md)
is a completeness check: goal and user outcome, users, existing evidence and authoritative
sources, in scope, non-goals and unchanged behavior, observable acceptance including
failure cases, constraints and authorization, risks, verification expectations and any
unavailable checks, user decisions with their source references, unresolved decisions
(which block dependent implementation), and the owner of any durable documentation. It is
not a mandate to write a document for every small fix.

Durable product or design facts are recorded in the target project's documentation by an
assigned writing role before dependent work. Read-only roles return specifications in
their reports and never write files, so a writer persists them. Intake does not change
lifecycle, permissions or question contracts, and an answered intake is not runtime
acceptance.

## Worker-side questions

Workers do not run intake and cannot read the coordinator-only skill. The coordinator
registration in `roles.py` excludes it from worker and consultant channels.

When a worker finds a material gap in its delegated brief, it uses its advertised
`ask_question` route. The specialist sources for UX design and frontend work say so
explicitly and forbid inventing product requirements. The runtime parks only that worker's
work; the real answer is delivered to the same healthy worker session. See
[roles and delegation](../roles-and-delegation/index.md#instruction-delivery) for the
worker question contract.

Missing project paths or tool access use the separate `access_request` route. A user
answer cannot grant access; only the runtime's policy decision can, and the runtime also
checks declared scope and writer conflicts.

## Verification and limits

- Registration, role visibility and the presence of the intake rules are covered by
  `tests/test_portable_roles.py` (coordinator design intake is situational and not
  worker-readable) and `tests/test_prompt_drift.py` (intake and design sources keep their
  evidence and ownership rules). They check source text and delivery, not the quality of
  a model's sufficiency judgment.
- Whether the coordinator asked the right questions, or stayed within the bounds, is
  model behavior. Runtime review of the plan and its evidence remains the control.
