# Desktop concept C: Focus Workspace

**Status: candidate direction pending owner Gate D. Not the approved design system.** Every token, font family, colour, board phase name and copy string in this document is a candidate for owner decision. [DESIGN.md](../DESIGN.md) still states that design tokens, typography, colours and spacing are TBD until Gate D; this document does not change that statement. Gate D is recorded in [desktop approval](desktop-approval.md).

Parent: [desktop concepts](desktop-concepts.md). Shared fixtures: [prototype fixtures](prototype-fixtures.md). Product and screen references: [PRODUCT.md](../PRODUCT.md), [SCREENS.md](../SCREENS.md).

Implementation status: not built. The shared mock shell in `prototypes/cortex-workspace/desktop/` currently provides placeholders for `/c`; concept C is not complete and nothing here is runtime-qualified.

## 1. Thesis

Show only what needs the person now, explain it fully, and reveal more only on request. Home is a ranked attention queue plus one focus pane. Context is layered in four disclosure levels. The board and code are explicit deeper destinations.

Differences from the other concepts:

- Decisions are the primary object.
- Spacious layout, larger type, monochrome ink for primary actions.
- Four levels of progressive disclosure instead of tabs or panels.

This is the closest desktop analogue to the Android "Now" emphasis in [PRODUCT.md](../PRODUCT.md). Mobile is out of scope until Gate D.

## 2. Navigation model

```
Attention queue + focus pane (home)
  → L1: decision or result detail (effect, scope, rejection, actions)
    → L2: disclosure sections, collapsed by default (per task)
      → L3: full surfaces (discussion, full diff, editor / search / terminal, Git, worker detail)

All tasks: List (default) or Board, replaces queue + focus; selecting a task returns to the focus pane with the queue restored.
```

- **Queue** groups, in this fixed order (ranking is defined by the fixture and shown as section order; it is never re-sorted under the cursor):
  1. Your decision blocks an execution.
  2. Blocked.
  3. Changed since last visit.
  4. Running.
  5. Quiet tasks (collapsed by default).
- New items are appended, and a "2 new · Show" control appears at the section top. They never shift the focused row.
- **Deep mode (L3):** "Open full diff", "editor", "Git" or "discussion" expands the focus pane to full width. The queue collapses to a 56 px rail showing "3 need you" and a "Back to queue" button (`Alt+←`). This is user-initiated and the queue selection is preserved.
- Code deep mode: tree 260 px plus editor. The terminal toggles as a bottom split inside deep mode. The layout still has at most two areas, because the queue is a rail.
- **All tasks:** replaces queue and focus with List (default) or Board, using the same columns and cards as concept A at this concept's type scale. Selecting a task returns to the focus pane showing that task, with the queue restored.
- **Connection settings:** the Connection item in the top bar opens a page in the focus pane.

## 3. Layout

Window sizes: 1280×800 and 1920×1080. Heights in px.

### 3.1 1280×800

```
y0   ┌ Prototype strip 28 ─────────────────────────────────────────────────────────┐
y28  ├ Top bar 48: Project ▾ │ Attention | All tasks | Code │ Search Ctrl+K │ Connected 14:02 │ New task · Mock │ Help ┤
y76  │ Queue 400 (sunken)        │ Focus pane 880 (surface), content max 720 centred, pad 32 │
     │ Needs your decision (3)   │ L1: decision anatomy (section 8.1)                         │
     │ Blocked (1)               │ L2 disclosures (collapsed): Why this is asked · Plan ·     │
     │ Changed since last visit  │  Result & evidence · Changes (diff) · Code & Git · Workers │
     │ Running (1)               │ L3 links: "Open full discussion / full diff / editor / Git"│
     │ Quiet tasks (490) ▸       │                                                            │
y776 ├ Status bar 24: Connection · Fixture · Mock ───────────────────────────────────────────┤
y800 └────────────────────────────────────────────────────────────────────────────────────────┘
```

- Queue rows are 64–72 px: line 1 is a type badge and a one-line ask (two-line clamp); line 2 is the task title, requested time (14:02) and "observed".
- Body height is 700 px. Focus pane content is centred with a maximum width of 720 px.

### 3.2 1920×1080

