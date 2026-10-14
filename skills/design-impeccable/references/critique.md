# Critique reference (review, read-only)

Cortex adaptation of the critique and audit method in the Impeccable skill.
Use it for a design-director read of a rendered surface. Do not edit files in
this role. Cite the observed element for every finding.

## Order of work

1. **Specificity first.** Before reading any automated output, judge whether the
   interface is authored for this product or could be swapped into an unrelated
   product unchanged. Record the verdict.
2. **Heuristics.** Score the ten heuristics below from 0 to 4. Mark a heuristic
   `n/a` with a one-line reason when it does not apply to the surface. Heuristics
   7 (flexibility) and 10 (help) often do not apply to landing pages or
   portfolios.
3. **Cognitive load.** Run the checklist below.
4. **Personas.** Pick two or three personas that match the surface. Walk through
   the primary action as each and write what breaks for them, naming elements.
5. **Automated evidence.** If a detector or audit script is available, run it
   after the design review is recorded, and treat its results as candidates.
6. **Synthesis.** Combine the review and the evidence. Say where they agree, where
   automation caught something the review missed, and where a finding is a false
   positive.

## Design Health Score

| # | Heuristic | What to check |
|---|---|---|
| 1 | Visibility of system status | Feedback for loading, saving, progress, current location, inline validation |
| 2 | Match with the real world | Plain terms, logical order, recognizable metaphors |
| 3 | User control and freedom | Undo, cancel, back, clear filters, escape from flows |
| 4 | Consistency and standards | Same action, same result, same terms, same components |
| 5 | Error prevention | Constraints, confirmation for destructive actions, smart defaults |
| 6 | Recognition over recall | Visible options, labeled icons, recent items, contextual help |
| 7 | Flexibility and efficiency | Shortcuts, bulk actions, power paths that do not burden novices |
| 8 | Aesthetic and minimalist design | Only necessary information, clear hierarchy, no decorative noise |
| 9 | Error recovery | Plain error text, the specific problem, a fix, preserved user work |
| 10 | Help and documentation | Searchable or contextual help, task-focused, concise |

Scoring per heuristic: 0 means absent or broken, 1 mostly fails, 2 partial,
3 mostly good with minor gaps, 4 excellent throughout. Most real interfaces
score between 20 and 32 out of 40. The maximum is 4 times the number of scored
heuristics, so two `n/a` marks give a maximum of 32. Report the total with its
applicable maximum and state which heuristics were `n/a`.

Rating band by percentage of the applicable maximum: 90% or more excellent,
70% or more good, 50% or more acceptable, 30% or more poor, below that critical.

## Cognitive load checklist

Count the failed items. Zero or one failure is low load, two or three is
moderate, four or more is high and needs a critical fix.

- Single focus: the primary task is not competing with other elements.
- Chunking: information groups hold about four items or fewer.
- Grouping: related items are visibly grouped by proximity or shared surface.
- Visual hierarchy: the most important element is clear at a glance.
- One thing at a time: one decision before the next.
- Minimal choices: about four visible options or fewer at each decision point.
- Working memory: no need to remember a previous screen to act on this one.
- Progressive disclosure: complexity appears when it is needed.

Common violations: a wall of options with no grouping, a memory bridge between
steps, hidden navigation with no current-location cue, a jargon barrier, a
visual noise floor where everything has equal weight, inconsistent patterns for
similar actions, several parallel tasks, and a context switch needed to make
one decision.

## Priority issue severity

- **P0 blocking:** prevents completing the task. Fix immediately.
- **P1 major:** causes significant difficulty or confusion. Fix before release.
- **P2 minor:** annoying, with a workaround. Fix in the next pass.
- **P3 polish:** nice to fix, little user impact.

If unsure between two levels, ask whether a user would contact support about it.
If yes, it is at least P1.

## Personas

- **Alex, impatient power user:** wants efficiency. Red flags: forced tutorials,
  no keyboard path to primary actions, slow animations that cannot be skipped,
  one-at-a-time flows where batch actions are natural, redundant confirmations.
- **Jordan, first-time user:** needs guidance at each step. Red flags: icon-only
  navigation, jargon in errors, no visible help, unclear next step after an
  action, no confirmation that an action worked.
- **Sam, assistive-technology user:** uses keyboard or a screen reader. Red flags:
  click-only controls, missing focus indicators, meaning carried by color alone,
  unlabeled fields and buttons, time limits without extension, custom components
  that break the reading order.
- **Riley, stress tester:** probes edges. Red flags: silent failures, errors that
  leave the UI broken, empty states with no guidance, data lost on refresh or
  navigation, inconsistent behavior between similar interactions.
- **Casey, distracted mobile user:** one-handed, interrupted. Red flags: primary
  actions out of thumb reach, progress lost when the user leaves, large text
  entry where selection would work, heavy assets on every screen, targets under
  about 44 points or too close together.

Pick personas from the surface type: landing and marketing pages favor Jordan,
Riley and Casey; dashboards and data tools favor Alex and Sam; checkout favors
Casey, Riley and Jordan; onboarding favors Jordan and Casey; forms favor Jordan,
Sam and Casey. Create a project-specific persona only from real audience
information in the project documentation, never from assumptions.

## Report format

1. Method line: the assessments run and their order, plus any limitation.
2. Design Health table with the applicable total.
3. Design specificity verdict.
4. Overall impression: what works, what does not, the biggest opportunity.
5. Priority issues (three to five), each with severity, what, why it matters,
   concrete fix and the refinement command that addresses it.
6. Persona red flags, naming the failing element.
7. Minor observations.
8. Two to four questions that would change the direction, each with concrete
   options tied to the findings.

Be direct and specific. Name the element, say what is wrong and why it matters,
and give a concrete fix. Do not soften findings to be polite.
