# Desktop concept B: IDE with Mission Control

**Status: candidate direction pending owner Gate D. Not the approved design system.** Every token, font family, colour, board phase name and copy string in this document is a candidate for owner decision. [DESIGN.md](../DESIGN.md) still states that design tokens, typography, colours and spacing are TBD until Gate D; this document does not change that statement. Gate D is recorded in [desktop approval](desktop-approval.md).

Parent: [desktop concepts](desktop-concepts.md). Shared fixtures: [prototype fixtures](prototype-fixtures.md). Product and screen references: [PRODUCT.md](../PRODUCT.md), [SCREENS.md](../SCREENS.md).

Implementation status: not built. The shared mock shell in `prototypes/cortex-workspace/desktop/` currently provides placeholders for `/b`; concept B is not complete and nothing here is runtime-qualified.

## 1. Thesis

The developer lives in the editor, and orchestration comes to them without taking the code away. Mission Control is a side-panel view, like Files, Search or Git. Tasks, decisions, results and the board open as editor tabs in a second editor group. The code group stays visible throughout.

Differences from the other concepts:

- Dark, compact, tab-and-panel IDE grammar.
- Status bar carries attention counts.
- Orchestration is a peer of code, not a container for it.

Board cards are tasks, never workers ([PRODUCT.md](../PRODUCT.md)). Phase is a projection set by the runtime; there are no drag-and-drop status changes.

## 2. Navigation model

```
Editor (home, last file)
  → Mission Control (side panel)
    → task opens in editor group 2
      → sub-tabs: Discussion & plan | Decisions | Result & diff | Workers
        → diff file "Open in editor" opens a new tab in group 1 (existing tabs stay)

Board / List: side-panel view "Board" lists tasks compactly, or "Open board" opens a Board tab in group 2
  (List by default at 476 px or narrower; Board at 760 px or wider)
```

**Mission Control panel sections**, each collapsible with counts:

- **Needs you:** decisions grouped Question / Permission / Result acceptance.
- **Running:** executions with state · provenance and elapsed time.
- **Changed since last visit.**
- **Recent tasks.**
- **Links:** Open board, New task.

Selecting an item opens or focuses that task's tab. A single click previews the task (italic tab title, like a preview tab); Enter or double-click pins it.

**Other surfaces:**

- Status-bar items are buttons. "Needs you 3" opens Mission Control focused on its first decision. "2 conflicts" opens the Git view. The connection item opens Connection settings.
- **Git** is a side-panel view (Changes groups, commit box). Diffs open in group 2.
- **Search** is a side-panel view. Opening a result goes to group 1.
- **Settings → Connection** opens as a tab in group 2.
- Optional proposal, owner to accept or drop: the editor context menu "New task from selection…" pre-fills the context with path and line range.

## 3. Layout

Window sizes: 1280×800 and 1920×1080. Heights in px.

### 3.1 1280×800

```
y0   ┌ Prototype strip 28 ────────────────────────────────────────────────────────┐
y28  │Act│ Side panel 280          │ Editor group 1 (code)  │ Editor group 2       │
     │bar│ view: Files | Search |  │ tabs 36                │ tabs 36: Task T-0142 │
     │48 │ Git | Mission Control | │ breadcrumbs 24         │  | Board | Diff …    │
     │   │ Board | Settings        │ CodeMirror editor      │ task header + sub-tabs│
     │   │                         │ 476                    │ 476                  │
y776 ├ Status bar 24: ⎇ feature/auth · 2 conflicts · Needs you 3 · Running 1 · Connected 14:02 · Mock ┤
```

- Work height is 748 px. Editor area is 952 px, split 476 / 476 when group 2 exists.
- The task tab has a minimum width of 420 px. Below that (200 % text) the task tab uses a compact layout: the header wraps and the sub-tabs become a labelled select. The user can hide the side panel with `Ctrl+B`.

