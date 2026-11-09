# Desktop direction walkthrough: Project Desk prototype

**Status: candidate direction, pending owner Gate D.** This records the rendered checks of the clickable mock that implements [the desktop direction](desktop-direction.md). It is mock-only evidence: fixture data, no runtime, no native build. Nothing here approves the direction.

## Summary

- The prototype serves the direction on `/` and `/a`; round-one B and C (and A) remain reachable only under `/archive/…`.
- Rendered with the Playwright MCP browser (headless Chromium, user agent `HeadlessChrome/156.0.0.0`, Linux x86_64, DPR 1) at 1280×800 and 1920×1080, in dark and light themes, keyboard-only and at 200% interface text.
- Scenarios run: all-projects dashboard → project → board → task → decision → result/diff → code/Git, create task, and Git conflict. All passed after the fixes listed below.
- `typecheck`, `test` (6 files, 52 tests) and `build` pass.
- Not verified: headed-browser shortcut capture, screen readers, native Tauri window, numeric contrast on rendered pixels, 125%/150% presets, forced colours and `prefers-contrast`.

## Environment

| Item | Value |
|---|---|
| Prototype | `prototypes/cortex-workspace/desktop` (Solid 1.9.18, Vite 8.3.5, Vitest 5.1.0, jsdom 29.1.1, TypeScript 6.0.3) |
| Node / npm | Node 24.14.1 |
| Browser | Playwright MCP, headless Chromium (`HeadlessChrome/156.0.0.0`), viewports set with `browser_resize` |
| Servers | `vite --host 127.0.0.1 --port 5191` (dev) and `vite preview --host 127.0.0.1 --port 5192` (production build); both stopped after use |
| Fixture | `cw-fixtures-v2`, seed 13014; fixture clock 01 Jan 2030 02:46 UTC |
| Theme selection | `localStorage["cw.theme"]` set to `dark` or `light` before load; one capture used the headless browser's system preference (light) and is named accordingly |

## Commands

Run from the repository root.

| Command | Exit | Result |
|---|---|---|
| `npm --prefix prototypes/cortex-workspace/desktop run typecheck` | 0 | `tsc --noEmit`, no errors |
| `npm --prefix prototypes/cortex-workspace/desktop test` | 0 | 6 files, 52 tests passed (fixtures 18, archived A 6, B 6, C 10, App 1, Project Desk 11) |
| `npm --prefix prototypes/cortex-workspace/desktop run build` | 0 | production build; main chunk 197 kB (59 kB gzip), archived concepts in separate lazy chunks |

New focused tests (`src/desk/desk.test.tsx`): theme default, live system following and explicit override; theme persistence across a prototype Reset and a remount; dashboard content and honest unknown/empty values; empty copy never contradicting counters in loading, error and empty scenarios; project landing (Board first, then last-used view); Overview stats from aggregates with Unknown workers on the disconnected machine; board "No tasks" only at count 0 and column virtualisation; create task with inert background, inline title error, mock outcome and navigation; disconnected project blocking create with a visible reason and focus return; permission decision recorded as mock without authorization; stale result acceptance disabled.

## Scenario walkthroughs

### Main path (keyboard only, dark, 1280×800)

1. `/` loaded; first Tab shows "Skip to navigation" with a visible ring (`kbd-01`).
2. Tab order: skip links → New task → Search → Needs you → All projects → Sort → project tree (one Tab stop). ↓ moved to Cedar Notes, → expanded it (`kbd-02`), Enter opened the board.
3. Tab continued through resize handle → Connections → Theme → Settings → Help → sidebar toggle → breadcrumb → search → Mock chip → More → view tabs (one stop) → Filter → board (one stop).
4. Arrow keys moved card focus; Enter on `#0002` opened its Decisions tab with focus on the task title heading (`kbd-03`, `kbd-04`).
5. Tab reached Copy reference → Decisions tab → Deny → Approve once. Enter on Approve once recorded the mock response; the buttons are replaced by "Response recorded (mock)… execution facts are unchanged" plus the receipt "No authorization was granted" (`kbd-05`). Sidebar and dashboard needs-you counts dropped to 1.
6. `Ctrl+Alt+3` switched to Result. On Cedar `#0006` the Accept button was focused by script and activated with Enter; the status reads "Accepted (mock) · mock-result-complete · digest mock-digest-amber-v1" and focus moved to the task heading, not `<body>`.
7. Code and Git were opened by URL (`/p/amber-components/t/0007/code` and `/git`); `` Ctrl+` `` toggled the fixture terminal (`s8`). `Ctrl+Alt+5` and `Ctrl+Alt+6` were not pressed in this run.

Every focus event in steps 1–5 was logged with its computed style: all had `outline: solid 2px` and matched `:focus-visible`, except the sidebar resize handle (`outline: none`), which was fixed.

### Create task (dark and light, 1280×800)

`Ctrl+Alt+N` on the Cedar Notes board opened the native modal with focus in Title; the background `#app` carried `inert`. Enter on an empty title showed "Enter a title" under the field (`flow-create-1`). A long title plus Enter closed the dialog and opened `#0501` with "Not started (mock)" and the receipt "Mock outcome: task recorded in memory only. No agent was started." (`flow-create-2`). Going back showed the new card at the end of Backlog (90 tasks) with a "Mock" tag (`flow-create-3`). Back was triggered with `history.back()`, because headless Chromium does not act on `Alt+←`.

