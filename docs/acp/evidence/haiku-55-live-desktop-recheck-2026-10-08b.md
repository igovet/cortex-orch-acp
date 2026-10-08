# Haiku 5.5 live desktop recheck (post-`catalogfix`) — 2026-10-08

Owner-authorized paid live check in `desktop` mode ("live dev разрешаю в desktop"),
re-verifying after the `catalogfix` task (`r_01efdcc48143`, which fixed the stale
dev `routing.json` and the refresh-killed-by-probe bug via a detached refresh
process). Prior desktop attempts (`r_04805579ce05`, `haiku-55-live-route-recheck-2026-10-08.md`)
found Haiku 5.5 absent; this recheck repeats the same rendered-GUI methodology
against the fixed runtime.

## Result

**Both checks PASS.**

1. **Haiku 5.5 route parity with 6 Luna: PASS.** After a rendered Refresh, the
   Cortex route picker's Discovered list shows exactly 7 models (Opus 5.5,
   Fable 5.1, Sonnet 5, **Haiku 5.5**, 6.1 Sol, 6 Astra, 6 Luna) — no Haiku 4.5.
2. **A worker running on Haiku 5.5 submitted a result that was accepted: PASS.**
   A small one-shot Cortex task ran one `worker-general` assignment on
   `claude-agent-acp`/`haiku` (Haiku 5.5) that completed and whose
   `result_submit` was accepted, confirmed independently in the disposable
   project's own `.cortex/cortex.sqlite3`.

## How GUI evidence was captured

