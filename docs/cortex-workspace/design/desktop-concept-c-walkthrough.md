# Desktop concept C walkthrough: Focus Workspace

**Status: clickable mock prototype for comparison at owner Gate D. Not the approved design system, not runtime evidence and not native qualification.** Specification: [desktop concept C](desktop-concept-c.md). Shared data: [prototype fixtures](prototype-fixtures.md). Every action changes only in-memory mock state; no agent, ACP session, service, file or Git operation is started.

## 1. What exists

Concept C lives in `prototypes/cortex-workspace/desktop/src/concepts/c/` and uses the shared fixture loader (`src/fixtures.ts`) and mock store (`src/store.ts`) unchanged.

| File | Content |
|---|---|
| `index.tsx` | Root: prototype strip, top bar, queue or deep-mode rail, focus pane, status bar, dialogs, keyboard map, text-size preset |
| `model.ts` | Fixture-derived presentation model: fixed queue ranking, connection mode, labels, synthetic diff from the fixture file record |
| `Queue.tsx` | Attention queue (L0) with roving focus |
| `Focus.tsx` | Decision block (L1) and the six L2 disclosure sections |
| `Deep.tsx` | L3 deep mode: full discussion with 10,000-event history, full diff, code (tree, search, editor, terminal), Git, worker detail |
| `Pages.tsx` | All tasks (list and board), create task, connection settings, settings, empty and error states |
| `ui.tsx` | Badges, mock buttons, inline mock receipts, check table, long-text collapse, roving helpers, windowed list, modal dialog |
| `c.css` | Candidate tokens from the specification, scoped to the concept |
| `c.test.tsx` | 10 interaction and state tests (jsdom) |

Run it:

```sh
cd prototypes/cortex-workspace/desktop
npm ci
npm run dev -- --mode c     # http://127.0.0.1:5183/c
```

The shared shell (concept links, scenario selector, reset) stays above the concept. The concept fills one viewport height below it; the captures below scroll the concept to the top of the window.

## 2. Environment

| Item | Value |
|---|---|
| OS | Linux x86_64 |
| Node / npm | v24.14.1 / 11.11.0 |
| Base commit | `27e655af26a835a4106c5b9b8aabef84d8f17dcc` plus uncommitted prototype work |
| Dev server | Vite 8.3.5, `npm run dev -- --mode c`, port 5183, loopback only |
| Browser | Playwright MCP, headless Chromium; user agent `HeadlessChrome/156.0.0.0`, Linux |
| Viewports | Page viewport set to 1280×800 and 1920×1080 CSS px with `setViewportSize` |
| Fonts | Source Sans 3 / Source Code Pro are not bundled; rendering used the `Ubuntu` and `Ubuntu Mono` / `DejaVu Sans Mono` fallbacks |

## 3. Commands and outcomes

All from the repository root unless stated.

| Command | Outcome |
|---|---|
| `npm --prefix prototypes/cortex-workspace/desktop run typecheck` | Exit 0 (final run). An earlier run exited 2 on errors in concept A and B files that other contributors were editing at the time; concept C had no errors. |
| `npm --prefix prototypes/cortex-workspace/desktop test` | Exit 0: 5 files, 39 tests (10 of them concept C). |
| `npm --prefix prototypes/cortex-workspace/desktop run build` | Exit 0; separate lazy chunk `c-*.js` 84.26 kB (23.42 kB gzip) and `c-*.css` 17.06 kB. |
| `npm exec -- vitest run src/concepts/c` (in the prototype directory) | Exit 0 after one failed run caused by a wrong expected count in a new test (4 decisions, not 3). |
| `npm run dev -- --mode c` (prototype directory, detached) | Served HTTP 200 on `/c`; stopped after the walkthrough, endpoint closed. |

Browser console during the walkthrough: one error, a 404 for `/favicon.ico` from the shared page; no errors from concept C.

## 4. Scenario walkthroughs

### 4.1 Create → decision → result (keyboard only)

Run at 1280×800 and again at 1920×1080 from a fresh page load. Keys pressed in the page; the request text was typed with Playwright `fill` into the focused field, then submitted with Enter.

