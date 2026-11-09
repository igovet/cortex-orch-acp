# Desktop concepts: side-by-side comparison

**Status: mock-only comparison prepared for owner Gate D. No winner is selected and no direction is recommended. Gate D is open; see [desktop approval](desktop-approval.md).**

Nothing in this document is runtime evidence, native qualification, an ACP or service integration result, or an approved design system. Every prototype action changes only in-memory mock state. Mock outcomes are not runtime evidence.

## 1. Scope and evidence basis

This document compares the three candidate desktop concepts from [desktop concepts](desktop-concepts.md) on the same fixture and the same scenario scripts:

- [Concept A: Task Desk](desktop-concept-a.md) (route `/a`, board first)
- [Concept B: IDE with Mission Control](desktop-concept-b.md) (route `/b`, code first)
- [Concept C: Focus Workspace](desktop-concept-c.md) (route `/c`, attention queue first)

Evidence used, in order of weight:

1. The three walkthrough records: [A](desktop-concept-a-walkthrough.md), [B](desktop-concept-b-walkthrough.md), [C](desktop-concept-c-walkthrough.md). These record the browser runs (headless Chromium through Playwright), the screenshots and the limits of each run. The browser walkthroughs were not repeated for this document.
2. Two independent reviews dated 2026-11-04: a side-by-side visual review that opened 24 of the 209 screenshot files, and a source-level accessibility review that ran no browser and no assistive technology. Their findings are restated below with the verification status this document observed.
3. Checks run for this document on 2026-11-04 (section 2.5), and direct reads of the prototype source for each finding marked "confirmed in source".

Where a claim comes from a walkthrough or a review and was not re-observed here, it is marked as recorded. Nothing here upgrades a mock walkthrough into a qualified result.

## 2. Identical basis

### 2.1 Fixture

| Item | Value |
|---|---|
| Fixture revision | `cw-fixtures-v1` (`FIXTURE_VERSION` in `prototypes/cortex-workspace/desktop/src/fixtures.ts`) |
| Seed | 13014 |
| Tasks and events | 500 tasks, 10,000 events, exactly one running execution |
| Named cases | blocked permission, question, failed check, complete result, stale result, disconnected machine, file conflict, Git conflict, unavailable capability (Android push) |
| Data | synthetic; no production credentials, logs or network calls |

The fixture definition and the shared scenario scripts are described in [prototype fixtures](prototype-fixtures.md) and the [prototype README](../../../prototypes/cortex-workspace/desktop/README.md).

### 2.2 Routes and sources

| Concept | Route | Dev server (port) | Source | Walkthrough |
|---|---|---|---|---|
| A Task Desk | `/a` | `npm run dev -- --mode a` (5181) | `prototypes/cortex-workspace/desktop/src/concepts/a/` | [A](desktop-concept-a-walkthrough.md) |
| B IDE with Mission Control | `/b` | `npm run dev -- --mode b` (5182) | `prototypes/cortex-workspace/desktop/src/concepts/b/` | [B](desktop-concept-b-walkthrough.md) |
| C Focus Workspace | `/c` | `npm run dev -- --mode c` (5183) | `prototypes/cortex-workspace/desktop/src/concepts/c/` | [C](desktop-concept-c-walkthrough.md) |

All three routes share the fixture loader (`src/fixtures.ts`), the mock store (`src/store.ts`) and the scenario scripts (`src/scenarios.ts`). Each concept's walkthrough states that the shared files were used without change.

### 2.3 Environment

- Linux x86_64, kernel 7.0.0-42-generic; Node v24.14.1; npm 11.11.0; SolidJS 1.9.18, Vite 8.3.5, Vitest 5.1.0, TypeScript 6.0.3.
- Browser: headless Chromium 156 driven by Playwright (recorded in the walkthroughs). Chromium is the only browser tested. Firefox, Safari/WebKit and the Tauri WebKitGTK webview were not run.
- Viewports: 1280×800 and 1920×1080 CSS pixels, device pixel ratio 1.
- Fonts: Source Sans 3 and Source Code Pro are not bundled; the walkthroughs record fallback fonts (`Ubuntu`, `Ubuntu Mono` or `DejaVu Sans Mono`).
- Interaction timings used the development (unminified) Vite server, not a production build.

### 2.4 Git state and build identifiers

