# Direct-driver live Claude qualification — worker dispatch and route refusal

Observed 2026-09-23 (UTC times below), against the real production
`src/cortex_acp.py` entry point, installed `claude-agent-acp` 0.81.0
(bundles `@anthropic-ai/claude-agent-sdk` 0.3.270; node `22.23.2`), package
`1.0.0+sha256.dc3eaba84cbf86d7`. This is not Paseo or Zed: no qualified
GUI/real-client harness for a full worker cycle exists in this checkout
(the two authorized paid entry points, `scripts/cortex_claude_probe.py`
and `scripts/cortex_codex_probe.py`, are explicitly single-turn transport
probes only — "no worker cycle, runtime completion or GUI qualification
is claimed"). To observe the two goals below, a throwaway ~180-line ACP
stdio client driver was written for this run only, under
`/tmp/cortex-live-claude-qual/driver.py` (never committed, never part of
this checkout); it sends real `initialize`/`session/new`/
`session/set_config_option`/`session/prompt` frames and auto-approves any
`session/request_permission` the agent forwards. No source file in this
checkout was modified.

**Setup (AGENTS.md live-policy rules).** Disposable project root and
disposable `XDG_CONFIG_HOME`/`XDG_STATE_HOME` under `/tmp/cortex-live-claude-qual/`
(a `.cortex`-store-safe location per `_assert_not_nested_in_project_workspace`),
never this checkout's own `.cortex`, never the user's real
`~/.config/cortex`/`~/.local/state/cortex`. `HOME` was the real
`/home/igovet` so `cortex_runtime.connections.discovery`/
`onboarding.sync_discovered_connections` (the exact call
`src/cortex_acp.py`'s own `_ensure_first_run_configuration` makes on every
real startup) auto-registered two **referenced** connections
(`EngineNativeDir(owned=False)`, pointing at the user's existing
`~/.claude`/`~/.codex`, matching D-T035-02) with `state=CONFIGURED` and no
login step and no credential copy: `cn_...4dca` (claude-agent-acp) and
`cn_...bbe2` (codex-acp). **Codex was never selected or dispatched in any
of these runs** — confirmed by grepping every saved transcript for
`codex`: it appears exactly once per run, only as the unselected
`session/set_config_option` "provider" option — per this task's explicit
"Claude only, no Codex" instruction. The install record used for
`claude-agent-acp` 0.81.0 was built from Cortex's own discovered,
qualified-launch copy at `~/.local/share/cortex/engines/claude-agent-acp/0.81.0/`
(kept current by `connections.engine_update`, not the Zed registry copy);
`engine.claude_install.verify_install` accepted it (re-hashed
`package.json`/`dist/index.js` fresh, no drift). A `.cortex/routing.json`
project-local pin was written per run (`docs/acp/real-client-runbook.md`'s
documented Luna recipe, applied here to Claude instead): run 1 pinned
`{"pin": {"*": "claude-agent-acp/haiku"}}`; run 2 pinned the same shape to
a deliberately nonexistent model id. The coordinator's own route (not
governed by `routing.json`, which only governs `routing.auto_route.choose`
for worker roles) was selected explicitly via two real
`session/set_config_option` calls (`provider` → the Claude connection,
`model` → `haiku`) before the first prompt, confirmed by the live
`config_option_update` echoing `"currentValue": "haiku"`.

## Goal 1 — a worker routed to Claude `haiku` starts and completes a trivial assignment: **PASS**

Prompt: "Create and submit a plan with exactly one implementation worker.
The worker's only job: create a file named `hello.txt` ... containing
exactly the text `cortex-live-qual-ok` ... complete through the normal
mandatory review and verification flow." Full transcript retained at
`/tmp/cortex-live-claude-qual/run2-worker.json` (private qualification
artifact, not checked in; 341 `session/update` events, `stopReason:
"end_turn"`).

Observed, from the disposable project's own `.cortex/cortex.sqlite3`
(task `t_4b6c6eb61763`):

- `runtime_tasks.state = COMPLETED` (`review_cycle=1`, `blocker=NULL`).
- `attempt_route_audit`: coordinator `co_7f3151ebf1cc` (engine
  `claude-agent-acp`, model `haiku`, connection `cn_...4dca`, decision
  `authorized`); worker `xa_f9cc00d7f9a4` role `worker-general` (same
  engine/model/connection, `decision_detail_json.route_basis: "auto:pin"`
  — confirms `routing.json`'s pin actually drove the automatic worker
  route, not a coincidental default); reviewer `xa_9108745472d4` role
  `worker-code-reviewer` (same engine/model, also `auto:pin`).
- Live ACP `tool_call_update` titles read literally `"Worker: general ·
  claude · haiku — ..."` and `"Worker: code-reviewer · claude · haiku —
  ..."`, both reaching terminal `status: "completed"`.
