# Desktop concept A: Task Desk

**Status: candidate direction pending owner Gate D. Not the approved design system.** Every token, font family, colour, board phase name and copy string in this document is a candidate for owner decision. [DESIGN.md](../DESIGN.md) still states that design tokens, typography, colours and spacing are TBD until Gate D; this document does not change that statement. Gate D is recorded in [desktop approval](desktop-approval.md).

Parent: [desktop concepts](desktop-concepts.md). Shared fixtures: [prototype fixtures](prototype-fixtures.md). Product and screen references: [PRODUCT.md](../PRODUCT.md), [SCREENS.md](../SCREENS.md).

Implementation status: not built. The shared mock shell in `prototypes/cortex-workspace/desktop/` currently provides placeholders for `/a`; concept A is not complete and nothing here is runtime-qualified.

## 1. Thesis

The project is a desk of tasks. The home view answers "what is the state of all my work?". Opening a task places a task list beside a task workspace. Discussion, plan, decisions, result and diff, code, Git and workers are tabs of that one task.

Differences from the other concepts:

- Spatial overview of the whole project on the home view.
- Light, paper-like surfaces at medium density.
- Tabs inside the task, not a global IDE frame.

Board cards are tasks, never workers ([PRODUCT.md](../PRODUCT.md)). Columns are read-only projections of runtime state. Cards carry no drag handles, because phase is set by the runtime, not by the user.

## 2. Navigation model

```
Board / List (home)
  → select task
    → Task workspace [tabs: Discussion & plan | Decisions | Result & diff | Code | Git | Workers]
      → Decisions → Result & diff → Code (file tree + editor + search, inside the tab) → Git
```

- **Needs you** (top bar) opens a cross-task Decisions list inside the task-workspace pane. It does not switch top-level views.
- **Code** and **Git** are reached from a task and are scoped to that task's worktree, shown in the header. A project-level Code entry in the Project menu opens in the same workspace pane for work not tied to a task.
- **Terminal** is a bottom split inside the Code tab (user-resizable, 240 px default). It is part of the Code tab area, not a separate primary area.
- **Connection settings** live under Project → Machine & connection as a full-pane page that replaces the workspace pane. The list stays visible.
- The list can be collapsed to a 48 px "Tasks" rail with `Ctrl+\`. It is never collapsed automatically.

## 3. Layout

Window sizes: 1280×800 and 1920×1080. The prototype strip (28 px) is always the top row. Heights in px.

### 3.1 1280×800, board home

```
y0   ┌ Prototype strip 28 ─────────────────────────────────────────────────────┐
y28  ├ Top bar 48: Project ▾ │ Board | List │ Search (Ctrl+K) │ … │ Needs you 3 │ New task · Mock │ Help ┤
y76  ├ Filter row 36: Phase ▾  Wait reason ▾  Execution ▾  Showing 500 tasks  ─────┤
y112 │ Board grid: 7 columns, min 176 / max 280 px, gap 8, padding 16;          │
     │ 5 columns fully visible; horizontal scroll by column (H/L or Shift+wheel)│
     │ each column: header 32 (name + count), virtualized card list            │
