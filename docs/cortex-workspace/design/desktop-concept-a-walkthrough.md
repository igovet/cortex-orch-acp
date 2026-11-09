# Desktop concept A walkthrough: Task Desk

**Status: clickable mock prototype of a candidate direction pending owner Gate D.** Nothing here is runtime evidence, native qualification or an approved design. Every action in the prototype changes only in-memory mock state. See the [concept A specification](desktop-concept-a.md), [desktop concepts](desktop-concepts.md) and [prototype fixtures](prototype-fixtures.md).

## Where it lives and how to run it

Source: `prototypes/cortex-workspace/desktop/src/concepts/a/` (route `/a`). It uses the shared fixture loader and mock store (`src/fixtures.ts`, `src/store.ts`) and the scenario scripts in `src/scenarios.ts` without changing them.

```sh
cd prototypes/cortex-workspace/desktop
npm ci
npm run typecheck
npm test
npm run build
npm run dev -- --mode a   # http://127.0.0.1:5181/a
```

The desk sits below the shared prototype chrome (mock notice, concept links, scenario selector, reset). Scroll down once, or press Tab from the top, to reach it; at normal text size it fills one window height.

## Environment

| Item | Value |
|---|---|
| OS | Linux 7.0.0-42-generic x86_64 |
| Node / npm | v24.14.1 / 11.11.0 |
| Locked packages | SolidJS 1.9.18, TypeScript 6.0.3, Vite 8.3.5, vite-plugin-solid 2.11.15, Vitest 5.1.0, jsdom 29.1.1 |
| Browser | Headless Chrome 156.0.0.0 (Linux x86_64), driven by the Playwright MCP tools; device pixel ratio 1 |
| Server | Vite development server, mode `a`, 127.0.0.1:5181 (unminified development build) |
| Fixture | `cw-fixtures-v1`, seed 13014: 500 tasks, 10,000 events |
| Viewports | 1280×800 and 1920×1080 CSS px, set with the Playwright viewport control (exact inner sizes confirmed in page) |

## What concept A implements