| Step | Keys | Observed |
|---|---|---|
| 1 | `Alt+N` | Composer "What should be done?" opens with focus in the field. |
| 2 | `Enter` on an empty field | Inline error "Describe what should be done."; nothing is created. |
| 3 | Type the request, `Enter` | Task created in memory, focus on its heading, badge "Not started (mock)", inline receipt "Mock outcome: recorded in memory only. No agent was started." Row appended under "Changed since last visit" with "New · Not started (mock)"; polite announcement. No execution exists for the task. |
| 4 | `Esc`, `Home`, `J` | Focus moves to the queue, then down one row. The selected row (teal tint) and the focus pane do not change: browsing never selects. |
| 5 | `K`, `Enter` | Permission decision opens at L1; focus on the decision heading. Effect, scope and "If you reject … Nothing is reverted." shown from fixture text. |
| 6 | `Tab`, `Enter` | "Approve once · Mock" (accessible name "Approve once (mock)"). Result: "Answered (mock) … no authorization was granted and no agent was started." Focus returns to the decision heading. Row moves to "Changed since last visit". |
| 7 | `Esc`, `Home`, `J`, `Enter` | Result acceptance decision for `mock-result-complete`. L2 "Result & evidence" is open by default; evidence counts Passed 1 · Failed 0 · Not run 0 · Unverified 0, "Source: fixture check records, not runtime evidence." |
| 8 | `Tab`, `Enter` | "Accept mock-result-complete · Mock". Shows "Accepted (mock) · mock-result-complete · digest mock-digest-amber-v1. Not runtime evidence." and the receipt "No artifact was accepted, committed, pushed or deployed." |
| 9 | `Alt+5` | "Code & Git" expands and takes focus: "1 file changed, not committed · mock acceptance did not commit". |

Stale case: with the shared scenario selector on "stale", the decision for `mock-result-stale` shows a warn band "Result changed after review: digest mock-digest-quartz-v1 → mock-digest-quartz-v2. Previous acceptance (mock) is invalidated." The Accept button is `aria-disabled` with that text as its description. Playwright refused to click it (not enabled); no receipt, no `acceptedResultIds` entry and no "Accepted (mock)" text appeared. "Review new result" opens the evidence section, which states that no newer result exists in the fixture.

### 4.2 Code / Git conflict (keyboard only)

Scenario "conflict" selected in the shared shell selector (outside the concept), then keys only.

| Step | Keys | Observed |
|---|---|---|
| 1 | `F6` ×3 | Focus cycles top bar → queue → focus-pane heading of `mock-task-00006`, which shows a danger banner "Merge stopped: 1 file has conflicts on synthetic-layout". |
| 2 | `Alt+5`, `Tab`, `Enter` | "Code & Git" expands; "Open editor" enters Code deep mode. The queue collapses to a 56 px rail "N need you" with a Back button. |
| 3 | `Tab` ×9, `Enter` | File-conflict banner; Save is disabled with reason "Save is disabled while the file conflict is unresolved…". Compare shows Base / Local (yours) / Incoming (disk) columns from the fixture. |
| 4 | `Tab` ×3, `Enter` | "Keep local · Mock" records the mock choice: "Mock resolution recorded: local. No file or Git mutation occurred." Focus moves to the editor. Save becomes available as a mock. |
| 5 | `Ctrl+Shift+G` | Git deep mode: "Merge stopped … No auto-stash or reset is performed." Conflicted file listed first; Commit and Push disabled with "Resolve conflicts before committing." |
| 6 | `Tab` ×8 (1280×800), `Enter` | "Abort merge · Mock" opens a confirmation dialog restating effect and scope; focus on Cancel. At 1920×1080 focus was placed on the Abort button by script before pressing `Enter`. |
| 7 | `Esc` | Dialog closes, focus returns to "Abort merge", receipt "Abort cancelled. Nothing changed." |
| 8 | `Alt+←` | Back to the attention queue with focus on the selected row; the browser did not navigate back. |
| 9 | `G` `T`, `Tab` ×2, `Enter`, `Tab`, `↓`, `Enter` | All tasks list (500 rows) → Board (4 columns) → a card opens the task in the focus pane with the queue restored. |

### 4.3 Progressive disclosure and reach from the queue

| Target | Pointer actions from the queue | Keyboard |
|---|---|---|
| Decision (L1) | 1 (row) | `Enter` on the row |
| Result and evidence | 1 for acceptance decisions (open by default); otherwise 2 | `Enter`, then `Alt+3` |
| Full diff (L3) | 3 (row, "Changes", "Open full diff") | `Enter`, `Alt+4`, `Tab`, `Enter` |
| Code & Git (L3) | 3 via the task (row, "Code & Git", "Open editor"/"Open Git"), or 1 via top-bar "Code" | `G` `C`, or `Ctrl+Shift+G` |
| Board | 2 (top-bar "All tasks", "Board") | `G` `T`, then the Board toggle |