y776 ├ Status bar 24: Connected · last confirmed 14:02 · Fixture fx-… · Mock ──┤
y800 └────────────────────────────────────────────────────────────────────────┘
```

- Board height is 664 px. Each column scrolls vertically on its own.
- Horizontal board scrolling is the only horizontal scrolling in this concept. It is explicit, has visible column-navigation buttons, and is keyboard-driven.
- **Card** (minimum 72 px, 12 px padding, small text): line 1 task title (two-line clamp); line 2 execution badge and wait-reason badge (wait reason only when waiting); line 3 task ID (mono xs) · "updated 14:02 · observed". No avatars, progress bars or decorative tags.

### 3.2 1280×800, task open

```
├ Top bar 48 ──────────────────────────────────────────────────────────────┤
│ Task list 360 (compact rows 36,      │ Task workspace 920                 │
│ same filters; selected row tinted)   │ Header 72: title (lg), ID, Phase,  │
│                                      │  Execution, Wait reason, last conf. │
│                                      │ Tabs 40: Discussion & plan |        │
│                                      │  Decisions (2) | Result & diff |    │
│                                      │  Code | Git | Workers              │
│                                      │ Tab content (scroll)               │
```

Two primary areas are visible: the list and the task workspace.

- The Discussion & plan tab shows the plan as an inline section at this width.
- At 200 % text the list pane is user-collapsible and the header wraps to two or three lines. Nothing is truncated.

### 3.3 1920×1080

- Board: seven columns at about 256 px each, all visible. The filter row adds a saved-view selector.
- Task open: list 420 px, task workspace 1500 px.
- Inside Discussion & plan: timeline up to 760 px plus a non-scrolling plan rail of 360 px showing plan steps with state labels. The rail is a supporting panel, not a third primary area.
- Result & diff: side-by-side merge view.

## 4. Candidate tokens

Ratios are WCAG 2.x relative-luminance contrast computed from the listed hex values. Text pairs must reach at least 4.5:1; non-text boundaries and focus rings at least 3:1.

### 4.1 Typography (owner font decision)

UI family IBM Plex Sans; monospace IBM Plex Mono. Fallbacks: `Ubuntu, Cantarell, system-ui, sans-serif` and `"Ubuntu Mono", "DejaVu Sans Mono", monospace`. Font bundling is an open owner question.

| Token | Size / line height | Weight | Use |
|---|---|---|---|
| a-text-xs | 12 / 16 | 400 | metadata, provenance, status bar |
| a-text-sm | 13 / 18 | 400/500 | card body, table cells, tabs |
| a-text-base | 14 / 20 | 400 | UI default |
| a-text-prose | 14 / 22, max 72ch | 400 | discussion messages |
| a-text-md | 16 / 22 | 600 | pane and section headings |
| a-text-lg | 20 / 26 | 600 | task title in workspace header |
| a-text-xl | 24 / 30 | 600 | empty-state headline only |
| a-mono | 13 / 20 | 400 | IDs, digests, paths, code, terminal |

### 4.2 Colour

| Role | Hex | Measured contrast |
|---|---|---|
| canvas | #F6F6F4 | base |
| surface | #FFFFFF | base |
| sunken (column background, code gutter) | #EEEEEA | base |
| separator (non-essential) | #D6D6D0 | decorative only |
| control border | #85857E | 3.71 on surface, 3.43 on canvas |
| text | #1C1C1A | 17.07 on surface, 15.77 on canvas |
| text muted | #55554F | 7.50 surface, 6.93 canvas, 6.45 sunken, 6.26 on selection tint |
| accent (links, primary fill, selection border) | #1F4FB8 | 7.32 surface, 6.76 canvas, 6.11 on selection tint |
| on-accent | #FFFFFF | 7.32 on accent |
| selection tint | #E3EBFA | primary text 14.25 |
| danger / tint | #B42318 / #FCEDEB | 6.57 surface; 5.78 on tint; primary text on tint 15.00 |
| warn / tint | #8A5300 / #FBF2DE | 6.33 surface; 5.68 on tint |
| warn border | #A86B00 | 4.40 on surface (non-text boundary) |
| success / tint | #1D6B34 / #E6F2E9 | 6.55 surface; 5.69 on tint |
| neutral / unknown tint | #55554F / #ECECE8 | 6.34 on tint |
| mock / tint | #6A3D9A / #F2ECF8 | 7.64 surface; 6.60 on tint |
| diff add / delete background | #E3F1E6 / #FBE7E5 | primary text 14.62 / 14.36 |
| disabled label | #8A8A84 | 3.47 on surface (exempt; disabled reason text uses muted role) |

### 4.3 Spacing, radius, elevation, focus, motion

- **Spacing:** 4 · 8 · 12 · 16 · 24 · 32 · 48 (4 px base).
- **Radius:** controls 4 px; panels and cards 6 px; popovers 8 px. No pills.
- **Elevation:** flat panels separated by 1 px lines. Popovers and menus use `0 4px 16px rgba(28,28,26,.14)` plus a 1 px control border.
- **Focus:** 2 px solid #1F4FB8 outline with 2 px offset (6.76 on canvas). Inside virtualized rows and cards, an inset 2 px ring (`outline-offset: -2px`) avoids clipping.
- **Selection:** tint plus 1 px full accent border, 600-weight title and `aria-selected`. Focus (2 px, offset) and selection (1 px, inner) differ by weight and position. Both use the same hue family, so the rendered review must confirm they are distinguishable.
- **Motion:** pane open 120 ms ease-out (transform and opacity only); everything else 0 ms; reduced motion sets all durations to 0.

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

- A status badge is static markup, single line, no wrapping for the badge label. Truncated values open their full value on focus or click.
- Provenance is muted text after a middle dot, for example `Tests: Failed · observed`. "Estimated" values are prefixed "est." and never drive layout emphasis.
- An agent-reported claim such as "tests pass" is shown as conversation text with "reported". It is never rendered as a Passed check unless an observed check record exists in the fixture.
- There are no percentage bars and no ETAs. Progress is shown as observed facts, for example "3 of 5 plan steps reported done".

## 6. Keyboard map and focus order

**Focus order:** skip link → top bar (Project, Board/List, Search, Needs you, New task, Help) → filter row → board or list → task workspace (header → tabs → tab content) → status bar. F6 and Shift+F6 cycle the same regions.

| Keys | Scope | Action |
|---|---|---|
| `Ctrl+K` | global | command palette / search tasks |
| `Ctrl+N` | global | new task |
| `G B` / `G L` / `G D` | global, can be disabled | board / list / decisions |
| `Ctrl+\` | global | collapse or expand the list pane |
| arrows or `J`/`K`, `H`/`L` | board grid focused | move card focus within and between columns (roving tabindex; one Tab stop for the board) |
| `Home` / `End` | board column | first / last card |
| `Enter` | card or row | open task; focus moves to the task title heading |
| `Alt+1`…`Alt+6` | task workspace | Discussion, Decisions, Result, Code, Git, Workers |
| `Ctrl+Shift+G`, `` Ctrl+` `` | task workspace | Git tab / terminal |
| `Esc` | workspace | return focus to the originating card (does not close the task) |
| `Alt+←` / `Alt+→` | global | history back / forward |
| `?` / `F1` | global | keyboard help |