Board-first navigation as specified: Board or List home; opening a task places a compact task list beside a task workspace with the tabs Discussion & plan, Decisions, Result & diff, Code, Git and Workers. Cross-task **Needs you**, **New task · Mock**, **Machine & connection** (Project menu or status bar) and project-level Code open in the same workspace pane; the list stays visible and collapses to a rail with `Ctrl+\`.

| Screen ([SCREENS.md](../SCREENS.md)) | Where |
|---|---|
| Project board and list | Home: seven read-only phase columns (virtualized cards) or a sortable, virtualized table |
| Create task | Workspace pane form; `Ctrl+N` |
| Task discussion and plan | Tab 1: description, task or whole-project timeline (virtualized, 10,000 events), plan rail at wide widths |
| Worker detail | Tab 6: execution → attempt → worker row with an expandable detail |
| Decisions | Tab 2 for one task; **Needs you** for all tasks |
| Evidence and diff | Tab 3: result header, checks (failed first), did-not-run group, file list, side-by-side or unified diff, acceptance block after the evidence |
| File tree, editor, search | Tab 4: file tree or search, editor with lease line and save-conflict handling |
| Terminal | Bottom of tab 4: fixture terminal that records input and never executes it |
| Git | Tab 5: worktree line, conflicted group first, commit form, pull, push with confirmation |
| Connection settings | Machine & connection page |

States: loading (an explicit preview toggle in the scenario guide, never a timer), empty, error with recovery, offline (cached), disconnected, blocked permission, failed check, stale result, file and Git conflict, unavailable capability (push notifications), ambiguous push outcome, long text (collapsed description), simulated phone session with "Changes since you were last here".

### Mock and completion safeguards

- A persistent prototype strip shows the fixture revision, scenario, guide step and "All actions are mock and change only in-memory state". The status bar repeats "Fixture data, not runtime evidence".
- Every mutating control carries a visible "· Mock" tag and "(mock)" in its accessible name. Each mock action leaves an inline receipt where it was taken, for example "Mock outcome: recorded in memory only. No agent was started."
- Acceptance reads "Accepted (mock) · mock-result-complete · digest …", never a plain "Accepted", and the footer "Accepting does not commit, push or deploy." stays visible. The Git tab then states that the mock acceptance did not commit or push.
- A stale result shows the digest change, keeps acceptance disabled with the reason, and offers no newer revision because the fixture has none.
- Offline and disconnected scenarios show the last confirmed time (2030-01-01 00:00 UTC from the fixture), turn known executions into "Unknown" with the last known state, and disable every mock mutation with the visible reason "Disconnected; actions unavailable." (or the offline equivalent).
- Decisions answered in the prototype keep the fixture's execution facts unchanged ("the fixture still reports …").

## Scenario outcomes

Keyboard input was sent with the Playwright key-press tool (key events through the browser's input pipeline). Text was typed character by character into the field that already had focus.

| Scenario | 1280×800 | 1920×1080 |
|---|---|---|
| S1 create → decision → result | Passed, keyboard only (see notes) | Passed, keyboard only except switching to the stale scenario (script click) |
| S2 code / Git conflict | Passed, keyboard only | Passed, keyboard only except selecting the conflict scenario (script click) |
| S3 state sweep | Passed; states switched with the shared scenario selector by script | Passed, same method |
| S4 keyboard only | S1 and S2 above; visible focus observed in every opened capture | Same flows |
| S5 text 150 % / 200 % | 200 % in-app preset: create, accept and Git conflict passed after fixes; 150 %: result with unified diff; author root font 32 px: board usable | Not run |
| S6 desktop → phone → desktop | Passed with the simulated phone dialog (script clicks) | Not run |

S1 path at 1280×800: `F6` ×3 and Tab reach the board's single Tab stop; arrows and End move between 496 Discussion cards. `Ctrl+N`, type "Add rate limiting to /login", Enter: the task opens with focus on its title, execution "Not started (mock)" and the receipt. `Ctrl+K` "task 3", Enter, `Alt+2`, Tab, Space, Tab, Enter answers the question (mock). Scenario guide (from the top bar with Shift+Tab) → Next step → Apply "selectScenario blocked", Escape returns focus to the guide button; `Alt+2`, Tab, Tab, Enter approves the permission once (mock). `Ctrl+K` "task 4", `Alt+3` shows the failed check first; Request changes (mock). `Ctrl+K` "task 6", `Alt+3`, Tab, Tab, Enter accepts the complete result (mock); `Alt+5` shows Git unchanged. Guide step 6 applies the stale scenario; `Alt+3` shows acceptance disabled with the digest reason.

S2 path at 1280×800: guide script "code-git-conflict", Apply "selectScenario conflict"; `Ctrl+K` "task 7"; `Alt+4`; `Ctrl+P`, Enter focuses the editor; type; Tab, Enter on Save · Mock opens the save-conflict banner (focus moves to it); Compare; Keep mine · Mock; `Alt+5`; Mark resolved · Mock. Commit · Mock stays disabled: first "Resolve 1 remaining conflict.", then "The merge is still conflicted in the fixture; a mock resolution does not change Git." `Ctrl+\`` opens the fixture terminal; "npm test" is echoed as "Not executed". Escape returns focus to the task's row in the list.

Automated checks of the same behavior: `src/concepts/a/a.test.tsx` (6 tests: virtualized board and single Tab stop, arrow focus and `Alt+3`, create without an agent start, stale acceptance blocked and mock acceptance, disconnected mutations disabled, conflict commit stays disabled).

### Defects found during the walkthrough and fixed

All were rechecked in the same state and viewport after the fix.

1. Board area collapsed to zero height when the changes panel was absent (the main area fell into an auto-sized grid row). Fixed with a flex body.
2. Top bar and board overflowed the window (board minimum width inflated the column). Fixed with `min-width: 0` and a `minmax(0, 1fr)` desk column.
3. "Your permission needed" badge clipped at the card edge and card metadata cut at the bottom. Column minimum raised to 12.5 rem and metadata split into two single-line rows.
4. Scope values broke into separate rows; editor text was not monospace; disabled-reason text stretched buttons.
5. Escape from the scenario guide and reaching its last step dropped focus to the page. Focus now returns to the guide button or a live control.
6. The scenario guide stayed above the simulated phone dialog. It is hidden while the phone session is open, and focus returns to the guide button afterwards.
7. At 200 % text the fixed-height desk left almost no room for the board, and the Discussion tab overlapped the description with the plan box. Large text now switches to a vertically scrolling page with a pinned prototype strip and 80 vh primary areas. The switch measures the actual text unit, so it also responds to a larger root font or browser zoom.

