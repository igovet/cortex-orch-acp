# Performance targets

**Status: targets are planned; recorded local measurements below describe implemented behavior, not full product qualification.**

Targets, not measurements or guarantees. Record hardware, OS, GPU, build, dataset and measurement method.

| Metric | Target | Measurement |
|---|---|---|
| Cold desktop board | <700 ms | process launch to interactive board on reference Ubuntu |
| Keystroke to glyph | <50 ms | instrumented input-to-paint, physical devices where feasible |
| Scroll/drag | 16.7 ms per frame, no run over 3 dropped frames | frame timing in representative scenarios |
| Desktop idle memory | <150 MB per window | settled process RSS, report shared-process caveat |
| 10,000 events | 60 fps, viewport-bounded DOM | synthetic history with frame and node counts |
| Terminal | no visible stutter; graphics-context loss falls back safely | bounded PTY output fixture and context-loss test |
| Android Now | <2 s cold start, <5% janky frames | physical-device startup and gfxinfo |

Baseline both 500 tasks and 10,000 events; avoid full-screen rerender on streaming updates. Missing or unreliable instrumentation must be reported, not called a pass. Feasibility and budget failures block a performance claim.

## Proposed budgets pending acceptance

The rows below are proposals from backlog items. They are not measurements and are not accepted targets until the owner accepts them and a recorded measurement exists.

| Metric | Proposed target | Measurement | Source |
|---|---|---|---|
| Git status, diff and log, warm read on a 5,000-commit fixture | 100 ms each, with zero Git subprocess spawns during the call loop | in-process timing on recorded hardware | [CW-G01](backlog/CW-G01.md) |
| Android small-file editor bridge | 256 KiB bound per load or save | payload size and load timing on a physical device | [CW-I05](backlog/CW-I05.md) |

## Idle and background work

With nothing changing, CPU is effectively zero; with work running, cost follows real updates.

| Work | When it runs | Bound |
|---|---|---|
| Project queues in the service | every 2 s the owning child compares file stamps of the store and the product record; a full pass runs only if they moved | no database connection while nothing moved; one full pass at least every 60 s, because project file changes announce nothing |
| Reading a result for the queues | once per task version, at most 60 s old | a finished or removed task's result is not read at all |
| Writing the product record | only when a pass changed it | an identical write would otherwise wake the next pass and every reader |
| Review actions in a product read | once per task version, at most 30 s old | kept in the machine's state directory; accepting a result still reads it fresh |
| Event journal poll in the service | 500 ms after activity, doubling to 4 s while empty | any request to the project's child resets it |
| Facts poll in a window | every 30 s, doubling to 120 s while unchanged, stopped while hidden, immediately on return to the window | only Git and machine facts rely on it; a project's facts reach the screens only when they differ, as data, from what the screens last received (from the poll or the full read), and a project whose facts did not change keeps its records in place |
| Machine facts | every 5 minutes | same visibility rule |
| Conversation history | on task or execution change, relevant journal events, or owner input | complete pages reused in the window; incomplete reads never cached |
| Working indicators | static icon and words | no infinite spin, shimmer, typing cursor or busy underline animation |
| Refresh on machine events | first event at once, the rest coalesced to one per 2 s, or 15 s while hidden | only the projects the events came from are reread |

What the queues change reaches the screens as a push notification from the child, not by rereading on a timer. Store signals and index stay the same objects when a read changes nothing. `createTicker` in `packages/workspace-ui/src/freshness.tsx` is the timer a screen should use: it stops when the window is hidden or the surrounding `ScreenActive` value is false, so a kept-alive screen that is not on view does no background work.

### Recorded on the reference machine

2026-11-09, Intel Core i5-14600K (20 logical CPUs), NVIDIA GeForce RTX 4060 Ti, Ubuntu 26.04.1, Linux 7.0.0-42, WebKitGTK 2.52.6. CPU below is **percent of one core**, from process/thread CPU ticks divided by `SC_CLK_TCK=100` and monotonic elapsed time, not lifetime `ps %CPU`. Production was observed read-only: no window interaction, debugger attachment, signal, restart or store mutation.

The running production package was `1.0.6+z20261109230427716121.sha256.a8a03c9ae02f22fd.local`. Its runtime identity matched the checkout's preceding stamp. Installed scheduler, launcher and ACP agent hashes matched the checkout: this was **not merely an old installation missing Gate**. Archive timestamps are normalized to epoch, not useful build times. The queue threads were already cheap; synchronous reads on `acp-reader` were the largest cost.