Decision buttons are reached with Tab only. Decision actions have no single-key shortcuts.

## 7. Component inventory and states

| Component | Anatomy | State notes |
|---|---|---|
| Board column | Header: phase name and count (for example "In execution 37"); virtualized cards | Loading: six skeleton cards. Empty: "No tasks" (muted), header stays. Disconnected: header suffix "as of 14:02". |
| Task card | Section 3.1 | Waiting on you: full 1 px warn border and warn-tinted wait badge (no side stripe). Failed: danger execution badge. Unknown: neutral "?" "Unknown". Stale result: badge "Result stale". Long title: two-line clamp, full value on focus and in the accessible name. Mock-created: "Mock" tag on line 3. |
| List table | Sticky header, sortable, 36 px rows, virtualized for 500 rows | Same badges as cards. No-results search uses the empty-search copy. |
| Workspace header | Title, ID, Phase, Execution with provenance, Wait reason, Last confirmed | Disconnected: "Last known Running at 14:02 · current state unknown" in neutral. |
| Discussion timeline | Messages (prose 72 ch), grouped tool events ("48 tool events · reported, Show"), decision events as links | 10,000 events: virtualized; "Load earlier" at top; "Jump to latest" when not pinned to the bottom. Page error: inline row with Try again. Unavailable history: "Discussion history beyond 30 days unavailable from this agent" when the fixture says so. |
| Plan (rail at 1920, inline section at 1280) | Numbered steps, each with state text (Done · reported / In progress · reported / Not started / Unknown) | Unavailable capability: section shows circle-slash copy "Plan updates unavailable: this agent did not negotiate plan reporting." No percentages. |
| Decision panel | Section 8.1 | Stale, expired and answered-elsewhere per section 8.1. Disconnected: disabled actions with reason. |
| Result view | Section 8.2 | Failed and Not run rows first. Stale banner above the acceptance block. |
| Code tab | Tree 260 px (git status letters as text: M, A, D, U = conflict); editor tabs; lease line in editor header ("You can edit · lease held (mock)" / "Agent is writing this path; editing paused") | File conflict banner (section 7.1). Unsaved state is text "Unsaved" in the tab, never a dot alone. |
| Terminal | Terminal area; header "Fixture terminal: output is recorded; commands are not executed" | Disconnected: input disabled, "Output paused at 14:02". |
| Git tab | Worktree line (path · branch · HEAD short), groups, commit form | Conflict: Conflicted group first, danger banner, Commit disabled with "Resolve 1 remaining conflict". Push: confirmation dialog naming remote and branch; never force. Ambiguous push: unknown state with Inspect remote. |
| Connection page | Machine name, state, last confirmed, relay URL (default plus override field), device list with Revoke · Mock, push notifications "Unavailable (not configured)" | Not permitted: device not enrolled copy with a link to the fix. |

