# Task side chat

Source: `crates/workspace-service/src/machine/side_chat.rs` (actions, context, history),
`crates/workspace-service/src/short_text_engine.rs` (`side_answer`, the read-only engine run),
`packages/workspace-ui/src/side-chat.tsx` and `packages/workspace-ui/src/port/side-chat.ts` (window and port).

The side chat lets the owner ask questions about one Workspace task without touching it. It knows
the task, but it is isolated from it: a side-chat question never enters the task's conversation,
never reaches the task's coordinator or workers, never creates a runtime task and never changes the
task's state, plan, progress, files or project store.

## Window

- Open it with **Side chat** in the task header (also in the header's **Details** menu) or
  **Ctrl+Shift+J** on a task page. The same shortcut and button collapse and expand it.
- The panel floats over the task. Drag it by its header, resize it from its lower right corner;
  it always stays wholly inside the window and keeps a minimum size. **Esc** with focus anywhere in the
  panel (the question field, the answers, the header after a drag), or the
  minus button, collapses it to a pill in the bottom right corner that reads **Side chat (n)** with
  the number of kept exchanges; the cross closes it. Place, size and the open or collapsed state are
  remembered for the next task and the next launch (local UI preferences, `cw.ws.sideChat`).
- The header says **Doesn't affect the task** (Russian UI: **Побочный чат**, **Не влияет на задачу**).
- On a phone the panel is a bottom sheet across the screen, without drag or resize.
- Model and effort start at the task's coordinator route (the task's own choice, or the machine
  default it uses) and can be switched in the panel, with the same model and effort pickers as the
  composer, to any model the task's model picker offers, with that model's efforts. A switch applies
  to the side chat only, never to the task.
- An answer's Markdown is rendered as task results are. Each answer shows the model and effort that wrote it. A failed, timed-out or interrupted answer is shown
  as such, never as an answer.

## What an answer knows

Each question is sent with a bounded task context read on the machine without any write
(the project store is opened `LiveReadOnly`):

| Part | Bound |
| --- | --- |
| Title and description | 6,000 bytes of description |
| Runtime state and open obstacle facts (`lifecycle::obstacles::capsule`, without its coordinator instruction) | 3,000 bytes |
| Newest committed reports: title, author and summary of up to 6, and the newest report's body through one bounded cursor page | 600 bytes per summary, 4,000 characters of body |
| The latest 12 coordinator conversation messages (owner and coordinator text, not worker rows, not tool output) | 1,500 bytes each, 9,000 in all |
| The task's changed files and diff excerpts against its base revision (the same reader as commit and squash drafts) | 16,000 bytes |
| The side chat's own 4 latest answered exchanges | 1,500 bytes per question and per answer |

The whole task context is cut at 40,000 bytes. Every part passes the draft filter: Unicode control
and format characters are removed, known credential-like lines and private-key material are left
out, private files (`.env*`, keys, credential names) and `.git`, `.cortex` and `.cortex-dev` are never
read for diff excerpts. This filter cannot recognize every secret.

### Project files

On a Claude route the session works in the task's checkout (its worktree, or the project folder) and
Claude Code's native **Read**, **Grep** and **Glob** tools read it. Shell, edit, write, notebook,
skill, subagent, scheduling, worktree, plan-mode and notification tools are removed from the session
as in every Cortex Claude session, and the side chat also removes **WebFetch** and **WebSearch**. The
session has no MCP server, loads only the project's shared Claude settings, and runs in the engine's
read-only mode. Cortex private state (`.cortex`, `.cortex-dev`) is denied; credential files and any
path outside the checkout make Claude Code ask, and every such request is answered no. Only `read`
and `search` tool calls are accepted; any other tool call discards the answer.

On a Codex route the session works in an empty private scratch directory (Cortex's hardened Codex
launch keeps the native shell off) and its App Server gets exactly one MCP server: the side chat's
read-only project files server (`crates/workspace-service/src/side_chat_files.rs`), started from the
service binary (`--side-chat-files <checkout>`) and handed over through the hardened launcher's
verified relay layers like every Cortex Codex MCP relay, never as ACP `session/new` servers. Short
texts and Claude sessions never get it. It offers `read_file` (bounded line ranges, text files up to
2 MiB), `list_directory` and `search_files` (literal text, bounded), and nothing that writes, deletes
or runs anything. Paths are project-relative and checked like a channel's `project_read` (no absolute
path, `..`, `.git`, `.cortex`, `.cortex-dev` or dev owner file), resolved with symlinks and kept inside
the checkout, and refused when they name Cortex private state or credential material; listings and
searches leave those entries out and do not follow symlinked folders. Its tool calls are allowed; any
other tool call discards the answer, and every other permission request is answered no.

## Lifecycle and history

- `product.task` `side_question` (`taskId`, `question` up to 4,000 characters, optional `model` and
  `effort`) resolves the route at once: with no usable route it is refused with
  `side_chat_no_route` and nothing is kept. Otherwise it records a running entry and answers its
  `entryId`; the answer is written in the background (context read within 30 s, the run within
  240 s). `side_chat_poll` (`entryId`) reads one entry, `side_chat_history` all of them,
  `side_chat_clear` removes the history (a running answer stays until it ends).
- At most two side-chat answers run at once on a machine, apart from title and draft jobs; a third
  is refused with `side_chat_busy`.
- The history is kept per task in the machine's private state
  (`<state>/side-chat/<project>/<task digest>.json`, files 0600 under `side-chat.lock`), across app
  and service restarts; the newest 60 exchanges are kept. It is never written to the project's
  `.cortex` store or the task conversation. An answer that was running when the service stopped
  reads as interrupted.
- The private `short-text.log` records the side chat's route, outcome code and duration
  (`side_chat_selected`, `side_chat_answered`, `side_chat_failed`, `side_chat_refused`), never a
  question, context or answer.
- A paired phone may use the four actions for a named task, like the task drafts.

## Verification

No-paid tests: `crates/workspace-service/src/machine/side_chat_tests.rs` (isolation from the task and
its store, private history, restart, route choice, refusal without a route, context bounds),
`crates/workspace-service/tests/short_text_engine.rs` (the read-only run against the no-model fake
engine: working folder, removed tools, accepted and refused tool kinds, denied permissions, unchanged
checkout, and for Codex the project files server attached only to side-chat sessions, reads inside the
checkout, refused escapes, private and credential paths and writes),
`crates/workspace-service/src/side_chat_files.rs` (the server's containment), `crates/workspace-service/src/remote_policy.rs` and
`packages/workspace-ui/test/side-chat.test.tsx` (placement clamp, drag, resize, collapse, close,
phone sheet, model switch, polling, no task command). How the installed codex-acp names and asks for this server's tool calls was not observed (the
acceptance matches its `mcp__project_files__`, `mcp.project_files.` and `rawInput.server` forms). A rendered window and a paid answer from a real
engine have not been observed.
