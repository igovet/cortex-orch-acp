# Desktop direction: Project Desk (Paseo-informed)

**Status: superseded on 2026-11-05.** The [rework proposal](desktop-rework-proposal.ru.md) and the static mockups in `prototypes/cortex-workspace/mockups/`, which the owner agreed as the design basis, govern where this document differs. In particular, the theme is chosen only in Settings (System default, with no sidebar, status bar or palette toggle), and selection never changes label weight or moves layout, so the selected-weight 500 rules in sections 2, 3 and 5 are superseded. This document is kept as historical evidence. Gate D is OPEN; no direction is approved. Historically, this was a mock-only, implementation-ready specification for the desktop prototype. No native build, qualification or approval is implied. Rendered contrast, screen-reader behaviour and native window behaviour are not established (section 13).

## Decision brief

**Conclusion.** One candidate direction: a native-feeling, project-first desktop workspace. It keeps the board-first flow of round-one concept A ([concept A](desktop-concept-a.md)) and moves everything inside a persistent **project sidebar**, as Paseo keeps its project groups in a sidebar. Visual language: layered neutral surfaces, 1px subtle borders, 14px system UI type at weight 400/500, a compact 40px title band, grouped rounded row containers, a neutral selection fill and one restrained accent used for focus, selection and links. Dark and light themes ship with a System / Light / Dark preference. Projects contain tasks. Both an all-projects dashboard and a per-project dashboard are first-class views.

**Observed evidence.** Seven Paseo captures were inspected: dark timeline 1280×800, light timeline 1920×1080, dark appearance 1920×1080, dark new-tab menu 1280×800, light settings 1280×800, dark overview 1280×800 and light keyboard focus 1280×800. The Paseo reference evidence set (source revision 1) and its sampled CSS measurements were read in full. Three round-one concept A captures were opened: 1280×800 board default, 1280×800 question decision and 1920×1080 accepted mock. The project's design documents (DESIGN.md, SCREENS.md, desktop-concept-a.md, desktop-approval.md, prototype-fixtures.md) and the prototype fixture source were read. The other six Paseo captures were not opened; the description of them is consistent with the seven that were.

**Checks.** WCAG 2.x contrast was recomputed on 2026-11-04 with a read-only Python script (`python3 -I -B -c`, exit 0) for every text and accent token in both themes across all seven surfaces (window, canvas, raised, overlay, hover, selected, input). All text and accent minimums in section 5 reproduce. One correction is recorded in section 5.2: `border.control` is below 3:1 on the hover and selected surfaces. The draft tokens failed the first computation and were adjusted (section 5.2 notes).

**Contradictions found.**
1. [DESIGN.md](../DESIGN.md) still asks for "three genuinely distinct alternatives". The owner has narrowed the work to one direction, so DESIGN.md, [desktop concepts](desktop-concepts.md) and [desktop approval](desktop-approval.md) are updated to say so. Gate D itself stays open.
2. The prototype fixture recorded in [desktop approval](desktop-approval.md) is `cw-fixtures-v1` (500 tasks). This specification maps `cw-fixtures-v2` (eight projects, four task phases) as read from the prototype source. Round-one concept A captures are v1, which used seven candidate phase columns. The prototype fixture revision must be aligned before implementation (section 11).
3. Concept A's `Ctrl+Alt+1…6` shortcuts are replaced here where browser or desktop environments may reserve them (section 7).

**Limits.**
- Paseo has no board, project dashboard or multi-column work view. Those layouts apply Paseo principles (sidebar, density, surfaces, title band, grouped rows) and are not evidenced Paseo layouts (risk R1).
- The Paseo reference has no hover journey, only one focus observation, no native outer window frame, and DOM measurements taken at 1249×900 rather than the screenshot sizes.
- Nothing was rendered for this direction. Rendered contrast, the screen-reader experience and real shortcut capture are unverified.

**Next action.** A frontend writer implements this as the new prototype route and records it in DESIGN.md and SCREENS.md (documentation sync applies to that writer). A rendered visual QA then runs at 1280×800 and 1920×1080 in both themes, and the owner reviews the result for Gate D.

---

## 1. Why round-one A read as "web", and the fix for each

Each defect is observed in the round-one A captures listed above.