### 7.1 Shared state copy for concept A

| State | Treatment in A | Copy | Actions |
|---|---|---|---|
| Loading | Skeleton rows shaped like the final layout; stable size | "Loading tasks…" | Navigation stays usable; mutating actions disabled |
| Empty (project) | Plain text block in the primary area, no illustration | "No tasks in this project yet." + "Create a task to start a discussion." | `New task · Mock` |
| Empty (decisions) | Same | "Nothing needs your decision. Last checked 14:02 · observed." | Link to all tasks |
| Empty (search) | Same, with the query echoed | "No files match `auth*`. Try fewer characters or turn off Regex." | Clear query |
| Offline / stale cache | Header shows "As of 14:02" with clock glyph, neutral tint | "Showing last confirmed state from 14:02 (12 min ago)." | Read-only; mutating actions disabled with reason |
| Stale result | Warn-tint banner across the result header; acceptance block disabled | "Result changed after review: digest 9f2c → 41ab. Previous acceptance (mock) is invalidated." | `Review new result` |
| Disconnected | Global "Disconnected" indicator (plug-off, danger); banner at top of each primary area; running executions become Unknown | "Machine disconnected. Last confirmed 14:02 (12 min ago). Current states are unknown. Actions are unavailable." | `Reconnect · Mock`; Connection settings |
| Reconnecting | Warn indicator; no spinner after 2 s; static "Reconnecting (attempt 3)" | — | Cancel reconnect |
| Blocked: your permission | Wait reason "Your permission needed" (warn, key glyph) on row, card and list; decision panel | "Worker requests permission to run `npm test` in `worktrees/t-0142`." | `Approve once · Mock` / `Deny · Mock` |
| Not permitted (device or capability) | Neutral lock glyph; disabled control with the reason below it (not hover-only) | "This device is not authorized to push. Enroll it in Connection settings." | Link to the fix |
| Unavailable capability | circle-slash + "Unavailable"; section still present | "Plan updates unavailable: this agent did not negotiate plan reporting." | — |
| Failed check | Danger x-in-circle "Failed · observed", listed first | "`unit tests` failed · observed · 3 of 412 tests failed" | Open log; `Request changes · Mock` |
| Not run / unverified | Neutral hollow dash / "?"; never green | "`e2e` not run (required)." / "`lint` reported by agent; no receipt (unverified)." | — |
| File conflict (save) | Editor top banner, warn; save disabled | "This file changed on disk since you opened it (agent edit at 14:03)." | `Compare` · `Reload` · `Save as…` (never overwrite) |
| Git conflict | Git tab top banner, danger; conflicted files first; no auto-stash or reset | "Merge stopped: 2 files have conflicts on `feature/auth`." | Open file · `Mark resolved · Mock` · `Abort merge · Mock` (confirm) |
| Decision version conflict | Inline in the decision panel | "Answered on Phone at 14:05…" | View outcome (`role="status"`) |
| Ambiguous external outcome (push) | Neutral "?" "Outcome unknown"; **no Retry button** | "Push to `origin/feature/auth` did not confirm. Check the remote before trying again." | `Inspect remote · Mock` |
| Error (typed) | Danger glyph and message near the source; never silent | "Could not load history page 4 (fixture error E_PAGE). Nothing was changed." | `Try again` only for idempotent reads |
| Long text | Titles clamp to two lines in list and card only; full text in detail and accessible name. Messages over 12 lines collapse with "Show full message (48 lines)". IDs, paths and URLs wrap anywhere. Decision titles, action labels and errors are never truncated. | — | Expand (button with `aria-expanded`) |

