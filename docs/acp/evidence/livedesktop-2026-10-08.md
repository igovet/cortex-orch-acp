# Live desktop qualification — 2026-10-08

Owner-authorized paid live check in `desktop` mode ("live dev разрешаю в desktop"),
retry of the attempt blocked at r_bed3c097b1bb by npm lock drift (fixed by the
`lockfix` task) and a disconnected playwright MCP server.

## Environment

- Installed Paseo Electron: **0.11.1** (the docs' 0.10.2 observation is stale;
  recheck before future runs). Daemon reported `daemonVersion: "0.11.1"`.
- Dev home: `~/.paseo-dev`, server id `srv_dLC3aKF5GZxO`, listen
  `127.0.0.1:16767`, started via `python3 -B scripts/cortex_paseo_dev.py -- start`
  from `outside_command` (required: a `command_run`-launched daemon breaks later
  `/var/tmp` bubblewrap mounts per AGENTS.md/dev-paseo-environment.md).
- Electron profile: `~/.config/Paseo-dev`, launched via
  `python3 -B scripts/cortex_paseo_dev.py gui --xvfb` from `outside_command`,
  detached; DevTools on `ws://127.0.0.1:34671`. Private Xvfb display, never the
  user's desktop `DISPLAY`.
- Plugin: `cortex-acp`, installed from the absolute current worktree path
  `/home/igovet/Web_Projects/cortex-orch-acp/integrations/paseo`; `plugin ls`
  confirmed it already matched that exact path, `enabled: true`,
  `status: "running"` (install attempt returned `handler_error` "already
  configured", which is expected reuse, not a failure).
- Native engine login (checked without printing credentials):
  - Claude: `claude -p "/usage"` → "using your subscription", current-session
    and weekly usage percentages printed, no token/credential text.
  - Codex: `codex login status` → "Logged in using ChatGPT".
  - Daemon `status --json` providers: `claude: available`, `codex: available`,
    `opencode: available`, `cortex: available`.
- Disposable project: `/home/igovet/paseo-live-desktop-20261007h` — an
  existing disposable project from the prior (interrupted) attempt series for
  this same owner request, reused because it was already open in the dev
  Electron window with a completed prior conversation; not a newly created
  directory. It is outside this checkout and was never `~/.paseo` or this repo.

## Pre-flight checks (no-paid)

Run from `/home/igovet/Web_Projects/cortex-orch-acp`:

| Check | Command | Result |
|---|---|---|
| plugin deps install | `npm --prefix integrations/paseo ci` | passed, 316 packages, exit 0 |
| typecheck | `npm --prefix integrations/paseo run typecheck` | passed (`tsc --noEmit`, no output = clean), exit 0 |
| activation tests | `npm --prefix integrations/paseo run test:activation` | passed, 2/2 tests | 
| steer tests | `npm --prefix integrations/paseo run test:steer` | passed (3 suites: steer, question, question-batch), all `ok` |

These confirm the `lockfix` fix holds: `npm ci` no longer fails on lock drift.

## GUI inspection mechanism

The `playwright` MCP server failed to connect this session
(`CONNECTION_CLOSED`). Per the assignment's fallback instruction, rendered-GUI
evidence was captured through the DevTools CDP endpoint the `gui` helper
printed (`ws://127.0.0.1:34671`), driven by a small Node 24 script
(`node --experimental-strip-types`-free, uses the built-in `WebSocket` global)
sending `Runtime.evaluate` (element lookup by text), `Input.dispatchMouseEvent`
/ `Input.dispatchKeyEvent` (clicks, typing, Enter) and `Page.captureScreenshot`.
Screenshots were copied from the dev home into
`docs/acp/evidence/livedesktop-*.png` for citation below (all under the
`docs/acp/evidence/**` scope). This is a genuine rendered-window inspection
(real CDP screenshots of the live Electron renderer), not log inference.

## Check 1 — worker `result_submit` with the plain-string `report` fix

Dispatched two tasks in the existing workspace's chat (model switched to
**Sonnet 5** = `claude-sonnet-5` for the coordinator turn, see
`livedesktop-model-selected.png`):

1. A direct question ("list files") — the coordinator answered inline via its
   own file-read tool without delegating (`livedesktop-progress1.png`); no
   worker dispatched, so this did not exercise `result_submit`.