### 3.2 1920×1080

Activity bar 48 px, side panel 300 px, editor groups 786 / 786 px. The diff in group 2 is side-by-side because the pane is 760 px or wider.

### 3.3 Two-area rule in this concept

- Primary areas are the editor groups and the bottom panel. The side panel is navigation.
- With one editor group, the terminal opens in the bottom panel (220 px default, resizable).
- With two groups visible, `` Ctrl+` `` opens the terminal **as a tab in group 2**. It is never a third area and never replaces group 1's code context.
- This is a deliberate deviation from VS Code convention and is an owner decision (open question 4).
- Open gap: the report does not state where the bottom panel's Problems and Output tabs go when two groups are visible. The prototype must choose one placement and record it as a decision for the owner; until then, treat it as unresolved.

## 4. Candidate tokens

Ratios are WCAG 2.x relative-luminance contrast computed from the listed hex values. Text pairs must reach at least 4.5:1; non-text boundaries and focus rings at least 3:1.

### 4.1 Typography (owner font decision)

UI family Inter; monospace JetBrains Mono. Fallbacks: `Ubuntu, system-ui, sans-serif` and `"Ubuntu Mono", "DejaVu Sans Mono", monospace`. Font bundling is an open owner question.

| Token | Size / line height | Weight | Use |
|---|---|---|---|
| b-text-xs | 12 / 16 | 400 | status bar, provenance, tree metadata |
| b-text-sm | 13 / 18 | 400/500 | UI default, panels, tabs |
| b-text-base | 14 / 20 | 400 | task tab prose (max 72 ch) |
| b-text-md | 16 / 22 | 600 | task title, section headings |
| b-text-lg | 18 / 24 | 600 | empty-state headline |
| b-mono-editor | 14 / 21 | 400 | editor, diff, terminal |
| b-mono-sm | 12 / 18 | 400 | IDs and digests in tables |

### 4.2 Colour (dark only)

| Role | Hex | Measured contrast |
|---|---|---|
| editor / background | #16181D | base |
| surface (panels) | #1D2026 | base |
| raised (tabs, popovers) | #252931 | base |
| separator | #343944 | decorative only |
| control border | #6B7280 | 3.38 on surface, 3.67 on background |
| text | #E6E8EC | 14.48 background, 13.30 surface, 11.89 raised, 9.41 on selection tint |
| text muted | #A3A9B5 | 7.53 background, 6.92 surface, 6.18 raised |
| accent (links, selection border) | #6CA8FF | 6.75 surface, 7.34 background |
| primary button fill / text | #6CA8FF / #0E1420 | text 7.62; fill against surface 6.75 |
| selection tint | #23395E | primary text 9.41 |
| focus ring | #F5F7FA | 15.21 surface, 16.55 background, 10.76 on selection tint, 13.59 raised |
| danger / tint | #FF8A80 / #3A1F21 | 7.15 surface; 6.58 on tint |
| warn / tint | #F2B84B / #3A2E17 | 9.12 surface; 7.41 on tint |
| success / tint | #6CCB8F / #1B3326 | 8.22 surface; 6.83 on tint |
| neutral / unknown tint | #A3A9B5 / #2A2E36 | 5.77 on tint |
| mock / tint | #CDA8FF / #2E2440 | 8.27 surface; 7.38 on tint |
| diff add / delete / conflict line background | #1E3A2A / #432326 / #3D3418 | primary text 10.11 / 11.36 / 10.05 |
| disabled label | #7A808C | 4.11 on surface (exempt; disabled reason text uses muted role) |

Rejected candidate: primary fill #2F64C9 measured 2.94 against surface, below 3:1 for a non-text boundary. It was replaced by #6CA8FF.

### 4.3 Spacing, radius, elevation, focus, motion

- **Spacing:** 2 · 4 · 6 · 8 · 12 · 16 · 24 (compact).
- **Radius:** controls 2 px; tabs 3 px; popovers 4 px.
- **Elevation:** none, except popovers and menus: `0 6px 20px rgba(0,0,0,.45)` plus raised fill.
- **Row heights:** tree and list rows 24 px (meets the 24 px target); panel headers 32 px; tabs 36 px.
- **Focus:** 2 px #F5F7FA outline with 1 px offset; inset −2 px in tree and list rows. Selection is tint plus a 1 px #6CA8FF border. White focus against blue selection is a hue-and-lightness distinction.
- **Motion:** panels 0–80 ms, opacity only; reduced motion sets durations to 0.

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
- Provenance is muted text after a middle dot, for example `Tests: Failed · observed`. "Estimated" values are prefixed "est." and never drive layout emphasis.
- An agent-reported claim such as "tests pass" is shown as conversation text with "reported". It is never rendered as a Passed check unless an observed check record exists in the fixture.
- No percentage bars and no ETAs. Progress is shown as observed facts, for example "3 of 5 plan steps reported done".

## 6. Keyboard map and focus order

**Focus order:** skip link → activity bar (roving, one Tab stop) → side panel → editor group 1 → editor group 2 or bottom panel → status bar (roving). F6 cycles the same order. `Ctrl+1` and `Ctrl+2` focus group 1 and group 2.

| Keys | Scope | Action |
|---|---|---|
| `Ctrl+Shift+M` | global | Mission Control view |
| `Ctrl+Shift+E` / `Ctrl+Shift+F` / `Ctrl+Shift+G` / `Ctrl+Shift+B` | global | Files / Search / Git / Board views |
| `Ctrl+P` | global | quick open file |
| `Ctrl+K` | global | command palette (tasks, decisions, commands) |
| `Ctrl+B` | global | toggle side panel |
| `Ctrl+\` | editor | split or unsplit |
| `` Ctrl+` `` | global | terminal (two-area rule, section 3.3) |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | group | next / previous tab in group |
| `Ctrl+W` | group | close tab. Verify against the Tauri and WebKit default; block window close in the prototype |
| `Alt+D` | global, can be disabled | focus the next pending decision in Mission Control (focus only, no action) |
| `Ctrl+Shift+N` | global | new task |
| `Esc` then `Tab` | editor | leave the CodeMirror editor if Tab is bound to indent. Verify against the installed CodeMirror 6; prefer not binding Tab |
| arrows | tree and list | move; `Enter` opens; `→` / `←` expand or collapse |
| `Alt+←` / `Alt+→` | global | history back / forward |