| Process / state | Production with window, 12.103086572999018 s | Checkout / isolated observation | Method and limit |
|---|---|---|---|
| Launcher, project A | 48.08690712816958%; reader thread 47.839036472869736%, queue 0.1652471035332288% | 0.49999453364311486% for one **empty activated** disposable project over 6.000065596999775 s | Real service binary with current checkout Python and fake-engine fixture, not an equal populated-project comparison |
| Launcher, project B | 26.191665910016763%; reader thread 25.94379525471692%, queue 0.1652471035332288% | No equal populated-project process measurement | Copied-history comparison below isolates the changed code instead |
| Launcher, project C | 0.5783648623663008% | No equivalent third-project fixture | Same live process delta |
| Rust service | 1.9829652423987454% | 0 ticks in the activated fixture's 6 s window; also 0 ticks over 5 s unactivated | Installed Rust executable, isolated HOME/XDG and socket, current checkout Python |
| Tauri desktop | 4.213801140097334% | **Not measured with rebuilt checkout assets** | The GTK harness below is not Tauri; cached desktop binaries embed pre-edit assets |
| WebKit web process | 9.584332004927269%; main thread 7.105625451928838% | 0.3999948493462252% unchanged, 5.999927302482397% with one visible fixture update/s | Separate software-rendered fixture, not identical production data or GPU |
| WebKit network process | 0 ticks | 0 ticks in both 5 s fixture windows | Same process-delta method |
| GNOME Shell / Xwayland | 14.376498007390905% / 0 ticks | Not comparable: private Xvfb used instead | GNOME's total includes other applications; this cannot all be attributed to Workspace |
| Short-lived helpers | 82 new descendants observed across **both release and dev service trees**: 54 Python, 24 Git, 2 bwrap, 2 opencode | No engine/store helpers in the UI-only fixture | 100 ms discovery sampler; lower bound, not an exhaustive process count or production-only CPU attribution |

The production window later exited without intervention. With no window, the same production service was 0 ticks and its three launchers were 0.24940823637275134%, 0.12470411818637567%, 0.24940823637275134% over 8.018981365999934 s, with no new descendants observed. That is a second production state, **not a checkout-after result**. A copied populated store showed seven COMPLETED runtime records, not paid tasks executing.

#### Proven costs and repairs

- Full conversation replay: four executed tasks, 1,105 visible rows in 22 pages on the **same private database copy** took 1197.3164510000001 ms CPU with installed history code versus 302.5701240000001 ms with checkout code. This is measured CPU per complete read, not a process-percent prediction. The copy passed SQLite quick-check; source file stamps moved during copying, so it is diagnostic data, not an exact live snapshot. A separate stable copy also passed quick-check. Profiling attributed 2.524 of 2.973 seconds to repeated validation of growing page prefixes. Construction now validates single rows and the final page with additive UTF-8 byte accounting. Cursor, identity, unavailable-content and page-size checks remain.
- The window previously replayed every task's history on every refresh. Complete histories are reused until the task or execution changes, relevant journal events arrive, or the owner sends input. If the journal cannot be read completely, changed project history is reread conservatively; incomplete history reads are not cached.
- Slow Git completion previously emitted a full-refresh request. If the refresh outlasted the Git cache, it spawned another slow Git read and repeated. Completion now publishes facts, not another full task/history read. A failed or superseded completion keeps the last confirmed facts until the regular poll.
- Permanent status animations consumed frames with no progress. In the same 29,120-node fake worker view on private Xvfb, two `spin` animations produced 52.199276486711554% WebKit CPU, 18.7997394243329% GTK host CPU and 14.19980318220889% Xvfb CPU over 5.000069302999691 s. Disabling motion in that window reduced these to 5.999922910590467%, 0.3999948607060311%, 0 ticks. After static-indicator CSS, a fresh unmodified fixture measured 0.3999948493462252%, 0.1999974246731126%, 0 ticks respectively. The status words/icons remain; the busy control retains a delayed static underline. Actual streamed updates still paint.

