# Design direction

## Product basis
Link PRODUCT and SCREENS; state the user goal and approved creative scope.

## Art direction
Distinctive concept, rationale, references and transferable principles (not copies).

## Foundations
Typography roles and scale; semantic palette and contrast; spacing/grid;
radius, elevation, icon and imagery rules; asset licensing and font availability.

## Components and interactions
Variants, loading/empty/error/disabled/focus states, navigation and motion;
keyboard, assistive technology and reduced-motion behavior.

## Responsive behavior
Mobile/tablet/desktop dimensions or justified platform-specific size classes;
content priorities and reflow rules.

## Avoid and preserve
Unwanted traits, existing identity/components to preserve, non-goals.

## Banned AI-tell patterns

Never use a decorative accent stripe on only one side of any card, list item, callout,
quote, alert, panel or section. This includes `border-left`, `border-inline-start`,
inset-shadow stripes, pseudo-element bars and framework utilities such as Tailwind
`border-l-4`. No exceptions in generated designs. Use a subtle border around the whole
card, a restrained background tint, or spacing and typography hierarchy instead.

Never add a decorative dot/bullet before category, tag, date, status or metadata text,
including inside pills, chips and badges. Keep category/date/meta as plain, well-set
text. Do not add decorative pill/eyebrow metadata by default. A status indicator is
allowed only when the product spec explicitly requires its real semantic meaning; a dot
must never be its only signal. Review and remove any such patterns unless explicitly
required by that status exception.

## Visual acceptance
Concrete criteria, evidence location, target states and unresolved decisions.