Decision actions have no single-key shortcuts; `Alt+D` only moves focus.

## 7. Component inventory and states

| Component | Anatomy | State notes |
|---|---|---|
| Activity bar | 48 px icon buttons, each with an accessible name and a tooltip on focus and hover. Count badge on Mission Control ("3", with "3 decisions need you" in the accessible name) | Disconnected: no icon change except the connection item in the status bar |
| Mission Control panel | Sections per section 2; rows 24–40 px with title, type and state badges, provenance | Empty: "Nothing needs you. Last checked 14:02 · observed." Disconnected: section headers "as of 14:02"; running rows "Unknown". Loading: skeleton rows. |
| Editor tab | Title, "Unsaved" text marker, close button | Conflict: tab title suffix "Conflict" (warn text). Task tab: "T-0142 · Waiting" text suffix. |
| Task tab | Header (title, ID, phase, execution, wait reason, last confirmed); sub-tabs | Compact layout under 420 px. Stale: banner under the header. |
| Editor (CodeMirror 6) | Gutter; diagnostics listed with text in the Problems list | LSP unavailable: Problems header "Diagnostics unavailable: language server not running (fixture)". File conflict banner (section 7.1). Lease line in the breadcrumbs row. |
| Diff (merge view) | Side-by-side at 760 px or wider, otherwise unified; `+`/`−` gutter text | Long lines wrap through an optional toggle; no forced horizontal page scroll |
| Bottom panel | Tabs: Terminal, Problems, Output (fixture) | Terminal header copy as in concept A; disconnected input disabled. See the open gap in section 3.3. |
| Git view | Worktree identity, Conflicted / Staged / Unstaged groups, commit box, Branch ▾, `Pull · Mock`, `Push · Mock` | Conflict and ambiguous-push states per section 7.1 |
| Status bar | Branch, conflicts, Needs you, Running, connection, Mock; each a button | Disconnected: connection item in danger text "Disconnected 14:02 (12 min)" |