- Queue 440 px; focus pane 1480 px, with content maximum 880 px.
- When "Changes (diff)" is expanded, the diff may use up to 1400 px side by side.

## 4. Candidate tokens

Ratios are WCAG 2.x relative-luminance contrast computed from the listed hex values. Text pairs must reach at least 4.5:1; non-text boundaries and focus rings at least 3:1.

### 4.1 Typography (owner font decision)

UI family Source Sans 3; monospace Source Code Pro. Fallbacks: `Ubuntu, system-ui, sans-serif` and `"Ubuntu Mono", "DejaVu Sans Mono", monospace`. Font bundling is an open owner question.

| Token | Size / line height | Weight | Use |
|---|---|---|---|
| c-text-xs | 13 / 18 | 400 | provenance, metadata |
| c-text-sm | 14 / 20 | 400/600 | queue rows, tables |
| c-text-base | 16 / 24, max 68 ch | 400 | UI default and prose |
| c-text-md | 18 / 26 | 600 | section and disclosure headings |
| c-text-lg | 22 / 28 | 600 | focus-pane task title |
| c-text-xl | 28 / 34 | 600 | decision title |
| c-mono | 14 / 22 | 400 | code, IDs, paths, terminal |

### 4.2 Colour

| Role | Hex | Measured contrast |
|---|---|---|
| canvas | #FAFAF8 | base |
| surface | #FFFFFF | base |
| sunken (queue background) | #F1F1EE | base |
| separator | #DEDED9 | decorative only |
| control border | #7E8187 | 3.91 on surface, 3.74 on canvas |
| text | #17191C | 17.61 surface, 16.85 canvas, 15.18 on selection tint |
| text muted | #4D5259 | 7.87 surface, 7.54 canvas, 6.96 sunken, 6.79 on selection tint |
| primary action fill (ink) / text | #1F2328 / #FFFFFF | text 15.80; fill against canvas 15.12 |
| link and focus (teal) | #0A6C74 | 6.16 surface, 5.89 canvas, 5.31 on selection tint |
| selection tint | #E2F1F2 | primary text 15.18 |
| danger / tint | #A8261B / #FBECEA | 7.10 surface; 6.18 on tint |
| warn / tint / border | #7F4F00 / #FAF0DA / #B07A12 | 6.95 surface; 6.14 on tint; border 3.72 on surface (non-text boundary) |
| success / tint | #2C6A1E / #EAF3E5 | 6.59 surface; 5.79 on tint |
| neutral / unknown tint | #4D5259 / #EEEFEC | 6.82 on tint |
| mock / tint | #6B3B99 / #F3EDF9 | 7.75 surface; 6.75 on tint |
| diff add / delete background | #E6F2E1 / #FAE6E3 | primary text 15.23 / 14.67 |
| disabled label | #83868C | 3.65 on surface (exempt; disabled reason text uses muted role) |

Focus against the ink fill: the teal focus ring against the ink button fill measures 2.57, below 3:1. The ring therefore sits outside the button with a 2 px canvas gap, where it measures 5.89 against canvas.

### 4.3 Spacing, radius, elevation, focus, motion

- **Spacing:** 4 · 8 · 12 · 16 · 24 · 32 · 48 · 64 (8-led rhythm; focus pane padding 32).
- **Radius:** controls 6 px; sections 8 px; dialogs 12 px. No pills.
- **Elevation:** flat. Dialogs use `0 8px 28px rgba(23,25,28,.16)`.
- **Focus:** 3 px #0A6C74 outline with 2 px offset; inset −3 px in queue rows. Selection is teal tint, a 1 px teal full border and `aria-current`.
- **Motion:** disclosure expand 160 ms (opacity plus 4 px vertical translate). Queue reorder is not animated. Reduced motion sets durations to 0.

## 5. Status encoding

Identical semantics in all three concepts; only colour values differ. Every state is a text label, a glyph and a colour. Colour is never the only signal. Glyphs are 16 px, decorative (`aria-hidden`) and always paired with visible text. The glyph family is an open owner question (candidate: Lucide, ISC licence).