### Git conflict (dark, 1280×800)

Amber Components `#0007` → Git: the danger notice "Merge stopped: 1 file has conflicts on synthetic-layout", Base / Yours / Incoming hunks, Commit disabled with "Resolve 1 remaining conflict". Keep yours · Mock recorded "Marked resolved with yours (mock). No file or Git mutation occurred." and enabled Commit once a message was typed. Push · Mock opened a confirmation with Cancel focused (`flow-git-1`); three Tab presses stayed inside the dialog; `dialog:modal` and `#app[inert]` were both true. Confirming showed "Outcome unknown" with "Inspect remote · Mock" and no Retry, and focus returned to the Push button (`flow-git-2`). The fixture fact is unchanged, so the task chip still reads Conflict.

### States (dark, 1280×800)

Offline (`s1`), Empty (`s2`), Error (`s3`, `s4`), Loading (`s6`) and the prototype popover (`s5`) were selected through the palette or the Mock chip. Empty copy appears only with a loaded zero count; Error shows the error with "Try again" and "—" for unavailable values; Loading shows skeletons with final dimensions. Disconnected (Mosaic Studio) is shown on the Overview, Machine/Settings and board in every capture set; permission-blocked is the Cedar decision; failed check and stale result are Quartz; conflict is Amber; long text is Willow `#0009`.

### 200% interface text (both themes, 1280×800)

`cw.textSize = 200` (root 32px). No document-level horizontal scroll (`scrollWidth` 1280). The board scrolls horizontally inside its region, the view-tab strip scrolls, labels ellipsise with full names in accessible names, and decision titles wrap fully (`t200-*`).

## Defects found during the walkthrough and fixed

1. Opening a project looped updates (the last-view effect read and wrote the same preference). Fixed by reading untracked and writing only on change.
2. Opening a task looped updates (task selection re-ran on every store change). Fixed by keying on the task id.
3. Recording a decision removed the focused button and dropped focus to `<body>`. Focus now moves to the recorded status; a guard moves focus to the view heading whenever the focused element is removed.
4. Resize and terminal handles suppressed the outline. They now show the 2px ring.
5. The palette input showed a clipped box ring; it now has a 2px accent rule as its focus indicator.
6. In light theme, the waiting-on-you and selected card borders were overridden; fixed.
7. At 200% text the sidebar truncated every label and its footer overlapped the main area, and the search box covered the breadcrumb. The default sidebar width now scales with text size (capped at 30% of the window), the footer wraps, ancestors collapse to "…" when space is short, the task list pane is capped, and icons scale with text.
8. Column error notices wrapped word by word in narrow columns; the action now drops below the text.
9. The 1920 dashboard Tasks column wrapped to three lines; columns rebalanced.
10. In the Error scenario the dashboard said "No activity"; it now shows "—" (unavailable).
11. Count chips stretched across table cells; the select chevron was hidden in dialogs; the archived concept B covered the archive banner. All fixed.
12. Enter on an empty title in New task did nothing (implicit submission is blocked by the disabled button); Enter now shows the inline error.

## Performance (method and observations)

Measured in the dev server with `browser_evaluate`, timing with `performance.now()` and `requestAnimationFrame`; headless, so frame times are capped at the 60 Hz timer and are indicative only.

| Observation | Value |
|---|---|
| Initial navigation, DOMContentLoaded / load | 37 ms / 41 ms |
| Route change to Mosaic Studio board (219 Backlog cards), to second frame | 6 ms; 11 card nodes in the DOM |
| 60 frames scrolling that column by 400px each | 16.7 ms average, 17.6 ms max; 15 card nodes |
| All activity (10,000 events) opened | 49 ms; 14 rows in the DOM |
| 60 frames scrolling it by 4,000px each | 16.7 ms average, 18.2 ms max; 19 rows |
| Mosaic Studio list (220 rows) route change | 32 ms; 23 rows in the DOM |
| Whole document after these steps | 510 elements |