- Repository HEAD: `27e655af26a835a4106c5b9b8aabef84d8f17dcc`, branch `mb/evaluate-standalone-acp-client-proposal` (observed 2026-11-04).
- The prototype directory `prototypes/cortex-workspace/desktop/` and `docs/cortex-workspace/` are untracked. No commit contains the concept builds, so the source fingerprint below is the identifier for the prototype state.
- Ignored by `.gitignore`: `prototypes/cortex-workspace/desktop/node_modules/`, `prototypes/cortex-workspace/desktop/dist/`, `prototypes/cortex-workspace/desktop/.design-evidence/` and the repository-root `/.design-evidence/`.
- Source fingerprint (observed 2026-11-04), computed from the prototype directory:

  ```sh
  cd prototypes/cortex-workspace/desktop
  find src src-tauri index.html vite.config.ts tsconfig.json package.json package-lock.json -type f | sort | xargs sha256sum | sha256sum
  ```

  Result: `ae08c1db87dc7fc1b7cf86089a64dde3dde91812e172388db6c7b69ee4299cb2` (39 files under `src/`). Any later edit changes this value.
- Build output (`npm run build`, observed 2026-11-04): the shared shell and one lazy chunk per route with a content hash in its name.

| Concept | JS chunk | JS size (gzip) | CSS chunk | CSS size (gzip) |
|---|---|---|---|---|
| A | `dist/assets/a-DT0HdZ8o.js` | 91.95 kB (27.22 kB) | `a-CxlOHwMR.css` | 21.09 kB (4.67 kB) |
| B | `dist/assets/b-C-yNO5Zu.js` | 115.24 kB (32.59 kB) | `b-yM29g2E0.css` | 21.10 kB (4.52 kB) |
| C | `dist/assets/c-DSE5yPYO.js` | 84.26 kB (23.42 kB) | `c-Bd_gM446.css` | 17.06 kB (3.98 kB) |

The B and C sizes match the values recorded in their walkthroughs.

### 2.5 Checks run for this document (2026-11-04)

| Command | Working directory | Exit | Outcome |
|---|---|---|---|
| `npm --prefix prototypes/cortex-workspace/desktop run typecheck` | repository root | 0 | `tsc --noEmit` completed with no errors |
| `npm --prefix prototypes/cortex-workspace/desktop test` | repository root | 0 | 5 files, 39 tests passed: shell 1, concept A 6, concept B 6, concept C 10, fixtures 16 |
| `npm --prefix prototypes/cortex-workspace/desktop run build` | repository root | 0 | shared shell plus lazy chunks for A, B and C built |

Not run for this document: `npm ci` (existing `node_modules` were used), the dev servers, the browser walkthroughs and the native Tauri build (Cargo and rustc are not installed; see section 10).

## 3. Scenario coverage

The scenario set is defined in [desktop concepts](desktop-concepts.md): create → decision → result (S1), code/Git merge conflict (S2), the state sweep (S3), keyboard-only operation (S4), increased text size (S5) and desktop → phone → desktop (S6). Outcome labels used below:

- **Passed**: the walkthrough records every step of the scenario as observed.
- **Recorded**: captures exist and the walkthrough describes the state, but it does not record a pass or fail verdict for the whole run.
- **Not run**: the walkthrough says the run did not happen, or the combination is not recorded.

| Scenario | A 1280×800 | A 1920×1080 | B 1280×800 | B 1920×1080 | C 1280×800 | C 1920×1080 |
|---|---|---|---|---|---|---|
| S1 create → decision → result | Passed, keyboard only | Passed; the stale-result step was switched by a script click | Passed, keyboard only | Passed, pointer (single click previews, double-click pins) | Passed, keyboard only | Passed (§4.1 run at both viewports; steps as observed) |
| S2 code / Git conflict | Passed, keyboard only | Passed; conflict scenario selected by script click | Passed, keyboard (conflict set from prototype controls) | Passed, pointer | Passed, keyboard only (scenario set from the shared selector) | Passed; the Abort focus was placed by script before Enter |
| S3 state sweep | Passed; states switched by script through the scenario selector | Passed, same method | Passed, both viewports | Passed, both viewports | Recorded (captures 19–26) | Recorded (captures 18–25) |
| S4 keyboard only | Passed (S1 and S2 flows) | Same flows; see S1 and S2 | Passed in Chromium (covered by S1 and S2) | Not recorded | Passed for listed keys (§6) | Not recorded |
| S5 text size | 150%: unified diff shown. 200% preset: passed after fixes. 32 px root font: board usable | Not run | 150% and 200% (root 24 px and 32 px): passed | Not recorded | 200% preset: passed in opened captures. 150%: not run | Not recorded |
| S6 desktop → phone → desktop | Passed with the simulated phone dialog (script clicks) | Not run | Not run in browser; code path only | Not run | Not covered by concept C | Not covered by concept C |