Early captures from before these fixes are kept separately in `_sweep/` and are not evidence of the final rendering.

## Performance observations

Method: `performance.mark` at component start and after the second animation frame following mount (`a-desk-first-render`); interaction timing by dispatching keydown events on the focused card from a page script and measuring until the next animation frame; DOM counts with `querySelectorAll`. Headless Chrome on the development server, not the native shell or reference hardware. These observations do not qualify the targets in [performance](../performance.md).

| Observation | Result |
|---|---|
| Board first render with the full fixture (component start → painted) | 36.4 ms on the first cold load; 24.5–28.3 ms on reloads at 1280×800; 26.2 ms at 1920×1080 |
| Desk start after navigation start | 326.8 ms on the first cold development-server load (on-demand transform of the lazy chunk); 31–36 ms on reloads |
| DOM with 500 tasks | 11 cards / 291 elements (1280×800); 13 cards / 321 elements (1920×1080) |
| Board keyboard roving, 60 keydowns (End, Home, ArrowDown) | Handler 1.3 ms median, 1.7 ms p95, 2.3 ms max; next frame 16.6 ms median, 16.8 ms max |
| Open a task from a card | 5.1 ms handler, 16.7 ms to next frame |
| 10,000-event timeline | 12 rows in the DOM; jump to the middle and repaint 7.7 ms |
| JS heap (Chrome `performance.memory`, approximate) | 12 MB at 1920×1080 |

## Screenshots

Directory: `.design-evidence/cortex-workspace/desktop/a/` (git-ignored). All are viewport captures at CSS scale; dimensions were read from the PNG headers.

Opened and visually inspected: 1280×800 files 01, 02, 06, 11, 14, 16, 23, 25, 31, 41, 44, 45, 46, 48, 50 and text200-03 to text200-06; 1920×1080 files 01, 11, 20, 30 and 44. The other files were captured after an in-page check of the state they show (focused element, receipt text, disabled state and reason, title), but were not opened as images; their visual result is unverified.