Reproduce the current no-paid UI measurement with `timeout 30 node packages/workspace-ui/test/webkit-cpu.mjs`. It builds production transforms and measures two 5 s windows, isolated HOME/XDG and an automatically selected private display; it asserts no infinite animations and that at least four updates arrive. With one update/s, GTK host was 0.5999927302482397%, Xvfb 0.19999757674941324%, network 0 ticks, WebKit 5.999927302482397%. The software display disables compositing **only in this harness**; production compositing and sandbox configuration are unchanged. These are not real-GPU or whole-installed-application qualification.

The older per-call queue measurements remain useful historical diagnostics: 365 ms before caching versus 2.9 ms for a forced cached pass and 0.04 ms for `Gate.due`; product-read helper 452 ms versus 116 ms. They are not substituted for observed after-process figures. No inferred “below 0.1%” or calculated facts-round percentage is used as a measured result.

**Rebuild and reinstall the production package for these changes to take effect.** Source edits and restamping do not change an already running installation. No desktop archive was built or installed during these measurements.

## Editor bundle and input, recorded

These are recorded facts about the built desktop bundle, not a qualification. The input figure comes from WebKitGTK under Xvfb with software rendering, not the reference machine, so it does not satisfy the keystroke target above.

| Fact | Value | Method |
|---|---|---|
| Editor chunk | 335,682 bytes, 106,950 gzipped | `vite build` output, `gzip -9`; loaded the first time a file opens, never at startup |
| Language grammars | one chunk each, largest 103,894 bytes | same build; loaded only for a file of that language |
| All JavaScript in the bundle | 2,059,446 bytes in 20 files | same build, including the 1,144,519-byte main chunk |
| Input to second frame, 5,000-line file | median 33 ms, worst 43 ms over two runs of 40 inserts | `insertText` to two animation frames; two frames at 60 Hz is about 33 ms, so this is bound by the frame rate |

## Sprint graph, recorded

The sprint plan's Graph view draws its tasks and edges as SVG and moves the whole drawing as one layer: a pan or a zoom gesture writes only that layer's transform, once per animation frame, and touches no signal. When the gesture has been still for 120 ms the drawing is redrawn at the new scale, with its detail level, label lengths and minimap visibility. Selection and search change only the classes that differ. Layout is a pure function of the plan's keys and dependencies, memoized on them.

These are recorded facts, not a qualification. They come from `node packages/workspace-ui/test/sprint-graph-render.mjs`: WebKitGTK in the layout gate's container under Xvfb with software rendering, on a generated plan of 200 tasks and 382 edges in the Graph view at 1440×900 with the conversation shown (a 780×583 canvas, the whole graph fitted). Frame times are intervals between animation frames while a synthetic wheel event moves the view each frame, so they cannot fall below the 60 Hz interval. The budgets are the design's, for the real desktop app, which this environment is not.

| Measure | Budget | Recorded |
|---|---|---|
| Layout, 200 tasks | 30 ms | 0.2 to 2.6 ms by plan shape, median of 5 in Node (`test/plan-graph.test.ts`) |
| Pan frame, p95 | 16 ms | median 16 ms, p95 17 ms, worst 22 ms over 120 frames |
| Zoom frame (Ctrl+wheel), p95 | 33 ms | median 16 ms, p95 17 ms over 120 frames |
| Selection, press to second frame | 50 ms | median 22 ms, worst 37 ms over 6 picks |
| Status filter, change to second frame | 50 ms | median 36 ms, worst 53 ms over 6 changes |
| Redraw at a new zoom after a key press | none set | median 98 ms, worst 159 ms over 5 presses |
| List to Graph, to second frame | none set | 85 ms |

With WebKit's compositing left on, as the desktop window runs it, the same run (`sprint-graph-render.mjs bench composited`) recorded pan and zoom p95 18 ms, selection worst 64 ms, filter worst 45 ms and a 182 ms worst redraw. Before the drawing moved as one layer, writing the transform into the SVG for every frame gave a pan p95 of 53 to 60 ms in the same setup. The pan p95 is 1 ms over its budget as measured, and one filter change of six went over 50 ms. No culling of tasks outside the view is built; at 200 fitted tasks every task is on screen anyway.

## Scrolling, recorded

