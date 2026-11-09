# Desktop concept B walkthrough: IDE with Mission Control

**Status: clickable mock prototype; candidate direction pending owner Gate D.** Nothing here is runtime evidence, native qualification, a Gate D selection or an approved design system. Specification: [desktop concept B](desktop-concept-b.md). Shared fixtures: [prototype fixtures](prototype-fixtures.md). Prototype setup: [desktop prototype README](../../../prototypes/cortex-workspace/desktop/README.md).

## What was built

Concept B lives in `prototypes/cortex-workspace/desktop/src/concepts/b/` and is served at `/b`. It uses the shared fixture loader and mock store (`cw-fixtures-v1`, seed 13014) without changing them.

- **Code first.** The route opens on the Files view with `src/auth/limiter.ts` in editor group 1 and the terminal in the bottom panel. Nothing opens automatically.
- **Mission Control** is a side-panel view with Needs you (Question, Permission, Result acceptance), Running, Changed since last visit and Recent tasks, plus Open board, New task and Project history links. A single click previews a task in group 2 (italic tab); Enter or double-click pins it.
- **Group 2** holds task tabs (Discussion & plan, Decisions, Result & diff, Workers), Board, New task, Diff, Compare (merge), Settings → Connection, Project history, and Terminal/Problems/Output when two groups are visible. Group 1 keeps the code in every flow.
- **Other screens:** Files tree, Search (literal and regex, empty and invalid-regex states), Git (Conflicted/Staged/Unstaged, commit box, branch, Pull/Push), Settings (text size 100/125/150/200 %, character-key shortcuts, board layout), command palette (`Ctrl+K`), quick open (`Ctrl+P`) and keyboard reference (`F1`/`?`).
- **States:** loading (simulated toggle with skeleton rows), empty, error (typed message plus reset), blocked permission, failed check, stale result, offline/cached, disconnected, file conflict, Git conflict, unavailable capability, long text, ambiguous push outcome.
- **Mock labelling.** A persistent prototype strip states the fixture revision, fixture state, script step and "All actions are mock and change only in-memory state". Every mutating control carries a visible `· Mock` tag and "(mock)" in its accessible name. Each mock action leaves an inline receipt that does not disappear. The status bar always shows "Fixture data, not runtime evidence" and "Mock".
- **Disconnected and offline.** Every mutating mock control is disabled with the reason "Disconnected; actions unavailable" (or the offline equivalent). Only `Reconnect · Mock` stays enabled; it records a receipt saying no reconnect was attempted and changes nothing.
- **No implied real effects.** Creating a task records "no execution started. No agent was started" and shows "Not started (mock)". Accepting records "Mock acceptance recorded in memory. Not runtime evidence or a Git action", the footer says "Accepting does not commit, push or deploy", and the Git view still lists the changes as unstaged. A stale result cannot be accepted.

While `/b` is mounted, the concept takes the whole window and hides the shared shell header with a CSS rule scoped to its own root; its prototype strip carries the mock statement and a "Prototype controls" panel with the fixture-state selector, loading toggle, store reset, concept links and the shared scenario scripts. Leaving `/b` restores the shell.

## Decisions and deviations for the owner