Shared state coverage: empty, error, loading (an explicit preview toggle), blocked permission, failed check, stale result, offline (cached), disconnected, and the file and Git conflicts are captured at both viewports for A, B and C, with the exceptions listed under each concept below.

Notes on the table:

- S5 at 1920×1080 is recorded as not run for A and is not recorded for B and C.
- B's walkthrough marks the 1920 S1 captures `1920-s1-08`, `-10` and `-11` as predating the change that shows full fixture digests; use the stale-result captures for digest display.

## 4. Scenario observations

### 4.1 S1: create → decision → result

- **A**: the create form is an inline panel. Decisions are reached from the board card (Decisions tab) or from "Needs you". The result and diff screen shows the revision, the artifact digest, a checks table with an outcome source column, and the diff. The accept screen reads "Accepted (mock) · mock-result-complete" and states that accepting does not commit, push or deploy.
- **B**: "Needs you" in Mission Control lists the same decision types; a decision opens in group 2. The result view is the densest of the three: a freshness row, a checks summary (Passed, Failed, Not run, Unverified counts) and a per-file change table with "Open diff" and "Open in editor" actions. The accept step records "Accepted (mock)" and "Not runtime evidence or a Git action".
- **C**: a decision is a full-width block with labelled Effect, Scope, If you reject and Evidence fields above the actions. The block stays visible after the decision resolves.

### 4.2 S2: code / Git merge conflict

- **A**: the Git tab shows a conflict banner ("Merge stopped … Mock merge conflict; no Git operation runs"), a Conflicted/Staged/Unstaged count, and disabled Commit, Pull and Push, each with a specific reason. The save-conflict banner, a three-way compare and "Keep mine · Mock" are in the editor tab.
- **B**: the same conflict semantics appear in a side panel beside the editor. Commit is disabled. The fixture has one conflicted file (`synthetic/layout.ts`), so the walkthrough notes that "resolve one, leave another conflicted" cannot be shown.
- **C**: the conflict banner appears on the task's detail pane. "Keep local · Mock" records a mock choice and moves focus to the editor. Git deep mode lists the conflicted file first; Commit and Push stay disabled with "Resolve conflicts before committing."

In all three, a mock resolution does not unblock Commit: after "Mark resolved · Mock" the walkthroughs record that Commit stays disabled.

### 4.3 S3: state sweep

Each concept has captures for empty, error, loading, blocked permission, stale result, offline (cached) and disconnected states at both viewports (see section 5). Stale results disable acceptance and show the previous and new digests. Offline and disconnected states show the last confirmed time (2030-01-01 00:00 UTC in the fixture), relabel running executions as "Unknown", and disable mutating controls with a visible reason. B's walkthrough records a DOM query in the disconnected state that found five mock-labelled buttons, of which only the two "Reconnect · Mock" buttons were enabled.

### 4.4 S5: increased text

- **A** switches to a vertically scrolling layout at large text, with a pinned prototype strip and primary areas of 80 vh; the walkthrough records a fix for overlapping Discussion content at 200%.
- **B** switches to a compact task layout with a section select; at 200% with the side panel open both groups are 400 px wide. The bottom panel covered group 1 at 200% before a recorded fix.
- **C** switches to one column (queue, then focus pane) at 200%; the top bar wraps to three rows. The concept's root font is set through an in-app preset, not browser zoom.

### 4.5 S6: desktop → phone → desktop

Only A records a browser run, at 1280×800, using a simulated phone dialog. B's walkthrough reports that the phone answer shows as "Answered on Phone (simulated)" in the code path but was not run in the browser. C does not cover this scenario.

## 5. Screenshot paths

All paths are relative to the repository root. The directories are git-ignored (`/.design-evidence/`), so they exist only in a checkout where the captures were made. Counts observed 2026-11-04: A 71 entries (including the `_sweep/` subdirectory of superseded early captures, which the walkthrough says are not evidence of the final rendering), B 74 files, C 64 files.

### 5.1 Concept A (`.design-evidence/cortex-workspace/desktop/a/`)

