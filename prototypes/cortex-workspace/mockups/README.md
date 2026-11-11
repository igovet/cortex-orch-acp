# Cortex Workspace static mockups

Static screens for agreeing the desktop design. They are not an implementation: nothing runs, all data is synthetic, and no button performs real work. Buttons only move between screens and states.

## How to open

Open `index.html` in a browser, directly from disk. There is no build step, server or dependency. The page lists every screen and state.

A recent Chromium-based browser shows the popups in the right place, because the menus use the native `popover` attribute and CSS anchor positioning.

The theme follows the system by default and is changed only in Settings, Appearance. To force one for a review, add `?theme=light`, `?theme=dark` or `?theme=system` to any page address.

Other states are also URL parameters, and `index.html` links each one. For example, `conflict.html?cx=running`, `merge.html?state=conflict` or `task.html?dialog=close`.

## Screens

| Page | Shows |
| --- | --- |
| `index.html` | List of all screens and states |
| `home.html` | All projects, what needs you, review and merge queues, engine limits |
| `board.html` | Project board, list view, sprint scope |
| `task.html` | Task session: conversation, plan, workers, queued messages, steer, close dialog |
| `worker.html` | Live worker: activity stream, delivery states, voice input, stop one command |
| `result.html` | Result card, finished workers, changes review |
| `merge.html` | Ready to merge, merge and pull request dialogs, conflict and Cortex resolve states |
| `conflict.html` | Merge conflict: Resolve yourself or Let Cortex resolve, with running, finished and failed states |
| `new-task.html` | New task and new sprint |
| `sprint.html` | Sprint planning, dependencies, created tasks |
| `sprint-graph.html` | Approved: sprint planning for a large plan of 64 tasks |
| `sprint-summary.html` | Sprint summary |
| `code.html` | Code view, inline and side by side diffs, search |
| `git.html` | Git status, history, branches and worktrees, conflict |
| `stop.html` | Stopped and stopping task |
| `archive.html` | Archive, unmerged work question, delete preview |
| `settings.html` | Appearance, voice, shortcuts, machines, orchestration, engine connections and limits, storage, diagnostics, remote access |

`styles.css` is the one stylesheet shared by every page.

## Approved large-plan layout

`sprint-graph.html` shows the layout for sprint plans too large for the small graph under the list. The owner approved it on 2026-11-10 with the graph inside the plan pane plus Full view, and the minimap inset in a corner of the canvas, shown only while part of the graph is off screen. The Workspace builds it; [SCREENS.md](../../../docs/cortex-workspace/SCREENS.md#sprint-plan-list-graph-and-full-view-desktop) describes the built screen. It is the only page here that adds behaviour rather than moving between states.

The page opens on the Graph view. Try the switch in the plan header, Full view, the panel toggles and the graph controls with the mouse, or use the keys listed under the **Approved layout** chip in the title band. Ctrl+wheel zooms, wheel and drag pan.

States are URL parameters and can be combined, for example `sprint-graph.html?full=1&q=export`: `view=list`, `full=1`, `q=`, `status=`, `select=` and `details=` (task numbers), `collapse=` (layer numbers, comma separated), `zoom=` (percent), `conv=0`, `sb=0`, `warn=1`, `state=planning|past|draft`, `plan=small|one|empty`.

The page also carries a regression check for two interactions. Add `selftest=request&select=12&expect=room` at a window at least 980 px wide, with `&zoom=80` for a moved view, or `expect=noroom` below that width, to try Request changes from Full view. Add `selftest=map&zoom=100&expect=narrow` when the graph canvas is under 760 px, or `expect=wide` otherwise, to try the first minimap toggle by key, and `selftest=mapmenu` for the same through the More menu. The result is in the tab title and in `data-selftest` on the `html` element, and `chrome --headless --window-size=1280,720 --virtual-time-budget=3000 --dump-dom <address>` prints it.