- Runtime-executed verification (`Verify project` tool calls, real
  `subprocess` argv, not the worker's own claim): `["test", "-f",
  "hello.txt"]` → `exit_code: 0`; `["sh", "-c", "cat hello.txt | grep -x
  'cortex-live-qual-ok'"]` → `exit_code: 0`, `output: "cortex-live-qual-ok\n"`.
  The file exists on disk at `/tmp/cortex-live-claude-qual/project/hello.txt`
  with exactly that content (checked again directly after the run, outside
  the runtime).
- `WorkerModeNotConfirmed` does not appear anywhere in the retained
  transcript or driver stderr — no worker-mode confirmation failure was
  raised for either dispatched worker.
- Final coordinator text: "Task completed. ... independent code review
  approved the work, and all runtime verification checks passed. ...
  Runtime: COMPLETED."
- Real cost: 24,345 of a 200,000-token session window per the engine's own
  `usage_update` (Haiku pricing; no other model was touched in this run).

One incidental, non-blocking observation: `instruction_bundle_deliveries`
recorded `engine_version = "0.78.0"` for every bundle in this run even
though the real connected engine is 0.81.0. This is the documented,
intentional `_pinned_version_for` compatibility shim in
`delegation/live_adapters.py` (comment dated 2026-09-22: resolving the
real installed version broke every Claude worker's first prompt with an
UNSUPPORTED delivery, because only 0.78.0 has an exact
`instruction_delivery_registry` entry) — not a new defect. This live run
is the first retained confirmation that the shim's assumption (0.81.0
still honors the same `session/new _meta.systemPrompt` channel as 0.78.0)
holds in practice: both workers' full role-instruction bodies were present
verbatim in the delivered `_meta`, and both behaved according to their
role (the general worker wrote exactly the requested file; the reviewer
read status/diff and returned an explicit verdict).

## Goal 2 — an unavailable route stops with a named blocker instead of looping: **PASS** (at the runtime level)

Fresh disposable project (`project2`), same coordinator config selection
(Claude/haiku), `.cortex/routing.json` pinning `"*"` to
`claude-agent-acp/nonexistent-model-xyz`. Same trivial-file prompt.
Transcript at `/tmp/cortex-live-claude-qual/run3-blocker.json`
(`stopReason: "end_turn"`).

From the project's own `.cortex/cortex.sqlite3` (task `t_12da164e0570`):

- `runtime_tasks.state = WAITING_USER` — the task stopped cleanly on a
  real coordinator question, not an unbounded loop.
- `task_events` grouped by type: `delegation_requested=4`,
  `assignment_failed=4`, `assignment_cancelled=4`, `progress=2`,
  `user_message=1`. **Zero** `ASSIGNMENT_RETRIED` events — the runtime
  itself never internally retried this refusal, matching
  `docs/acp/routing-and-fallback.md`'s documented rule that
  `pinned_route_unavailable` "is never retried."
- Every one of the 4 dispatch attempts (for 4 different role names the
  coordinator itself chose across 4 separate plan revisions —
  `worker-general`, `worker-fullstack-dev`, `worker-backend-dev`,
  `worker-devops-engineer`) failed immediately with the identical,
  correctly-named, non-retryable reason, verbatim: `"Not delegated/failed:
  pinned_route_unavailable routing.json pins role '<role>' to
  claude-agent-acp/nonexistent-model-xyz, which no configured connection
  currently offers; connect it or change the pin."` No worker ever opened
  an engine session for this task (`attempt_route_audit` holds only the
  coordinator's own row).
- The real Haiku *coordinator* itself did not recognize this as a routing
  problem right away: it tried four different specialist role names
  across four plan revisions before stopping and asking the user how to
  proceed (offering, correctly, to "fix the routing.json configuration").
  That is coordinator-model judgment, not a runtime defect — the runtime's
  own behavior was exactly "stop with a named blocker, never retried",
  once per attempt, every time; the total real cost was small (four cheap
  refused delegations plus one extra coordinator turn, 29,921 of 200,000
  tokens).

**Not observed / explicit scope limit:** the assignment's other named
scenario — "the runtime moves the retry to another available route" (an
actual authorized cross-provider or cross-model fallback after a route
recorded unavailable) — was **not exercised** in this run. Doing so would
require either a second qualified, distinct Claude connection/model to
fail into, or Codex as the fallback target, and this task explicitly
forbids using Codex in this session (the user's paid quota for it is
exhausted). Only the "stops with a named, non-retried blocker" half of
goal 2 was actually observed; the "moves to another route" half remains
**NOT RUN**, not fabricated.

## Explicit scope and limits

- Single Claude connection, single model (`haiku`), two short tasks. This
  is not a GUI/Zed/Paseo qualification (no row in
  `docs/acp/real-client-runbook.md` is touched or claimed passed by this
  note) and not a broad regression sweep.
- The `pinned_route_unavailable` case is a structural refusal (an
  unresolvable model name), not a real quota-exhaustion/rate-limit trigger
  (`routing.authorization`'s `ROUTE_UNAVAILABLE` recording from an actual
  engine-reported `usageLimitExceeded`/`billing_error` never fired here);
  that specific recording path remains unverified live.
- The ad hoc driver script is this qualification run's own tool, not a
  new supported entry point; it is not retained in the repository and
  should not be treated as a documented harness for future runs.
- All engine subprocesses this run started exited cleanly when the driver
  closed each session (checked directly after each run: no leftover
  process under `/tmp/cortex-live-claude-qual/`).