- **1280×800 (`1280x800-*.png`)**
  - S1: `03-s1-create-form`, `04-s1-created-not-started`, `06-s1-question-decision`, `09-s1-blocked-permission`, `11-s1-failed-check-first`, `13-s1-result-diff`, `14-s1-accepted-mock`, `16-s1-stale-result-accept-disabled`, `17-s1-stale-acceptance-block`
  - S2: `20-s2-guide-conflict-script`, `22-s2-save-conflict-banner`, `23-s2-compare-keep-mine-mock`, `24-s2-git-conflict-commit-disabled`, `25-s2-marked-resolved-commit-still-disabled`
  - S3 states: `40-state-empty`, `41-state-error`, `42-state-offline-cached`, `43-state-disconnected-board`, `44-state-disconnected-decision-disabled`, `45-state-loading-preview`
  - S4: `02-keyboard-board-end-of-column`, `49-keyboard-help-dialog`
  - S5: `text150-result-unified-diff`, `text200-01-board-top` to `text200-06-discussion-no-overlap`, `browser-root-32px-board`
  - S6: `50-s6-simulated-phone-session`, `51-s6-changes-since-answered-on-phone`
  - Screens: `01-board-default`, `46-list-view-500-virtualized`, `47-needs-you-cross-task`, `48-connection-settings-unavailable-capability`, `30-workers-detail-running`, `31-timeline-10000-events-virtualized`
- **1920×1080 (`1920x1080-*.png`)**
  - S1: `02-s1-create-form` to `11-s1-stale-result-accept-disabled`
  - S2: `20-s2-save-conflict-compare-keep-mine`, `21-s2-git-conflict-commit-disabled`, `22-s2-marked-resolved-commit-still-disabled`
  - States: `40-state-empty` to `45-state-loading-preview`
  - Screens: `01-board-default`, `30-discussion-plan-rail-10000-events`, `46-list-view-500-virtualized`, `47-needs-you-cross-task`, `48-connection-settings-disconnected`
  - Not captured: S5 and S6 at 1920×1080.

### 5.2 Concept B (`.design-evidence/cortex-workspace/desktop/b/`)

- **1280×800 (`1280-*.png`)**
  - S1: `1280-s1-01-start-editor` to `1280-s1-17-git-still-unstaged`
  - S2: `1280-s2-01-prototype-controls-conflict` to `1280-s2-07-marked-resolved-commit-disabled`
  - States: `1280-state-empty`, `1280-state-error`, `1280-state-loading`, `1280-state-blocked-permission`, `1280-state-stale-result`, `1280-state-disconnected`, `1280-state-offline-cached`
  - Screens: `1280-screen-board-list`, `1280-screen-history-10000`, `1280-screen-search-results`, `1280-screen-search-empty`, `1280-screen-connection-settings`, `1280-screen-terminal-group2-tab`, `1280-screen-discussion-plan`, `1280-screen-worker-detail`, `1280-screen-keyboard-reference`
  - S5: `1280-text150-01-permission-task`, `1280-text200-01-settings-text-size` to `1280-text200-07-f6-region-focus`
  - Not captured: S6.
- **1920×1080 (`1920-*.png`)**
  - S1: `1920-s1-01-start-editor` to `1920-s1-11-git-still-unstaged`; `-08`, `-10` and `-11` predate the full-digest change
  - S2: `1920-s2-01-git-conflict-view` to `1920-s2-04-marked-resolved-receipt-focused`
  - States: `1920-state-empty`, `1920-state-error`, `1920-state-loading`, `1920-state-blocked-permission`, `1920-state-stale-result`, `1920-state-disconnected`, `1920-state-offline-cached`
  - Screens: `1920-screen-board-columns`, `1920-screen-history-10000`, `1920-screen-search-and-connection`, `1920-screen-terminal-bottom-one-group`
  - Not captured: S4 and S5 at 1920×1080 are not recorded; S6 not run.

### 5.3 Concept C (`.design-evidence/cortex-workspace/desktop/c/`)

- **1280×800 (`1280x800-*.png`)**
  - S1: `01-attention-default` to `10-accepted-codegit-summary-kbd` (`-07-acceptance-decision-kbd` and `-09-accepted-mock-kbd` show the decision block and the accepted state)
  - S2: `11-conflict-task-codegit-kbd` to `16-abort-merge-confirm-kbd`
  - Navigation: `17-all-tasks-list-kbd`, `18-board`
  - States: `19-state-stale-result`, `20-state-failed-check`, `21-state-blocked-permission`, `22-state-disconnected`, `23-state-offline`, `24-state-empty`, `25-state-error`, `26-state-loading`
  - Screens: `27-full-diff`, `28-history-10000-events`, `29-command-palette-kbd`, `30-long-text`, `31-keyboard-help-kbd`, `32-quiet-493-expanded`
  - S5: `text200-01-after-preset-top-kbd` to `text200-05-git-deep-kbd`
  - Not captured: S6; 150% text.