## 8. Decision and result anatomy

### 8.1 Decision panel (same order in all concepts)

1. Type label and glyph: **Question** / **Permission** / **Result acceptance**.
2. Title: one sentence, wraps fully.
3. Line: task ID and title; requested by (execution → worker); requested at (observed); decision version (for example `v3`).
4. **Effect:** what happens if approved or answered.
5. **Scope:** exact paths, commands, network or remote, entity version, result ID and digest the decision applies to; affected file count with a link to the list.
6. **If you reject:** the concrete consequence. Nothing is reverted.
7. **Evidence** (acceptance only): counts of Passed / Failed / Not run / Unverified, each labelled, plus required checks that did not run, listed by name.
8. **Actions:**
   - Question: answer field and `Send answer · Mock`.
   - Permission: `Approve once · Mock` and `Deny · Mock`. "Always allow" appears only when the fixture marks the capability as negotiated.
   - Acceptance: `Accept r-0142.3 · Mock`, `Request changes · Mock`, `Reject · Mock`.
   - There is never an approve-all.
9. **Footer (acceptance only):** "Accepting does not commit, push or deploy."
10. **States:**
    - Stale: "This result changed after it was requested (digest 9f2c → 41ab). Review the new result." Accept is disabled, with a link to the new result.
    - Expired: "This request expired at 14:20."
    - Answered elsewhere: "Answered on Phone at 14:05: Approved once (mock). Your choice was not applied."
    - Disconnected: actions disabled, with the last-confirmed time.

### 8.2 Result and evidence

- **Header:** result ID, revision, workspace or worktree identity, digest (short form with copy button; full value in the accessible name), produced at (observed), acceptance state (`Not reviewed` / `Accepted (mock) for digest …` / `Acceptance invalidated: result changed`).
- **Checks table:** Check · Outcome · Source (observed receipt or reported by agent) · Duration · Details. Sorted Failed, Not run, Unverified, Passed. A "Required but not run" group always appears, reading "All required checks ran" when empty.
- **Changes:** file list with path, change type as text (Added / Modified / Deleted / Renamed / Conflict) and +/− line counts. Diff in a CodeMirror 6 merge view: side-by-side at 760 px or wider, unified below that or at 150 % text. Diff colours are paired with `+`/`−` gutter text.
- **Acceptance block** sits beside or below the evidence, never above it.

## 9. Mock-state labelling

Non-negotiable in every concept.

