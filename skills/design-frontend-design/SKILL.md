---
name: design-frontend-design
description: Distinctive, intentional visual design for new or reshaped user-facing UI, covering type, palette, layout and restraint so the result does not read as a templated default. Use at the implementation stage when visual choices must be made or an approved design direction must be built.
---

# Frontend Design

Use this skill when a user-facing screen or component must be built or reshaped
and its visual choices matter. Ordinary frontend work with an approved design
direction still follows that direction; this skill then adds only the
craft rules below.

Adapted for Cortex from Anthropic's `frontend-design` skill (Apache-2.0; see
`LICENSE.txt` and `CORTEX-SOURCE.md`).

## Ground the design in the subject

Identify the product, its audience and the screen's primary job before choosing
any visual treatment. If material facts are missing, use your advertised
ask_question route rather than inventing the subject, audience or primary job.
Label only non-material implementation assumptions. Distinctive choices come
from the subject's own materials, vernacular and context. A
children's reading app and a financial analysts' dashboard should not share a
visual language.

If the brief pins a visual direction, a palette, a font or an era, follow it
exactly, including when it asks for a look this skill would otherwise avoid.

## Plan, review, then build

Work in two passes.

1. **Plan.** Write a compact token plan:
   - Color: 4 to 6 named values with their roles.
   - Type: one or two families, their roles, and a clear type scale.
   - Layout: one concept, described in a sentence, with alignment guidance
     (left, center or justified) and a simple ASCII sketch if it helps compare options.
   - Principles: the two or three rules that make this screen specific.
2. **Review the plan against the brief.** If any part reads like the default
   you would produce for any similar page, revise it and say what changed and why.
   Only then write code.

When writing CSS, check selector specificity. Type selectors and class
selectors can cancel each other's padding and margin, especially between
sections.

## Typography

- Typography carries the page's character. Choose typefaces for this brief,
  not the families you reach for on every project.
- Use one family, or two that are clearly distinct. A display face and a body
  face that look alike add noise.
- Keep line length under about 80 characters. Serif body text can run slightly
  longer and needs slightly more line height than sans-serif.
- Treat a headline type treatment as an active design element, not a neutral
  container for the text.

## Avoid the common tells

These treatments mark generated pages. Avoid them unless the brief asks for them:

- Accenting one word of a headline with italic, bold or a second color.
- All-caps labels, especially tracked-out eyebrow labels above every heading.
- Typographic labels added above content with no informational role.
- Middle-dot meta strings ("A · B · C") and "WORD — fragment" labels.
- Numbered markers (01, 02, 03) unless the content is truly a sequence.
- A single warm cream background with a terracotta accent, or a near-black
  background with one acid accent, used by default.
- Identical rounded cards with one radius and one soft shadow for everything,
  and gradient washes used as decoration.
- A "big number, small label, gradient accent" stat block used by default.
- A monospace face for small data labels, or a trailing arrow on every link.

Structural devices such as rules, borders, dividers and labels should encode
information. If removing one changes nothing the reader needs, remove it.

## Motion, copy and restraint

- Use one orchestrated motion moment (for example one page-load sequence or one
  reveal) rather than scattered fades on every section and hover effects on
  every card. Motion that answers a user action and shows what changed is welcome.
- Write copy as design content. Name things in the user's words, use active
  voice for actions ("Save changes", not "Submit"), keep errors specific about
  what happened and how to fix it, and make empty states an invitation to act.
- Spend boldness in one place. Let one element be the memorable thing and keep
  the rest disciplined. Cut any decoration that does not serve the brief.

## Quality floor

Build to this floor without announcing it: responsive from phone width to
desktop, visible keyboard focus, reduced motion respected, text and control
contrast that meets WCAG AA, and a harmonious palette with semantic color tokens.

## Rendered verification

Verify the rendered result, not only the source:

- Capture desktop and mobile widths when a capture tool is available in this
  session (a browser tool if one is listed, or the project's own test tooling).
- Open each captured image and note concrete defects, with the region and the
  width where each appears.
- If no capture tool is available, say so in the report. Rendered claims then
  stay unverified, and the work goes to review with that limitation stated.

Hand the screenshot paths and the list of open visual risks to the review
stage (see `design-visual-qa`).

## No self-scoring as evidence

Do not rate your own design (for example "9/10") and treat the rating as proof
of quality. Quality claims rest on opened screenshots, concrete findings, and
re-checks after changes. A number without those is not evidence.

## Output

Report the token plan, the revisions made after the review pass, the files
changed, the screenshots opened (paths and widths), the open visual risks, and
what was not verified.