- **1920×1080 (`1920x1080-*.png`)**
  - S1: `01-attention-default` to `09-accepted-codegit-summary-kbd`
  - S2: `10-conflict-task-codegit-kbd` to `15-abort-merge-confirm`
  - Navigation: `16-all-tasks-list-kbd`, `17-board`
  - States: `18-state-stale-result` to `25-state-loading`
  - Screens: `26-full-diff`, `27-history-10000-events`
  - Not captured: S5 and S6 at 1920×1080.

Files ending in `-kbd` were reached by keyboard in the C walkthrough. In C's 1920×1080 captures, `15-abort-merge-confirm` was reached with focus set by script.

## 6. Keyboard observations

All keyboard observations come from the Chromium runs recorded in the walkthroughs. Keys were sent through Playwright, not a physical keyboard; reserved browser and desktop shortcuts were not tested in a desktop browser or in the Tauri webview.

Common to the three concepts:

- Visible focus was recorded on the captured controls. A uses a 2 px accent ring; B uses a near-white ring on its dark surfaces; C uses a 3 px teal outline with a 2 px offset (`c/c.css:21`).
- The S1 and S2 keyboard flows at 1280×800 reached the mutating controls without the pointer in all three concepts. Receipts stay in place after keyboard actions.
- Each concept has a skip link.
- Composite widgets use roving tabindex with arrow keys, Home and End. C adds J and K in the queue.
- Some shortcuts may be reserved by a desktop browser. The walkthroughs list `Ctrl+N` (A, C), and `Ctrl+W`, `Ctrl+Tab` and `Ctrl+Shift+N` (B) as candidates. C provides `Alt+N` for `Ctrl+N`. B provides `Ctrl+PageDown`/`Ctrl+PageUp` and `Ctrl+F4` alternatives and the command palette. A states that every action also has a visible control.

Concept-specific:

