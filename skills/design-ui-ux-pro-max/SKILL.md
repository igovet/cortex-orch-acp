---
name: design-ui-ux-pro-max
description: Design-system foundations for product UI. Choose or check a palette, type pairing, spacing scale and UX rules from bundled reference tables, with a pre-delivery checklist. Use when a visual system must be chosen or reviewed for a product interface, not for backend or logic-only work.
---

# UI UX Pro Max (Cortex adaptation)

Use this skill to establish or check the foundations of a product interface:
palette, type pairing, spacing, component conventions and UX rules. Skip it for
pure backend logic, data design, infrastructure, or other work that does not
change how something looks, feels or is operated.

Adapted for Cortex from the MIT-licensed `ui-ux-pro-max` skill (see `LICENSE`
and `CORTEX-SOURCE.md`). The upstream search scripts are not bundled. Read the
reference files directly; they are the only data
this skill uses.

## Reference files

- `references/colors.csv`: product-type palettes. Columns include `Product Type`,
  `Primary`, `On Primary`, `Secondary`, `Accent`, `Background`, `Foreground`,
  `Card`, `Muted`, `Muted Foreground`, `Border`, `Destructive`, `Ring` and `Notes`.
- `references/typography.csv`: font pairings with heading and body families,
  mood keywords, best-for uses and notes. Google Fonts URLs and CSS snippets in
  the data are references only; this skill does not fetch them.
- `references/ux-guidelines.csv`: UX rules with `Category`, `Issue`, `Platform`,
  `Do`, `Don't`, example code and `Severity`.
- `references/pro-rules.md`: app-specific pre-delivery checklist (icons, touch
  feedback, light and dark contrast, safe areas, accessibility) for native and
  mobile app UI.

These files live in the installed Cortex package, not the target project.
Read them through Cortex read_skill using the corresponding registered id:
design-colors, design-typography, design-ux-guidelines, design-pro-rules.
Read only needed tables; do not assume the whole set is loaded in context.

## Procedure

1. **Define the product.** Take product type, audience, platform and the one
   primary user goal from the brief or `docs/PRODUCT.md` when present. If a
   material fact is missing, use your advertised ask_question route before choosing
   the system. Label only non-material implementation assumptions; never invent
   product functions.
2. **Choose a palette.** Find the closest product-type rows in `colors.csv`.
   Use them as starting points, then adapt to the brief and to any `DESIGN.md`
   direction. Keep semantic roles: text on background at 4.5:1 or better for body
   text, a visible focus ring, a clearly distinct destructive color.
3. **Choose a type pairing.** Use `typography.csv` rows whose mood and best-for
   fields match the product. Check that the chosen families are already
   available to the project or approved for it before adding any dependency.
4. **Apply UX rules.** Work through `ux-guidelines.csv` for the product's
   categories, highest severity first. Cite the rule numbers (`No`) you applied.
5. **Run the checklist.** For native or mobile app UI, apply `pro-rules.md`
   before delivery.
6. **Return the system as tokens.** List color roles with hex values, the type
   scale, the spacing scale, radius and elevation rules, and the state styles
   (default, hover, focus, active, disabled, error). Record the decisions in the
   project design documentation only through a role that is allowed to write.

## Rules

- Apply the banned AI-tell patterns (read_skill id `design-banned-patterns`): never add a one-sided accent stripe or decorative leading dot/meta-chip; only a spec-required semantic status indicator is allowed, and a dot cannot be its only signal.

- Table rows are starting points, not mandates. The brief and the project's
  design direction win.
- Contrast, focus visibility, touch target size and reduced-motion support are
  not optional.
- Do not mix flat and skeuomorphic styles in one product, and do not use emoji
  as functional icons.
- Keep one icon family with consistent stroke weight and sizing.

## No self-scoring as evidence

A system that matches the tables is not proof that the interface is good. Show
the applied rule numbers and the rendered result. Rendered review is handled by
`design-visual-qa`; quality claims rest on opened screenshots, concrete findings
and re-checks.

## Output

Report the product definition and assumptions, the palette and type choices
with the rows they came from, the rule numbers applied, the token list, and the
open decisions that need the owner.