### 7.1 Shared state copy for concept B

| State | Treatment in B | Copy | Actions |
|---|---|---|---|
| Loading | Skeleton rows shaped like the final layout | "Loading tasks…" | Navigation usable; mutating actions disabled |
| Empty (project) | Plain text in the primary area | "No tasks in this project yet." + "Create a task to start a discussion." | `New task · Mock` |
| Empty (decisions) | Same | "Nothing needs your decision. Last checked 14:02 · observed." | Link to all tasks |
| Empty (search) | Same, with the query echoed | "No files match `auth*`. Try fewer characters or turn off Regex." | Clear query |
| Offline / stale cache | Header shows "As of 14:02" with clock glyph | "Showing last confirmed state from 14:02 (12 min ago)." | Read-only; mutating actions disabled with reason |
| Stale result | Warn banner under the task header; acceptance disabled | "Result changed after review: digest 9f2c → 41ab. Previous acceptance (mock) is invalidated." | `Review new result` |
| Disconnected | Connection item danger; banner at top of each primary area; running executions become Unknown | "Machine disconnected. Last confirmed 14:02 (12 min ago). Current states are unknown. Actions are unavailable." | `Reconnect · Mock`; Connection settings |
| Reconnecting | Warn indicator; no spinner after 2 s; static "Reconnecting (attempt 3)" | — | Cancel reconnect |
| Blocked: your permission | Wait reason "Your permission needed" on rows and in Mission Control | "Worker requests permission to run `npm test` in `worktrees/t-0142`." | `Approve once · Mock` / `Deny · Mock` |
| Not permitted (device or capability) | Neutral lock glyph; disabled control with the reason below it | "This device is not authorized to push. Enroll it in Connection settings." | Link to the fix |
| Unavailable capability | circle-slash + "Unavailable"; section still present | "Plan updates unavailable: this agent did not negotiate plan reporting." | — |
| Failed check | Danger x-in-circle "Failed · observed", listed first | "`unit tests` failed · observed · 3 of 412 tests failed" | Open log; `Request changes · Mock` |
| Not run / unverified | Neutral hollow dash or "?"; never green | "`e2e` not run (required)." / "`lint` reported by agent; no receipt (unverified)." | — |
| File conflict (save) | Editor top banner, warn; save disabled | "This file changed on disk since you opened it (agent edit at 14:03)." | `Compare` (opens merge view in group 2) · `Reload` · `Save as…` (never overwrite) |
| Git conflict | Git view top banner, danger; conflicted files first; no auto-stash or reset | "Merge stopped: 2 files have conflicts on `feature/auth`." | Open file · `Mark resolved · Mock` · `Abort merge · Mock` (confirm) |
| Decision version conflict | Inline in the decision | "Answered on Phone at 14:05…" | View outcome (`role="status"`) |
| Ambiguous external outcome (push) | Neutral "?" "Outcome unknown"; **no Retry button** | "Push to `origin/feature/auth` did not confirm. Check the remote before trying again." | `Inspect remote · Mock` |
| Error (typed) | Danger glyph and message near the source; never silent | "Could not load history page 4 (fixture error E_PAGE). Nothing was changed." | `Try again` only for idempotent reads |
| Long text | Titles clamp to two lines in lists only; full text in detail and accessible name. Messages over 12 lines collapse with "Show full message (48 lines)". IDs, paths and URLs wrap anywhere. Decision titles, action labels and errors are never truncated. | — | Expand (button with `aria-expanded`) |