| Dimension | Visible text | Glyph | Colour role |
|---|---|---|---|
| Check outcome | Passed · Failed · Not run · Unverified | check-in-circle · x-in-circle · hollow circle with dash · question-in-circle | success · danger · neutral · neutral |
| Execution state | Not started · Running · Waiting · Idle · Failed · Cancelled · Unknown | half-circle (Running is static, no animation) · pause · minus · x · slash · question | info · warn (Waiting) · neutral · danger · neutral · neutral |
| Wait reason (separate field) | Your answer needed · Your permission needed · Result review needed · Review (runtime) · Verification (runtime) | message-question · key · file-check · eye · clipboard-check | warn for the three user-blocking reasons; neutral for runtime-internal |
| Board phase (projection) | Draft · Discussion · Planned · In execution · Result review · Accepted · Closed (candidate names) | none; text only | neutral |
| Connection | Connected · Reconnecting · Disconnected | plug · plug-zap · plug-off | neutral · warn · danger |
| Freshness | Current · Stale (result changed) · As of HH:MM (cached) | none · history · clock | neutral · warn · neutral |
| Provenance (fixed slot, muted text) | observed · reported · estimated · unknown | none; unknown also gets "?" | muted text |
| Capability | Available · Unavailable (reason) | none · circle-slash | neutral |
| Mock | Mock · Fixture | flask | mock (violet family) |

Rules:

- A status badge is static markup, single line, with a shrinkable label. Truncated values open their full value on focus or click.
- Provenance is muted text after a middle dot, for example `Remaining: unknown`. "Estimated" values are prefixed "est." and never drive layout emphasis.
- An agent-reported claim such as "tests pass" is shown as conversation text with "reported". It is never rendered as a Passed check unless an observed check record exists in the fixture.
- No percentage bars and no ETAs. Progress is shown as observed facts, for example "3 of 5 plan steps reported done".

## 6. Keyboard map and focus order

**Focus order:** skip link → top bar → queue (roving, one Tab stop; arrows and J/K inside) → focus pane (decision heading → actions → L2 headings in order → L3 links) → status bar. F6 cycles the same order.

| Keys | Scope | Action |
|---|---|---|
| `J` / `K` or arrows | queue | next / previous item. Selection follows focus only on Enter, so browsing never changes the pane |
| `Enter` | queue row | show in focus pane; focus moves to the decision heading |
| `[` / `]` | focus pane | previous / next L2 section heading |
| `Alt+1`…`Alt+6` | focus pane | toggle L2 section n and focus it |
| `Ctrl+Shift+E` | focus pane | expand all sections |
| `G A` / `G T` / `G C` | global, can be disabled | Attention / All tasks / Code |
| `Ctrl+N` / `Ctrl+K` | global | new task / command palette |
| `Alt+←` | global | back: from deep mode to queue, or from All tasks to the previous view |
| `Esc` | focus pane | return focus to the queue row |
| `?` / `F1` | global | keyboard help |

Decision actions are reached with Tab only; there are no single-key decision shortcuts.

## 7. Component inventory and states