Counts are the same at both viewports; layout changes widths, not steps. L2 sections start collapsed with a one-line summary; their open state is remembered per task. Deep mode is entered only by the user and keeps the queue selection.

## 5. State coverage

| State | Where it shows | Captures |
|---|---|---|
| Default / attention queue | Queue sections in fixed order: Needs your decision (4), Blocked (1), Changed since last visit, Running (1), Quiet tasks (493, collapsed) | `*-01-attention-default` |
| Empty | Each queue section stays; "Nothing needs your decision in this view…"; focus pane "No tasks in this project yet." with "New task · Mock" | `*-state-empty` |
| Blocked permission | Permission decision with "Your permission needed" in the queue and L1 | `*-state-blocked-permission`, `*-05-permission-decision-kbd` |
| Failed check | "Blocked" row; danger banner with the failed check, "Open log", "Request changes · Mock"; check table lists Failed first | `*-state-failed-check` |
| Stale result | See section 4.1 | `*-state-stale-result` |
| Disconnected | Danger banner with last confirmed time; Running becomes "Unknown"; every mock mutating control `aria-disabled` with "Disconnected; actions unavailable."; "Reconnect · Mock" only records that nothing was attempted | `*-state-disconnected` |
| Offline / stale cache | Queue headings "as of 2030-01-01 00:00 UTC"; read-only banner | `*-state-offline` |
| Conflict | File conflict in Code, merge conflict in Git, conflict row under "Changed since last visit" | `*-11`…`*-16` (1280), `*-10`…`*-15` (1920) |
| Error | Typed error "Could not load tasks (fixture error E_FIXTURE_READ…). Nothing was changed." with "Try again" (reloads the fixture and resets mock state) | `*-state-error` |
| Loading | "Preview loading" in the prototype strip: four skeleton rows in the queue, skeleton in the focus pane and in All tasks; mutating actions disabled | `*-state-loading` |
| Long text | Description over 600 characters clamps to 12 lines with "Show full description (1,776 characters)" | `1280x800-30-long-text` |
| 500 tasks / 10,000 events | Quiet tasks expanded (493 rows); All tasks list (500 rows); project history windowed list (10,000 events) | `1280x800-32-quiet-493-expanded`, `*-history-10000-events` |

SCREENS.md desktop screens: board/list (All tasks), create task (composer), discussion/plan (L2 + full discussion), worker detail (L2 + deep), decisions (queue + L1), evidence and diff (L2 + full diff), file tree/editor/search (Code deep mode), terminal (bottom split in Code, mock only; not opened in the browser or in tests), Git (Git deep mode), connection settings (focus-pane page). Permission denied appears as the unavailable "Android push" capability from the fixture.

## 6. Keyboard and focus

Exercised in the browser: skip link, `Alt+N`, `Enter`/`Shift+Enter` in the composer, queue roving focus with `J`/`K`, `↑`/`↓` and `Home`, `Enter` to select, `Esc` back to the queue, `Tab` to decision actions, `Alt+3`/`Alt+5` section toggles, `F6` region cycling, `Ctrl+Shift+G`, `Alt+←`, `G` `T`, `Ctrl+K` palette with `Enter`, `?` help, dialog focus on the safe button and `Esc` with focus restore, board arrow keys. `Alt+digit`, `Alt+←` and `F6` were captured by the page in headless Chromium; browser behaviour in a desktop browser or the Tauri webview was not checked.

