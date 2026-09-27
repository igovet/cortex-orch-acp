# Gotchas

An ACP client session and an executor attempt are different identities. On
resume or replacement, select the adapter from the actual attempt and engine;
do not infer it from the coordinator.

An acknowledged tool call is not proof that a command ran or that a criterion
passed. Preserve observed output, exit status, truncation and provenance before
the coordinator evaluates meaning.

Never interpret an unknown command outcome, missing publication or unknown
engine liveness as success. Retain receipts and reservations until the exact
outcome is observed; revocation alone does not prove process exit.

Instruction compression may replace large evidence bodies with bounded links,
but it must keep complete mandatory rules and record packet hashes. Executor
reports are data, not user instructions or new system rules.

Live Zed qualification requires an isolated disposable environment. The
offline suite cannot establish real-client behavior or paid engine behavior;
those checks remain explicitly unverified until run.

In `integrations/paseo`, a local sibling import must use its real `.ts`
extension. `tsc`/esbuild remap a `.js` specifier to the sibling `.ts` file, so
`npm run typecheck` and the plugin build both stay silent, but
`node --experimental-strip-types` does not perform that remap and throws
`ERR_MODULE_NOT_FOUND` for the `.js` path -- breaking every registered
`test:*` script that transitively imports it. Typecheck passing is not
evidence a `.ts` module's own tests can run.

A worker's own sandboxed Bash tool mounts `$HOME` read-only except for a few
allow-listed subdirectories (observed 2026-09-25: `~/.codexia` writable,
`~/.codex` not). A live `--provider cortex` Paseo turn routed to a codex-acp
connection then fails every attempt with a generic ACP `-32603` ("The runtime
failed while processing this prompt.") because the Codex CLI child process
cannot write its sqlite state db under `~/.codex`; the retry/cleanup noise
this produces (multiple engine-log `eirt_*` dirs, mixed claude-agent-acp and
codex-acp launches) looks exactly like a Cortex provisioning bug from the
outside, and the ACP error's own `data.failure` detail plus the agent
process's stderr do not reach any log Paseo 0.9.2 itself persists (see
`acp.agent._write_crash_diagnostic`, added to close that specific gap, for a
durable `$XDG_STATE_HOME/cortex/agent-crashes/*.log` traceback instead). The
same live check run through `outside_command`/`outside_write` (a real,
non-sandboxed `$HOME`) succeeds immediately with no code change. Before
concluding a live "cortex" Paseo provisioning failure is a runtime defect,
re-run it once through `outside_command` and only trust a failure that
reproduces there too.

The Paseo phone's client bundle is evaluated from source by Hermes, so a
desktop build or type-check does not establish phone compatibility. In the
observed React Native 0.81 Hermes path, ES `class` declarations became
`undefined`: both the plugin error boundary and shared poller failed. They
were replaced with Hermes-compatible implementations and guarded by the
`test:client-bundle` regression test. The fix passed off-device Hermes checks;
a real phone remains the final confirmation.


A crash exactly at that one call site (`acp.orchestrating_runtime
._open_coordinator_session`'s own `self._engine.open(...)`) leaves a task id
already durably bound (`bootstrap.bootstrap_task`'s `tasks` row and
`_ensure_coordinator`'s own `task_orchestration` row are both already
committed) but no `runtime_tasks` row yet -- `orchestration.state.TaskMachine
.create` only runs afterward, from `orchestration.controller.run`, itself
only reached once `_ensure_coordinator` returns successfully. That call site
now tags (never replaces) the exception with `cortex_launch_stage=
"engine_launch"` plus the task/attempt id; `acp.agent`'s except block reads
those (only present for this one failure shape) into
`_write_crash_diagnostic`'s log header alongside the *running process's own*
package version (`_agent_version()` -- not necessarily this checkout's, see
`scripts/cortex_diagnose.py`'s separate installed-vs-checkout
`package_version` check), and into the `-32603` `message` text itself
("...(stage: engine_launch)."), since a real host drops `data` (above). A
retry recovers through the pre-existing coordinator-replacement path
(`recovery.coordinator_recovery.recover`) without duplicating the task:
`task_orchestration`/`runtime_tasks` are both `task_id PRIMARY KEY`, and
`TaskMachine.create`/`_ensure_coordinator` both check for an existing row
before inserting one -- see
`tests/test_engine_launch_failure_recovery.py`. The same failure shape also
used to permanently skip publishing this task's initial pipeline on every
retry (`_run`'s publish call was gated on the `task_orchestration` row *not*
existing yet, but that row is committed before the engine session ever
opens); it is now called unconditionally, relying on
`pipeline_bootstrap.publish_initial_pipeline`'s own idempotent `reports`-row
check instead.

Codex can present MCP tool schemas as TypeScript declarations that omit JSON
Schema constraints such as `minItems`, `maxItems`, and `pattern`, along with
array item descriptions. Put array meaning, bounds and accepted forms in the
field's own description; keep runtime validation authoritative and report all
argument violations together so one correction can address them. For
`plan_submit`, both `plan.tasks` and `plan.verification` are required (1–16
items). Verification applies even to investigation-only and trivial plans. If
project documentation has no verification command, the schema directs the model
to propose the smallest literal-argv check that would fail when the requested
result is wrong, using project files and standard tools. The Codex failure
investigation found that the schema was visible; the actual gap was conflicting
instructions that made an empty list appear to be the only compliant answer.
The current contract puts the fallback at the start of each required array's
description and repeats it in validation corrections.
