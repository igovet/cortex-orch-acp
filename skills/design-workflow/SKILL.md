---
name: design-workflow
description: Coordinate adaptive UI/UX intake, art direction, implementation and evidence-backed visual review using existing Cortex roles, without a separate lifecycle or design store.
---

# Design workflow

## Activation

Use for new UI/UX design, a visual redesign, a design system or significant visual
improvement. Do not activate for backend work, nonvisual bug fixes, routine frontend
coding or purely functional UI changes without design decisions. A routine change
normally needs one frontend-dev, not a mandatory design team.

This skill informs the plan; the runtime still owns scheduling, permissions,
independent code review, verification and completion. Do not create agents,
parallel orchestration or a new store.

## Intake and durable context

Build on requirements-intake when material inputs are missing. First read the
target project's instructions and docs/PRODUCT.md, docs/DESIGN.md and docs/SCREENS.md
when present, plus established tokens and components. Respect equivalent existing
documents; do not duplicate or replace an approved design system blindly.

Ask only unanswered material design questions: product goal, audience, platforms,
screens/content/states, visual direction, references and what they demonstrate,
avoid list, constraints and observable success criteria. Use grouped structured
questions, meaningful options and free text, at most two rounds. Missing files
alone do not justify questions if the request and code already answer them.
See the [design intake bank](references/design-intake.md).

When documents are missing or insufficient, plan a technical-writer assignment
to record agreed answers into PRODUCT, DESIGN and SCREENS before design work,
preserving existing facts and clearly identifying unresolved choices. Never invent
functionality. Templates: [PRODUCT](references/PRODUCT.md),
[DESIGN](references/DESIGN.md), [SCREENS](references/SCREENS.md).

Read linked package references through Cortex read_skill using design-intake,
PRODUCT, DESIGN, SCREENS or visual-review, not target-project file paths.

## Adaptive phases and ownership

Scale the phases to design risk, not to a fixed five-worker ritual. Reuse approved
direction; omit redundant research or documentation work when already sufficient.
For substantial visual change, make evidence and independent visual/accessibility
review explicit in objectives and dependencies.

1. **Art direction — ux-designer, read-only.** Research available references;
   return audience-specific principles, flows, type/color/spacing tokens, component
   states, responsive rules and rationale in the report. Do not write DESIGN.md.
   Prefer originality and usability over generic card grids or decorative trends. Apply the hard bans in the [DESIGN](references/DESIGN.md) reference's "Banned AI-tell patterns" section.
2. **Persist — technical-writer or assigned writing role.** Consume the design
   report and record the chosen direction in docs/DESIGN.md, keeping PRODUCT and
   SCREENS aligned. Dependent implementation consumes that exact approved spec;
   do not make developers silently invent a different direction.
3. **Implement and capture — frontend-dev or mobile-dev.** One active writer per
   screen/shared UI files. Follow the approved direction and platform conventions.
   Capture the actual render at mobile, tablet and desktop sizes relevant to the
   product; native mobile uses supported device classes, with desktop explicitly
   not applicable where justified. Record exact dimensions and platform.
   Save screenshots in a project-local git-ignored evidence path outside private
   storage (for example .design-evidence/ after verifying its ignore rule).
   A qa-engineer writing assignment can capture evidence if needed, not restyle UI.
4. **Independent visual review — ux-designer plus accessibility-auditor.** Use a
   fresh review assignment, not the implementation author's self-review. Compare
   opened screenshots with DESIGN and SCREENS; report concrete defects, user impact,
   severity, state, viewport and remediation. Accessibility review distinguishes
   screenshots from exercised keyboard/focus/assistive-technology evidence.
   Read-only roles consume writer-captured images; they do not create files.
   This visual review supplements, never replaces runtime mandatory code review.
5. **Refine — the same implementation owner.** Return actionable findings to
   that owner, then recapture/review affected states and viewports. Plan at most
   two refinement rounds; if material defects remain, report them and request a
   scope decision or replan rather than endlessly spawning new owners.

## Stage-to-skill map

Use only the session-listed skill for the relevant stage, read on demand.

| Stage | Skill | Owners |
| --- | --- | --- |
| Reference research | design-reference-analysis | ux-designer |
| Design system | design-ui-ux-pro-max | ux-designer, mobile-dev |
| Implementation | design-frontend-design | frontend-dev, mobile-dev |
| Critique / refinement | design-impeccable | ux-designer, accessibility-auditor (review); frontend-dev (edits) |
| Capture / rendered review | design-visual-qa | frontend-dev, mobile-dev, qa-engineer (capture); ux-designer, accessibility-auditor (review) |

Project-local .agents/skills extras take precedence when session-listed and
applicable, subject to project instructions, authorization and the agreed brief.
Do not load every skill for every worker. Skill opinions and reference tables
never override the product's approved direction. Derive principles from references,
not copied markup, brand assets or one-to-one layouts.

## Browser and evidence boundary

Use Playwright MCP only if its tools are actually advertised and usable in the
current session. Configuration is not availability. Check current tool discovery;
do not assume a browser based on a previous session. Read interactive-resources
before using a browser/server/device resource and preserve its single owner.
If unavailable, use the project's supported screenshot tooling or user-provided
captures. If neither exists, name the missing capability and mark rendered claims
unverified; do not infer appearance from code or install tools without authority.

The bundled skill does not install browsers, repair host registries, or promise
that a particular MCP server exists.

## Visual quality gate

Apply the [visual review checklist](references/visual-review.md). Evidence must
bind to the implemented revision, screen, state and viewport. Require opened
mobile/tablet/desktop captures (or justified platform exclusions), typography and
hierarchy, composition/spacing, responsive reflow, required content/states,
usable interactions and accessibility checks against DESIGN/SCREENS.

No model self-score, passing build or unopened screenshot proves visual quality.
Retain concrete findings and same-state rechecks. Missing evidence, hard failures
and unresolved material findings stay explicit blockers, not diagnostic passes.
The coordinator synthesizes these facts; only the runtime can accept completion.
