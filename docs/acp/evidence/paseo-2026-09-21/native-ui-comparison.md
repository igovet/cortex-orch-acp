# Paseo native ACP UI comparison

Status: **partial, serialized-only for retained Cortex history**. The CUA surface
available to this run reported no browser or desktop app and could not inventory
Paseo, so no UI screenshot, DOM, or accessibility-tree claim is made here. The
native Claude/Codex visual rows remain `NOT RUN` pending the separate disposable
Claude probe; no model prompt was sent by this audit.

## Evidence sources

The retained Cortex timeline was read from
`/var/tmp/cortex-paseo-live-final-3hq9_qec/sonnet-project/.cortex/cortex.sqlite3`
(session `acps_6d1bf056654b`). `host_updates` preserves exact serialized
Paseo timeline payloads; `tool_result_observations` preserves tool-card
kind/title/status transitions. This is a data-level observation, not a visual
comparison.

Paseo's local agent index currently contains 12 `cortex` records and 2
`claude` records, but no `codex` provider record. The Claude records retain
session metadata and titles only; no native Claude transcript or rendered DOM
was available through the permitted read-only surfaces. Therefore the native
Claude and native Codex columns below stay `NOT RUN`.

## Matrix

| Timeline variant | Cortex retained serialized evidence | Native Codex ACP UI | Native Claude ACP UI | Finding / fix guidance |
|---|---|---|---|---|
| Assistant message | `host_transcript` has `agent_message` rows, including empty streaming chunks and final text. | NOT RUN | NOT RUN | Keep assistant text separate from tool cards; verify visual grouping in each native client. |
| Final answer | `host_transcript` contains final `agent_message` text and a later blocked final message. | NOT RUN | NOT RUN | Verify final-answer card is visibly distinct from tool output and failure state. |
| Read collapsed | `host_updates` seq 26 emits title **`Read File`** with content text **`Read src/greeting.py — file_path=…`**. | NOT RUN | NOT RUN | The generic title loses the path; rename/format the card to expose the actual path at collapsed width. |
| Read expanded/path/content | Serialized update seq 27 carries `Read src/greeting.py — file_path=…`; seq 31/34/35/36 show multiple paths followed by generic `Read File`. No serialized file-content body is present in this snapshot. | NOT RUN | NOT RUN | Preserve path and bounded content preview in the expanded body; do not infer content rendering from source. |
| Edit/diff | `host_updates` seq 23/24 include **`Editing files`** as a worker timeline text, but no serialized diff payload in this session. | NOT RUN | NOT RUN | UI diff layout and expand/collapse behavior are unobserved; run a retained edit case before changing renderer. |
| Command | seq 7–12 and 40–42 carry exact **`Command — …`** titles; seq 57 carries **`Command — sleep 60 [failed]`** with `status=failed`. | NOT RUN | NOT RUN | Preserve command text and failure suffix; verify output expansion and failed-state styling visually. |
| MCP | seq 19–24 carry **`mcp.cua_repl.js — …`** in `rawInput`; `tool_result_observations` records `mcp.cortex.read_skill` and `mcp.cua_repl.js` titles with in-progress/completed states. | NOT RUN | NOT RUN | Keep MCP namespace/tool title visible and distinguish it from shell Command cards. |
| Failed tool | `tool_result_observations` has failed `ScheduleWakeup`; `host_updates` has failed Command text. | NOT RUN | NOT RUN | Failure is observable in serialized data; exact red/error icon, error body, and collapsed state remain unverified. |
| Completed tool | `tool_result_observations` records completed transitions for Terminal, `mcp.cua_repl.js`, and `mcp.cortex.read_skill`. | NOT RUN | NOT RUN | Verify completed state does not hide useful output or collapse to an ambiguous generic label. |

## Direct early finding

The retained Cortex Read path is materially clearer in the serialized body than
its card heading: the body names `src/greeting.py` and includes `file_path=…`,
while the heading is only `Read File`. The Paseo renderer should make the path
the collapsed title or subtitle. This is a serialized finding only; native
Codex/Claude visual behavior is not claimed.

## Scope boundary

No paid call, prompt submission, edit, approval, or destructive action was used
for this audit. Rows marked `NOT RUN` must remain unverified until a real native
chat is inspected and a retained screenshot/DOM or accessibility snapshot is
attached.