1. **Prototype strip:** a persistent 28 px strip above all app chrome. It cannot be dismissed or scrolled away. Text: `PROTOTYPE · Fixture fx-<revision> · Scenario: <name>, step n of m · All actions are mock and change only in-memory state`.
2. **Mutating controls** carry a visible `Mock` tag after the label, for example `Approve permission · Mock`, `Accept result · Mock`, `Commit · Mock`. The accessible name includes "(mock)". Controls: Start or Create task, Answer, Approve, Reject, Accept, Request changes, Cancel execution, Save, Stage, Commit, Pull, Push, Enroll, Revoke and Test connection.
3. **Mock outcome receipts:** after a mock action an inline result appears where the action was taken. It is not a toast and does not disappear. Examples: `Mock outcome: recorded in memory only. No agent was started.` and `Mock outcome: no artifact was accepted; not runtime evidence.` Acceptance renders as `Accepted (mock) · r-0142.3 · digest 9f2c…`, never as a plain "Accepted".
4. **No simulated progress:** the fixture never advances on a timer. Scenario steps advance only through the prototype controls in [prototype fixtures](prototype-fixtures.md) and show "Scenario step n of m (fixture)".
5. **Fixture provenance:** fixture facts keep their in-fixture provenance ("observed", "reported"). The status bar shows `Fixture data, not runtime evidence`.
6. **Disconnected fixture:** every mock mutating control is disabled with the reason "Disconnected; actions unavailable".

Interaction invariants for concept A:

- **No automatic navigation.** New events in a timeline scrolled away from the bottom show "12 new events · Jump to latest" and do not auto-scroll.
- **Announcements:** incoming decisions update one `role="status"` count region ("3 decisions need you"), throttled, and never move focus.
- **No single-key decision actions; no drag-and-drop.** Board columns are not drop targets.
- **Destructive mock actions** (Reject result, Cancel execution, Push, Revoke device) use a confirmation dialog that restates effect and scope. Focus goes to the safe button. Escape cancels and returns focus to the invoker.
- **Character-key shortcuts** (J/K, G-sequences) are active only while their component has focus. Global ones can be turned off in Settings → Keyboard.

## 10. Screen coverage

