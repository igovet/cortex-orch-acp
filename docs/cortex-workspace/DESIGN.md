# Design direction

**Status: Gate D passed on 2026-11-05 for the clickable browser prototype. Production implementation is planned; no implementation or qualification is implied. The Conversation interaction model, selected by the owner on 2026-11-10, is built in the shared UI that the desktop app renders; what remains open is listed under [Conversation model](#conversation-model).**

Quiet, task-first tooling: neutral surfaces, restrained motion, readable state labels alongside color, no focus or press highlight for the pointer, and one quiet focus line for keyboard movement, which a dialog or panel also draws on the control it focuses as it opens, until the next pointer press. Keep at most two primary work areas visible. Never switch a user's active view automatically. Emphasize blocking decisions and concrete evidence rather than false progress percentages. Preserve observed, reported, estimated and unknown distinctions.

The tab strip uses the same quiet surfaces as the editor's tabs: a 36 px band on the sidebar surface, the active tab on the canvas surface, hairline separators and an always visible close control. A tab that is not on view keeps its layout and stops its work; the strip never animates and never switches tabs by itself.

Gaps are equal on the left and the right of everything, and nothing sticks to the edge of what holds it. WebKitGTK reserves no scrollbar gutter for a scroller with nothing to scroll, so scrollers keep a transparent track at all times and their right padding is the gap minus that track. Tier lists read from Light to Flagship.

Nothing moves or redraws unless what it shows changed. A read of the machine keeps every unchanged record and row as it was, so a row under a resting pointer never blinks. A loader sits inside its control and leaves the control's box alone. Something pending always moves, so the window never looks frozen: after 150 ms a pressed control's lower-edge bar sweeps back and forth, a wait for a reply or a page shows a briskly turning loader beside its words, and a long-running working status turns its loader slowly. Each moves only while its work is pending and pauses in a hidden window or a tab out of view; under reduced motion it pulses slowly instead of travelling. Rows that say nothing, such as a freshness stamp on live data, are left out. The one planned movement is a card crossing the board, which lasts 280 ms and is off under reduced motion. A sprint's dependency graph pans and zooms only under the owner's hand and never animates on its own.

Panels never float over the work. When a window runs out of room, its panels give way in a fixed order and come back when room returns, and giving way never changes a remembered choice; the panel the owner opened last stays. The sprint planning screen is the first built this way ([SCREENS.md](SCREENS.md#sprint-plan-list-graph-and-full-view-desktop)).

The native window has no OS title bar: the title band is the frame. Its minimize, maximize or restore and close buttons are real, the free part of the band moves the window, a double click on it maximizes, and the window edges resize. The demo prototype keeps the mockups' inert button group. The page, `body` and `#root` are the theme surface from edge to edge, and the window background is set to the same value before the first paint.

The layout is built from nine primitives in `packages/workspace-ui/src/layout.tsx`, styled only by `packages/workspace-ui/src/layout.css` in the `layout` cascade layer: Page (head, status lane, body and foot, where only the body scrolls), Toolbar (one fixed-height line in three zones), ActionBar (start group, reason slot and actions, used in a card, at a page's foot and in a dialog's footer), Action (a button with one box in every state), ReasonSlot (the one place a bar says why a control cannot be used), SectionHead (the only heading inside a panel), Strip (at most two lines with one action), Row (lead, a one-line title, up to two detail lines and an end) and Region (the inner scroller for content that must stay in view beside something pinned). Recorded output is the one other inner scroller: each labelled output box and edit diff scrolls sideways and vertically inside its own box and never wraps or clips a line, as described in SCREENS.md under live worker view. Colour, type and radii stay in the design tokens, and a surface adjusts a primitive through its variables rather than by overriding it. A reason never changes a control's box and never sits inside it: it goes to the bar's reason slot, or, for a control with no slot, to a box that a press or keyboard focus opens beside the control (`ReasonTip`). Text that is cut opens whole the same way, and never on hover; the title band's current-branch chip is capped in width and opens its whole name this way. Collapse follows the width of the pane or the title band, not the window: below 800 px the title band's branch chip and leading crumb shorten, and in the narrowest band, at the smallest window with the sidebar dragged wide, its state item and empty control places are dropped and the title keeps less room.

`mockup-styles.css` keeps the token blocks and the rules the app still uses from the accepted static mockup stylesheet, and `workspace.css` layers the frame on top of it. The static mockups in `prototypes/cortex-workspace/mockups` remain the visual reference. A test keeps the design tokens identical to them; the whole sheet is no longer kept byte-identical.

The interface uses the same fonts as Paseo. Paseo ships no typeface: its default interface stack is `system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif` and its code stack is `SFMono-Regular, Menlo, Monaco, Consolas, 'Liberation Mono', 'Courier New', monospace`, which on a Linux desktop resolve to Noto Sans and Liberation Mono. The shared `packages/workspace-ui` bundles those faces so every page looks the same on every machine and depends on no installed font: Noto Sans for the interface and Cousine, the open face Liberation Mono is built from, for code. `packages/workspace-ui/src/fonts.css` declares them, and the `--font` and `--mono` tokens in `mockup-styles.css` name them first, ahead of Paseo's stacks as fallbacks. Both are SIL Open Font License 1.1, pinned as `@fontsource-variable/noto-sans` 5.3.0 and `@fontsource/cousine` 5.3.0 in `packages/workspace-ui/package.json`, with Latin and Cyrillic subsets served from the app under `font-src 'self'`, with no network access. The licence text ships in the package at `/usr/share/doc/<package>/fonts.html`. The demo prototype renders the shared package, so it uses the same faces.

Labels are centred on their capitals. A control centres its label's line box, and each face's ascent is larger than its descent, so the capitals sit above the middle of that box: within half a pixel for Noto Sans, but about a pixel and a half for Cousine at 12.5 px, as measured in WebKitGTK. A code label inside a control or bar is therefore drawn 0.12 em lower (`mockup-styles.css`), which moves nothing around it. Font-metric overrides (`ascent-override`) are dropped by the installed WebKitGTK, and trimming the line with `text-box` clips the letters of a label cut with an ellipsis, so neither is used. `packages/workspace-ui/test/label-fit.js` holds every control, tab, chip, badge, status word and table head to one line and to capitals within 1 px of the middle of its box; the rendered layout gate and the page captures run it, and the captures also check the drawn pixels.

The accepted desktop direction is the Paseo-style variant based on concept A, accepted at Gate D on 2026-11-05 ([desktop approval](design/desktop-approval.md)). Its design tokens are the values in the accepted static mockup stylesheet, [`prototypes/cortex-workspace/mockups/styles.css`](../../prototypes/cortex-workspace/mockups/styles.css): the `:root` and `[data-theme="dark"]` blocks hold the dark values, `[data-theme="light"]` holds the light values, and the theme is chosen only in application settings. Mobile design follows desktop Gate D and starts with [CW-M01](backlog/CW-M01.md); mobile production implementation still requires Gate M. The round-one concepts and the [desktop direction](design/desktop-direction.md) are historical. The accepted presentation is promoted into the shared `packages/workspace-ui` package. The local desktop slice renders it in real mode, and the labelled demo prototype renders it with the mock backend; the production bundle contains no demo backend ([CW-S03](backlog/CW-S03.md)). See also [desktop concepts](design/desktop-concepts.md), [mobile concepts](design/mobile-concepts.md) and [fixtures](design/prototype-fixtures.md).

## Conversation model

**Status: selected by the owner on 2026-11-10 for every desktop page. Built in `packages/workspace-ui`: the task conversation, the shared composer for every message draft, the row control sizes, Home's single Needs you list, the planning conversation on narrow panes, the narrow-window sidebar give-way on every page and brand-free copy. Still planned: the rest of the page table in [SCREENS.md](SCREENS.md#conversation-adoption), whose status line names each area.** The static reference is [`prototypes/cortex-workspace/concepts/simple-b.html`](../../prototypes/cortex-workspace/concepts/simple-b.html), titled "Conversation: task screen concept". It uses sample data, and its visible branding is not a copy source.

The owner keeps every accepted visual token, font, radius and quiet interaction rule above. What changes is how a task is read and acted on. The journey is: describe the work, start it explicitly, read progress, answer an inline question, inspect the inline result, accept that exact result, and separately commit, merge or close it. Accepting a result never commits, merges or pushes it.

### Task anatomy

A task opens on its conversation, which has four parts:

1. A stable title band: project and task identity, one plain state, one next-action link for actionable content that is off screen, and the task menu.
2. One conversation column: the owner's brief, assistant messages, compact factual activity, settled answers, current decisions inline, and the current result only when it is eligible (see [result invariant](#result-invariant)). Entries keep their recorded order.
3. The message composer at the foot of the column.
4. Details (plan, team, usage, check output and diff), one activation away. At most the conversation plus one detail area is visible at a time.

The permanent plan strip and the five-tab bar leave the default view. Work details, Changes, Code and Terminal stay as secondary entries with their routes and deep links. Progress is stated as actual sentences; no percentage is invented. Context occupancy measures resource use, not completion, so it moves out of the composer into Details.

### Attention state

One presentation selector feeds the title, sidebar, Home, Inbox, board, list, Inspector and content. It reads current service facts, dispatches nothing and never replaces a service guard. Priority, highest first:

1. Disconnected, state unconfirmed or required data incomplete: Re-check state or the existing Retry. No message, Accept or integration is offered against unknown preconditions.
2. Stopping: re-check the stop; workers are not called stopped early.
3. Paused with a pending decision: Resume task first. The question stays readable, but submission is off until the task resumes.
4. Actionable permission, access, question or conflict: View question, Review permission or Resolve conflict, naming the real recipient and type.
5. Confirmed draft: Start task.
6. Fresh reviewable result with no pending decision: Review result, then Accept vN where eligible. When blocked, the primary action is the real recovery, never an Accept that looks enabled.
7. Accepted work awaiting integration: Review changes, then a separate Commit…, Merge… or Close task… confirmation. None of them is called Accept.
8. Working without a decision: a working status and the message input; Stop task is secondary.
9. Closed, archived, failed or cancelled: the recorded outcome and only the supported actions. Archive offers Restore.

When a question and a conflict coexist, the service's actual actionable dependency comes first. Confirmations and safety gates outrank convenience.

### Result invariant

While any question, permission or access decision is pending, including a held batch that is not yet complete, or while work or verification is active, no expanded completed-result card and no Accept control appears beside it. An earlier result stays available as "Previous result vN", marked historical and read-only. When service facts contradict completion, the view shows "State needs confirmation" with Refresh and never manufactures success. The rule applies to Home, board and list, Changes, Inspector and deep links alike.

## Unified message composer

One composer draws every message draft: task, answer, worker, planner, new task, new sprint, request changes, saved comment, guidance and stop reason. Where the mode applies, its anatomy is the same, from top to bottom:

1. An optional recipient or effect line, such as "Replying to QA Engineer" or "Sending reopens this task".
2. Attachment chips and the dictation transcript, only when they have content.
3. One labelled writing area that grows to a bounded height.
4. One tools row: Attach, Voice, model, effort, Settings (access and language), the delivery choice where offered, and the plane at the far end.
5. One reason or error slot, outside the controls.

Attach and Voice keep their positions in every mode. A control that cannot be used stays visible and disabled, with its reason. Model and effort appear wherever an engine route applies. They are omitted only where no engine call exists, such as a saved comment or a stop reason, and are never shown as a fake choice. A worker's recorded model and effort are read-only.

The send control is a paper-plane icon named for its real consequence: "Send message", "Send answer", "Send to <role>", "Send and reopen", "Save comment", "Save draft" or "Start planning". No generic "Send" may hide starting work or clearing an acceptance. The plane is the standard control in the row, not a round exception.

Start, Resume and Stop are task actions in the title band or an inline lifecycle item. They never replace the plane, and the plane never turns into Stop.

Where delivery is a choice (task messages), it sits beside the plane as Send now or Queue. A queued item is one line above the composer, with Edit, Send now, remove and reorder. Its files do not appear in the queue row; they return in Edit and in the sent conversation. Worker messages show their own delivery state and offer no interruption.

## Control sizing and spacing

One row-level size applies to every control in a row. Buttons, icon buttons, selects, segmented choices and adjacent single-line inputs share one border-box height, and the controls in a row match in top and bottom edges, radius, text and icon size.

| Context | Height | Text | Padding / icon |
| --- | --- | --- | --- |
| Compact (isolated dense rows only) | 24 px | 12/16 px, weight 500 | 8 px / 14 px |
| Standard toolbar or input row | 28 px | 13/18 px, weight 500 | 10 px / 16 px |
| Composer, dialog and form-action row | 32 px | 13/18 px, weight 500 | 10 px / 16 px |

These sizes hold at every window width and with every pointer: a phone-width pane gets the same compact controls as the desktop, never enlarged touch-size ones (owner decision, 2026-11-10). A narrow pane may move controls to another row; it never makes them bigger.

- No compact button beside a regular one, and no round control beside rectangular neighbours.
- Icon-only controls are square. A help button (`?`) has a fill and an edge like its neighbours; in a bar it is a square control of the bar's height beside them. The gap between text and icon is 6 px. Primary, line, plain and danger differ only in colour and border. The border is always 1 px, and hover, focus, selected and busy states never change geometry.
- Gaps: 8 px between inline controls, 12 px between groups, 16 px between sections and 12 px of composer padding. Base spacing is 4, 8, 12, 16, 24 and 32 px.
- Equal height does not force equal width; labels vary in width. A reason never sits inside a control box; it goes to the row's reason slot.
- Radii: control 6 px, menu 8 px, card 10 px, composer 16 px. These are the `--r-ctl`, `--r-menu`, `--r-card` and `--r-composer` tokens of the accepted mockup stylesheet.
- A menu or popover opens whole inside the window: one placed against a trigger near an edge, or opened from another menu, moves back in by its overflow, and one taller than the window scrolls inside it.
- The shared stylesheet enforces the row size: in any row that holds an actionable control and at least one more control, every button, icon button, select, segmented choice, chip and single-line field takes the row's `--ctl-h`. That is 28 px by default and 32 px in a dialog bar or composer, at every width; the controls in a section head are 24 px. A row of status chips alone keeps its own size.

## Brand-free interface copy

No interface text names the product. The product name remains only in the exceptions listed under [open choices](#open-choices). The prototype's labels are replaced in production:

| Prototype label | Production label |
| --- | --- |
| Message Cortex | Message |
| Message Cortex instead | Send a separate message |
| Tell Cortex something… | Write a message… |
| Latest from Cortex | Latest update |
| Cortex author | Assistant (see [open choices](#open-choices)) |

Configuration files and variables are shown by friendly names: "Project routing file", "User routing file", "Environment routing file", "No environment override", "Engine updates", "Engine version following" and "OpenCode executable". Exact paths and variable names stay internal and appear only behind an explicit "Copy file location" or "Copy variable name" action on the expert page. Wire identifiers, internal keys, imports, MIME types and code comments are not visible copy and are not renamed. User-entered names, source code, terminal output and historical text are never search-replaced.

## Responsive principle

Layout is fitted to the measured width of the pane, not the window. The same conversation, decision and result logic runs from the 960 × 540 desktop minimum down to phone width; the viewport rules are in [SCREENS.md](SCREENS.md#conversation-adoption). Below 820 px of window width every page holds the sidebar away without changing the owner's choice, and asking for the sidebar brings it back. This describes the shared UI only. It is not Android availability, a remote-access claim or a Gate M result.

## Visual rules kept

No one-sided accent stripes on cards, messages or callouts: use a whole border, a restrained semantic tint and spacing. No decorative dots, bullets or metadata pills, no gradients, glass, sparkle or floating blobs, and no second colour language. A required status uses text plus an icon, never a dot alone.

## Open choices

These are unresolved. Each stays open until the owner or a later review decides it; the text above does not decide it silently.

1. Queue shortcut. The shared UI follows the Conversation rule: the visible delivery choice alone changes the mode, and Ctrl/Cmd+Enter no longer queues ([SCREENS.md](SCREENS.md#task-send-modes)). The owner can still reverse this.
2. Coordinator message author. The copy rule above uses "Assistant" for coordinator messages, while SCREENS.md's "Interface words" rule uses "Coordinator" where a name is needed. Owner decision needed.
3. Product name in technical identifiers. The Diagnostics page's build and connection identifiers, real file paths such as the default worktree folder, and environment variable names keep the product name, because they are real identifiers. Whether the owner's "no Cortex in the interface" rule covers these exceptions needs confirmation.
4. Push-to-talk, and whether Esc cancels a recording, remain open as recorded in [SCREENS.md](SCREENS.md#voice-input).
5. Key bindings in general are proposals until the owner decides them.

## Verification status

Rendered WebKitGTK captures from fake data support the built statements: the task conversation in three states at 960 × 540, 1280 × 800, 1920 × 1080 and 390 × 844, plus dark at 1280 × 800 (`packages/workspace-ui/test/conversation-shots.mjs`), and every other page at 1280 × 800, 960 × 540 and 390 × 844 in the light theme (`packages/workspace-ui/test/pages-shots.mjs`, which fails when a page scrolls sideways, a row mixes control heights or the product name is shown). No contrast measurement, keyboard or assistive-technology pass, native window, installed package or voice run supports them yet, and the 820 px width and the other pages' dark theme were not captured.
