---
name: design-visual-qa
description: Visual QA of a rendered interface at desktop and mobile widths. Judges composition, typography, spacing, wrapping, clipping, states and journeys from opened screenshots and exercised interactions, with an evidence-level completion gate. Use after a UI is rendered; audit-only unless a fix is explicitly requested.
---

# Visual QA

Use this skill to judge an interface that is already rendered: what a person
would actually see at the sizes they use, in the states they reach. Build
success, a passing unit test and the presence of DOM nodes are supporting
signals only.

Adapted for Cortex from the MIT-licensed `frontend-visual-qa` skill (see
`LICENSE`, `references/defect-checklist.md` and `CORTEX-SOURCE.md`). Tool-bound
steps of the upstream skill are replaced by the Cortex roles below.

Read the packaged references/defect-checklist.md through Cortex read_skill with
the registered id design-defect-checklist, and the banned AI-tell patterns with
the id design-banned-patterns, not as target-project files.

## Roles in the review

- **Capture (write role, such as `frontend_dev` or `qa_engineer`):** produces the
  screenshots and state captures inside the project, at least at desktop and a
  phone width, and reports their paths and the state each one shows.
- **Review (read-only role, such as `ux_designer` or `accessibility_auditor`):**
  opens each cited screenshot with the image-capable read tool and writes the
  findings. A read-only role does not create or edit files.

Audit-only is the default. Do not edit implementation, add tests, change
snapshots or rebuild a target unless the assignment explicitly asks for a fix.
A fix is then rechecked against the same target, state and viewport.

## Audit contract

Before judging, write down:

- the artifact and the actor, and the job the actor is trying to do;
- the target and its state (route, data, logged-in or not, theme);
- the viewports: at least desktop and a phone width;
- the journeys and states in scope (loading, empty, error, success, long content);
- the reference the design should match, if one exists (`DESIGN.md`, a brief or
  a named screenshot), and what is out of scope.

## Evidence levels

Use the weakest level that answers the question, and say which level each
finding rests on:

1. **Static observation:** an opened screenshot. It supports visual observations
   only, such as composition, clipping and wrapping that is visible.
2. **Measured observation:** a geometry or DOM measurement tied to the element
   in the screenshot. Use it to confirm a specific claim such as overflow,
   overlap or a target size.
3. **Exercised behavior:** the interaction was performed and the resulting state
   was captured. Use it for focus order, menus, forms, transitions and outputs.

A screenshot that was never opened has no evidence value. A visible control is
not proof that it works. A claim about data, copy correctness or domain facts
needs the matching subject-matter review; a rendering pass cannot certify it.

## Procedure

1. Establish the audit contract above.
2. Choose the evidence level for each question.
3. Prove the target, state and viewport: the captured file and the state it shows.
4. Capture and open the macro view (whole page or window) first, then the local
   regions with disputed details, then the responsive widths.
5. Exercise affected journeys and their outputs (downloads, dialogs, copies),
   including keyboard-only traversal.
6. Report findings, the checks that ran, and what remains unverified. Re-run the
   affected states after any authorized fix.

For each finding, record: the observable symptom, the region, the viewport, the
state, the impact on the actor, and how to reproduce it. Use the defect
catalog in `design-defect-checklist` for the usual defect families, including
banned one-sided accent stripes and decorative leading-dot/meta-chip patterns
(clipping, orphan lines, dense-item collisions, broken images, focus
obstruction, repeated-card density) and its triage table for the evidence each
one needs.

## Completion gate

Call the current render **verified** only when all of these hold:

- the target, data and actor state are established;
- every screenshot cited in the report was opened and inspected;
- the relevant viewports and the lower sections of the page were covered;
- the affected interactions and outputs were exercised at the required level;
- no unresolved blocker or major finding contradicts the pass;
- every explicit requirement in the brief or `DESIGN.md` was checked, including
  information necessity when density or layout is in scope;
- remaining limitations are stated.

Otherwise report **partial** or **blocked**. A fix is verified only after it is
rechecked against the same target, state and viewport.

If no capture tool is available in this session, the result is **blocked** for
any claim about the rendered result. Say so; do not infer the look from source.

## No self-scoring as evidence

Do not produce a score such as "9/10" as proof of visual quality. Findings,
opened screenshots and re-checks are the evidence. A finding labeled as taste
or minor does not override a requirement the user stated.
