# Design direction: worker overview and details

## Product basis
This is a focused design for monitoring multiple workers in the coordinator timeline and inspecting them in the existing Workers tab. The coordinator surface answers status and navigation questions; terminal narratives and check evidence live in the selected worker's details. See [PRODUCT.md](PRODUCT.md), [SCREENS.md](SCREENS.md), the [design workflow](features/design-workflow/index.md), and [observability](features/observability/index.md).

## Art direction and foundations
Quiet, information-dense, host-native. Preserve the supplied theme and inherited UI sans-serif; do not introduce a palette, font, shadow, gradient, status-colored fill, or decorative branding. Existing host semantic colors take precedence over sampled screenshot colors.

| Token | Direction |
| --- | --- |
| Surface / boundary | `theme.colors.surface1` / `theme.colors.border` |
| Primary / secondary text | `foreground` / `foregroundMuted`; use foreground if measured contrast of muted text is insufficient |
| Active / warning / failure / success | `accent` / `statusWarning` / `statusDanger` / `statusSuccess` |
| Spacing | 4, 8, 12, 16 layout units |
| Card padding | 12 horizontal, 10 vertical; compact vertical 8 |
| Boundary | radius 8; uniform 1-unit neutral border on every side, including left, identical in every state |
| Elevation | none |
| Task title | 14/20, weight 600, up to two lines at normal size |
| Metadata | 12/18; status weight 600, other metadata 400 |
| Icons | existing icon family: status 12, worker marker 14 |
| Motion | executing indicator only: opacity 1 → 0.35 → 1 over 1.6 s, ease-in-out; static under reduced motion |
| Context meter | existing 32 × 4, radius 2; denotes context occupancy only |
| Existing outer spacing | preserve compact/regular margins 6/8 |

Do not assume semantic tokens pass contrast: measure rendered foreground/background combinations in light and dark themes. No external assets are needed.

## Card hierarchy and interaction
Use one whole-card action. Replace the all-caps worker banner with a small worker marker and role at the leading edge; put one status icon and full status label at the trailing edge. Follow with task title, actual engine · model (wrap when needed), then measured usage. The owner removed the visible **Open worker ›** label: the whole card is the action, and its accessible name still ends with “Open worker”. Only attention states may add one short category/action prompt, never a reason list. Do not fix card height or truncate essential status/action text.

Keep only the uniform neutral 1-unit boundary, never a colored left border. The owner rejected the colored whole-card highlight: no state (focused, selected, pressed, hovered, opened or any status) adds or recolors a card perimeter, outline, ring or thicker border, and none is to be reintroduced as a replacement. Hover/pressed may use surface2 without scaling or layout shift. Keyboard focus stays perceptible through the surface2 fill that hover uses, never around the card; the browser's own focus ring is suppressed on the card. Disabled state remains readable and explicitly says “Worker unavailable”; do not merely dim the card. The owner asked for executing workers to blink again: while the worker's authoritative status is executing, only the small worker marker and the status icon pulse (a gentle opacity cycle of about 1.6 s, never below 35%); the card, its text, fill and border never animate. Queued/starting waits, terminal, cancelled and interrupted states are static.

The card may show task title, role, engine/model, one authoritative status, exact issue count only when already supported, measured context/token snapshot with explicit coverage, and at most one brief attention category. Never render terminal summary, check names, evidence-gap text, receipts, commands, per-check blockers, full failureReason, transcript, or route-selection provenance in the coordinator card. Do not restore those bodies in an accordion or hover detail.

## Status and data integrity
Terminal state takes precedence over obsolete active/wait markers for the same identity. Render only distinctions established by the selected assignment's durable status: queued, starting, working, completed, completed with issues, failed, cancelled, interrupted, waiting/needs input only when the exact wait category is known, blocked, or status unavailable. An unresolved assignment shows queued/loading rather than silently selecting another worker. Until a user-input wait is actually established, label it “Waiting”, not “Needs input”. Transport/read failures do not imply worker failure. Never infer progress from elapsed time, zero events, or an RPC error. Keep existing status reconciliation and bridge-fence protections.