## 8. Decision and result anatomy

### 8.1 Decision panel (same order in all concepts)

1. Type label and glyph: **Question** / **Permission** / **Result acceptance**.
2. Title: one sentence, wraps fully.
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
- **Changes:** file list with change type as text and +/− counts. Diff opens side-by-side in group 2 at 760 px or wider, otherwise unified. The "Open in editor" action opens the file as a new tab in group 1.
- **Acceptance block** sits beside or below the evidence, never above it.

## 9. Mock-state labelling

Non-negotiable in every concept.

1. **Prototype strip:** a persistent 28 px strip above all app chrome. It cannot be dismissed or scrolled away. Text: `PROTOTYPE · Fixture fx-<revision> · Scenario: <name>, step n of m · All actions are mock and change only in-memory state`.
2. **Mutating controls** carry a visible `Mock` tag after the label, for example `Approve permission · Mock`, `Accept result · Mock`, `Commit · Mock`. The accessible name includes "(mock)". Controls: Start or Create task, Answer, Approve, Reject, Accept, Request changes, Cancel execution, Save, Stage, Commit, Pull, Push, Enroll, Revoke and Test connection.
3. **Mock outcome receipts:** an inline result appears where the action was taken. It is not a toast and does not disappear. Examples: `Mock outcome: recorded in memory only. No agent was started.` and `Mock outcome: no artifact was accepted; not runtime evidence.` Acceptance renders as `Accepted (mock) · r-0142.3 · digest 9f2c…`.
4. **No simulated progress:** the fixture never advances on a timer. Steps advance only through the prototype controls and show "Scenario step n of m (fixture)".
5. **Fixture provenance:** fixture facts keep their in-fixture provenance. The status bar shows `Fixture data, not runtime evidence`.
6. **Disconnected fixture:** every mock mutating control is disabled with the reason "Disconnected; actions unavailable".

Interaction invariants for concept B:

- **No automatic navigation.** Activity and tab changes happen only on user action. New events in a timeline scrolled away from the bottom show "12 new events · Jump to latest".
- **Announcements:** decision counts update one `role="status"` region, throttled, without moving focus.
- **No single-key decision actions.** No drag-and-drop status changes.
- **Destructive mock actions** (Reject result, Cancel execution, Push, Revoke device) use a confirmation dialog restating effect and scope. Focus goes to the safe button; Escape cancels.
- **Character-key shortcuts** are active only while their component has focus and can be turned off in Settings → Keyboard. Every action also has a Ctrl-based or menu equivalent.

## 10. Screen coverage