| File | Size | Shows |
|---|---|---|
| 1280x800-01-board-default.png | 1280×800 | Board home, default scenario |
| 1280x800-02-keyboard-board-end-of-column.png | 1280×800 | Keyboard focus on the last Discussion card |
| 1280x800-03-s1-create-form.png | 1280×800 | New task form filled by keyboard |
| 1280x800-04-s1-created-not-started.png | 1280×800 | Created task, Not started (mock), receipt |
| 1280x800-05-s1-search-palette.png | 1280×800 | Search palette |
| 1280x800-06-s1-question-decision.png | 1280×800 | Question decision before sending |
| 1280x800-07-s1-question-answered-mock.png | 1280×800 | Answered (mock) receipt |
| 1280x800-08-s1-guide-step2-blocked.png | 1280×800 | Scenario guide on the blocked step |
| 1280x800-09-s1-blocked-permission.png | 1280×800 | Blocked permission decision |
| 1280x800-10-s1-permission-approved-once-mock.png | 1280×800 | Approve once (mock) receipt |
| 1280x800-11-s1-failed-check-first.png | 1280×800 | Failed check listed first |
| 1280x800-12-s1-request-changes-mock.png | 1280×800 | Request changes (mock) receipt |
| 1280x800-13-s1-result-diff.png | 1280×800 | Result header, checks and diff |
| 1280x800-14-s1-accepted-mock.png | 1280×800 | Accepted (mock) with the no-commit footer |
| 1280x800-15-s1-git-unchanged-after-accept.png | 1280×800 | Git unchanged after mock acceptance |
| 1280x800-16-s1-stale-result-accept-disabled.png | 1280×800 | Stale result banner |
| 1280x800-17-s1-stale-acceptance-block.png | 1280×800 | Stale acceptance block, Accept disabled with reason |
| 1280x800-20-s2-guide-conflict-script.png | 1280×800 | Guide on the conflict script |
| 1280x800-21-s2-editor-unsaved.png | 1280×800 | Editor with unsaved edit |
| 1280x800-22-s2-save-conflict-banner.png | 1280×800 | Save-conflict banner, Save disabled |
| 1280x800-23-s2-compare-keep-mine-mock.png | 1280×800 | Three-way compare, Keep mine (mock) |
| 1280x800-24-s2-git-conflict-commit-disabled.png | 1280×800 | Git conflict, Commit disabled |
| 1280x800-25-s2-marked-resolved-commit-still-disabled.png | 1280×800 | Resolved (mock), Commit still disabled |
| 1280x800-26-terminal-not-executed.png | 1280×800 | Fixture terminal, command not executed |
| 1280x800-30-workers-detail-running.png | 1280×800 | Workers tab and worker detail |
| 1280x800-31-timeline-10000-events-virtualized.png | 1280×800 | Whole-project timeline, Jump to latest |
| 1280x800-40-state-empty.png | 1280×800 | Empty project |
| 1280x800-41-state-error.png | 1280×800 | Fixture read error with recovery |
| 1280x800-42-state-offline-cached.png | 1280×800 | Offline, cached state |
| 1280x800-43-state-disconnected-board.png | 1280×800 | Disconnected board |
| 1280x800-44-state-disconnected-decision-disabled.png | 1280×800 | Disconnected decision, actions disabled |
| 1280x800-45-state-loading-preview.png | 1280×800 | Loading skeletons |
| 1280x800-46-list-view-500-virtualized.png | 1280×800 | List home |
| 1280x800-47-needs-you-cross-task.png | 1280×800 | Needs you |
| 1280x800-48-connection-settings-unavailable-capability.png | 1280×800 | Machine & connection, push unavailable |
| 1280x800-49-keyboard-help-dialog.png | 1280×800 | Keyboard reference dialog |
| 1280x800-50-s6-simulated-phone-session.png | 1280×800 | Simulated phone session |
| 1280x800-51-s6-changes-since-answered-on-phone.png | 1280×800 | Changes since you were last here |
| 1280x800-browser-root-32px-board.png | 1280×800 | Root font 32 px, large-text layout |
| 1280x800-text150-result-unified-diff.png | 1280×800 | 150 % text, unified diff |
| 1280x800-text200-01-board-top.png | 1280×800 | 200 % text, top of desk |
| 1280x800-text200-02-create-form.png | 1280×800 | 200 % text, create form |
| 1280x800-text200-03-created-task.png | 1280×800 | 200 % text, created task (before the Discussion fix) |
| 1280x800-text200-04-accepted-mock.png | 1280×800 | 200 % text, mock acceptance |
| 1280x800-text200-05-git-conflict.png | 1280×800 | 200 % text, Git conflict |
| 1280x800-text200-06-discussion-no-overlap.png | 1280×800 | 200 % text, Discussion after the fix |
| 1920x1080-01-board-default.png | 1920×1080 | Board, all seven columns visible |
| 1920x1080-02-s1-create-form.png | 1920×1080 | New task form |
| 1920x1080-03-s1-created-not-started.png | 1920×1080 | Created task |
| 1920x1080-04-s1-question-answered-mock.png | 1920×1080 | Question answered (mock) |
| 1920x1080-05-s1-blocked-permission.png | 1920×1080 | Permission decision |
| 1920x1080-06-s1-permission-approved-once-mock.png | 1920×1080 | Approve once (mock) |
| 1920x1080-07-s1-failed-check-first.png | 1920×1080 | Failed check |
| 1920x1080-08-s1-result-diff-side-by-side.png | 1920×1080 | Side-by-side diff |
| 1920x1080-09-s1-accepted-mock.png | 1920×1080 | Accepted (mock) |
| 1920x1080-10-s1-git-unchanged-after-accept.png | 1920×1080 | Git unchanged |
| 1920x1080-11-s1-stale-result-accept-disabled.png | 1920×1080 | Stale result |
| 1920x1080-20-s2-save-conflict-compare-keep-mine.png | 1920×1080 | Save conflict, compare, terminal |
| 1920x1080-21-s2-git-conflict-commit-disabled.png | 1920×1080 | Git conflict |
| 1920x1080-22-s2-marked-resolved-commit-still-disabled.png | 1920×1080 | Resolved (mock), Commit disabled |
| 1920x1080-30-discussion-plan-rail-10000-events.png | 1920×1080 | Plan rail beside the 10,000-event timeline |
| 1920x1080-40-state-empty.png | 1920×1080 | Empty |
| 1920x1080-41-state-error.png | 1920×1080 | Error |
| 1920x1080-42-state-offline-cached.png | 1920×1080 | Offline |
| 1920x1080-43-state-disconnected-board.png | 1920×1080 | Disconnected board |
| 1920x1080-44-state-disconnected-permission-disabled.png | 1920×1080 | Disconnected permission, actions disabled |
| 1920x1080-45-state-loading-preview.png | 1920×1080 | Loading |
| 1920x1080-46-list-view-500-virtualized.png | 1920×1080 | List home |
| 1920x1080-47-needs-you-cross-task.png | 1920×1080 | Needs you |
| 1920x1080-48-connection-settings-disconnected.png | 1920×1080 | Machine & connection while disconnected |