Not exercised in the browser: `Ctrl+N` (reserved by browsers; `Alt+N` is provided), `F1`, `Shift+F6`, `[`/`]`, `Ctrl+Shift+E`, `` Ctrl+` ``, `G` `A`/`G` `C`. `Alt+4` is covered by a jsdom test only; `[`/`]`, `Ctrl+Shift+E` and the terminal toggle have no automated test either.

Visible focus: 3 px teal outline with 2 px offset, inset in queue rows. It was visible on queue rows, headings, buttons (including outside the ink primary button), dialog buttons, the editor and section toggles in the opened captures. Focus never moved because of incoming data; the fixture has no timed updates.

## 7. Increased text size

Settings → Text size → 200% (reached with `Ctrl+K`, `Tab`, arrow keys) sets the root font size to 32 px; all concept sizes are in rem. At 1280×800 the concept switches to one column (queue, then focus pane), the top bar wraps to three rows, and no horizontal page overflow was measured (`scrollWidth` ≤ `innerWidth`). Decision title, metadata, effect/scope/rejection and actions stayed readable without clipping in the opened captures; the focused "Approve once · Mock" button measured 369×80 CSS px. The deep-mode rail becomes a full-width row with the Back button. 150% and browser zoom were not run.

## 8. Performance observations

Method: in the page, `performance.now()` before a programmatic click and after two `requestAnimationFrame` callbacks, in headless Chromium with the dev (unminified) build. Each value is one sample and includes up to two frame intervals (about 33 ms at 60 Hz), so values near 33 ms mean the update fitted within the frames.

| Interaction | 1280×800 | 1920×1080 |
|---|---|---|
| Expand Quiet tasks (493 rows) | 50 ms | not measured |
| Select a queue row with 493 quiet rows rendered | 33 ms | not measured |
| All tasks → List (500 table rows) | 47 ms | not measured |
| All tasks → Board (54 cards rendered, "Show more" per column) | 33 ms | not measured |
| Open project history (10,000 events, windowed) | 18 ms | 33 ms |
| Jump to the end (1280) / middle (1920) of the history | 33 ms | 33 ms |
| Rows in the DOM for the history list | 19 | 27 |

Whole document with the quiet list expanded: 4,672 elements. No long-task measurement was made.

## 9. Defects found and fixed during the walkthrough

1. Focus fell to the page body after answering or accepting, because the action buttons are replaced. Focus now returns to the decision heading, and to the editor after a conflict choice (test added).
2. The shared shell's global `nav { display: flex }` rule laid out the queue sections side by side in the empty, error and loading states. The queue now sets `display: block`.
3. The concept's paragraph reset removed queue margins for "None." and error text.
4. Table cells broke words mid-word ("Passe d"); badges now stay on one line and table text wraps at word boundaries.
5. L2 summaries inherited bold weight; badges in queue rows wrapped.
6. All tasks ignored the loading preview.
7. The palette ran "Connection settings" for the query "Settings"; prefix matches now rank first.
8. The board showed 2 columns at 1280×800; it now shows 4.
9. The acceptance receipt repeated "not runtime evidence".

`1280x800-01-attention-default.png` predates fix 5; `1280x800-10-accepted-codegit-summary-kbd.png` predates fix 4 and still shows the broken table wrapping. The 1920×1080 captures show the fixed rendering.

## 10. Screenshots

Stored under `.design-evidence/cortex-workspace/desktop/c/` (git-ignored). Every file was opened and inspected. Dimensions are the PNG pixel size, equal to the CSS viewport.

| File | Size |
|---|---|
| `1280x800-01-attention-default.png` … `1280x800-32-quiet-493-expanded.png` (32 files: 01–10 create → decision → result, 11–16 conflict, 17–18 all tasks, 19–26 states, 27 full diff, 28 history, 29 palette, 30 long text, 31 keyboard help, 32 quiet tasks) | 1280×800 each |
| `1280x800-text200-01-after-preset-top-kbd.png` … `1280x800-text200-05-git-deep-kbd.png` (5 files at 200% text) | 1280×800 each |
| `1920x1080-01-attention-default.png` … `1920x1080-27-history-10000-events.png` (27 files: 01–09 create → decision → result, 10–15 conflict, 16–17 all tasks, 18–25 states, 26 full diff, 27 history) | 1920×1080 each |

Files ending in `-kbd` were reached with the keyboard. In `1920x1080-20-state-blocked-permission.png` the permission had already been answered earlier in the same session; the open permission at 1920×1080 is in `1920x1080-05-permission-decision-kbd.png`.

## 11. Deviations from the specification and open gaps

- The concept renders inside the shared shell's `main`, so the focus pane is a labelled region rather than a second `main`, and the status bar is a labelled footer, not `contentinfo`.
- "Needs your decision" holds 4 items from the fixture (permission, question, complete result, stale result), not the 3 in the specification example.
- The "2 new · Show" control is not implemented: the fixture never adds items by itself.
- Requested and produced times are shown as "unknown" because the fixture has no such fields. Queue times use the last fixture event.
- "Not permitted" is shown only through the fixture's unavailable "Android push" capability.
- The desktop → phone → desktop scenario is not covered by concept C.
- The Running glyph (◐) renders small with the fallback fonts.
- Not run: native Tauri build and window (Cargo/rustc absent), screen readers, measured contrast in the rendered page (tokens come from the specification's computed ratios), reduced-motion rendering, physical devices, 150% text, browsers other than headless Chromium. Paid live checks were not run: paid live check not requested by the owner.