- **A**: `F6` and Tab reach the board's single Tab stop; `Ctrl+N` creates a task; `Ctrl+K` opens search; `Alt+2` to `Alt+5` switch task tabs; `Ctrl+\`` opens the fixture terminal. Not implemented: `Alt+←` and `Alt+→` history, saved views, keyboard terminal resize (pointer only), and a Settings → Keyboard page.
- **B**: `Ctrl+Shift+N` creates a task; `Alt+D` focuses the Decisions list; `Ctrl+K` opens the palette and `Ctrl+P` quick open; `Ctrl+Shift+G` opens Git; `F6` cycles status bar, activity bar, group 1 and group 2; `F1` or `?` opens the keyboard reference. Character-key shortcuts can be turned off in Settings.
- **C**: `Alt+N` opens the composer; `J`/`K`, arrow keys and Home move through the queue, and Enter selects; `Alt+3` and `Alt+5` toggle sections; `Ctrl+Shift+G` opens Git deep mode; `Alt+←` returns to the queue; `G` then `T` opens All tasks; `Ctrl+K` opens the palette; `?` opens help. Not exercised in the browser: `Ctrl+N` (browser-reserved), `F1`, `Shift+F6`, `[`/`]`, `Ctrl+Shift+E`, `` Ctrl+` ``, and the `G A` and `G C` sequences.

Keyboard defects recorded in the walkthroughs and fixed before capture include focus falling to the page body after a mock action (B and C), focus dropping to the page after Escape from the scenario guide (A), and a compact section select that was focusable while hidden (B). The source-level accessibility review found one remaining keyboard-focus problem in C (section 7.2, C-1).

## 7. Accessibility observations

### 7.1 Method and limits

- The source-level review read the shared shell and each concept's shell, primitives, dialogs and stylesheet. It did not render any page. The session had no working browser or assistive-technology tool.
- Contrast ratios were computed by hand from the literal token values (WCAG relative luminance). They are computed, not measured from rendered pixels.
- Not run: a screen-reader pass (NVDA, JAWS, VoiceOver or Orca), an automated scan (axe-core or similar), rendered focus visibility, real browser zoom and OS text scaling, and reduced-motion rendering.

### 7.2 Findings

| ID | Concept | Observation | Evidence | Status observed 2026-11-04 |
|---|---|---|---|---|
| A-1 | A | The custom `Dialog` renders `role="dialog"` and `aria-modal="true"` with a manual Tab loop and Escape to close. It does not set `inert` or `aria-hidden` on the background and does not use a native `<dialog>`. Screen-reader browse-mode navigation is therefore not constrained to the dialog. | `prototypes/cortex-workspace/desktop/src/concepts/a/ui.tsx:73` (the file contains no `inert` attribute, no `showModal` call and no `<dialog>` element); B and C call `HTMLDialogElement.showModal()` in `src/concepts/b/ui.tsx` and `src/concepts/c/ui.tsx` | Confirmed in source; assistive-technology impact not verified. Moderate. |
| C-1 | C | The focus pane is focused with `.c-focus:focus { outline: none; }`, which overrides the shared `:focus-visible` ring. The focus target falls back to the pane when no heading with id `c-focus-heading`, `c-deep-heading` or `c-all-heading` exists. The error and loading branches render no such heading, so a keyboard user entering those states from the skip link or region cycling gets no visible focus ring. | `c/c.css:21` and `c/c.css:88`; `c/index.tsx:55-57` (focus fallback); `c/index.tsx:269-273` (error and loading branches) | Confirmed in source; not confirmed in a rendered engine. Moderate. |
| C-2 | C | The status bar always reads `Fixture cw-fixtures-v1 · 500 tasks · 10,000 events`, even when the content area shows an empty project. A user reading only the status bar could take the count for the project. | `c/index.tsx:286` (unconditional fixture totals) | Confirmed in source. Moderate (state honesty; wording is labelled "Fixture"). |
| C-3 | C | The visual review saw a banner that combined the scenario "conflict" with the script "create-decision-result". Source shows the banner reads the shared scenario (`st().scenario`) and a concept-local script selector (`c/index.tsx:182`, `:195`, `:197`). The two selectors are independent, so the combination can occur. | `c/index.tsx:182`, `:195`, `:197` | Source explains the combination; it is a labelling and state-coupling issue, not confirmed as a capture artifact. Low. |
| A-2 | A | The live-data chip reads "Connected (simulated)"; B and C read "Connected (mock)". The mock disclaimer wording differs by concept. | `a/index.tsx:314`; `b/chrome.tsx:110`; `c/index.tsx:184` | Confirmed in source. Terminology only. |

No finding was recorded for B within the files the review read. The review states that this is not a clean bill of health for the whole of B: B's Git conflict, terminal, Problems and Output surfaces and Settings were not reviewed to the same depth.

### 7.3 Observations that held in the reviewed surfaces

- **Colour is not the only status signal.** Status uses text, a glyph and colour in all three concepts (sampled). "Mock" is marked in visible text and in the accessible name `(mock)`.
- **Live regions.** A uses persistent receipts with `role="status"`; B uses `role="status"` receipts; C has an `aria-live="polite"` region with a clear-then-set pattern to re-announce repeated text.
- **Disabled actions explain why.** All three keep stale or disconnected decisions visible with disabled actions and a reason, rather than hiding them.
- **Reduced motion.** All three stylesheets contain a `prefers-reduced-motion` rule (`a/a.css`, `b/b.css`, `c/c.css`). Rendering with the preference enabled was not checked.
- **Contrast (computed, sampled pairs).** All sampled pairs pass WCAG 1.4.3 AA (4.5:1 for text):

| Concept | Pair | Ratio |
|---|---|---|
| A (light) | `--mock` `#6A3D9A` on `--mock-t` `#F2ECF8` | 6.60:1 |
| A | `--muted` `#55554F` on `--canvas` `#F6F6F4` | 6.93:1 |
| A | `--danger` `#B42318` on `--danger-t` `#FCEDEB` | 5.78:1 |
| B (dark) | `--muted` `#A3A9B5` on `--bg` `#16181D` | 7.53:1 |
| B | `--warn` `#F2B84B` on `--warn-t` `#3A2E17` | 7.41:1 |
| C (light) | `--c-muted` `#4D5259` on `--c-canvas` `#FAFAF8` | 7.54:1 |
| C | `--c-teal` `#0A6C74` on canvas (link and focus colour) | 5.90:1 |

Disabled text is lower in contrast in all three, as WCAG permits for disabled controls. These are token-level values, not rendered measurements.

## 8. Performance observations

These are browser-page timings on development builds in headless Chromium. They are not process-launch, device, memory-on-device or reference-hardware measurements, and they do not qualify the targets in [performance](../performance.md). The three concepts used different measurement methods, so the values are not a benchmark comparison.

Frame granularity is about 16.7 ms at 60 Hz. Values near 33 ms mean the update fitted within two frames.

**Concept A** (`performance.mark` at component start and after two frames; keydown dispatch from a page script):