## Deviations from the specification

- The shared shell owns the page `header` and `main`; the desk uses a labelled `nav`, labelled `section` regions and a `footer` inside it instead of nested landmarks.
- The fixture has four phases. Concept A projects them onto the seven candidate columns: the fixture's not-yet-started phase → Discussion, `working` → In execution, `review` and `done` → Result review (runtime completion is not acceptance), mock acceptance → Accepted (mock), "Save as draft" → Draft. Planned and Closed stay empty.
- The fixture has one conflicted file, so S2 resolves one file instead of leaving a second one conflicted; Commit stays disabled anyway. The Git conflict appears only on the conflict scenario's task. The one fixture file is also used as the diff of every result.
- The fixture carries no plan step states, terminal output, timestamps for decisions or results, remote names, worktree paths or engine/model data; the prototype says so where each would appear.
- Outside the offline and disconnected scenarios the connection reads "Connected (simulated)", while the connection page still shows the fixture machine record (disconnected).
- Column minimum width is 12.5 rem (200 px) instead of 176 px, so the longest wait badge fits; about five columns are visible at 1280×800. Compact list rows are 3.75 rem rather than 36 px to hold the title and badges.
- No CodeMirror: the editor is a plain text area and the diff a static side-by-side or unified view (unified below 760 px or at 150 % text and above).
- Not implemented: `Alt+←`/`Alt+→` history, saved views, keyboard resizing of the terminal (pointer resize only), Settings → Keyboard (the G-sequence switch is in the keyboard reference).

## Unrun and unverified

- Native Tauri 2 build and WebKitGTK rendering: Cargo and rustc are not installed.
- Real OS keyboard and browser-reserved shortcuts: keys were sent through Playwright. A desktop browser may keep `Ctrl+N` or `Alt+1`…`Alt+6` for itself; every action also has a visible control.
- Screen reader behavior, contrast measurement of the rendered tokens, physical devices and real browser zoom (the in-app preset and a 32 px root font were used instead).
- 150 % and 200 % text at 1920×1080; S6 at 1920×1080.
- Scenario steps marked "script click" above were driven by page scripts, not by keyboard.
- Performance targets in [performance](../performance.md) (they need the native build on the reference machine).

## Open observations

- In offline and disconnected scenarios, cards keep the last known wait badges and the Needs you count stays; they are labelled as cached but could still be read as current.
- "New task · Mock" stays enabled while disconnected; the form's Create button is disabled with the reason.
- Focus and selection share the accent hue. In the opened captures the focus ring (2 px) and the selection border (1 px with tint) were distinguishable; this is an observation, not a measurement.