2. An explicit delegation request ("dispatch one worker ... create
   `qa_check.txt` ... have it submit its result") — this did dispatch workers.

Observed in the rendered timeline (`livedesktop-progress4.png`,
`livedesktop-final.png`) and cross-checked against the disposable project's
`.cortex/cortex.sqlite3` (opened read-only via `sqlite3 -readonly`):

| # | Role / engine | Status (UI) | `task_events` row | `assignment_ref` | `report_ref` |
|---|---|---|---|---|---|
| 1 | `general` · codex · `6 Luna` | Completed | id 1241, `worker_result`, 2026-10-08T00:07:39Z | `xa_5e7bbcef275a` | `r_ccc0ce3503d9` |
| 2 | `code-reviewer` · **claude · Sonnet 5** (claude-agent-acp) | Completed | id 1344, `worker_result`, 2026-10-08T00:09:51Z | `xa_8407ad8b7477` | `r_856ce912c2f2` |

Both `worker_result` payloads are clean JSON with `report_ref` and `summary`,
no error/refusal field. `SELECT ... WHERE event_type='assignment_failed'`
scoped to this task returned **zero rows** for the whole task history,
including this session's window. A `find`/`grep` sweep of engine stderr logs
modified during the session for `result_submit` found no matches (no engine
reported a result_submit-related stderr error).

**Refusal/error count for this live run: 0 of 2 `result_submit` calls refused,
both accepted**, including one on claude-agent-acp Sonnet 5 — the engine the
objective specifically wanted re-exercised after the plain-string `report` fix.

Filesystem confirmation: `cat /home/igovet/paseo-live-desktop-20261007h/qa_check.txt`
→ `livedesktop qa ok` (exact single line, matching the delegated instruction
and the worker's own summary).

**Verdict: PASS.**

## Check 2 — Haiku 5.5 (claude-agent-acp `haiku`) route parity with gpt-6-luna

Opened the Cortex model picker ("Open Cortex settings" button in the
per-message model dropdown) → **Discovered: 6** models
(`livedesktop-cortex-settings1.png`):

- Opus 5.5 — `opus[1m]`
- Fable 5.1 — `claude-fable-5-1[1m]`
- Sonnet 5 — `claude-sonnet-5`
- 6.1 Sol — `gpt-6.1-sol`
- 6 Astra — `gpt-6-astra`
- 6 Luna — `gpt-6-luna`

No Haiku entry of any kind. Clicked **Refresh** and re-captured
(`livedesktop-cortex-settings-refreshed.png`): still exactly 6 discovered
models, "Updated just now", Haiku still absent. Also checked the Paseo-native
"Agents" settings page (`livedesktop-agents1.png`) — that page configures
Paseo agent profiles (e.g. a `Cortex QA — Codex gpt-5.6-luna` profile), not
the Cortex engine route catalogue, and likewise shows no Haiku reference.

This is rendered, refreshed, genuine evidence that **Haiku 5.5 is not
currently a discovered/enabled claude-agent-acp route** in this dev
environment, at any parity level with gpt-6-luna. It does not by itself
distinguish "adapter doesn't expose it yet" from "not configured in this
preset" — both are plausible given the original goal text asked for Haiku 5.5
to be *added*, which is separate implementation work outside this QA
assignment's `docs/acp/evidence/**` scope.

**Verdict: FAIL (not present).** This directly answers the user's standing
question: it is not a rendering glitch — the route genuinely does not appear
in the live, refreshed Cortex model catalogue of this dev instance.

## Cleanup

- `python3 -B scripts/cortex_paseo_dev.py -- daemon stop --timeout 15 --json`
  → `{"action": "stopped", "pid": 3962971, ...}`.
- Sent SIGTERM to the `gui --xvfb` helper; confirmed via `pgrep` that no
  `Paseo.bin` process remains under `--user-data-dir=/home/igovet/.config/Paseo-dev`
  (only the user's unrelated production `~/.config/Paseo` instance is still
  running, untouched by this session).
- Production `~/.paseo` and its plugin were never started, stopped, or
  modified by this session.

## Screenshot evidence index (all under `docs/acp/evidence/`)

`livedesktop-shot1.png`, `livedesktop-model-dropdown.png`,
`livedesktop-settings1.png`, `livedesktop-agents1.png`,
`livedesktop-cortex-settings1.png`, `livedesktop-cortex-settings-refreshed.png`,
`livedesktop-composer-filled.png`, `livedesktop-model-selected.png`,
`livedesktop-composer-filled2.png`, `livedesktop-sent1.png`,
`livedesktop-sent2.png`, `livedesktop-progress1.png` .. `-progress4.png`,
`livedesktop-final.png`.

## Limitations

- The disposable project was reused from a prior attempt in this same owner
  request rather than freshly created; its pre-existing `.cortex` store
  contains history from earlier (also disposable) sessions, visible in the
  `task_events` table before this session's timestamps. This session's own
  evidence is scoped to the rows/screenshots timestamped 2026-10-08T00:0x,
  cited above by exact id.
- Engine stderr logs were swept for the literal string `result_submit`; the
  MCP JSON-RPC traffic itself is not persisted to those log files in this
  build, so the sqlite `task_events` table (not the stderr sweep) is the
  primary evidence for check 1.