| Screen ([SCREENS.md](../SCREENS.md)) | Where in concept A | Reached by |
|---|---|---|
| Project board and list | Home; Board or List toggle. List is a sortable table: Title, Phase, Execution, Wait reason, Updated · provenance | Launch; `G B` / `G L` |
| Create task | Workspace pane form: Title (required, single line, visible label); Details (optional, multiline); Context files (optional picker); start mode "Discuss first" (default) or "Save as draft"; `Create task · Mock` | `New task · Mock` / `Ctrl+N` |
| Task discussion and plan | Tab 1 | Open task |
| Worker detail | Tab "Workers": execution → attempts → worker table; row opens detail in the tab (role, engine and model if exposed, state · provenance, last event · observed, tool events, permissions requested) | Tab, or worker link in a timeline event |
| Decisions | Tab "Decisions" for this task; cross-task list from "Needs you" | Tab; top bar |
| Evidence and diff | Tab "Result & diff" | Tab; decision link |
| File tree, editor and search | Tab "Code": tree 260 px plus editor; search replaces the tree column with results | Tab; `Ctrl+P` (file), `Ctrl+Shift+F` (search) |
| Terminal | Bottom split inside the Code tab | `` Ctrl+` `` |
| Git | Tab "Git": worktree identity line, Changes (Conflicted / Staged / Unstaged), diff, commit message, Branch ▾, `Pull · Mock`, `Push · Mock` (confirm remote and branch) | Tab; `Ctrl+Shift+G` |
| Connection settings | Project ▾ → Machine & connection page | Menu; status-bar connection item |

Create-task success: the new card appears in Discussion, selected but not scrolled away from the user's place. The workspace shows the task with execution "Not started (mock)" and the mock receipt.

## 11. Scenarios

Run identically in all three concepts at 1280×800 and 1920×1080, with pointer and keyboard, and at 100 % and 200 % text.

- **S1 Create → decision → result.** Create task "Add rate limiting to /login". The prototype control advances the fixture: a question appears, answered. Next step: permission request to run tests, approved once. Next step: result with one failed check and one required check not run; request changes. Next step: new result revision, all checks run; accept (mock). The copy "does not commit or push" is visible; Git still shows the unstaged changes.
- **S2 Code / Git conflict.** Open `src/auth/limiter.ts`, edit, save. The fixture reports an agent change; the save-conflict banner offers Compare, Reload and Save as. Git shows a merge conflict in two files; resolve one in the editor and mark it resolved (mock); leave the other conflicted. Commit stays disabled with the reason.
- **S3 State sweep:** empty project; blocked permission; failed check; stale result (accept disabled, mock acceptance shown invalidated); disconnected (no mock mutation possible, last-confirmed time visible); unavailable capability; long text; 500-task board and list scroll; 10,000-event timeline.
- **S4 Keyboard only:** S1 and S2 with no pointer; every focus position visible.
- **S5 Increased text size:** S1 at 150 % and 200 % at 1280×800; no clipped essential text, no lost actions, no two-dimensional scrolling of prose.
- **S6 Desktop → phone → desktop (desktop side):** a decision is answered on the phone while the user is away. On return, a "Changes since you were last here" list shows it with "Answered on Phone 14:05 (mock)". The open task and view are preserved; nothing auto-opens.

Walkthrough for S1 with the keyboard: `Ctrl+N`, type the title, Enter → the task opens. Advance the step; the Decisions tab count changes from 0 to 1 and the card gets the "Your answer needed" badge, without a view switch. `Alt+2`, Tab to the answer field, type, Tab to `Send answer · Mock`, Enter. Advance: permission decision. Advance; `Alt+3` shows Failed first, then Not run; `Request changes · Mock`. Advance; `Accept · Mock` with the footer copy visible. `Alt+5` shows the unstaged changes still present.

## 12. Accessibility baseline

- Text pairs at least 4.5:1 (most at least 6:1 per section 4.2). Focus rings and control boundaries at least 3:1 against adjacent colours. Disabled labels are exempt; disabled reason text uses the muted role.
- Pointer targets at least 24 × 24 CSS px, including dense tree and list rows.
- Landmarks: `header`, `nav`, labelled `main` for each primary area, and `contentinfo` for the status bar. Board is `role="grid"` with column headers; list is a table or grid. Decisions are `<section>` elements with `h2`.
- All sizes in rem from a root set by the text-size preset (100 / 125 / 150 / 200 %). Line heights are unitless. Containers are content-height.
- Skip link "Skip to main work area". F6 and Shift+F6 cycle landmarks in visual order.
- `?` or `F1` opens the keyboard reference.
- Focus moves to a pane's heading when it opens and returns to the invoker when it closes. Dialogs trap and restore focus. Focus never moves because of incoming data.
- Motion: only transform and opacity are animated. The only continuous animation is a loading indicator. "Running" execution is static text with elapsed time updated each minute.
- Z-index scale: base 0 · sticky 10 · menus and popovers 20 · dialogs 30 · inline receipt layer 40 · prototype strip 50.

## 13. Colour pairs to confirm in rendered review

- Focus and selection differ by weight and position, and both use the accent hue family (accent on canvas 6.76; accent on selection tint 6.11). Rendered review must confirm they stay distinguishable.
- Warn border (#A86B00, 4.40 on surface) is used as a non-text boundary; confirm it reads at 3:1 or more in the rendered state.

## 14. Concept-specific risks and open questions

- The kanban affordance invites dragging. Columns have no drag handles, and the column header help text states "Phase is set by the runtime".
- Seven columns at 1280 px need horizontal scrolling. The List view is the recommended alternative at 200 % text, offered in copy and not forced.
- Board phase names are candidates, not a product decision. Prototypes use them labelled as fixture.
- Font families are an owner decision (bundle OFL fonts, or use Ubuntu system fonts). Icon family is an owner decision.
- Tauri 2 / WebKitGTK rendering of focus outlines, text zoom and accelerators such as `Ctrl+W`, `Ctrl+R` and `Ctrl+P` is unverified. The prototype may need to suppress webview defaults.
- CodeMirror 6 Tab and Escape behaviour and the merge-view side-by-side and unified switch must be checked against the installed version.
- Performance targets in [performance](../performance.md) are unmeasured. They are reported as unrun until instrumentation exists.
- Rendered review, keyboard runs and screenshots for this concept do not yet exist.