| Topic | Choice in this prototype | Reason |
|---|---|---|
| Problems and Output with two editor groups (open gap in the spec) | They open as tabs in group 2, like the terminal. With one group they stay in the bottom panel. | Keeps at most two primary areas and never replaces group 1. Needs owner decision. |
| Editor | Plain-text editor (textarea with line gutter), not CodeMirror 6 | CodeMirror is not among the shared prototype dependencies; adding it is a shared change. Tab leaves the editor. |
| File bodies, result file lists, diff bodies, terminal transcript | Concept-local synthetic data in `data.ts`, labelled in the UI | The shared fixture has no workspace file bodies or result file lists. Task, decision, check, result, conflict and Git facts all come from the shared fixture. |
| Git conflict | One conflicted file (`synthetic/layout.ts`) | The shared fixture has one conflict file, so "resolve one, leave the other conflicted" cannot be shown. Commit stays disabled while it is unresolved and then because nothing is staged. |
| Board phases | Fixture values backlog / working / review / done | Candidate phase names are not in the fixture. |
| Reserved browser shortcuts | `Ctrl+PageDown`/`PageUp` and `Ctrl+F4` alternatives, plus the command palette | Chrome reserves `Ctrl+Tab`, `Ctrl+W` and `Ctrl+Shift+N`. In the Playwright run they reached the page (see Limits). |
| Prototype strip height | 28 px at 100 % text; it wraps to three lines at 200 % | The strip cannot be dismissed. |
| Landmarks | Groups are labelled regions inside the shell's single `main`; the status bar is a toolbar | Nested `main` and `contentinfo` are not valid inside the shell's `main`. |

## Environment

- Linux x86_64, Node v24.14.1, npm 11.11.0; SolidJS 1.9.18, Vite 8.3.5, Vitest 5.1.0 from the locked shared manifest.
- Browser: Playwright MCP with Chromium "HeadlessChrome/156.0.0.0" (user agent reported by the page), viewports set with `setViewportSize` to 1280×800 and 1920×1080, CSS-pixel screenshots.
- Dev server: `npm run dev -- --mode b` in `prototypes/cortex-workspace/desktop`, strict port 5182, URL `http://127.0.0.1:5182/b`. HTTP 200 observed. The server was stopped after the run and port 5182 confirmed closed.

## Commands and outcomes

All from the repository root unless noted.

| Command | Exit | Outcome |
|---|---|---|
| `npm --prefix prototypes/cortex-workspace/desktop run typecheck` | 0 | No type errors |
| `npm --prefix prototypes/cortex-workspace/desktop test` | 0 | 5 files, 39 tests passed, including 6 concept B tests |
| `npm --prefix prototypes/cortex-workspace/desktop run build` | 0 | Separate lazy `b` chunk (115.24 kB JS, 21.10 kB CSS) |

Concept B tests (`src/concepts/b/b.test.tsx`) cover: start on the editor and open Mission Control, the same task's permission, result and board beside the code; the disconnected fixture leaving every mock control disabled with no store change; mock-only acceptance of a complete result and a blocked stale result; viewport-bounded rendering of 500 tasks and 10,000 events; file and Git conflict without a real resolution; and the terminal two-area rule.

## Scenario outcomes

**S1 create → decision → result, 1280×800, keyboard only.** Skip link → editor. `Ctrl+Shift+N` opened New task in group 2; typed title, Enter created it (receipt: no agent started; "Not started (mock)"). `Alt+D` focused the Question row; Enter opened Decisions; Tab to the answer, Enter on `Send answer · Mock`. `Alt+D` moved to the Permission row; `Approve once · Mock` recorded "No authorization was granted". `Ctrl+K` palette opened the failed-check task; arrows moved to Result & diff; `Request changes · Mock`. `Alt+D` opened the result acceptance; Open diff showed a unified diff in group 2 (476 px pane); Open in editor opened `tests/limiter.test.ts` as a new tab in group 1; `Accept mock-result-complete · Mock`; `Ctrl+Shift+G` showed both files still unstaged. Group 1 code was visible in every step. **Passed.**

**S1 at 1920×1080, pointer.** Same flow by clicking; preview tab (italic) from a single click, pin by double-click; side-by-side diff in the 786 px group 2. **Passed.**

**S2 code / Git conflict, 1280×800 keyboard and 1920×1080 pointer.** Fixture state `conflict` set from Prototype controls by keyboard (select + arrow keys). Git view: "Merge stopped: 1 file has conflicts on `synthetic-layout`", Commit disabled. Quick open `layout`, edit, `Ctrl+S` → save-conflict banner (Save disabled; Compare, Reload, `Save as… · Mock`). Compare opened base/local/incoming in group 2 while group 1 kept the file; `Keep local · Mock`; `Mark resolved · Mock` with a receipt; Commit stayed disabled. **Passed**, with the one-file fixture limit above.

