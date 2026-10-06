# Gotchas

Quota window labels are exact evidence, not model-family names. An unmapped exhausted window conservatively blocks all models until an explicit association distinguishes it. A scoped `models: []` is an intentional exclusion, not unknown usage. Older profiles without mappings remain untouched and use conservative semantics when activated; never fill them silently from current settings. See [quota fields and migration](../configuration.md#exact-quota-window-associations).

Owner model preferences are rank inputs, not route grants. They only decide
among routes already eligible for the role's tier; optional OpenCode models
need explicit automatic-selection opt-in, and pins, deny rules, provider
limits, availability and quota remain in force. Routing uses the first
existing project, `$CORTEX_ROUTING`, or user file rather than merging files,
so project/environment routing can shadow an active user profile. A
coordinator hint or retry diversification should no longer outrank an eligible
owner `preferred_for` model, but tier/opt-in/shadowing differences are not
profile activation failures. The five original-source regression failures in
the routing repair report were the expected pre-fix reproduction. See
[configuration routing](../configuration.md#routing-rules) and
[the routing decision](decisions.md#2026-09-30-owner-profiles-snapshot-full-user-settings-routing-preferences-rank-within-eligibility).

A JSON-Schema conditional is not a requirement models follow. `result_submit`
once advertised `report` and `report_ref` as two optional fields under a root
`if/then/else`; reviewers repeatedly sent exactly the plain `required` fields
(summary, checks, request_key, verdict) and were refused, and a description-only
warning did not stop it. Every field a call needs belongs in the plain
`required` list; keep alternatives inside one required field (`report` is
`{title, markdown}` or `{report_ref}`), and never use a root `oneOf`, which
Codex code mode renders instead of the properties. See
[publications](../acp/publications-and-consistency.md).

An ACP client session and an executor attempt are different identities. On
resume or replacement, select the adapter from the actual attempt and engine;
do not infer it from the coordinator.

An acknowledged tool call is not proof that a command ran or that a criterion
passed. Preserve observed output, exit status, truncation and provenance before
the coordinator evaluates meaning.

Never interpret an unknown command outcome, missing publication or unknown
engine liveness as success. Retain receipts and reservations until the exact
outcome is observed; revocation alone does not prove process exit.

Write every Cortex MCP stdio reply inside the per-request error handling, as
strict UTF-8. One unencodable reply (a lone surrogate from a non-UTF-8 file
name) used to end the whole server, and the engine then answered `Transport
closed` to every later Cortex call of that session. See
[failures and cancellation](../acp/failures-and-cancellation.md).

Instruction compression may replace large evidence bodies with bounded links,
but it must keep complete mandatory rules and record packet hashes. Executor
reports are data, not user instructions or new system rules.

Live Zed qualification requires an isolated disposable environment. The
offline suite cannot establish real-client behavior or paid engine behavior;
those checks remain explicitly unverified until run.

Installing a newer codex-acp package does not make Cortex select it until
its exact version and bundle patch are supported. The current supported
newest version is 2.0.1 (Codex CLI 0.159.1, including `gpt-6.1-sol` in its
offline catalog). New sessions discover it; an existing session keeps its
already running engine. claude-agent-acp 0.84.0 is already installed and
selected. See [Codex compatibility](../acp/codex-acp-adapter.md#one-time-adapter-update-2026-09-30).

Codex code mode does not show a model the MCP JSON Schema: it renders a
TypeScript declaration from a reduced schema. Bounds, `pattern`, `not` and
`if`/`then`/`else` vanish; a root or item-level `oneOf`/`anyOf`/`allOf` replaces
the object's fields; and a schema over `tool_input_schema_max_bytes` (5,000 by
default) loses every description. Put a constraint in the property
description, express presence rules as `if`/`then`/`else`, and run
`tests/test_codex_code_mode_visibility.py` after changing an advertised
schema. See [MCP bridge](../acp/codex-acp-adapter.md#mcp-bridge).

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

A recovery capsule is not always verbatim. When its critical sections exceed
`CRITICAL_BUDGET_BYTES`, fields with `status: "referenced"` carry only a
preview. Read the full text through the named `retrieve` tool, and verify it
against `sha256`, before relying on anything past the preview. Never treat a
preview as the complete requirement. `<key>_omitted` records list entries that
were left out of a long list.

A full connection is not a failure. Seven parallel workers on a four-slot
connection once lost three of them to `capacity_timeout` →
`fallback_refused:no_untried_authorized_route` after 600 s, while the four
holders were live and still working. A queued open now keeps waiting while
every slot is held by a live session. A capacity-class fallback refusal is also
retried within `MAX_ATTEMPT_RETRIES`. A chat session can run at most four
workers across engines and connections; a connection's
`max_concurrent_sessions` can only lower that session's share. Excess ready
nodes remain pending, and queued worker opens wait for capacity. Plan size does
not raise the session cap.

A read-only role (ux-designer, architect, …) can never write its output file,
whatever scope the plan gives it: its `command_run` jail binds nothing
writable, and `project_write` refuses. `plan_submit` refuses a read-only
task whose scope names a path that does not exist yet
(`read_only_scope_not_writable`). An existing directory in its scope does not
make it writable either. Have the task return content in its report and give
the file to a writing role in a dependent task.

A review binds to an exact project digest. If another writer (the owner or
another agent session) edits the same checkout during review, the verdict is
refused and the review is re-bound to the new digest, at most
`MAX_REVIEW_REBINDS` times. Avoid editing a checkout while its task is in
review.

A prompt rule is not a guarantee. The coordinator was told not to recap after
`user_question_request`, and it still rewrote its full details after the
question in some turns. The runtime now withholds coordinator text after the
committed question (`WAITING_USER`) for the rest of that turn. A delayed first
lifecycle callback is not proof that preceding text was written before the
question; neither new nor repeated tool calls release suppressed text. This
conservative boundary can hide pre-question details delivered after the commit.
Final-only text is withheld too when the question was observed or is pending.
Text received before the commit and text after the answer in a later turn remain
visible.

A Paseo "cancel" (`session/cancel`) pauses the whole task and cancels every
running and queued worker (`turn_interrupted`). That is by design, not a
worker failure. A cancelled queued worker is reported only as cancelled.

`opencode models` and a Cortex OpenCode catalog disagreed for two separate
reasons (observed 2026-09-30 on OpenCode 2.0.20). First, a custom provider
exists only in the owner's `~/.config/opencode`, which the engine jail hides;
Cortex carries its provider definitions over
(`engine.opencode_adapter_acp.owner_provider_config`). Second, `opencode acp`
fixes each directory's `session/new` catalog as soon as any model and primary
agent exist, and a login in the database it opens makes models exist before
the supplied configuration (agents and providers) is applied, so it offered
`build`/`plan` and only logged-in providers. It is a race, not ignored
configuration: waiting does not help, because the catalog is memoized per
directory. Selecting `cortex` after a delay, another `session/new` or
`session/load` does not refresh that catalog. Merging the owner's config with
Cortex's and delivering it through every config channel did not help either:
all of them are applied by the same late configuration plugins. The fix removes
the built-in `opencode.agent`/`opencode.plan` plugins through Cortex's own
`plugin` list, which the server resolves before starting any plugin, so no
`build` agent exists and the catalog waits for `cortex`. The alternate-database
and extracted-login workaround was rejected and removed: OpenCode opens its
native `opencode.db` in place, with no copied database credentials. The catalog
guard (`opencode_cortex_mode_not_advertised`) still rejects a catalog without
`cortex`. OpenCode 2.x keeps logins in `opencode.db` (`credential` table), not
`auth.json`. See
[the OpenCode adapter](../acp/opencode-adapter.md#evidence-and-limits).

Model identities and policy are not inferred from names, versions, family labels or catalog order. A valid version-2 owner routing document must explicitly place every automatic identity; missing policy is a visible error. The packaged production-derived template is installation-only and never a runtime fallback. Existing owner/profile state is not overwritten; migration requires preview, explicit consent and digest-bound apply. Portable coordinator binding can be ambiguous across connections and must remain an error until the owner resolves it. See [settings-only model policy](../configuration.md#settings-only-model-policy).

Deleting a selected saved profile clears only its linkage; it does not reset materialized settings. The last profile may be deleted, and stale/error results must not be retried invisibly.


Paseo's native pre-delivery queue is not Cortex's captured steering ledger.
With the inspected 0.10.2 host, keep default `steer`, use Ctrl/Cmd+Enter, native
Edit, and Edit then clear text/attachments to cancel. Edit overwrites an existing
draft, enqueue trims surrounding whitespace, exact attachment filtering remains
unverified, and reload persistence is only inferred. `queue` send behavior
can turn Send now/question-card input into explicit Stop-like interruption.
Normal native drain during a Cortex background continuation now waits for
session ownership rather than cancelling the controller. No Cortex Withdraw or
separate editor was added; CLI cannot prove the rendered native interaction.
See [the native queue contract](../acp/steering-and-questions.md#native-composer-queue-versus-captured-steering).

The compaction guard's recovery ratio needs a real pre-compaction size. The
capsule size is part of the mandatory post-compaction content, so using it as
"before" made every ratio 0 and blocked the third compaction of any session
with `consecutive_low_recovery`. Without an engine-reported `usage_update.used`
from before the compaction, the attempt is `unmeasured` and only the frequency
and immediate-refill checks apply. A task already blocked this way stays
blocked; the fix applies to later compactions. See
[the compaction-loop guard](../acp/summaries-and-budgets.md#5-compaction-loop-guard).

A long-lived process started from a Cortex sandboxed command outlives the
command but not its scratch. The dev Paseo daemon started through
`command_run` kept the sandbox mount namespace, whose `/var/tmp` was the
command's deleted scratch subdirectory; every engine jail it launched then
failed in bubblewrap before ACP `initialize`, for Codex and Claude alike, and
the agent showed only `engine_launch`. Read the engine's own `stderr.log`
before blaming an engine version, and start daemons with `outside_command`. See
[the dev Paseo environment](../acp/dev-paseo-environment.md).