| Component | Anatomy | State notes |
|---|---|---|
| Queue section | Heading with count; rows | Empty "Needs your decision": the section stays with "Nothing needs your decision · last checked 14:02 · observed". Loading: four skeleton rows. Disconnected: section headings "as of 14:02"; rows keep their state, but Running shows "Unknown". |
| Queue row | Section 3.1 | Stale decision: badge "Stale" and copy "Result changed; review again". Accept is disabled in L1 (stale decisions never show a real acceptance). Answered elsewhere: moves to "Changed since last visit" with "Answered on Phone 14:05 (mock)". |
| Decision block (L1) | Section 8.1; decision title in c-text-xl | Blocking emphasis: full 1 px warn border (#B07A12) with a warn-tint header band; no side stripe |
| Disclosure section (L2) | Heading button with one-line summary when collapsed (for example "Result r-0142.3: 1 Failed · 1 Not run · 410 Passed · observed") | Unavailable capability: summary "Unavailable: agent did not negotiate plan updates"; still expandable with the explanation. Error: summary "Could not load (fixture error)" with Try again. |
| Deep-mode rail | 56 px: count text; Back button with accessible name "Back to attention queue, 3 decisions need you" | — |
| Composer | One labelled textarea with auto-grow; Enter submits, Shift+Enter inserts a newline; "Add details" disclosure | Validation inline under the field |
| Code and Git deep mode | Same components as concept A's Code and Git tabs at this concept's type scale | Conflict rows per section 7.1 |

### 7.1 Shared state copy for concept C

| State | Treatment in C | Copy | Actions |
|---|---|---|---|
| Loading | Skeleton rows shaped like the final layout | "Loading tasks…" | Navigation usable; mutating actions disabled |
| Empty (project) | Plain text in the focus pane | "No tasks in this project yet." + "Create a task to start a discussion." | `New task · Mock` |
| Empty (decisions) | Queue section stays | "Nothing needs your decision. Last checked 14:02 · observed." | Link to all tasks |
| Empty (search) | Same, with the query echoed | "No files match `auth*`. Try fewer characters or turn off Regex." | Clear query |
| Offline / stale cache | Heading shows "As of 14:02" | "Showing last confirmed state from 14:02 (12 min ago)." | Read-only; mutating actions disabled with reason |
| Stale result | Warn-tint header in the focus pane; acceptance disabled | "Result changed after review: digest 9f2c → 41ab. Previous acceptance (mock) is invalidated." | `Review new result` |
| Disconnected | Connection danger; banner at top of the focus pane; running executions become Unknown | "Machine disconnected. Last confirmed 14:02 (12 min ago). Current states are unknown. Actions are unavailable." | `Reconnect · Mock`; Connection |
| Reconnecting | Warn indicator; no spinner after 2 s; static "Reconnecting (attempt 3)" | — | Cancel reconnect |
| Blocked: your permission | Wait reason "Your permission needed" in the queue and in L1 | "Worker requests permission to run `npm test` in `worktrees/t-0142`." | `Approve once · Mock` / `Deny · Mock` |
| Not permitted (device or capability) | Neutral lock glyph; disabled control with reason below | "This device is not authorized to push. Enroll it in Connection settings." | Link to the fix |
| Unavailable capability | circle-slash + "Unavailable"; section still present | "Plan updates unavailable: this agent did not negotiate plan reporting." | — |
| Failed check | Danger x-in-circle "Failed · observed", listed first | "`unit tests` failed · observed · 3 of 412 tests failed" | Open log; `Request changes · Mock` |
| Not run / unverified | Neutral hollow dash or "?"; never green | "`e2e` not run (required)." / "`lint` reported by agent; no receipt (unverified)." | — |
| File conflict (save) | Banner at top of the Code deep mode; save disabled | "This file changed on disk since you opened it (agent edit at 14:03)." | `Compare` · `Reload` · `Save as…` (never overwrite) |
| Git conflict | Git deep mode top banner, danger; conflicted files first; no auto-stash or reset | "Merge stopped: 2 files have conflicts on `feature/auth`." | Open file · `Mark resolved · Mock` · `Abort merge · Mock` (confirm) |
| Decision version conflict | Inline in the decision | "Answered on Phone at 14:05…" | View outcome (`role="status"`) |
| Ambiguous external outcome (push) | Neutral "?" "Outcome unknown"; **no Retry button** | "Push to `origin/feature/auth` did not confirm. Check the remote before trying again." | `Inspect remote · Mock` |
| Error (typed) | Danger glyph and message near the source; never silent | "Could not load history page 4 (fixture error E_PAGE). Nothing was changed." | `Try again` only for idempotent reads |
| Long text | Titles clamp to two lines in the queue only; full text in the focus pane and accessible name. Messages over 12 lines collapse with "Show full message (48 lines)". IDs, paths and URLs wrap anywhere. Decision titles, action labels and errors are never truncated. | — | Expand (button with `aria-expanded`) |

## 8. Decision and result anatomy

### 8.1 Decision block at L1 (same order in all concepts)

1. Type label and glyph: **Question** / **Permission** / **Result acceptance**.
2. Title: one sentence, wraps fully, in c-text-xl.
3. Line: task ID and title; requested by (execution → worker); requested at (observed); decision version.
4. **Effect:** what happens if approved or answered.
5. **Scope:** exact paths, commands, network or remote, entity version, result ID and digest; affected file count with a link to the list.
6. **If you reject:** the concrete consequence. Nothing is reverted.
7. **Evidence** (acceptance only): counts of Passed / Failed / Not run / Unverified, each labelled, plus required checks that did not run, listed by name.
8. **Actions:** Question → answer field and `Send answer · Mock`. Permission → `Approve once · Mock` and `Deny · Mock` ("Always allow" only when negotiated). Acceptance → `Accept r-0142.3 · Mock`, `Request changes · Mock`, `Reject · Mock`. No approve-all.
9. **Footer (acceptance only):** "Accepting does not commit, push or deploy."
10. **States:** stale (accept disabled, link to the new result); expired ("This request expired at 14:20."); answered elsewhere ("Answered on Phone at 14:05: Approved once (mock). Your choice was not applied."); disconnected (actions disabled with the last-confirmed time).

### 8.2 Result and evidence

- **Header:** result ID, revision, workspace or worktree identity, digest (short form with copy button), produced at (observed), acceptance state.
- **Checks table:** Check · Outcome · Source · Duration · Details. Sorted Failed, Not run, Unverified, Passed. A "Required but not run" group always appears.
- **Changes:** file list with change type as text and +/− counts. Diff with `+`/`−` gutter text; side-by-side when there is room, otherwise unified.
- **Acceptance block** sits beside or below the evidence, never above it.
- **Evidence by default:** L2 "Result & evidence" opens by default only for acceptance decisions, because the evidence is the decision.

## 9. Mock-state labelling

Non-negotiable in every concept.

1. **Prototype strip:** a persistent 28 px strip above all app chrome. It cannot be dismissed or scrolled away. Text: `PROTOTYPE · Fixture fx-<revision> · Scenario: <name>, step n of m · All actions are mock and change only in-memory state`.
2. **Mutating controls** carry a visible `Mock` tag after the label, for example `Approve permission · Mock`, `Accept result · Mock`, `Commit · Mock`. The accessible name includes "(mock)". Controls: Start or Create task, Answer, Approve, Reject, Accept, Request changes, Cancel execution, Save, Stage, Commit, Pull, Push, Enroll, Revoke and Test connection.
3. **Mock outcome receipts:** an inline result appears where the action was taken. It is not a toast and does not disappear. Examples: `Mock outcome: recorded in memory only. No agent was started.` and `Mock outcome: no artifact was accepted; not runtime evidence.` Acceptance renders as `Accepted (mock) · r-0142.3 · digest 9f2c…`.
4. **No simulated progress:** the fixture never advances on a timer. Steps advance only through the prototype controls and show "Scenario step n of m (fixture)".
5. **Fixture provenance:** fixture facts keep their in-fixture provenance. The status bar shows `Fixture data, not runtime evidence`.
6. **Disconnected fixture:** every mock mutating control is disabled with the reason "Disconnected; actions unavailable".

Interaction invariants for concept C:

- **No automatic navigation.** Queue order is fixed by the fixture's ranking and never re-sorted under the cursor. Newly arrived items wait behind the "2 new · Show" control. Focus never moves because of incoming data.
- **Announcements:** new decisions are announced once through a polite region and appended without moving the focused row.
- **Browsing does not select.** J/K moves focus only; the focus pane changes on Enter.
- **No single-key decision actions; no drag-and-drop.**
- **Destructive mock actions** use a confirmation dialog restating effect and scope; focus goes to the safe button; Escape cancels.
- **Character-key shortcuts** are active only while their component has focus and can be turned off in Settings → Keyboard.

## 10. Screen coverage

| Screen ([SCREENS.md](../SCREENS.md)) | Where in concept C | Reached by |
|---|---|---|
| Project board and list | "All tasks" (List default, Board toggle) | Top bar; `G T` |
| Create task | Focus-pane composer: one large field "What should be done?" with `Create task · Mock`. "Add details" reveals Details and Context files | `Ctrl+N`; top bar |
| Task discussion and plan | L2 excerpt ("Why this is asked", max 12 lines with "Show more") and Plan section; L3 full timeline | Disclosure |
| Worker detail | L2 Workers summary → L3 worker detail | Disclosure |
| Decisions | Queue and L1 | Home |
| Evidence and diff | L2 "Result & evidence" and "Changes" → L3 full diff | Disclosure |
| File tree, editor and search | L3 Code deep mode; also top-bar "Code" | Link; `G C` |
| Terminal | Bottom split inside Code deep mode | `` Ctrl+` `` |
| Git | L3 Git deep mode from the "Code & Git" section | Link; `Ctrl+Shift+G` |
| Connection settings | Connection item in top bar → focus-pane page | Click; Project ▾ |

Disclosure state is remembered per task. Headings are real buttons with `aria-expanded` and `h3` semantics.

## 11. Disclosure model

| Level | Content | Trigger |
|---|---|---|
| L0 | Queue row: what is asked, of which task, since when | Always |
| L1 | Decision or result detail with effect, scope, rejection consequence and actions | Select row (Enter) |
| L2 | Collapsed sections: Why this is asked, Plan, Result & evidence, Changes, Code & Git summary (worktree, branch, conflicts), Workers | Enter or Space on heading; `Alt+1`…`Alt+6` |
| L3 | Full surfaces: full discussion timeline, full diff, editor / search / terminal, Git, worker detail | "Open full …" link inside L2 |

## 12. Scenarios

Run identically in all three concepts at 1280×800 and 1920×1080, with pointer and keyboard, and at 100 % and 200 % text. The scenario definitions S1–S6 are written out in [concept A](desktop-concept-a.md) section 11; the scenario controls are in [prototype fixtures](prototype-fixtures.md).

- **S1 Create → decision → result.**
  1. `Ctrl+N`, type the request, Enter. The task appears under "Changed since last visit" with "Not started (mock)".
  2. Advance the step. The question appears under "Needs your decision" with an announcement. Focus stays put.
  3. `J` to the row, Enter. Answer. The "Why this is asked" excerpt is available.
  4. Permission, then acceptance. L2 "Result & evidence" is open by default for acceptance decisions.
  5. `Accept · Mock`. The Code & Git summary reads "3 files changed, not committed".
- **S2 Code / Git conflict.** From the task, L2 "Code & Git" → "Open editor" (deep mode) → save conflict banner → Compare. Git deep mode shows the merge conflict.
- **S3 State sweep:** as in concept A (empty, blocked, failed check, stale, disconnected, unavailable capability, long text, 500-task scroll, 10,000-event timeline).
- **S4 Keyboard only** and **S5 Increased text size** (150 % and 200 %): as in concept A. Larger type reduces density at 1280×800.
- **S6 Desktop → phone → desktop:** as in concept A.

**Validation focus.** Count the actions needed to reach the board, result and diff, and Code & Git from the queue at both viewports.

## 13. Accessibility baseline

- Text pairs at least 4.5:1 (most at least 6:1 per section 4.2). Focus rings and control boundaries at least 3:1, with the ink-fill exception handled by the outside ring (section 4.2). Disabled labels are exempt; disabled reason text uses the muted role.
- Pointer targets at least 24 × 24 CSS px.
- Landmarks: `header`, `nav`, labelled `main` for the focus pane, `contentinfo` for the status bar. Decisions are `<section>` elements with `h2`.
- All sizes in rem from a root set by the text-size preset. Line heights are unitless. Containers are content-height.
- Skip link "Skip to main work area"; F6 cycles regions in visual order.
- `?` or `F1` opens the keyboard reference.
- Dialogs trap and restore focus. Focus never moves because of incoming data.
- Motion: disclosure expand only; reduced motion sets durations to 0.

## 14. Concept-specific risks and open questions

- **Ranking.** The queue ranking is fixture-defined and needs owner confirmation (open question 5).
- **Discoverability.** Experienced users may find the board and code too far away. Top-bar shortcuts mitigate this.
- **Density at 1280×800.** Larger type reduces visible density.
- **Focus and selection.** Focus (teal outline) and selection (teal tint) use the same hue family and must be confirmed distinguishable in rendered screenshots.
- **Ink-button focus.** The teal ring against the ink fill is 2.57 and is placed outside the button; the rendered review must confirm this.
- Rendered review, keyboard runs and screenshots for this concept do not yet exist.