**S3 state sweep, both viewports.** Empty, error, loading, blocked permission, stale result (both digests shown in full, accept disabled, "Previous acceptance (mock) is invalidated"), offline/cached, disconnected (banners in each primary area, Unknown running states, last confirmed 2030-01-01 00:00 UTC), board, history, search, connection settings with the unavailable capability, terminal. **Passed.** In the disconnected state at 1280×800 a DOM query found 5 mock-labelled buttons, of which only the two `Reconnect · Mock` buttons were enabled.

**S4 keyboard only.** Covered by S1 and S2 at 1280×800. `F6` cycled status bar → activity bar → group 1 → group 2 (side panel hidden), focus was visible on every stop in the inspected screenshots. Focus moves to the inline receipt when the control used disappears or becomes disabled. **Passed** in Chromium only.

**S5 increased text size, 1280×800.** 150 % and 200 % through Settings → Text size (root font 24 px and 32 px). At 200 % with the side panel hidden (`Ctrl+B`) both groups are 592 px; with the side panel open they are 400 px and the task tab switches to the compact layout (section select). `Approve once · Mock` was reached by Tab and scrolled into view; the receipt appeared. Document scroll width stayed 1280 (no page-level horizontal scroll). Group 1 code stayed visible in every capture. **Passed**, with the dense-layout note under Limits.

**S6 desktop → phone → desktop.** The shared `desktop-phone-desktop` script can be stepped from Prototype controls (apply step simulates the phone answer; Changed since last visit then lists it and the decision shows "Answered on Phone (simulated)"). **Not run** in the browser; covered only by the code path, so unverified.

## Performance observations

Method: Playwright `browser_evaluate` in the 1280×800 Chromium page, development build, default fixture.

| Observation | Value |
|---|---|
| Project history list (10,000 events): option rows in the DOM | 29 at 1280×800, 39 at 1920×1080 (`aria-setsize` 10000) |
| Same list, scrolled top to bottom over 120 animation frames | frame interval median 16.7 ms, p95 17.3 ms, max 17.7 ms, 0 frames over 33.4 ms |
| Board columns at 1920×1080 | Backlog 496 tasks with 20 rows rendered (measured at a 4 rem row height, before the final 4.75 rem); other columns 1–2 |
| Side Board list (500 tasks) | 16 rows rendered; opened and painted within 15.4 ms (two frames) |
| Mission Control, open task tab, open history (click to second frame) | 33.2 ms, 33.3 ms, 33.4 ms (two-frame floor) |
| Editor input event to next frame, 20 samples | median 16.7 ms, max 16.8 ms |
| Whole document elements; JS heap | 520 elements; 20 MB used heap |

These are browser-page timings on a development build, quantised to frame boundaries. They are not the process-launch, physical-device or RSS measurements that [performance targets](../performance.md) require.

## Screenshots

Saved under `.design-evidence/cortex-workspace/desktop/b/` (git-ignored). Every file below was opened and inspected. All are CSS-pixel PNGs.

