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
| `sprint-summary.html` | Sprint summary |
| `code.html` | Code view, inline and side by side diffs, search |
| `git.html` | Git status, history, branches and worktrees, conflict |
| `stop.html` | Stopped and stopping task |
| `archive.html` | Archive, unmerged work question, delete preview |
| `settings.html` | Appearance, voice, shortcuts, machines, orchestration, engine connections and limits, storage, diagnostics, remote access |

`styles.css` is the one stylesheet shared by every page.
