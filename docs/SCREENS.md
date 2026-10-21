# Screen contract: coordinator worker card and worker tab

## Coordinator conversation: worker overview card
**Purpose:** Let an owner scan worker identity, current state, usage snapshot and the path to details without letting long terminal output dominate the conversation.

**Required content and order**
1. Worker marker and role; one status icon plus full status label.
2. Task title, with up to two lines at standard size.
3. Actual engine · model route, wrapping as needed; missing route reads “Route unavailable”.
4. Measured usage, explicitly labeled, and visible **Open worker ›** action.
5. At most one short category/action prompt for an attention state.

Show only the exact issue count when complete data is already available. Usage must communicate partial/unknown coverage. Do not show summary text, check rows/names, evidence-gap explanations, blockers copied from individual checks, receipts, commands, transcript, full failureReason or route-selection provenance. No inline expansion or hover popover may reveal those details.

**Navigation and states**
- Activating the single card action, by click/tap, Enter or Space, selects the exact assignment and opens the existing Workers workspace tab.
- If the assignment has no bound attempt yet, show its identity and queued/loading state; never substitute another worker.
- Map only authoritative assignment state. Use “Needs input” only for a proven user-input wait; otherwise use “Waiting”. Unknown status remains “Status unavailable”; a failed read does not mean failed worker.
- Terminal state wins over obsolete active/wait markers for that identity. Completed-with-issues, cancelled and interrupted remain distinct.
- Disabled card says “Worker unavailable” and exposes disabled semantics while remaining readable.
- Hover/pressed use surface2 without movement; keyboard focus has a visible outline. No pulse or loop.
- Missing context/tokens are not zero: show “Usage unavailable” only if neither is known. Context occupancy is labeled “Context”, never task progress or provider-plan usage.

**Layout and accessibility**
Use a uniform neutral 1-unit border and radius 8, no colored left stripe or status-colored fill. Respect host theme. Wrap and stack groups at narrow widths; keep full status/action labels visible and avoid horizontal scrolling. Keep one card action (no nested buttons). Accessible name includes full retained task title, role, status and “Open worker”. Keep visual, reading and focus order aligned, preserve user text scaling, and do not move focus during refresh. Meet measured 4.5:1 text and 3:1 meaningful icon/focus contrast in both themes.

## Workers workspace tab: selected worker details
**Purpose:** Provide the complete available result and retained activity for the exact selected assignment and attempt.

**Content order**
1. Full retained assignment title.
2. Role, actual engine/model route and authoritative status.
3. Honest usage snapshot and explicit attempt selector.
4. Terminal Result section, when a result exists.
5. Checks section, when a result exists.
6. Retained Activity/transcript with existing paging.

**States and behavior**
- No recorded assignments: “No worker assignments recorded yet.”
- Initial load: stable selected-assignment identity and loading message.
- Read failure/unavailable permission: safe reason and retry-read action; do not show false empty success.
- Failed refresh with retained content: keep history and mark it stale/read-failed.
- Active attempt: show incremental activity; omit terminal sections until a result exists.
- Result/check data is bound to the selected attempt. Do not attach a latest assignment summary to an older attempt or parse a truncated failureReason as checks.
- Distinguish every check outcome, resolved-history marker, weak-evidence warning and blocker. A known empty list says “No checks reported”; absent check data says “Check details unavailable”.
- Preserve cursor paging and “Load more history”; append without losing expansion/scroll. Do not auto-jump while the reader inspects older entries. Ignore out-of-order responses for a no-longer-selected attempt.
- State any clipping/truncation and keep retained report/activity paths available for details. A bounded UI projection is not the full underlying evidence.
- Questions and access decisions remain in the existing host question channel. No new permission, grant, retry-worker or resume action.

## Responsive and interaction acceptance
Use wrapping/shrinkable layouts; compact-density signal does not equal measured viewport width. Verify narrow and wide sizes with long titles/routes/check text, normal and enlarged text, light/dark themes, keyboard focus, touch target and reduced motion. Worker tab prose/check rows wrap; existing technical command/diff horizontal scrolling remains local to those surfaces. Hide optional open-engine-chat navigation if host navigation is unavailable.

## Evidence required before visual approval
Capture and inspect actual rendered states bound to the implementation revision, route/state and viewport; cover ordinary, zero-check and long-result cases, queued-to-attempt transition, attention/terminal states, partial usage and failed history read. Confirm the coordinator has no narrative/check detail in either rendering or accessibility tree and that the same available evidence is reachable in the correct tab/attempt. Independently check keyboard and assistive-technology behavior and measure contrast. Source inspection, tests, DOM presence, model self-score or unopened screenshots do not establish rendered quality. Report missing host capabilities and unavailable/unrun checks rather than treating them as passes.

This screen contract is a target, not evidence that the redesign has shipped or passed review.