A production-build tour of 22 routes (all views, task tabs, settings, not-found) logged no console errors.

## Screenshots

All under `.design-evidence/cortex-workspace/desktop/v2/` (git-ignored), PNG at CSS scale. Every file listed was opened and inspected.

| File | Size |
|---|---|
| `a-dashboard-{dark,light}-1280x800.png` | 1280×800 |
| `a-dashboard-{dark,light}-1920x1080.png` | 1920×1080 |
| `b-overview-mosaic-disconnected-{dark,light}-1280x800.png` | 1280×800 |
| `b-overview-mosaic-disconnected-{dark,light}-1920x1080.png` | 1920×1080 |
| `c-board-cedar-{dark,light}-1280x800.png` | 1280×800 |
| `c-board-cedar-{dark,light}-1920x1080.png` | 1920×1080 |
| `d-decisions-cedar-{dark,light}-1280x800.png` | 1280×800 |
| `d-decisions-cedar-{dark,light}-1920x1080.png` | 1920×1080 |
| `e-result-stale-{dark,light}-1280x800.png` | 1280×800 |
| `e-result-stale-{dark,light}-1920x1080.png` | 1920×1080 |
| `f-git-conflict-{dark,light}-1280x800.png` | 1280×800 |
| `f-git-conflict-{dark,light}-1920x1080.png` | 1920×1080 |
| `g-create-dialog-{dark,light}-1280x800.png` | 1280×800 |
| `g-create-dialog-{dark,light}-1920x1080.png` | 1920×1080 |
| `h-settings-connection-{dark,light}-1280x800.png` | 1280×800 |
| `h-settings-connection-{dark,light}-1920x1080.png` | 1920×1080 |
| `kbd-01-skip-link`, `kbd-02-tree-expanded`, `kbd-03-board-card-focus`, `kbd-04-task-heading-focus`, `kbd-05-decision-mock-receipt` (`-dark-1280x800.png`) | 1280×800 |
| `flow-create-1-title-error-light-1280x800.png` | 1280×800 |
| `flow-create-2-workspace-receipt-dark-1280x800.png`, `flow-create-3-board-new-card-dark-1280x800.png` | 1280×800 |
| `flow-git-1-push-confirm-dark-1280x800.png`, `flow-git-2-outcome-unknown-dark-1280x800.png` | 1280×800 |
| `s1-offline-dashboard`, `s2-empty-dashboard`, `s3-error-dashboard`, `s4-error-board`, `s5-prototype-popover`, `s6-loading-board`, `s7-list-mosaic`, `s8-code-terminal`, `s9-palette`, `s10-help-dialog` (`-dark-1280x800.png`) | 1280×800 |
| `s11-archived-b-1280x800.png` | 1280×800 |
| `t200-{dashboard,board,decisions}-{dark,light}-1280x800.png` | 1280×800 |

Capture timing notes: the `t200-*` captures predate the breadcrumb "…" collapse (verified afterwards in the DOM: only the current crumb remains at 200%); `g-create-dialog-light-1280x800.png` predates the light waiting-card border fix (visible behind the scrim); `s2-empty-dashboard` and the `kbd-*` files predate the heading-ring width fix. All later behaviour was re-checked in the browser.

## Not verified and remaining risks

- **Shortcuts in a headed browser and on GNOME.** Only headless Chromium was used. `Alt+←` did not navigate there; `Ctrl+Alt+N`, `Ctrl+Alt+3`, `Ctrl+K`, `` Ctrl+` `` and `?` reached the page. F6 region cycling is not implemented; skip links are the path.
- **Screen readers.** Not run. ARIA was inspected through DOM queries only.
- **Board semantics.** The board uses labelled lists with a single roving Tab stop instead of `role="grid"`, because independently scrolling virtualised columns cannot form a row-major grid. Positions are announced as "Backlog, 3 of 212".
- **Tooltips** use native `title` attributes, so the 500 ms delay and Esc dismissal from the direction are not implemented; no essential information depends on them.
- **Rendered contrast** was judged visually only; computed token ratios are in the direction document.
- **Not exercised:** 125% and 150% presets, reduce-motion setting, forced colours, `prefers-contrast: more`, pointer drag of the resize handles, Prototype → Next step, and the simulated phone answer.
- **Wording.** "Offline" marks a disconnected machine in the sidebar (as the direction specifies) and also the device-offline scenario in the sidebar footer.
- **Native shell.** Tauri, WebKitGTK rendering and window controls are out of scope; Rust is not installed.