| Observation | Result |
|---|---|
| Board first render, component start to paint | 36.4 ms cold; 24.5–28.3 ms on reloads at 1280×800; 26.2 ms at 1920×1080 |
| Desk start after navigation start | 326.8 ms cold (on-demand transform of the lazy chunk); 31–36 ms on reloads |
| DOM with 500 tasks | 11 cards and 291 elements at 1280×800; 13 cards and 321 elements at 1920×1080 |
| Board keyboard roving, 60 keydowns | handler 1.3 ms median, 1.7 ms p95, 2.3 ms max; next frame 16.6 ms median, 16.8 ms max |
| Open a task from a card | handler 5.1 ms; 16.7 ms to next frame |
| 10,000-event timeline | 12 rows in the DOM; jump to middle and repaint 7.7 ms |
| JS heap (approximate, `performance.memory`) | about 12 MB at 1920×1080 |

**Concept B** (Playwright `browser_evaluate`, 1280×800 unless stated):

| Observation | Result |
|---|---|
| Project history, 10,000 events: option rows in the DOM | 29 at 1280×800; 39 at 1920×1080 (`aria-setsize` 10000) |
| Same list scrolled top to bottom over 120 frames | frame interval median 16.7 ms, p95 17.3 ms, max 17.7 ms; 0 frames over 33.4 ms |
| Board, 500 tasks: Backlog column at 1920×1080 | 496 tasks, 20 rows rendered (measured at a 4 rem row height, before the final 4.75 rem) |
| Side Board list (500 tasks) | 16 rows rendered; painted within 15.4 ms (two frames) |
| Mission Control: open task tab, open history | 33.2, 33.3, 33.4 ms (two-frame floor) |
| Editor input to next frame, 20 samples | median 16.7 ms, max 16.8 ms |
| Whole document | 520 elements; about 20 MB used heap |

**Concept C** (`performance.now()` before a programmatic click and after two animation frames; one sample per interaction):

| Interaction | 1280×800 | 1920×1080 |
|---|---|---|
| Expand Quiet tasks (493 rows) | 50 ms | not measured |
| Select a queue row with 493 quiet rows rendered | 33 ms | not measured |
| All tasks → List (500 table rows) | 47 ms | not measured |
| All tasks → Board (54 cards rendered) | 33 ms | not measured |
| Open project history (10,000 events, windowed) | 18 ms | 33 ms |
| Jump to end (1280) or middle (1920) of history | 33 ms | 33 ms |
| Rows in the DOM for the history list | 19 | 27 |

Whole document with the quiet list expanded: 4,672 elements. No long-task measurement was made.

What the values support: all three keep the DOM bounded for 500 tasks and 10,000 events by virtualizing or windowing the long lists. What they do not support: any statement about a performance target, device behaviour or a native build.

## 9. Trade-offs for the owner

This section lists trade-offs as the walkthroughs and reviews recorded them. It does not rank the concepts. Where all three recorded a pass for a scenario, the difference between them lies in the points below and in the open items in section 7.2.

| Dimension | A Task Desk | B IDE with Mission Control | C Focus Workspace |
|---|---|---|---|
| Entry point | Board, with a list option | Editor and Files, with Mission Control in a side panel | Attention queue, with the first decision open |
| Code access | Reached from the task (Code and Git tabs) | Always beside the task in group 1 | Reached from the task (Code and Git deep mode) |
| Decision clarity | Effect, scope and rejection are shown with other metadata | Shown with other metadata | Shown in a labelled block that persists after the decision |
| Information density at default view | Medium: board and filters | Medium: file tree, editor, collapsed rail | Medium-high: five grouped queue sections and the first decision |
| Two-work-area limit (sampled screens) | Board is one pane; an opened task adds a compact list beside the task workspace (two areas). The sources disagree on whether the list stays visible (section 11) | Two areas at most (group 1 and group 2); the Problems and Output placement is an open decision | One pane; deep mode collapses the queue to a rail |
| Mock disclaimer | Two-line disclaimer on acceptance | Concise and consistent | Both disclaimer wordings stacked |
| Known open findings | A-1 (dialog pattern), A-2 (terminology) | None recorded within the bounded review; open walkthrough gaps listed in section 10 | C-1 (focus ring), C-2 (status-bar count), C-3 (banner coupling) |
| Scenario gaps | S5 and S6 not at 1920×1080; S6 covered at 1280×800 only | S6 not run; S5 not recorded at 1920×1080 | S6 not covered; 150% not run |
| Editor | Plain text area with diff views | Plain text area (CodeMirror 6 not used; shared dependency change) | Plain text area |
| Fixture-driven limits | Four fixture phases projected onto seven columns | Board uses four fixture-derived phase values (backlog, working, review, done); candidate phase names are not in the fixture; one conflicted file | Four items in "Needs your decision" versus three in the specification example |

