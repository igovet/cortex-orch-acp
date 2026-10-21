# Product brief: worker overview and detail

## Outcome and audience
Help a project owner monitoring several workers in the coordinator conversation answer “what is this worker doing, and does it need me?” at a glance, then inspect complete available result evidence in that worker's tab.

## Scope
This is a focused design contract for worker cards in the coordinator timeline and the existing Workers workspace tab. It is not a redesign of the whole application or a new product-wide design system. The intended hierarchy is a compact, quiet overview card with task, role, actual engine/model route, one authoritative status, honest usage; the whole card opens the worker, without a separate visible **Open worker** label. Detailed terminal narrative and checks belong in the selected worker tab, not inline in the coordinator stream.

The card is one action that opens the existing worker tab and selects the exact assignment. It must not add an inline accordion, hover detail, duplicate tab system, or new permission/retry controls.

## Platforms and constraints
Use existing Paseo extension points, host theme and Cortex's existing Workers workspace. Do not depend on whole-app CSS, host virtualizer behavior, active-tab lifecycle callbacks, guessed host APIs, or styling host chat chrome. Current installed host/SDK observations found the extension operations used by the design, but static presence does not prove every contract behaves identically at runtime. Verify actual behavior in the supported installed host before claiming it.

Status, result and usage are only what retained runtime/app data supports. Never infer completion, needs-input, progress, zero usage, billing or recovery from missing data, elapsed time, or a transport error. Preserve exact attempt association and distinguish unknown or partial evidence.

## Success criteria
- Coordinator cards are compact and scannable regardless of result/check count; no summary paragraph, check row, evidence-gap explanation, command, receipt, transcript or detailed failure reason is rendered there.
- The selected worker tab retains all available result/check distinctions and history, with explicit unknown, partial, stale and unavailable states.
- Workers remain distinguishable without a colored left stripe, status-colored card fill or a highlighted card border in any state.
- The selected worker tab shows the assignment prompt the runtime sent to that exact attempt, says truthfully when sending was not recorded or the text is unavailable, and shows each result paragraph once.
- The card and tab are usable at narrow and wide layouts, with keyboard/touch access, readable status labels and measured contrast.
- Evidence binds to the implementation revision and actual host/rendered state. Visual, keyboard, accessibility and data-integrity claims are not inferred from source inspection or builds alone.

## Related product references
This focused contract complements the [project overview](project/index.md), [feature index](features/index.md), [UI/UX design workflow](features/design-workflow/index.md), [observability](features/observability/index.md), and [roles and delegation](features/roles-and-delegation/index.md). Those remain canonical for broader product behavior.