| # | Observed in A | Why it reads as web, not desktop | Fix in this direction |
|---|---|---|---|
| W1 | A 28px full-width violet prototype strip with a long disclaimer, plus a bottom status bar repeating "Fixture data, not runtime evidence" and "Mock" chips | A loud site-wide announcement banner and a footer of chips | One compact **Mock chip** in the title band. It opens a Prototype popover holding the scenario, step, reset and full disclaimer. No status bar: connection lives in the sidebar footer, as Paseo keeps utilities there. |
| W2 | Top bar of bordered default controls: "Project: Synthetic Mosaic ▾", Board/List boxed buttons, "Search tasks Ctrl+K" button, "Text size 100% ▾", "Help" | A form toolbar of HTML button and select rectangles | A 40px title band with a breadcrumb in plain text, ghost icon buttons (28×28) and one search trigger. Text size moves to Settings → Appearance (Paseo keeps interface and content size there). |
| W3 | A filter row of three labelled native select elements, plus "Showing 500 of 500 tasks" | A search-results form page | One ghost "Filter" button opens a popover with checkable menu items. Active filters show as removable chips in the view-tab row. Count text appears only when filters hide something. |
| W4 | No sidebar; projects only in a dropdown; the page heading "Task Desk" acts as the app title | Single-page site information architecture; projects are second-class | A persistent **project sidebar** (Paseo: fixed sidebar with project groups and nested items), with global shortcuts at the top and utilities at the bottom. |
| W5 | White cards with 1px borders on grey column wells; two or three bordered, saturated amber or red pill badges per card; full monospace ID and "updated 02:38 UTC · fixture" on every card | Bootstrap card noise | Cards are quiet raised surfaces on the canvas with no column wells. At most one status chip plus one wait-reason chip, as tinted fills with no border. The footer is a short `#0002` plus a relative time in tertiary text. |
| W6 | Saturated royal-blue primary call to action "New task · Mock" in bold | A marketing call to action | The primary button is an inverted neutral, as Paseo's primary colour is near-white in dark and near-black in light. "New task" lives in the sidebar shortcuts. |
| W7 | Bold label/value grids ("Revision", "Artifact digest"), 600-weight headings everywhere | A documentation page | Paseo-style **grouped rows**: label on the left, value or control on the right, inside one 8px-radius container with 1px row separators. Weights are 400/500 only. |
| W8 | Tabs drawn as bordered boxes, the selected one with a blue outline | Web pill buttons | The tab strip uses text tabs; the selected tab gets a neutral fill (Paseo's selected tab and settings row use fill, not a coloured stripe). |
| W9 | Seven columns, empty ones full-height with "No tasks", and "Previous/Next columns" buttons | Horizontal paging of a web table | Four v2 phase columns fit 1280 without horizontal scroll. An empty column shows a one-line muted "No tasks" under its header, not a full-height box. |
| W10 | Two stacked violet bordered boxes for the mock receipt | Alert boxes | A one-line inline receipt: flask glyph and secondary text, no border, persisting where the action happened. |
| W11 | Warm paper canvas `#F6F6F4`, light only | Generic CMS feel; no dark mode | Layered neutral surfaces in both themes, with a window or sidebar tone darker than the canvas, as in Paseo. |

## 2. Principles derived from Paseo (evidence → rule)

| Paseo observation | Rule for this direction |
|---|---|
| Fixed 320px sidebar on a distinct darker or greyer surface (sidebar `#141716` dark / `#f4f4f5` light); top shortcuts at about 30px pitch; a muted 12px "Workspaces" group label; project rows with a small identity glyph; nested rows with status dots and trailing stats; a 40px bottom utility bar of 28×28 icons (all timeline captures) | The sidebar is the navigation spine: shortcuts, then a Projects group, then footer utilities. Rows are 28px high at 30–32px pitch. Trailing slots carry exactly one signal. |
| Main title chrome about 35px plus a 36px tab strip; content starts around y72; no page header in the timeline | Title band 40px (taller than Paseo to fit a breadcrumb at 200% text) plus a 36px view-tab row. No hero headers inside work views. |
| Message and composer column centred at 820px on wide windows; settings column about 688px (light timeline 1920, light settings) | Reading content is width-capped (discussion 760, settings 680) and centred in its pane. Boards and tables use the full width. |
| Grouped settings rows: about 8px container radius, 1px separators, about 16px insets, trailing 32px selectors (settings, appearance, overview) | Detail facts, decision anatomy, connection settings and Git use **grouped rows**. |
| Type: system-ui stack, UI 14px, content 15px, code 12px, weight 400, headings about 26px at normal weight (measurements) | System stack, 14/15/12, weights 400/500 only, page titles 22px at 400. |
| Neutral selection fill (sidebar selected `#272A29` / `#d4d4d8`); accent only on focus ring, toggles and online dot (overview, focus captures) | Selection is a neutral fill; accent is for focus, selected-card border, links and toggle-on. |
| Floating menu about 200px wide, 8px radius, icon / label / shortcut columns, section labels (new-tab menu) | All menus and popovers use this anatomy. |
| Thin outline icons, 14px with 1.5px stroke (Lucide observed) | Lucide-style 16px in chrome, 14px in chips, 1.5px stroke, always paired with text for status. |
| Chips: small fills such as "● Online", "localhost:16767", "v0.15.0" (dark overview) | Status chips are tinted fills, 20px high, 4px radius, glyph and text. |
| Focus: 2px solid outline in accent (light keyboard focus) | 2px accent ring, 2px offset (section 5.6). |

Not copied: the Paseo logo, project identity glyphs, the green brand accent (open question Q1), window controls, and the one-to-one sidebar content.

## 3. Navigation model and information architecture

```
App frame
├─ Sidebar (nav)
│  ├─ Shortcuts: New task · Search · Needs you (n) · All projects
│  ├─ Projects (8, fixture order)
│  │   └─ [expanded] up to 5 attention tasks + "All 220 tasks"
│  └─ Footer: Connections (7 of 8) · Theme · Settings · Help
└─ Main
   ├─ Title band: sidebar toggle · breadcrumb · [view switch] · Search · Mock chip · overflow
   └─ Route content
      ├─ /                          All-projects dashboard
      ├─ /needs-you                 Cross-project decisions list
      ├─ /p/:project/overview       Per-project dashboard
      ├─ /p/:project/board          Project board (4 phase columns)
      ├─ /p/:project/list           Project list (table)
      ├─ /p/:project/decisions      Project decisions
      ├─ /p/:project/git            Project Git state
      ├─ /p/:project/machine        Machine and connection
      ├─ /p/:project/t/:task/:tab   Task workspace
      │    tabs: discussion · decisions · result · workers · code · git
      │    (terminal = bottom panel in code and git)
      └─ /settings/:section         Settings (sidebar becomes settings navigation, as Paseo)
```

Rules:
- **Every view is a URL route** using the History API. Browser Back and Forward and in-app Back behave identically. Opening a task, project or settings page happens only on direct user action. **Incoming data never changes the route, the selected tab or focus.**
- **At most two primary work areas.** The sidebar is navigation, not a work area. Primary areas are either one view (dashboard, board, list, overview or settings), or the task list pane plus the task workspace. The plan rail at 1920 is a supporting panel inside the task workspace.
- **Projects contain tasks.** Every task belongs to exactly one project and is reached from its project's board, list, overview or the needs-you list. The all-projects dashboard summarises projects; it does not list tasks outside their project.
- **Project landing.** Clicking a project opens its last-used view; the first visit opens the **Board** (board-first, from concept A). The per-project dashboard is the **Overview** tab, one click or `Ctrl+Alt+1` away. A one-line project summary strip sits above the board, so the board is also a glance dashboard. (Open question Q2: Overview or Board as the first-visit default.)
- **Task open.** Selecting a card or row opens the task workspace in the main area. The **task list pane** (the same project's tasks, compact rows) shows beside it by default. `Ctrl+\` hides or shows it and the choice persists. The board's scroll position and focused card are preserved; Back or the breadcrumb returns to them with focus on the originating card.
- **Needs you** (sidebar shortcut, with a count) opens `/needs-you`, a cross-project list of open decisions. Each row opens that task's Decisions tab.
- **Settings** replaces the sidebar content with settings navigation and a "← Back" row, as observed in Paseo. Sections: *App* (Appearance, Keyboard); *Projects* (one row per project, linking to Machine and connection); *Prototype* (About this mock).
- **Connection settings** for a project are reachable from three places: the project's Machine tab, Settings → Projects → project, and the sidebar footer "Connections" button (a popover listing the eight machines with state and a link to each).

### 3.1 Sidebar specification

| Element | Specification |
|---|---|
| Container | Width 260 at 1280, 280 at 1920. User-resizable 220–360 by dragging a 4px handle on the right edge (cursor `col-resize`; keyboard: focus the handle and press ←/→ for 8px steps). Collapsible to 0 with `Ctrl+B` or the title-band toggle. Width and collapsed state persist. Never collapses automatically. Background `bg.window`; 1px `border.subtle` right edge. |
| Header row (y0–40) | "Cortex Workspace" 13/500 `text.secondary`, x16, vertically centred. Aligns with the title band height. |
| Shortcuts (y48 onward) | Rows 28px high, 30px pitch, 8px horizontal inset, radius 6, 16px icon and 14px label: **New task** (plus icon, `Ctrl+Alt+N`), **Search** (`Ctrl+K`), **Needs you** (trailing count chip in warn tint, for example "2"; no chip when 0, never "0" in warn), **All projects** (home). The current route's row gets `bg.selected`, label weight 500 and `aria-current="page"`. |
| Divider | 1px `border.subtle`, 8px vertical margin. |
| Group label | "Projects", 12/16 `text.tertiary` at x16. Trailing 24×24 ghost "Sort" button: Fixture order / Name / Last activity, persisted. |
| Project row | 32px pitch, 28px high. 16×16 **monogram tile** (first letter, radius 4, 11/500 text, fill chosen from a fixed 8-hue low-saturation set by index; decorative, `aria-hidden`). Name 14px, single-line ellipsis; full name in the accessible name and a 500ms tooltip. **Trailing signal slot**, highest priority only: **Offline** (plug-off glyph and text, `danger`) > **Conflict** (git-merge glyph and text, `danger`) > **Needs you n** (count chip, warn tint) > **Failed n** (x-circle and count, `danger`) > nothing. The accessible name lists every signal, for example "Mosaic Studio, 220 tasks, disconnected, worker state unknown". Chevron (12px) on hover or focus toggles expansion, persisted per project. Selected (current project) = `bg.selected` and weight 500. |
| Expanded project | Nested rows 28px, indent 28: up to five attention tasks ordered waiting-on-you → failed → running → stale. Each shows a 6px status dot **plus** short status text in 12px `text.tertiary` ("Permission", "Question", "Failed check", "Running", "Result stale"). Last row "All 220 tasks →" opens the board. With no attention tasks: one muted row "Nothing needs attention" followed by "All n tasks →". For Harbor Sandbox: "No tasks yet" and "New task in Harbor Sandbox". |
| Footer (bottom 40) | 1px top `border.subtle`; padding 6/8. Left: **Connections** button (plug glyph and "7 of 8 connected", 13px; danger glyph when any project is disconnected). Right: **Theme** (sun, moon or monitor glyph reflecting the preference), **Settings** (`Ctrl+,`), **Help** (`?`). Icon buttons 28×28. |
| Overflow | The projects list scrolls inside the sidebar; header, shortcuts and footer stay fixed. |

### 3.2 Title band (main area, 40px)

Left to right: sidebar toggle (28×28, always shown for consistency) · breadcrumb (14px; ancestors `text.secondary`, current `text.primary`; `nav aria-label="Breadcrumb"`; ancestors are links), for example "All projects / Cedar Notes / #0002 Allow mock workspace edit…", with the last crumb ellipsised and its full text in `title`. Right group: **Search trigger** (200×28, `bg.input`, 1px `border.control`, placeholder "Search", trailing "Ctrl K" key hint in 12px `text.tertiary`) · **Mock chip** · overflow "⋯" (28×28; Reset prototype, Keyboard shortcuts, About). Bottom 1px `border.subtle`. In the browser prototype **no fake window controls** are drawn. For a later Tauri build, reserve the right 144px and mark the band as a drag region (not implemented now).

**Mock chip.** Height 22, radius 4, `mock.tint` fill, flask glyph 14px and "Mock" 12/500 in `mock`, then "· fixture v2" in `text.tertiary`. It is a button: `aria-haspopup="dialog"`, accessible name "Prototype: mock data, fixture cw-fixtures-v2. Open prototype controls". The popover (320px, elevation e2) holds the scenario select (Default, Blocked, Stale, Offline, Disconnected, Conflict, Empty, Error), "Step n of m (fixture)" with **Next step** and **Reset**, the scenario guide text, and the fixed disclaimer: "All actions change only in-memory state. Fixture data, not runtime evidence." It persists across all routes, cannot be hidden, and is visible in every screenshot.

## 4. Screens and exact layouts

Coordinates are CSS px at 100% text. W = window width. S = sidebar width (260 at 1280, 280 at 1920).

### 4.1 All-projects dashboard (`/`)

**1280×800** (main x260–1280, content padding 32, content x292–1248, width 956):

```
y0   Title band 40: "All projects"                     [Search Ctrl K] [Mock·v2] ⋯
y40  ── scroll region ──
y72  H1 "All projects" 22/28 400
y104 Subline 13 tertiary: "8 projects · 500 tasks · 7 of 8 machines connected · as of 01 Jan 2030 02:46 UTC (fixture)"
y144 Section "Needs you" (label 12 tertiary + count), grouped container, rows 48:
       [key] Permission · Allow mock workspace edit?   Cedar Notes · #0002   requested —      ›
       [msg] Question · Which synthetic layout?        Orbit Ledger · #0003                  ›
y264 Section "Projects", grouped container, header row 32 + 8 rows 52:
       Project (280) | Machine (130) | Needs you (90) | Checks (150) | Workers (100) | Last activity (rest) | ›
y~720 Section "Since your last visit" (collapsed disclosure at 1280; shows count)
```

Project row anatomy: monogram 20 + name 14/500 + repository 12 mono `text.tertiary` beneath (two lines, 52px row). Machine: "Connected" (plug, neutral) or "Disconnected" (plug-off, danger), plus "Git conflict" on a second line when relevant. Needs you: number or "—" (`aria-label="none"`). Checks: "1 failed · 1 not run", or "No failed or unrun checks" in tertiary. Workers: number, or "Unknown ?" with `title` and accessible description "Machine disconnected; current worker count unknown". Last activity: relative ("2 min ago", relative to the fixture clock) with the absolute time in a tooltip; for Harbor Sandbox "No activity" (`null`), never "0" or "now". Row hover `bg.hover`; the whole row is one link to the project's landing view; focus ring inset.

**1920×1080** (main x280–1920, padding 48, x328–1872, width 1544): two columns, Projects 1120 | gap 32 | rail 392.
- Left: H1, subline, Needs you, Projects (adds a **Tasks** column: "220 · Backlog 212 · Working 1 · Review 0 · Done 0" as text, no bars, and a **Stale** column).
- Right rail "Since your last visit": a list of changes, for example "Answered on Phone · Which synthetic layout? · 14:05 (mock)", with rows 44 and an empty state "No changes since 01 Jan 2030 02:40 (fixture)".

### 4.2 Per-project Overview (`/p/:id/overview`)

Common header (both sizes), below the title band:

```
y40  View tabs 36: Overview | Board | List | Decisions 1 | Git | Machine       [Filter] (board and list only)
y76  Project header 64: monogram 24 · "Cedar Notes" 22/400 · repo and branch 12 mono tertiary
                        chips: [plug Connected] [branch main] [git Clean]
```

Overview body, 1280 (content width 956):
- **Stat row** (y156, height 72). Six stat cells in one grouped container, divided by 1px vertical separators, no shadows: Tasks 90 · Needs you 1 · Failed checks 0 · Not run 0 · Running workers 0 · Stale results 0. Each cell: label 12 tertiary on top, value 20/24 500 `tabular-nums`. Unknown renders "Unknown" 14px with a "?" glyph and a second line "Machine disconnected". Non-zero attention values (Needs you, Failed, Not run, Stale) are links filtering the board. Zero renders "0" in `text.secondary`, with no colour.
- **Phases** grouped row: "Backlog 88 · Working 0 · Review 0 · Done 1", each a link to its board column.
- **Needs you** (project-scoped decision rows, as on the dashboard).
- **Attention**: failed checks, stale results, conflicts and unavailable capabilities, each a row with glyph, label and task link.
- **Machine** grouped rows: Machine "Synthetic workstation 2" · State · Last confirmed.
- **Recent activity**: the last 10 events, 13px rows, "Show all in task".

1920: stat row and phases full width; below, two columns (Needs you and Attention, 740 | Machine and Recent activity, 740).

### 4.3 Project board (`/p/:id/board`)

**1280×800:**

```
y0   Title band 40
y40  View tabs 36 (Board selected)                                   [Filter ▾]
y76  Summary strip 36 (13px secondary, numbers primary):
     "90 tasks · 1 needs you · 0 failed · 0 not run · 0 running · last activity 2 min ago"
y112 Board region (scrolls per column), padding 16 top / 20 sides
     4 columns: width (1020 − 40 − 3×12)/4 = 236, gap 12
     Column header 28: "Backlog" 13/500 secondary + count 12 tertiary tabular
     Cards stack, gap 8, column scrolls independently (virtualized)
y800
```

**1920×1080:** columns flex at min 240 / max 400 → (1640 − 48 − 3×16)/4 = 386, gap 16, padding 24. The summary strip adds "Stale 0".

**Card** (raised surface, 1px `border.subtle`, radius 8, padding 10/12, min height 76):
- Line 1: title 13/18 `text.primary`, two-line clamp (full title in the accessible name and on focus or hover via `title`).
- Line 2 (only when relevant): execution chip (Running / Blocked / Waiting / Failed / Unknown; "Not started" is **not** a chip, just muted text "Not started") + wait-reason chip ("Your permission", "Your answer", "Result review"). Check chip when present: "Failed check" / "Not run".
- Line 3: `#0002` 12 mono tertiary · "updated 2 min ago" 12 tertiary. A mock-created task adds a trailing "Mock" 12px in `mock`.
- **Waiting on you**: 1px `warn` border (≥5.8:1 against raised). No side stripe, no glow.
- **Selected** (the task open in the workspace): 1px `accent` border + `bg.selected`. **Focus**: 2px inset accent ring (`outline-offset: -2px`). Selected and focused: both, ring outside the border.
- Hover: `bg.hover` raised variant; cursor `default`.
- Columns are read-only projections: no drag handles, no drop targets.

### 4.4 Project list (`/p/:id/list`)

A table filling the main area. Sticky header 32 (12/500 secondary, sort button per column, `aria-sort`). Rows 36, virtualized for 220 rows. Columns at 1280: Title (flex), Status 130, Wait reason 140, Phase 100, Updated 120. At 1920 it adds ID 90 and Checks 140. Selected row `bg.selected` + `aria-selected="true"`; focus ring inset. One Tab stop; ↑/↓ move, Enter opens.

### 4.5 Task workspace (`/p/:id/t/:task/:tab`)

**1280×800:** sidebar 260 | task list pane x260–540 (280) | workspace x540–1280 (740).

```
y0   Title band 40 (breadcrumb ends with the task)
y40  Workspace header 76 (padding 16/24):
       title 18/24 500 (wraps fully, never truncated here)
       meta row 13: [Phase: Backlog] · [chip Blocked] · [chip Your permission] · "observed" · "Last confirmed 02:46 (fixture)" · #0002 (copy)
y116 Tab strip 36: Discussion · Decisions 1 · Result · Workers · Code · Git   (selected = bg.selected fill, radius 6, 28 high)
y152 Tab content (scrolls), padding 24
```

Task list pane: header 36 ("Cedar Notes · 90" + Filter icon + hide button), rows 44 (title 13 single-line ellipsis, status text 12 tertiary), selected row `bg.selected`.

**1920×1080:** sidebar 280 | list 360 | workspace 1280. The Discussion tab has a timeline column (max 760, left-aligned at 32 padding) plus a **plan rail** 320 on the right (sticky, own scroll; `aside aria-label="Plan"`), gap 32. The Result tab uses a side-by-side diff.

Tab contents:
- **Discussion.** Paseo-like timeline: user messages right-aligned in a `bg.raised` bubble (radius 12, max 70% of the column); agent and runtime text left, unboxed, 15/22. Tool events grouped in one collapsed row: "48 tool events · reported · Show". Decision events appear as inline links. At 10,000 events the list is virtualized with "Load earlier" at the top; when the user is not at the bottom, a floating "12 new events · Jump to latest" pill appears (it never auto-scrolls). The composer is pinned at the bottom: radius 12, `bg.raised`, 1px `border.default`, min 88 high, placeholder "Reply in this task's discussion", trailing **Send · Mock**. At 1280 the plan is a collapsible "Plan · 3 of 5 steps reported done" disclosure above the timeline (collapsed state persists).
- **Decisions.** One grouped container per decision (section 6.3 anatomy).
- **Result.** Header grouped rows, then Checks, Changes, Diff, and the Acceptance block (always below the evidence).
- **Workers.** Table: execution, attempts, worker rows (Role · Engine/model if exposed · State · provenance · Last event). A row opens the detail in place, with a back link.
- **Code.** File tree 240 plus editor area. The editor tab strip shows "Unsaved" as text. A lease line sits in the editor header. The terminal is a bottom panel toggled by `` Ctrl+` ``, default height 240, resizable, and carries the header "Fixture terminal: output is recorded; commands are not executed".
- **Git.** Grouped rows: Worktree (path · branch · HEAD), then Conflicted / Staged / Unstaged groups, commit message, then Pull · Mock and Push · Mock (confirm dialog). The terminal panel is available here as well.

### 4.6 Create task (modal)

Opened by New task (sidebar), `Ctrl+Alt+N`, the palette, or an empty-state button. Native `<dialog>` with `showModal()`, width 560, radius 12, elevation e3, centred, scrim.
- Title "New task" 16/500.
- Fields: **Project** (select-like menu button, preset to the current project, or the first project on the dashboard), **Title** (required, single line, visible label, inline error "Enter a title" under the field on submit), **Details** (optional, 6 lines, grows to 12), **Start** (radio: "Discuss first" (default) / "Save as draft").
- Footer: right-aligned **Cancel** (secondary) and **Create and open · Mock** (primary, disabled while the title is empty, with the reason "Enter a title" shown as helper text, not hover-only).
- Initial focus goes to Title. Esc or Cancel closes and returns focus to the invoker. Enter in Title submits.
- On submit: the dialog closes and the new task's workspace opens. The button label states this consequence, so it is not an automatic switch. The card appears in Backlog and the board scroll is preserved. The workspace shows "Not started (mock)" and the inline receipt "Mock outcome: task recorded in memory only. No agent was started."
- Disconnected project selected: the submit button is disabled with "Cedar Notes machine is disconnected; tasks can't be created".

### 4.7 Connection settings (`/settings/projects/:id` and the project Machine tab)

Settings layout (Paseo): the sidebar shows "← Back", then groups App / Projects / Prototype (rows at 30px pitch). Main: centred column 680 (x = S + (W − S − 680)/2), H1 22/400 at y72, section labels 12 tertiary, grouped rows (min 52; label 14 with description 12 tertiary on the left, value or control on the right).
- Machine: Name · State ("Connected" / "Disconnected · last confirmed 01 Jan 2030 00:00 UTC") · Current execution state ("Confirmed" / "Unknown").
- Relay: URL (default shown, plus an override text field and **Test connection · Mock**).
- Devices: a list, each row with **Revoke · Mock** (opens a confirm dialog).
- Capabilities: "Android push · Unavailable — Intentionally unavailable in the mock prototype." (circle-slash glyph, neutral; Indigo Maps).
- Disconnected (Mosaic Studio): inline danger notice at the top of the column, with **Reconnect · Mock**.
- Not permitted: a lock glyph and the reason text under the disabled control, for example "This device is not enrolled. Enroll it in Devices."
- Appearance section: Theme (System / Light / Dark segmented control), Interface text size (100/125/150/200%), Content size, Reduce motion (System / On / Off).

### 4.8 Command palette and project switcher

Both are native `<dialog>.showModal()`: width 640, top offset 96, radius 12, elevation e3.
- Input 44 high with 15px text and no border inside the container; 1px separator below.
- Results grouped by 12px tertiary labels (Projects, Tasks, Commands), rows 36: icon or monogram, label, secondary context (project for a task), right-aligned shortcut hint. Maximum 8 rows visible, then scroll.
- ↑/↓ move the active option (`aria-activedescendant`; the active row gets `bg.selected` and an inset ring), Enter runs it, Esc closes and restores focus.
- Empty results: "No matches for "foo"" plus "Search all tasks in all projects" as a command.
- The **project switcher** (`Ctrl+O`) is the same component scoped to projects. Rows show monogram, name, trailing signal and task count.
- Palette commands include: New task, Go to project…, Needs you, All projects, Toggle sidebar, Toggle task list, Theme: System / Light / Dark, Keyboard shortcuts, Prototype: Next step / Reset / Scenario….

## 5. Design tokens

### 5.1 Colour: dark theme (default when the system prefers dark)

WCAG 2.x ratios. "min" is the lowest ratio across window, canvas, raised, overlay, hover, selected and input surfaces (recomputed 2026-11-04; all values reproduce).

| Token | Hex | Use | Contrast |
|---|---|---|---|
| bg.window | #141618 | sidebar, title band, settings nav | none |
| bg.canvas | #1A1C1F | main content | none |
| bg.raised | #212427 | cards, grouped rows, composer | none |
| bg.overlay | #282B2F | menus, popovers, dialogs | none |
| bg.hover | #24272A | hover on window and canvas rows | none |
| bg.selected | #2C3034 | selected row, tab, nav | none |
| bg.input | #16181B | text inputs, search trigger | none |
| border.subtle | #2A2E32 | separators (decorative) | 1.14–1.33 (decorative, exempt) |
| border.default | #363A3F | group and dialog outlines | 1.36–1.58 (decorative) |
| border.control | #71777E | input, checkbox and radio boundaries | window 4.01 · canvas 3.77 · raised 3.45 · input 3.93 · overlay 3.14 · hover 3.32 · **selected 2.94** (see 5.2 note) |
| text.primary | #EDEEF0 | primary text | min 11.45 (canvas 14.71) |
| text.secondary | #A4A9AF | labels, ancestors | min 5.62 (canvas 7.21) |
| text.tertiary | #979DA4 | metadata, section labels | min 4.86 (canvas 6.24) |
| text.disabled | #6B7178 | disabled labels only | 2.70–3.68 (exempt; reasons use secondary) |
| accent | #7AA7FF | focus ring, links, selected-card border, toggle-on | min 5.57 (selected); canvas 7.15 |
| info (running) | #7AA7FF | Running state | as accent |
| success | #6CC287 | Passed, Connected dot | min 6.14 |
| warn | #E2A85F | Waiting, needs you, stale | min 6.33 |
| danger | #F28B80 | Failed, Disconnected, Conflict | min 5.56 |
| mock | #BC9EF2 | Mock chip and tags | min 5.90 |
| tint.* | status at 16% over raised: success #2D3D36 · warn #403930 · danger #423435 · info #2F394A · mock #3A3847 | chip fills | status text on its tint: 5.30 · 5.42 · 4.95 · 4.87 · 5.07; primary text on tints 9.80–10.20 |
| diff.add / diff.del | #25332E / #382C2D (14% over canvas) | diff line backgrounds | primary text 11.36 / 11.55; secondary 5.57 / 5.66; +/− gutter glyph 6.10 / 5.61 |
| primary.fill / on | #EDEEF0 / #141618 | primary button | 15.62 |
| danger.fill / on | #C2473C / #FFFFFF | destructive confirm button | 4.93; fill vs canvas 3.46 |
| scrim | rgba(0,0,0,.50) | modal backdrop | none |

### 5.2 Colour: light theme

| Token | Hex | Contrast |
|---|---|---|
| bg.window | #F3F4F5 | none |
| bg.canvas | #FFFFFF | none |
| bg.raised | #FAFAFB | none |
| bg.overlay | #FFFFFF (+ e2 shadow + border.default) | none |
| bg.hover | #ECEDEF | none |
| bg.selected | #E3E5E8 | none |
| bg.input | #FFFFFF | none |
| border.subtle | #E7E8EB | decorative 1.11–1.23 |
| border.default | #DADCE0 | decorative 1.25–1.37 (cards in light also rely on this; their content is identifiable without it) |
| border.control | #878C93 | window 3.07 · canvas 3.39 · raised 3.25 · input 3.39 · overlay 3.39 · **hover 2.89 · selected 2.68** (see note) |
| text.primary | #1A1C1F | min 13.53 (canvas 17.08) |
| text.secondary | #585D64 | min 5.26 (canvas 6.64) |
| text.tertiary | #5F646B | min 4.72 (canvas 5.96) |
| text.disabled | #9A9EA4 | 2.13–2.69 (exempt) |
| accent / info | #2A5BD0 | min 4.75 (selected); canvas 5.99 |
| success | #246C3B | min 5.07 |
| warn | #8A5710 | min 4.82 |
| danger | #B0372D | min 4.85 |
| mock | #6B42AE | min 5.52 |
| tint.* (10% over raised) | success #E5ECE8 · warn #EFEAE3 · danger #F3E6E6 · info #E5EAF7 · mock #ECE8F3 | status on tint 5.33 · 5.08 · 5.04 · 4.98 · 5.77; primary text 14.04–14.27 |
| diff.add / diff.del | #E9F0EB / #F7EBEA | primary 14.74 / 14.66; secondary 5.73 / 5.70; glyph 5.52 / 5.25 |
| primary.fill / on | #1A1C1F / #FFFFFF | 17.08 |
| danger.fill / on | #B0372D / #FFFFFF | 6.12 |
| scrim | rgba(16,18,20,.32) | none |

Notes:
- Hover tints for canvas rows (dark #222528, light #F5F6F7) sit between computed neighbours. Their text ratios are bounded by those neighbours (≥5.70 for tertiary in dark, ≥5.41 in light). This is inferred, not separately computed in the table, but the recomputation on 2026-11-04 confirms the bounds on the hover surface itself (dark tertiary 5.49, light tertiary 5.09).
- **Correction (2026-11-04 recomputation).** The earlier computation used five surfaces for control boundaries. Across all seven surfaces, `border.control` is 2.94 on dark `bg.selected` and 2.68 (light) on `bg.selected`, and 2.89 on light `bg.hover`. Control boundaries therefore reach 3:1 only on window, canvas, raised, input and overlay surfaces. Rows that show a hover or selected fill must not rely on `border.control` as the only boundary of a control. Raising the token is an owner or designer decision (risk R7).
- Draft tokens failed the first computation and were adjusted: dark border.control #61676E (2.73 on raised), dark tertiary #8A9097 (4.13 on selected), light tertiary #6A6F76 (4.01 on selected) and light success #2B7A43 (4.44 on its tint). The final values are the ones in the tables above.
- **Selection fill alone is 1.26–1.36:1 against the canvas** (recomputed 1.28 dark, 1.26 light). Selection is therefore never fill-only: rows add label weight 500 and `aria-selected` or `aria-current`, and cards add a 1px accent border (≥5.74:1). Paseo also uses a fill-only selection, so whether WCAG 1.4.11 requires more is recorded as risk R3.

### 5.3 Typography

- **UI stack:** `system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Ubuntu, Cantarell, "Noto Sans", Roboto, Helvetica, Arial, sans-serif` (Paseo's measured stack plus Ubuntu and Cantarell for the Ubuntu target).
- **Mono stack:** `ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Ubuntu Mono", "DejaVu Sans Mono", monospace`.
- No webfonts; nothing bundled.
- Sizes are rem (root 16px = 100% preset), line heights are unitless, and `font-variant-numeric: tabular-nums` applies to counts, times and stats.

| Token | Size / line | Weight | Use |
|---|---|---|---|
| text.xs | 12/16 | 400 (500 for chip labels) | metadata, section labels, chips, key hints |
| text.sm | 13/18 | 400/500 | cards, table cells, nested sidebar rows, breadcrumb ancestors at 1280 |
| text.base | 14/20 | 400/500 | UI default: sidebar rows, buttons, tabs, grouped rows |
| text.content | 15/22, max 72ch | 400 | discussion prose, decision titles in body |
| text.md | 16/22 | 500 | dialog titles, section headings in panes |
| text.lg | 18/24 | 500 | task title |
| text.xl | 22/28 | 400 | page titles (dashboard, overview project name, settings) |
| text.stat | 20/24 | 500 | stat values |
| mono.sm | 12/18 | 400 | IDs, digests, paths, code, diff, terminal |

Weights 400 and 500 only. No uppercase tracking labels, no italics for status.

### 5.4 Spacing, size, radii, borders

- **Spacing scale:** 2 · 4 · 6 · 8 · 12 · 16 · 20 · 24 · 32 · 48. Micro gaps 4 and 8 inside rows; 24–32 between groups (Paseo density).
- **Control heights:** 28 (buttons, tabs, icon buttons, search trigger), 32 (selects and segmented controls in settings), 36 (inputs in dialogs, table rows, palette rows), 44 (palette input). Minimum pointer target 24×24; all rows ≥28.
- **Padding:** buttons 0/10 (icon and label gap 6); inputs 8/12; grouped rows 12/16; cards 10/12; dialogs 20/24.
- **Radii:** 4 for chips, badges, checkboxes, monograms; 6 for buttons, inputs, rows, tabs; 8 for cards, grouped containers, menus, popovers; 12 for dialogs, palette, composer and user message bubble; full only for status dots, toggles and the "Jump to latest" pill.
- **Borders:** 1px everywhere. `border.subtle` for separators, `border.default` for containers, `border.control` for input boundaries. No 2px decorative borders, side stripes or coloured left bars.

### 5.5 Elevation

| Level | Use | Dark | Light |
|---|---|---|---|
| e0 | canvas, sidebar | none | none |
| e1 | cards, groups | none (raised fill + subtle border) | `0 1px 2px rgba(16,18,20,.04)` |
| e2 | menus, popovers, tooltips | `0 8px 24px rgba(0,0,0,.45)` + 1px border.default | `0 8px 24px rgba(16,18,20,.12)` + 1px border.default |
| e3 | dialogs, palette | `0 16px 48px rgba(0,0,0,.55)` + 1px border.default | `0 16px 48px rgba(16,18,20,.18)` + 1px border.default |

Z-order: base 0 · sticky headers 10 · resize handles 15 · popovers and menus 20 · toasts (none) · dialogs: browser top layer via `showModal`.

### 5.6 Focus ring

- `:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }`, radius following the element.
- Inset variant (`outline-offset: -2px`) inside scroll containers, virtualized rows, cards and tabs, so the ring is never clipped.
- Accent against every adjacent surface: dark ≥5.57, light ≥4.75.
- Elements that receive programmatic focus (headings with `tabindex="-1"`, panes, error and loading containers) **also show the ring** via `:focus`, so focus never lands on an invisible target.
- `forced-colors: active` uses `outline-color: Highlight`.

### 5.7 Motion

| Token | Duration / easing | Use |
|---|---|---|
| motion.instant | 0 ms | theme change, selection, route change |
| motion.fast | 100 ms linear | hover and pressed colour |
| motion.popover | 140 ms `cubic-bezier(.2,0,0,1)` | menu and popover: opacity 0→1 plus translateY 4→0 |
| motion.dialog | 160 ms, same easing | dialog and palette: opacity plus scale .98→1; scrim fade |
| motion.panel | 160 ms, same easing | sidebar or list-pane collapse via transform; layout snaps at the end |

- Only `opacity` and `transform` animate.
- No looping animation except the skeleton pulse (opacity .55↔1, 1.4s), which becomes static under reduced motion.
- **Running** is a static glyph and label, with elapsed time updated each minute.
- `prefers-reduced-motion: reduce` (or the Settings override) sets every duration to 0.

## 6. Components and state matrix

### 6.1 Status encoding (glyph + text + colour; colour never alone)

| Dimension | Values (visible text · glyph · colour) |
|---|---|
| Execution | Not started (muted text, no chip) · Running (half-circle, info) · Blocked (octagon-pause, warn) · Waiting (pause, warn) · Failed (x-circle, danger) · Complete (circle-check outline in neutral, never success-green) · Unknown (?, neutral) |
| Wait reason | Your permission (key) · Your answer (message-question) · Result review (file-check), all warn tint; Review (runtime) (eye) · Verification (runtime) (clipboard-check), neutral |
| Check | Passed (check-circle, success) · Failed (x-circle, danger) · Not run (hollow circle with dash, neutral) · Unverified (?, neutral). Not run and Unverified are **never green**. |
| Connection | Connected (plug, neutral text with a success dot) · Reconnecting (plug-zap, warn, static text "Reconnecting (attempt 3)") · Disconnected (plug-off, danger) |
| Freshness | Current (none) · Result stale (history, warn) · As of HH:MM (clock, neutral) |
| Provenance | observed · reported · estimated ("est." prefix) · unknown ("unknown ?"), muted text after a middle dot in a fixed slot |
| Mock | flask and "Mock", mock colour |

Chips: height 20, padding 0/6, radius 4, glyph 14, text 12/500, tint fill, no border, single line, never wrapping.

### 6.2 Component × state matrix

Legend: H hover · S selected · F focus · D disabled · Ld loading · E empty · O offline or stale · X disconnected · P permission blocked · C failed check · K conflict · Er error · L long text. "—" means not applicable.

| Component | Default / H / S / F / D | Ld / E / Er | O / X | P / C / K | L |
|---|---|---|---|---|---|
| Sidebar shortcut row | transparent; H bg.hover; S bg.selected + 500 + `aria-current`; F ring inset; D (Needs you when unknown) text.disabled + tooltip reason | Ld: count chip replaced by a 16px skeleton; Er: count shows "?" with reason "Couldn't load decisions" | X: Needs you count shows last known + "as of HH:MM" in the tooltip and accessible name | P: count includes permission requests | label ellipsis, full in accessible name |
| Project row | as above; chevron on H/F | Ld: skeleton name bar 60%; E (Harbor Sandbox): row normal, trailing none, nested "No tasks yet" | Offline signal (danger); worker count "unknown" | Conflict signal; Needs you n; Failed n | ellipsis + 500ms tooltip |
| Title band / breadcrumb | ancestors link H underline-free, text.primary; F ring | — | — | — | the last crumb ellipsises first; ancestors collapse to "…" menu when < 320px remains |
| View tab / task tab | text.secondary; H text.primary; S bg.selected + text.primary 500; F inset ring; D text.disabled + reason in tooltip **and** in the tab panel | count badge skeleton while loading | X: counts suffixed "as of" in the accessible name | Decisions tab shows a warn count | tabs never truncate; the strip scrolls horizontally with fade edges and an overflow menu |
| Button primary | primary.fill; H 92% mix toward canvas; pressed 85%; F ring offset; D 40% opacity **plus** a visible reason line below the group (never hover-only) | in progress: label kept, trailing 12px spinner only if >400ms (mock: never) | X: disabled with "Disconnected; actions unavailable" | — | labels never truncate; wrap to 2 lines at 200% |
| Button secondary / ghost / icon | secondary: bg.raised + border.default; ghost: transparent; icon 28×28 with tooltip + `aria-label` | — | — | — | — |
| Mock-suffixed action | label + trailing "Mock" 12px in `mock`; accessible name "… (mock)" | — | disabled per X | — | — |
| Danger action | ghost with danger text; confirm uses danger.fill | — | — | — | — |
| Chip / badge | static, not focusable unless it opens detail | — | "As of" chip neutral | Your permission / Failed check / Conflict | never wraps; long values truncated with full value on focus or click |
| Stat cell | value + label | Ld: value skeleton 32×20; Er: "—" + "Couldn't load" link to retry (idempotent read) | X: workers "Unknown ?" + reason; others "as of" | non-zero attention values link to filtered board | label wraps to 2 lines |
| Project table row (dashboard) | H bg.hover; F ring inset; whole row is a link | Ld: 8 skeleton rows of final height; E (no projects): "No projects yet. Projects appear when a machine is paired." | Mosaic: "Disconnected" + "Unknown" workers + last activity "as of" | Amber: "Git conflict"; Quartz: "1 failed · 1 not run" | name ellipsis, repo wraps anywhere |
| Needs-you row | H/F as rows | E: "Nothing needs your decision. Checked 02:46 (fixture)." shown **only** when the count is 0 and the load succeeded | X: rows of disconnected projects show "Actions unavailable · disconnected" | type glyph | title wraps to 2 lines |
| Board column | header name + count | Ld: 4 skeleton cards per column; E: one muted line "No tasks" under the header **only when count = 0**; filtered out: "No tasks match filters · 88 hidden · Clear filters"; Er: column body "Couldn't load tasks (fixture error E_PAGE). Nothing was changed." + Try again | X: header suffix "as of 00:00" | — | 220 cards virtualized |
| Task card | raised; H bg.hover; S accent border + bg.selected; F inset ring; waiting-on-you warn border | Ld: skeleton | Running on disconnected machine: chip "Unknown" + line "Last known: Running · 00:00" | P chip "Your permission"; C chip "Failed check"; K chip "Conflict" | 2-line clamp; full title on focus via tooltip and accessible name |
| List row | 36px; H/S/F as rows | same as column (table body) | same | same | single-line ellipsis; full in the accessible name |
| Task header | title wraps fully | Ld: title skeleton | X: "Last known Running at 00:00 · current state unknown" neutral | wait chip | title wraps; never truncated |
| Timeline | messages, grouped tools | Ld: 6 message skeletons; E: "No discussion yet. Write the first message below."; Er (page): inline row "Couldn't load earlier events. Try again" | X: composer disabled with reason; banner "Output paused at 00:00" | decision events link to the Decisions tab | messages over 12 lines collapse: "Show full message (48 lines)" (`aria-expanded`) |
| Plan rail / disclosure | numbered steps with state text | E: "No plan yet."; unavailable capability: "Plan updates unavailable: this agent did not negotiate plan reporting." | stale "as of" | — | steps wrap |
| Decision panel | section 6.3 | Ld skeleton; E handled by the list | X: actions disabled + "Disconnected; last confirmed 00:00" | P: Approve once · Mock / Deny · Mock | title wraps fully, never truncated |
| Result view | header, checks, changes, diff, acceptance | Ld; E: "No result yet for this task."; stale: warn notice "Result changed after review: mock-digest-quartz-v1 → v2. Acceptance is disabled." | C: Failed rows first, then Not run, Unverified, Passed; "Required but not run" group always present ("All required checks ran" when empty) | — | digests middle-ellipsis + copy; full in the accessible name |
| Diff | side-by-side at ≥760px pane width; unified below that, or at ≥150% text; +/− gutter text | Er: "Couldn't load diff" | — | K: conflict hunks labelled "Base / Yours / Incoming" | horizontal scroll only inside code blocks |
| Code tree / editor | tree rows 28 with git letter as text (M/A/D/U) | E search: "No files match `auth*`." | X: read-only notice | K: warn notice "This file changed on disk since you opened it." + Compare · Reload · Save as… (never overwrite) | paths wrap anywhere in notices |
| Terminal panel | header copy (4.5) | — | X: input disabled, "Output paused at 00:00" | — | — |
| Git view | grouped rows | E: "Working tree clean." | X: actions disabled | K: danger notice "Merge stopped: 1 file has conflicts on synthetic-layout." Commit disabled with "Resolve 1 remaining conflict"; Push with unknown outcome: "Outcome unknown · Inspect remote · Mock" (no Retry) | branch names wrap |
| Grouped settings row | label and description left, control right | — | — | not permitted: lock + reason under the control | descriptions wrap |
| Menu / popover | rows 30, icon / label / shortcut, H/F bg.selected | — | — | — | menus size to content up to 320px, then wrap |
| Dialog / palette | native `showModal` | — | — | — | — |
| Inline notice | 1px tinted border (status at 40%), tint fill, radius 8, min 36, glyph + text + optional action; placed at the top of the affected region, never full window width | Er variant danger | X variant danger; O variant neutral | P / C / K variants | wraps |
| Inline mock receipt | flask + text.secondary 13px, no border, below the action row; persists until the next action on that item | — | — | — | wraps |
| Skeleton | bg.selected blocks, radius 4, final dimensions | — | — | — | — |
| Tooltip | e2, 12px, 500ms delay, Esc dismisses, never the only carrier of essential information | — | — | — | max 320 wide |

### 6.3 Decision panel anatomy (from concept A, restyled as grouped rows)

1. Type line: glyph + "Permission" / "Question" / "Result acceptance" (12/500 secondary).
2. Title (15/22, wraps fully).
3. Meta (12 tertiary): task, requester, requested at (provenance), decision version "v1".
4. Grouped rows (label column 120px): **Effect** · **Scope** (paths, commands, entity version, result and digest) · **If you reject** (concrete consequence; "Nothing is reverted.") · **Evidence** (acceptance only): Passed n · Failed n · Not run n · Unverified n, each with glyph and label, plus required unrun checks named.
5. Actions, right-aligned:
   - Question: answer control + **Send answer · Mock**.
   - Permission: **Approve once · Mock** / **Deny · Mock**.
   - Acceptance: **Accept mock-result-complete · Mock** / **Request changes · Mock** / **Reject · Mock** (Reject confirms).
   - Never an approve-all.
6. Acceptance footer: "Accepting does not commit, push or deploy."
7. States:
   - Stale: Accept disabled, with "This result changed after it was requested (digest v1 → v2)" and a link to the new result.
   - Expired.
   - Answered elsewhere: "Answered on Phone at 14:05: Approved once (mock). Your choice was not applied." (`role="status"`).
   - Disconnected.

## 7. Keyboard map and focus order

Browser-hosted constraint: Chromium reserves Ctrl+N, Ctrl+T, Ctrl+W, Ctrl+Shift+N/T and, on Linux, Ctrl/Alt+1…8; F6 cycles browser panes. Ubuntu GNOME binds Ctrl+Alt+T (terminal) and Super combinations. The map avoids these.

**Requirement:** before acceptance, verify every global shortcut in **headed** Chromium on the target Ubuntu. Any intercepted shortcut is replaced, and every action stays reachable from a visible control and the palette.

| Keys | Scope | Action |
|---|---|---|
| `Ctrl+K` | global | Command palette |
| `Ctrl+O` | global | Project switcher ("Open project…") |
| `Ctrl+Alt+N` | global | New task (a native build may add `Ctrl+N`) |
| `Ctrl+Alt+D` | global | Needs you (decisions); verify GNOME does not bind it |
| `Ctrl+Alt+H` | global | All projects (home) |
| `Ctrl+B` | global | Toggle sidebar |
| `Ctrl+\` | task workspace | Toggle task list pane |
| `Ctrl+Alt+1…6` | project views and task tabs | Overview, Board, List, Decisions, Git, Machine; or Discussion, Decisions, Result, Workers, Code, Git |
| `Ctrl+,` | global | Settings |
| `` Ctrl+` `` | Code and Git tabs | Toggle terminal panel |
| `Ctrl+P` / `Ctrl+Shift+F` | Code tab only | Go to file / search in files |
| ← → ↑ ↓ (and `J`/`K`, `H`/`L` only while the board has focus) | board | Move card focus within and between columns (roving tabindex; one Tab stop) |
| `Home` / `End` | column, list | First / last item |
| `Enter` | card, row | Open task; focus moves to the task title heading (visible ring) |
| `Esc` | popover or dialog | Closes it. In the task workspace, focus returns to the originating card or row (the task stays open). |
| `Alt+←` / `Alt+→` | global | Back / Forward (native browser history, since routes are real) |
| `?` (outside text fields) / `Ctrl+/` | global | Keyboard shortcuts dialog |

- Single-character shortcuts work only inside their focused component and can be disabled in Settings → Keyboard (WCAG 2.1.4).
- Decision actions have **no** shortcuts.
- The theme has no default shortcut; it is set from the palette or the sidebar footer.

**Focus order (DOM = visual):** skip links ("Skip to navigation", "Skip to main content", and "Skip to task" when open) → sidebar (shortcuts → projects tree as one Tab stop with roving focus, ↑/↓, → expand and ← collapse, `role="tree"`) → resize handle → footer buttons → title band (toggle, breadcrumb links, search, Mock chip, overflow) → view tabs (one Tab stop, arrows move) → filter → main content (board grid as one stop; table as one stop; dashboard sections in order) → task list pane → task header → task tabs → tab content → composer.
- **Region cycling:** `F6` / `Shift+F6` where the page receives them (verify in a headed browser), with skip links as the guaranteed path.
- **Focus on open:** focus moves to the opened pane's heading.
- **Focus on close:** focus returns to the invoker.
- **Incoming data never moves focus.** New decisions update one throttled `role="status"` region ("2 decisions need you").

## 8. Theme toggle behaviour

- Preference values: `system` (default), `light`, `dark`.
- The resolved theme is applied as `data-theme="light|dark"` on `<html>`, plus `color-scheme: light|dark` so native scrollbars and form controls match.
- **Initial resolution before first paint:** a tiny inline script in `index.html` reads `localStorage["cw.theme"]`; if it is missing or `system`, it uses `matchMedia("(prefers-color-scheme: dark)")`. No flash of the wrong theme.
- With `system` selected, a `change` listener on the media query re-resolves live. With an explicit choice, system changes are ignored.
- **Persistence:** an explicit choice writes `cw.theme` immediately. **Prototype Reset does not clear the theme**: it is a user preference, not fixture state.
- **Control:**
  - The sidebar footer Theme button opens a 3-item menu (radio items with a check; the current one checked; labels "System (Dark)" showing what System resolves to, "Light", "Dark").
  - Settings → Appearance has a segmented control.
  - Palette commands.
  - The button's accessible name is "Theme: System, currently dark".
- **Transition:** instant (0ms). No cross-fade; disable transitions for one frame during the swap.
- Both themes must pass the rendered checks (section 10) independently. Syntax and diff colours have per-theme values; terminal colours follow the theme.
- `prefers-contrast: more`: border.subtle → border.default, and border.default → border.control.
- `forced-colors`: system colours throughout, with focus in Highlight.

## 9. Explicit behavioural requirements (testable)

1. **Modal inertness.** Every modal (create task, confirmations, palette, project switcher, keyboard help, simulated phone) is a native `<dialog>` opened with `showModal()`, so the rest of the document is inert, in the top layer, and Esc fires `cancel`. If any custom overlay is unavoidable, the app root gets the `inert` attribute while it is open. Focus goes to the first field, or to the **safe** button for destructive confirmations, and returns to the invoker on close. Background scroll is locked. *Test:* with a dialog open, Tab and Shift+Tab never reach background elements, and `document.querySelector('#app').inert` (or the dialog's modal state) is true. Addresses concept A defect A-1.
2. **Focus never lands on an invisible target.** No `outline: none` or `outline: 0` without an equal replacement. Programmatic focus targets (headings, panes, loading, error and empty containers) show the ring via `:focus`. Loading and error states must not swap the focused element for one with suppressed focus styling. If the focused element is removed by a state change, focus moves to the region heading with a visible ring, never to `<body>`. *Test:* after each Tab press and each programmatic focus in the S1–S4 scripts, the active element's computed `outline-style` is not `none` and `outline-width` is at least 2px, or it has an equivalent `box-shadow` ring. Addresses concept C defects C-1 and C-2.
3. **Empty states never contradict counters.** Counters and empty states read the **same selector result**.
   - "No tasks" and "Nothing needs your decision" render only when the count is exactly 0 and the load state is `ready`.
   - Loading shows a skeleton (never empty copy). Error shows the error (never empty copy).
   - A count above 0 with filters hiding all rows shows "No tasks match filters · n hidden · Clear filters".
   - An unknown count (disconnected) displays "Unknown", never 0.
   - *Test:* for every view in every scenario, assert `emptyVisible === (count === 0 && status === "ready")`.
4. **No automatic view changes.** Route, tab, selection, scroll position and focus change only on user input. New events show "n new · Jump to latest".
5. **No false progress.** No percentage bars, ETAs or animated progress. Progress is shown as facts ("3 of 5 plan steps reported done").
6. **Unrun is never styled as passed.** Complete, Not run, Unverified and Unknown use neutral styling. Only an observed Passed check record uses success. An agent-reported "tests pass" stays conversation text marked "reported".
7. **Mock is always visible but quiet.** The Mock chip appears on every route. Every mutating control carries the "Mock" suffix and "(mock)" in its accessible name. Mock outcomes appear as inline receipts. "Accepted (mock) · mock-result-complete · digest mock-digest-amber-v1" is never shown as a plain "Accepted".
8. **Disconnected project:** all mock mutating controls for that project are disabled with the visible reason "Disconnected; actions unavailable". The last-confirmed time is visible. Running becomes "Unknown (last known Running)".
9. **Native feel in the browser:**
   - The app frame is `100vh`, with no document scroll; each region scrolls independently (`overscroll-behavior: contain`).
   - `user-select: none` on chrome (sidebar, tabs, buttons); content text stays selectable.
   - `cursor: default` on buttons, rows and tabs; pointer only for inline content links.
   - Thin themed scrollbars (8px, thumb bg.selected, hover border.control).
   - No browser-blue underlined links in chrome.
   - Icons are inline SVG, with no icon font.
   - No remote assets.

## 10. Responsive and accessibility requirements

- **Targets:** 1280×800 and 1920×1080 at DPR 1, both themes, text presets 100/125/150/200%.
- Between and below these sizes nothing auto-collapses. The board scrolls horizontally when columns would go under 220px (visible scrollbar, Shift+wheel). The breadcrumb collapses ancestors into "…".
- At 200% text on 1280, the title band and tab rows grow in height (rem). The user can hide the sidebar and list pane. Prose never needs two-dimensional scrolling. Action labels, decision titles and errors are never truncated.
- **Landmarks:** `nav aria-label="Sidebar"`, `header` (title band), `main aria-labelledby` the view heading, `region aria-label="Task list"`, `aside aria-label="Plan"`. Board `role="grid"` with column headers; list as a table; projects tree `role="tree"`.
- **Contrast:** text pairs ≥4.5:1 and control boundaries and focus ≥3:1 per the computed tables, on the surfaces listed in section 5.2 (see the `border.control` correction). Disabled labels are exempt, and disabled reasons use text.secondary. **Rendered** contrast must be re-measured on screenshots of both themes.
- **Visual QA:** capture at least the board, task Decisions, Result (stale), Create dialog, dashboard, Overview of Mosaic (disconnected) and Settings → Connection in dark and light at both sizes, and judge them against this specification.

## 11. Fixture mapping (cw-fixtures-v2, from `src/fixtures.ts:22-95`)

| Project (sidebar order) | Tasks | Signal shown | Named cases |
|---|---|---|---|
| Mosaic Studio | 220 | **Offline** · workers Unknown | task 0 last confirmed Running → "Unknown (last known Running)" |
| Cedar Notes | 90 | **Needs you 1** | task 1 permission (Blocked · Your permission); task 5 complete result + Passed check (Done) |
| Orbit Ledger | 65 | **Needs you 1** | task 2 question (Waiting · Your answer) |
| Quartz Search | 50 | **Failed 1** | task 3 failed check (Review); task 4 stale result + Not run check (Review) |
| Amber Components | 35 | **Conflict** | task 6 file + Git conflict on `synthetic-layout` |
| Indigo Maps | 25 | — | task 7 unavailable capability "Android push" |
| Willow Archive | 15 | — | task 8 long text / keyboard case |
| Harbor Sandbox | 0 | — | empty project; last activity "No activity" |

Board columns = fixture phases Backlog / Working / Review / Done (read-only projection).

**Fixture revision caution.** [Desktop approval](desktop-approval.md) records the round-one prototype as `cw-fixtures-v1` (seed 13014, 500 tasks). The mapping above is the v2 fixture as read from the prototype source on 2026-11-04. Align the fixture revision and the approval record before implementation or any captured evidence is cited.

## 12. Open questions for the owner (not blocking implementation; defaults chosen)

- **Q1 Accent hue.** Default: calm blue (#7AA7FF dark / #2A5BD0 light), so focus and selection never read as "Passed" green and the Paseo brand green is not copied. Alternative: a Paseo-like green accent.
- **Q2 First-visit project view.** Default: Board (board-first, from concept A), with Overview one tab away. Alternative: Overview first.
- **Q3 Board column names.** Default: fixture v2's four phases. Concept A's seven candidate names would need a fixture change.
- **Q4 Project identity tiles.** Default: neutral-hue letter monograms. Alternatives: owner-chosen colours or icons per project.

## 13. Risks and residual uncertainty

- **R1.** The board and dashboard are not evidenced by Paseo (it has no such screens). Their "Paseo feel" depends on the shared principles and must be judged in rendered review.
- **R2.** Keyboard shortcuts in a browser-hosted app may be intercepted by the browser or GNOME. Untested here; verify headed.
- **R3.** The selection fill is low contrast (about 1.3:1), mitigated with weight, an accent border and ARIA. The owner or an accessibility reviewer should confirm this is acceptable under WCAG 1.4.11.
- **R4.** The system font differs across platforms (Ubuntu font on the target). Measured Paseo sizes come from its renderer at 1249×900, not from the screenshot sizes.
- **R5.** Tauri native window integration (drag region, native controls, WebKitGTK rendering) is out of scope and unqualified.
- **R6.** Contrast values are computed, not rendered. Antialiasing on dark surfaces can make 12px tertiary text look lighter than computed.
- **R7.** `border.control` is below 3:1 on hover and selected surfaces (section 5.2). Any row that shows a control boundary on those fills needs a token change or a second boundary cue.

## 14. Verification record (2026-11-04)

| Check | Result |
|---|---|
| WCAG 2.x contrast recomputation of all text, accent and control tokens in both themes, across window, canvas, raised, overlay, hover, selected and input surfaces; read-only `python3 -I -B -c` script | Exit 0. All text and accent minimums reproduce. `border.control` correction recorded in section 5.2 and risk R7. |
| Fixture project order against the prototype source | Recorded in section 11 (read from source; not rerun here). |

Not run, with blockers:
- Rendered screenshots and rendered contrast: this direction has no implementation to render yet.
- Screen-reader pass and headed-browser shortcut tests: no prototype build exists for this direction.
- Native Tauri build: the Rust toolchain is not installed (see [desktop approval](desktop-approval.md)).
- Paid live checks: not requested by the owner.
