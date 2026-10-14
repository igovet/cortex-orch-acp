---
name: design-impeccable
description: Critique, audit and refine an already-rendered product interface. Covers heuristic critique and technical audit (review, read-only) and targeted polish, bolder, quieter, distill, typeset, layout and clarify passes (refinement). Use when a visible interface exists and must be reviewed or improved within a named scope.
---

# Impeccable (Cortex adaptation)

Use this skill for a surface that already renders. It turns a visible
interface into a sharper one through review and then targeted refinement. For a
new visual direction, use `design-frontend-design` or `design-ui-ux-pro-max`.

Adapted for Cortex from the Apache-2.0 `impeccable` skill (see `LICENSE`,
`NOTICE.md` and `CORTEX-SOURCE.md`). Its launcher, live browser mode,
telemetry, persistence of critique snapshots and generated platform variants
are not included.

## Package references

Read the packaged references through Cortex read_skill: design-critique for
references/critique.md and design-refinement for references/refinement.md.
They are not files in the target project. Read only the relevant reference.

## Role split

- **Review (read-only roles, such as `ux_designer` or `accessibility_auditor`):**
  `critique` and `audit`. Produce findings and scores. Do not edit files.
  Follow `references/critique.md` for `critique`; `audit` reports findings by
  severity, with evidence, for the categories named in the table.
- **Refinement (write roles, such as `frontend_dev`):** `polish`, `bolder`,
  `quieter`, `distill`, `typeset`, `layout`, `clarify`. Apply the smallest
  change inside the named target. Follow `references/refinement.md`.

A read-only role that finds a defect reports it with the evidence. A write role
applies the fix and re-checks the same state.

## Commands

| Command | Role | Use when | Output |
|---|---|---|---|
| `critique` | review | You need a design-director read: specificity, hierarchy, cognitive load, heuristics, personas. | Findings with P0 to P3 severity and suggested command. |
| `audit` | review | You need technical quality: accessibility, performance, theming, responsive behavior, implementation integrity. | Findings by severity with the evidence for each. |
| `polish` | refinement | A feature is functionally complete and needs a final consistency pass. | Fixes for states, alignment, tokens, copy and cleanup. |
| `bolder` | refinement | One part of a surface reads flat next to its neighbors. | One section raised to the expressive level the system already uses. |
| `quieter` | refinement | A surface is too loud, saturated or busy. | Restrained palette, weights, decoration and motion. |
| `distill` | refinement | The surface has more elements, copy or variants than its job needs. | A simpler surface with the same functions. |
| `typeset` | refinement | Type hierarchy, scale, measure or pairing is weak. | A type scale and roles applied consistently. |
| `layout` | refinement | Spacing rhythm, grid alignment or visual hierarchy is off. | Spacing scale and grouping applied consistently. |
| `clarify` | refinement | Labels, errors, empty states or instructions are unclear. | Plain-language copy that says what happened and what to do next. |

## Rules

- **Scope is sovereign.** Touch only the named target. Do not restyle neighbors,
  add colors, fonts or primitives the surface does not already use, or migrate
  the page to a new idea.
- **The brief wins.** A pinned aesthetic, era, palette or font is followed even
  when a general rule would prefer otherwise.
- **Refinement preserves; redesign replaces.** Refinement keeps the current
  identity, behavior and copy. Replacing the visual world is a separate
  design-direction task.
- **Keep factual copy.** Ask before changing claims, numbers or product facts.
- **Automated results are candidates.** A detector or script finding is
  evidence of a possible defect, not proof of quality, and a clean scan does
  not replace visual judgment.
- **Verify on the rendered result.** Check desktop and mobile widths before and
  after the change, and open the captured images (see `design-visual-qa`).
- **No self-scoring as evidence.** Scores in a critique are judgments that must
  cite the observed element. They do not certify quality by themselves.

## Refinement procedure

1. Establish the system: project design documentation, tokens, shared components.
   Classify each drift as a missing token, a one-off implementation, a conceptual
   mismatch or a local defect. Fix it at the narrowest correct level.
2. Gather evidence: render the target at desktop and mobile widths and exercise
   its loading, empty, error, success and disabled states.
3. Triage: broken or blocked tasks and misleading state first, then missing
   states, then flow, hierarchy and responsive drift, then visual and motion
   inconsistencies, then cleanup.
4. Apply the change, remove dead code and temporary artifacts, and keep the diff
   limited to the target.
5. Re-check the same states and viewports, open the screenshots, and report
   what changed and what was not verified.

## Output

Name the command, the target, the evidence (paths of opened screenshots and the
states exercised), the findings or changes with their severity, the files
changed, and the checks not run with their reasons.