| File | Size | Shows |
|---|---|---|
| 1280-s1-01-start-editor.png | 1280×800 | Start: Files and editor, focus in editor |
| 1280-s1-02-new-task-form.png | 1280×800 | New task tab in group 2, title typed |
| 1280-s1-03-created-mock-receipt.png | 1280×800 | Mock create receipt, Not started (mock) |
| 1280-s1-04-altd-focus-decision.png | 1280×800 | `Alt+D` focus on the Question row |
| 1280-s1-05-question-decision-keyboard.png | 1280×800 | Question decision, answer radio focused |
| 1280-s1-06-question-answered-receipt.png | 1280×800 | Responded (mock), receipt focused |
| 1280-s1-07-permission-decision.png | 1280×800 | Permission anatomy, Approve focused |
| 1280-s1-08-permission-approved-mock.png | 1280×800 | Permission receipt |
| 1280-s1-09-command-palette.png | 1280×800 | `Ctrl+K` palette |
| 1280-s1-10-failed-check-result.png | 1280×800 | Failed check, no result revision |
| 1280-s1-11-request-changes-mock.png | 1280×800 | Changes requested (mock) |
| 1280-s1-12-result-evidence.png | 1280×800 | Result header, checks, changes |
| 1280-s1-13-diff-in-group2.png | 1280×800 | Unified diff in group 2 |
| 1280-s1-14-open-in-editor-group1.png | 1280×800 | `tests/limiter.test.ts` opened in group 1 |
| 1280-s1-15-accept-focused.png | 1280×800 | Acceptance block, Accept focused |
| 1280-s1-16-accepted-mock.png | 1280×800 | Accepted (mock), footer, receipt |
| 1280-s1-17-git-still-unstaged.png | 1280×800 | Git view after acceptance: unstaged |
| 1280-s2-01-prototype-controls-conflict.png | 1280×800 | Prototype controls, conflict state, script |
| 1280-s2-02-git-conflict-view.png | 1280×800 | Git conflict banner |
| 1280-s2-03-edit-unsaved.png | 1280×800 | Edited conflict file, Unsaved |
| 1280-s2-04-save-conflict-banner.png | 1280×800 | Save-conflict banner |
| 1280-s2-05-compare-merge-group2.png | 1280×800 | Compare in group 2 |
| 1280-s2-06-keep-local-mock.png | 1280×800 | Keep local (mock) receipt |
| 1280-s2-07-marked-resolved-commit-disabled.png | 1280×800 | Marked resolved (mock); Commit disabled |
| 1280-state-empty.png | 1280×800 | Empty project |
| 1280-state-error.png | 1280×800 | Typed fixture error with reset |
| 1280-state-loading.png | 1280×800 | Loading skeleton, actions disabled |
| 1280-state-blocked-permission.png | 1280×800 | Blocked permission |
| 1280-state-stale-result.png | 1280×800 | Stale result, full digests, accept disabled |
| 1280-state-disconnected.png | 1280×800 | Disconnected, actions disabled |
| 1280-state-offline-cached.png | 1280×800 | Offline cached state |
| 1280-screen-board-list.png | 1280×800 | Board tab in List mode (476 px) |
| 1280-screen-history-10000.png | 1280×800 | 10,000-event history |
| 1280-screen-search-results.png | 1280×800 | Search results |
| 1280-screen-search-empty.png | 1280×800 | Search empty state with query echoed |
| 1280-screen-connection-settings.png | 1280×800 | Settings → Connection |
| 1280-screen-terminal-group2-tab.png | 1280×800 | Terminal as a group-2 tab |
| 1280-screen-discussion-plan.png | 1280×800 | Discussion & plan |
| 1280-screen-worker-detail.png | 1280×800 | Worker detail inline |
| 1280-screen-keyboard-reference.png | 1280×800 | Keyboard reference dialog |
| 1280-text150-01-permission-task.png | 1280×800 | 150 % text, compact task layout |
| 1280-text200-01-settings-text-size.png | 1280×800 | 200 % text, one group with bottom panel |
| 1280-text200-02-created-side-hidden.png | 1280×800 | 200 %, side hidden, task created |
| 1280-text200-03-permission-compact-task.png | 1280×800 | 200 %, permission task |
| 1280-text200-04-side-open-compact-task.png | 1280×800 | 200 %, side open, compact section select |
| 1280-text200-05-approve-focused-scrolled.png | 1280×800 | 200 %, Approve focused and scrolled into view |
| 1280-text200-06-approved-receipt.png | 1280×800 | 200 %, receipt |
| 1280-text200-07-f6-region-focus.png | 1280×800 | 200 %, F6 focus on group 2 |
| 1920-s1-01-start-editor.png | 1920×1080 | Start |
| 1920-s1-02-new-task-form.png | 1920×1080 | New task form |
| 1920-s1-03-created-mock-receipt.png | 1920×1080 | Create receipt |
| 1920-s1-04-question-preview-tab.png | 1920×1080 | Preview (italic) task tab |
| 1920-s1-05-question-answered.png | 1920×1080 | Question answered (mock) |
| 1920-s1-06-permission-approved.png | 1920×1080 | Permission approved (mock) |
| 1920-s1-07-failed-check-request-changes.png | 1920×1080 | Failed check, changes requested |
| 1920-s1-08-result-evidence.png | 1920×1080 | Result evidence and acceptance block |
| 1920-s1-09-diff-side-by-side.png | 1920×1080 | Side-by-side diff in group 2 |
| 1920-s1-10-open-in-editor-and-accepted.png | 1920×1080 | File in group 1, Accepted (mock) |
| 1920-s1-11-git-still-unstaged.png | 1920×1080 | Git still unstaged |
| 1920-s2-01-git-conflict-view.png | 1920×1080 | Git conflict |
| 1920-s2-02-save-conflict-banner.png | 1920×1080 | Save-conflict banner |
| 1920-s2-03-compare-keep-local.png | 1920×1080 | Compare, keep local (mock) |
| 1920-s2-04-marked-resolved-receipt-focused.png | 1920×1080 | Marked resolved, receipt focused |
| 1920-state-empty.png | 1920×1080 | Empty |
| 1920-state-error.png | 1920×1080 | Error |
| 1920-state-loading.png | 1920×1080 | Loading |
| 1920-state-blocked-permission.png | 1920×1080 | Blocked permission |
| 1920-state-stale-result.png | 1920×1080 | Stale result |
| 1920-state-disconnected.png | 1920×1080 | Disconnected |
| 1920-state-offline-cached.png | 1920×1080 | Offline cached |
| 1920-screen-board-columns.png | 1920×1080 | Board with four phase columns |
| 1920-screen-history-10000.png | 1920×1080 | 10,000-event history |
| 1920-screen-search-and-connection.png | 1920×1080 | Search and Settings → Connection |
| 1920-screen-terminal-bottom-one-group.png | 1920×1080 | Terminal in the bottom panel with one group |