Points that depend on the owner's priorities, not on the evidence:

- Board-first triage (A) versus code-first work with the orchestration rail (B) versus attention-first decisions (C).
- Whether the B Problems and Output placement (open decision in B's walkthrough) is acceptable, or whether a single bottom panel should be the rule.
- Whether the status-bar count should be scoped to the current project (C-2), which affects C only.
- Whether the A dialog pattern should move to the native `<dialog>` pattern used in B and C (A-1).

## 10. Unrun and unverified checks

| Check | Concepts | Status | Blocker |
|---|---|---|---|
| Native Tauri 2 build and window | A, B, C | Not run | Cargo and rustc are not installed in the inspected environment (prototype README). |
| WebKitGTK rendering and webview shortcut handling (`Ctrl+W`, `Ctrl+P`, `` Ctrl+` ``, `Ctrl+Tab`) | A, B, C | Not run | Depends on the native build. |
| Screen-reader pass (NVDA, JAWS, VoiceOver, Orca) | A, B, C | Not run | No assistive-technology session was run for any concept. |
| Automated accessibility scan (axe-core or similar) | A, B, C | Not run | No scan was run; the review session had no browser automation. |
| Rendered focus visibility in the C error and loading states (C-1) | C | Not verified | Confirmed in source only; no rendered run. |
| Rendered contrast measurement | A, B, C | Computed only | No rendered measurement was taken. |
| Real browser zoom and OS text scaling; 150% and 200% at 1920×1080 | A, B, C | Not run | The in-app text-size presets were used instead of browser zoom. |
| S6 desktop → phone → desktop in a browser | A (1920×1080), B (both), C (not covered) | Not run | Not exercised in the browser runs. |
| S5 text size at 1920×1080 | A, B, C | Not run or not recorded | The walkthroughs do not record these captures. |
| Reduced-motion rendering | A, B, C | Not verified | Only the stylesheet rule was read. |
| Full screenshot inspection | A, B, C | Partly verified | The visual review opened 24 of 209 files. The walkthrough authors record opening all files for B and C and a subset for A; the remaining visual claims rest on filenames and walkthrough text. |
| Re-run of the browser walkthroughs for this document | A, B, C | Not run | The walkthrough results are cited as recorded, not re-observed. |
| Performance on reference hardware, physical devices, process launch and resident memory | A, B, C | Not run | The targets in [performance](../performance.md) require the native build on the reference machine. |
| Firefox, Safari/WebKit and other browsers | A, B, C | Not run | Only headless Chromium was used. |
| Real OS keyboard and reserved browser shortcuts | A, B, C | Not run | Keys were sent through Playwright. |
| ACP, service and network paths | A, B, C | Not applicable | The prototypes make no ACP, service, network or Git call. No paid live check is involved. |
| Gate D owner decision | A, B, C | Pending owner decision | Owner decision is required; see [desktop approval](desktop-approval.md). |

## 11. Project document discrepancies found while preparing this comparison

- `prototypes/cortex-workspace/desktop/README.md` (line 3) says concepts A, B and C "are placeholders for their owners, not completed concepts". The walkthroughs and the source show implemented concept modules and 39 passing tests. The README was not changed here because it is outside this document's scope.
- `docs/cortex-workspace/backlog/format.md` says "All entries are planned." It was not changed here; it is outside this document's scope.
- `docs/cortex-workspace/design/desktop-concept-a-walkthrough.md` says that opening a task places a compact task list beside the task workspace. The visual review says the task detail replaces the list pane. Section 9 follows the walkthrough. The disagreement was not resolved by re-inspecting the screenshot.
- The concept specifications ([A](desktop-concept-a.md), [B](desktop-concept-b.md), [C](desktop-concept-c.md)) were not changed. Their deviations are recorded in the walkthroughs (for example, the C "Needs your decision" count and the B Problems and Output placement).

## 12. Owner decision inputs

- The decision is recorded only in [desktop approval](desktop-approval.md). Until the owner records a selection or requested revisions, Gate D remains open and the mobile gate cannot start.
- A revision request is not approval, and this comparison does not treat any concept as selected.
- Items that the owner may want resolved before a decision, without any recommendation of a concept: A-1, C-1, C-2, the S6 coverage gap, the native build, and a screen-reader pass for the chosen direction.