These are recorded facts from the scroll gate, not a qualification: `node packages/workspace-ui/test/scroll-perf.mjs <label> [surface…]`. It builds the static bundle with the layout gate's fake data made long (`scroll` in `test/layout-fit-fixture.tsx`) and runs `test/scroll-perf.py` in the `cortex-workspace-rendered:ubuntu26.04` container: WebKitGTK 2.52.6 at 1440×900 with **compositing on, as the desktop window runs it**, software GL on a private Xvfb, not the owner's GPU or the Tauri window. On each surface it finds every element that scrolls more than 300 px and scrolls it three times: a scrollbar drag from one end to the other in 120 frames and back in 60 (twice: the first pass is the first look at never-painted content), and the real mouse wheel turned 24 notches each way through X (`xdotool`). A script installed before the page's own records frame intervals, the forced style/layout time at the start of each frame, the time of every scroll and wheel handler and ResizeObserver callback, and every DOM change. WebKitGTK gives page script no style-recalc or layout counters and its inspector server did not start in this build, so layout is measured as that forced-flush time. Page timers count whole milliseconds. `idle` instead watches pages untouched for 70 s; the `-live` surfaces scroll while the task streams one update every 400 ms, five times the service's coalesced pace. Limits are p95 ≤ 22 ms, no frame over 50 ms, at most 3 % of frames over 25 ms, scroll and wheel handlers ≤ 4 ms, an update taken in ≤ 8 ms; what does not meet them yet is listed in the script's `KNOWN` table as a ratchet, printed on every run, and fails the run if it gets worse. Evidence goes to the git-ignored `.design-evidence/conversation/scroll-perf/<label>`.

What was found, measured before and after on 2026-11-10 (fake data: 60-task sprint, 77-card project board, a 200-task sprint, 31 projects in the sidebar, a 300-turn conversation, a 1,500-note worker feed, a 4,000-line file, 400 commits, 40 changed files of 300 lines, 900 lines of terminal output):

| Cause | Evidence before | Change | After |
|---|---|---|---|
| The facts poll serialized every project's facts with `JSON.stringify` to compare them, then delivered every project's facts once more after the full read, and re-installing a project's facts moved its records to the end of each list, so the screens' structural sharing saw a changed state | a 1,268 ms and a 975 ms frame at 30 s and 60 s on an untouched board; 14,828 DOM changes (14,043 rewrites of an unchanged `class`), about 955 ms of serialization per round | facts compared as data (`same` in `src/stable.ts`), the full read's facts count as delivered, re-read records keep their place (`applyFacts`) | no frame over 25 ms and no DOM change in 70 s |
| Board columns and the list windowed their rows: each scroll event created, measured and re-placed cards, and corrected estimated heights mid-scroll | sprint board drag p95 42 ms, 7 frames over 50 ms, 516 DOM changes; list wheel 26 of 151 frames dropped | rows in normal flow (`RowList` in `src/kit.tsx`) | drag p95 18 ms, wheel 0–2 dropped frames, no DOM change and ~0 ms of handlers while scrolling, also with 200 cards |
| A streamed update re-ran every row of a long worker feed: thousands of `JSON.parse` errors thrown in `displayStep`, every step's presentation rebuilt, a new entry object and a woken memo for each of 1,500 rows | each update a 60–77 ms frame while scrolling (8 long frames per wheel pass); 46 ms average, 84 ms worst per update after the first part of the fix | non-object text skips `JSON.parse`; `feedOf` hands back unchanged entries; rows read only their own entry; per-record caches; the route is parsed once per address | 10 ms average, 18 ms worst per update; no long frame while wheeling |

After, in the same run: the sidebar, sprint plan list, Graph pan, git history, the review diff, the terminal and the code tree stay at p95 17–19 ms with no frame over 50 ms, as they did before. Not yet at target, and held by `KNOWN`:

- The first scrollbar jump to the far end of a 70,000–200,000 px history pays one 88–155 ms frame painting it, with or without compositing; later drags and the wheel are on time. Letting the engine skip off-screen rows (`content-visibility`) made it much worse (p95 74 ms, 122 long frames in the worker feed), because the feed's scroll owner measures rows. The 10,000-event target's viewport-bounded DOM is not built: a conversation renders every row.
- The code editor (CodeMirror) renders and measures what each scroll brings into view inside the scroll event: wheel p95 20–22 ms with 4–5 % of frames late, drag 13–49 % late, handlers up to 24 ms, the same before and after. Parsing the whole file ahead in small steps did not change it and was not kept.
- With an update every 400 ms the 1,500-note feed still drops 4–6 % of frames, and an update takes 10–18 ms, above the 8 ms limit.

The Tauri window on the owner's NVIDIA GPU, a touchpad, and Wayland were not measured.