Captures 1920-s1-08, 1920-s1-10 and 1920-s1-11 predate a later change that shows fixture digests in full (they show `mock-digest-am…`); the stale-result captures were retaken after it.

## Defects found and fixed during the run

Shell label styles centred form labels; the compact section select was focusable while hidden; focus fell to the page body after a mock action or after opening a diff; receipts for one file appeared on another; the active group-2 tab scrolled out of view; table cells broke words; the side-by-side diff overlapped and then wasted width; status-bar tone colours were overridden; board cards clipped wait reasons; digests were truncated to identical prefixes; at 200 % text the bottom panel covered group 1. Each was fixed and the affected state recaptured.

## Limits and unrun checks

- **Native Tauri, WebKitGTK and physical devices:** not run. Only Chromium through Playwright was used; Tauri/WebKitGTK shortcut handling (`Ctrl+W`, `Ctrl+P`, `Ctrl+\`, `Ctrl+Tab`), focus rendering and text zoom remain unverified. In Playwright, `Ctrl+Shift+N`, `Ctrl+P`, `Ctrl+1`/`Ctrl+2` and `Alt+D` reached the page; a desktop Chrome window may reserve some of them.
- **Screen readers:** not run. Labels, roles and live regions were set but no assistive-technology session was used.
- **S6 desktop → phone → desktop:** not exercised in the browser.
- **Contrast:** colours follow the specification's measured pairs; rendered contrast was not measured.
- **200 % text:** usable, but the strip and status bar wrap and leave about 560 px of work height; collapsing the side panel is needed for comfortable widths.
- **Performance:** see the method note above; no process-launch, RSS or device timing.
- **Fixture gaps:** one Git conflict file; no plan entries for some tasks (each task's synthetic events share one kind); no produced-at times, durations or elapsed time, shown as unknown.