Show “Context” before a percentage. Preserve measured input/output tokens and partial-coverage warning when available. Missing values are unknown, not zero; if nothing was reported say “Usage unavailable”. These are last-reported snapshots, not guaranteed live totals or comparable billing. Do not label context fill as provider-plan usage or add refresh polling for decoration.

## Worker tab detail
Order: full assignment title; role/engine/model/status; usage and explicit attempt controls; Assignment prompt; terminal Result; Checks; retained Activity/transcript. Active workers omit terminal sections until a result exists.

The Assignment prompt section shows the task text the runtime sent to the selected attempt: the dispatch request's brief (objective as Markdown, scope, dependency report references, original goal, review evidence) and its acceptance line, never runtime system or role instructions. It starts collapsed to its state and a one-line preview, with a **Show/Hide** control. Its state is truthful per attempt: “Sent to this attempt” only when the runtime's sent record matches that exact text; otherwise queued (not yet recorded as sent), unrecorded (older history, or ended before its first prompt), withheld on mismatch, or unavailable. It is never rebuilt from current, possibly re-planned, node text.

The Result paragraph appears once: an Activity row of the attempt's own `worker_result` whose text equals the shown summary is not repeated. Other prose, the final response and differing result text stay. Bind summary and checks to the selected attempt; never attach a latest assignment result to an older attempt. Show each available check's true disposition, resolved history, weak-evidence warning and blocker. “No checks reported” means a known empty list; absent data says “Check details unavailable”. Keep failed, not-run and unverified distinct; never present any as pass.

Preserve cursor paging, selected assignment/attempt, view and scroll. Refresh must not force-scroll or overwrite a newer selection with an out-of-order response. A failed refresh retains available history and identifies it as stale. Explain bounded or truncated projections; do not imply a bounded display contains all raw evidence. Keep permission decisions in the host question channel; do not invent grant, retry or resume controls.

## Responsive and accessibility acceptance
- Use wrapping flex layouts and shrinkable text with `minWidth: 0`; `layout.compact` is a density hint, not a physical width.
- At narrow widths stack identity/status and usage/action; keep status and action labels whole. Allow additional wrapping for enlarged text. No horizontal scrolling in coordinator cards.
- One Pressable, no nested competing actions. Accessible name includes full retained title, role, status and “Open worker”; reading and focus order match visual order. Support Enter/Space, visible focus and touch; never rely on hover. No assumed host focus-tab API.
- Body, metadata and status text measure at least 4.5:1 contrast; meaningful icons/focus boundaries at least 3:1. Pair state color with text/icon.
- Announce meaningful status transitions politely once with worker identity; do not announce token/context changes continuously.
- Preserve user font scaling and host safe-area behavior. Card naturally exceeds 44pt iOS / 48dp Android touch targets; web target is at least 24 CSS px.
- The executing indicator pulse is the only looping motion. It is decorative: status stays in the icon and text, the cycle is slow (no rapid flashing), it stops when the worker settles or the indicator unmounts, and a reduced-motion preference shows the indicator static.

## Host boundary and evidence
Use only the existing registered Paseo extension surfaces and app-owned data. Static SDK/bundle symbol presence is not proof of runtime behavior. Whole-app CSS, host virtualizer control, tab lifecycle callbacks and unobserved focus APIs are not dependencies. Hide optional open-engine-chat navigation when unavailable.

A future implementation review should inspect real rendered cards and tabs, not infer acceptance from source, DOM presence, builds or screenshots alone. Bind captures and checks to the implementation revision, state and viewport; cover light/dark, long content, narrow/wide layouts, enlarged text, keyboard and assistive technology. Record unavailable capabilities and unrun evidence explicitly. This document records a design direction, not an implemented or visually approved feature.