| Screen ([SCREENS.md](../SCREENS.md)) | Where in concept B | Reached by |
|---|---|---|
| Project board and list | Side-panel "Board" (compact list) or Board tab in group 2 | Activity bar; `Ctrl+Shift+B` |
| Create task | "New task" tab in group 2 with the same fields as concept A, plus optional pre-filled context from selection (proposal) | `Ctrl+Shift+N`; Mission Control |
| Task discussion and plan | Task tab → "Discussion & plan" sub-tab. Plan is a collapsible section above the timeline at 476 px, and a right rail at 1100 px or wider | Mission Control |
| Worker detail | Task tab → "Workers" sub-tab; worker row opens detail inline | Sub-tab; timeline link |
| Decisions | Mission Control "Needs you" and Task tab "Decisions" sub-tab | Panel; status bar |
| Evidence and diff | Task tab "Result & diff"; full diff can open as a dedicated Diff tab in group 2 | Sub-tab |
| File tree, editor and search | Files and Search side-panel views with editor group 1 | `Ctrl+Shift+E` / `Ctrl+Shift+F` / `Ctrl+P` |
| Terminal | Bottom panel (one group) or a group-2 tab (two groups) | `` Ctrl+` `` |
| Git | Git side-panel view; diffs in group 2 | `Ctrl+Shift+G`; status bar |
| Connection settings | Settings tab → Connection | Activity bar; status bar |

Create-task success state is not specified for this concept beyond the shared mock-outcome rule (section 9): the prototype shows the mock receipt where the action was taken and execution as "Not started (mock)".

## 11. Scenarios

Run identically in all three concepts at 1280×800 and 1920×1080, with pointer and keyboard, and at 100 % and 200 % text. The scenario definitions S1–S6 are written out in [concept A](desktop-concept-a.md) section 11; the scenario controls are in [prototype fixtures](prototype-fixtures.md).

- **S1 Create → decision → result.** Start in `src/auth/limiter.ts` in group 1. `Ctrl+Shift+N` opens the new task tab in group 2 while the code stays in group 1. Create · Mock. Advance: the status bar shows "Needs you 1" and Mission Control's count updates; nothing opens. `Alt+D` focuses the decision row; Enter opens it in the task tab's Decisions sub-tab; answer. Then permission, then result. Diff opens in group 2; "Open in editor" opens a tab in group 1. Accept · Mock; the Git view shows the changes still unstaged.
- **S2 Code / Git conflict.** Edit in group 1, `Ctrl+S`, conflict banner, Compare opens the merge view in group 2. Git view lists the conflicted files; resolve in group 1, mark resolved (mock), and leave the other conflicted.
- **S3 State sweep:** as in concept A (empty, blocked, failed check, stale, disconnected, unavailable capability, long text, 500-task scroll, 10,000-event timeline).
- **S4 Keyboard only** and **S5 Increased text size** (150 % and 200 %): as in concept A. At 200 % the side panel is collapsed manually.
- **S6 Desktop → phone → desktop:** as in concept A.

**Validation focus.** Editor group 1 is visible in every S1 and S2 step at both viewports. The disconnected fixture disables every mock mutation.

## 12. Accessibility baseline

- Text pairs at least 4.5:1 (most at least 6:1 per section 4.2). Focus rings and control boundaries at least 3:1. Disabled labels are exempt; disabled reason text uses the muted role.
- Pointer targets at least 24 × 24 CSS px, including dense tree rows.
- Landmarks: `header`, `nav`, labelled `main` for each primary area, `contentinfo` for the status bar. Tree is `role="tree"`. Decisions are `<section>` elements with `h2`.
- All sizes in rem from a root set by the text-size preset. Line heights are unitless. Containers are content-height.
- Skip link "Skip to main work area". `Ctrl+1` and `Ctrl+2` focus groups; F6 cycles regions in visual order.
- `?` or `F1` opens the keyboard reference.
- Dialogs trap and restore focus. Focus never moves because of incoming data.
- Motion: panels 0–80 ms, opacity only; reduced motion sets durations to 0.

## 13. Concept-specific risks and open questions

- **Two-area rule.** The terminal-as-tab rule may surprise IDE users. The report marks it as a deliberate deviation from VS Code convention for owner review. The Problems and Output placement with two groups is unresolved (section 3.3).
- **Dark only.** A light variant and OS high-contrast mode are owner decisions.
- **200 % text** makes the layout dense. The side panel must be collapsed manually.
- **Shortcut conflicts.** `Ctrl+W`, `Ctrl+P` and `Ctrl+\` may collide with WebKitGTK or Tauri defaults. Verify against the installed Tauri 2 and CodeMirror 6 versions; the prototype may need to suppress webview defaults.
- **Optional proposal.** "New task from selection…" is a proposal, not a requirement; owner to keep or drop it for comparison.
- **Focus versus selection.** Focus (white ring) and selection (blue border) must be confirmed distinguishable in rendered screenshots.
- Rendered review, keyboard runs and screenshots for this concept do not yet exist.