The `playwright` MCP server was disconnected this session (`CONNECTION_CLOSED`,
reported at session start). Per the assignment's fallback instruction, the
rendered Electron window was driven and inspected through the DevTools CDP
endpoint printed by `gui --xvfb` (`ws://127.0.0.1:47667`), using a small Node 24
script (built-in `WebSocket`) sending `Target.attachToTarget`, `Runtime.evaluate`,
`Input.dispatchMouseEvent` / `dispatchKeyEvent` / `insertText`, and
`Page.captureScreenshot`. All 26 screenshots in this directory
(`haiku-recheck-shot1.png` .. `shot26.png`) are genuine CDP screen captures of
that one Electron window, copied into the granted `docs/acp/evidence/**` scope
and read back to confirm their content before citing them. Key frames:
`shot19.png`/`shot20.png` (Cortex Settings dialog, Discovered list before/after
Refresh), `shot25.png`/`shot26.png` (the one-shot task's two worker cards).

## Preflight (no-paid) — all passed, exit 0

- `npm --prefix integrations/paseo ci` — 316 packages installed.
- `npm --prefix integrations/paseo run typecheck` — clean (`tsc --noEmit`).
- `npm --prefix integrations/paseo run test:activation` — 2/2 passed.
- `npm --prefix integrations/paseo run test:steer` — 3/3 suites `ok`.

## Environment

- Isolated `~/.paseo-dev` daemon (`srv_dLC3aKF5GZxO`), started via
  `python3 -B scripts/cortex_paseo_dev.py -- start --timeout 60 --json`
  through `outside_command` (per the `/var/tmp` bubblewrap-mount warning in
  `docs/acp/dev-paseo-environment.md`), listening on `127.0.0.1:16767`.
- The `cortex-acp` plugin record already pointed at this exact current
  checkout path (`/home/igovet/Web_Projects/cortex-orch-acp/integrations/paseo`),
  enabled and `running` (`plugin ls --json`); a reinstall attempt correctly
  refused with "Plugin ID already configured" since it was already the current
  checkout's fixed runtime — no change was needed.
- Separate `~/.config/Paseo-dev` Electron profile, `gui --xvfb` helper
  (Paseo Electron **0.11.1**, matching the environment-doc note that this
  differs from the last-recorded 0.10.2 package inspection), private Xvfb,
  DevTools bound to `127.0.0.1:47667`.
- **Fresh disposable project**, created this session and not reused from any
  prior attempt: `/home/igovet/paseo-haiku-recheck-20261008` (outside this
  checkout). Added to the dev Paseo workspace list via the in-app
  "Add project → Search for directory" quick-open (typed the absolute path;
  the root "Add project" menu's own filter field does not accept synthetic
  keystrokes reliably, but its "Search for directory" sub-screen does — this
  was confirmed by first successfully typing into the unrelated "Clone from
  GitHub" sub-screen, which proved the CDP typing mechanism itself worked).
- Production `~/.paseo` and the production plugin were never started, stopped,
  or modified.

## Native login (no credentials printed)

Reused from the current session state; both engines' production accounts are
the approved ones per `docs/acp/dev-paseo-environment.md` (not re-probed this
attempt beyond the dev daemon's own `status --json` provider `available: true`
flags observed live during the task, visible in the rendered timeline).

## Check 1 — Haiku 5.5 route parity, detailed evidence

- Opened the Cortex route picker ("Cortex" provider entry → gear icon →
  Cortex Settings dialog) in the fresh disposable project: `Discovered: 7`,
  listing `Opus 5.5 (opus[1m])`, `Fable 5.1 (claude-fable-5-1[1m])`,
  `Sonnet 5 (claude-sonnet-5)`, **`Haiku 5.5 (haiku)`**, `6.1 Sol (gpt-6.1-sol)`,
  `6 Astra (gpt-6-astra)`, `6 Luna (gpt-6-luna)` — `shot19.png`.
- Clicked **Refresh**; the dialog's footer changed from "Updated 7m ago" to
  **"Updated just now"**, with the same 7-entry Discovered list still showing
  Haiku 5.5 — `shot20.png`.
- Independently confirmed on disk (not just rendered text) that Refresh wrote
  a **genuine new catalog snapshot** at the exact Refresh timestamp:
  `~/.paseo-dev/cortex-xdg/state/cortex/catalog/cn_3aec66405544/cs_61514500d81c.json`,
  `created_at: 2026-10-08T02:10:57Z`. Its one `haiku`-route entry:
  `model_name: "Haiku 5.5"`, `route.model: "haiku"`, label
  `claude-agent-acp · claude-sonnet-live · haiku`. No `Haiku 4.5` entry exists
  anywhere in the snapshot.
- Independently confirmed Luna parity by reading the dev profile's
  `routing.json` directly (ground truth, not UI-derived):
  - `enabled.claude-agent-acp` includes `"haiku"`; `deny` includes
    `"claude-agent-acp/claude-haiku-4-5-20251001"` (old Haiku 4.5 explicitly
    denied, not enabled).
  - `models.claude-agent-acp.tiers.haiku: ["light"]` — same tier as
    `models.codex-acp.tiers.gpt-6-luna: ["light"]`.
  - `models.claude-agent-acp.policy.haiku` is **byte-for-byte parity** with
    `models.codex-acp.policy.gpt-6-luna`: `effort_floor: "high"`,
    `effort_ladder: ["high","xhigh","max"]`, `max_effort: "max"`,
    `also_standard_for: ["testing"]`, and the identical
    `preferred_for: ["analysis","writing","implementation","testing",
    "debugging","investigation","operations"]` list.

## Check 2 — a Haiku 5.5 worker's `result_submit` accepted, detailed evidence

- In the fresh disposable project, explicitly selected **Haiku 5.5** as the
  chat model (`shot22.png`) and sent one trivial one-shot prompt (a CDP typing
  artifact dropped literal `.` characters, so the requested filename rendered
  as `haiku-checktxt` instead of `haiku-check.txt` — functionally immaterial,
  the task still exercised a real worker end to end).
- The coordinator planned one `worker-general` assignment plus the mandatory
  independent `code-reviewer` review. The first worker card rendered:
  **`general · claude · Haiku 5.5 · high · light · by affinity · usage 32% ·
  completed`** (`shot25.png`) — engine `claude`, model `Haiku 5.5`, effort
  `high`, tier `light`, exactly matching the `routing.json` policy above.
- The task finished: `code-reviewer` (`codex · 6.1 Sol`) returned **APPROVE**
  after an independent byte-comparison check, synthesis submitted, pipeline
  shows "Завершено" / "Worked for 1m 24s" (`shot26.png`).
- Independently confirmed in the disposable project's own
  `.cortex/cortex.sqlite3` (not just rendered text):
  - `attempt_route_audit`: `('xa_a0286c0223e0', 'worker-general',
    'claude-agent-acp', 'cn_3aec66405544', 'haiku', 'authorized')` — the
    worker's route was genuinely `claude-agent-acp`/`haiku`, authorized.
  - `task_events` (`event_type='worker_result'`): two clean rows, no
    `assignment_failed` rows for this task —
    `(assignment_ref='xa_a0286c0223e0', report_ref='r_7607ab2276cb', ...)` for
    the Haiku 5.5 worker, and `(assignment_ref='xa_cd1e03660a52',
    report_ref='r_6cb39085279d', ...)` for the reviewer — both with a clean
    `summary` field and no `error` field.

## Cleanup

- GUI: SIGTERM to the `gui --xvfb` helper (PID `103470`); confirmed via
  `pgrep` that no Electron process under the dev `--user-data-dir=.../Paseo-dev`
  profile remained.
- Daemon: `python3 -B scripts/cortex_paseo_dev.py -- daemon stop --timeout 15
  --json` → `{"action":"stopped","pid":101246,"forced":false,...}`; a
  follow-up `status --json` confirmed `localDaemon: "stopped"`.
- The production `~/.config/Paseo` desktop profile and `~/.paseo` were never
  touched; only the dev home/profile/daemon described above were started and
  stopped.

## Limits

- No paid live model-prompt check beyond the one small task described above
  (per the brief, heavy/flagship models remain out of scope; only one light
  Haiku 5.5 worker task was requested and run).
- The CDP-typed prompt text lost literal `.` characters (a quirk of the
  synthetic `Input.dispatchKeyEvent` stream against this specific
  React-Native-Web app, confirmed by first reproducing reliable typing on an
  unrelated "Clone from GitHub" sub-screen); this did not affect either
  check's outcome, only the literal filename the worker created.
- Native engine login status was not re-probed this attempt beyond the dev
  daemon's own `status --json` `available: true` flags observed live; no
  credentials were printed or stored.

## Owner's post-deploy production verification steps (not yet performed)

This recheck covers the isolated dev Paseo instance only; **production
`~/.paseo` and its plugin were never touched**. Per the owner's decision to
finish this task now and verify production personally after deploying the
plugin (answer to `pq_3445a1c74e41`), the remaining steps are:

1. Deploy the current checkout's runtime to the production Paseo plugin.
2. Restart Paseo.
3. Press **Refresh** in the Cortex model picker (Cortex Settings dialog).
4. Expect the Discovered list to show **`Haiku 5.5`** (`claude-agent-acp`
   `haiku`) enabled and auto-eligible, with the same tier/effort/preferred
   settings as `6 Luna` (tier `light`; policy `effort_floor: "high"`,
   `effort_ladder: ["high","xhigh","max"]`, `max_effort: "max"`,
   `also_standard_for: ["testing"]`, and the full `preferred_for` list above).
5. Confirm the old **`Haiku 4.5`** is *not* enabled (absent from Discovered,
   present only in `deny`, as observed in dev above).

No credentials, raw logs or production config contents are reproduced here;
this is a procedural checklist only.
