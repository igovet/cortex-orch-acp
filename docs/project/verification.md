# Verification

Commands below are copied exactly from `AGENTS.md`, `README.md`,
`docs/project/test-matrix.md` and `integrations/paseo/package.json`.

## Verification coverage

Plan a real check for every changed surface before work starts; the
[orchestrator reference](../../skills/orchestrator/references/verification-coverage.md)
sets out portable GUI, installation, runtime and documentation coverage using
the target project's own policy. For this Cortex checkout, UI/plugin changes use
the type-check and applicable `npm --prefix integrations/paseo run test:*` scripts
below; rendered checks require an owner-requested `desktop` run. Installation or
packaging changes need the documented real install in a disposable environment.
Runtime/protocol changes use focused tests and the no-paid smoke harness where
applicable. The Cortex-specific live policy and isolation rules remain below
and in `AGENTS.md`, not in portable coordinator guidance. Close every
check that can be performed in the authorized environment. An unrun or
unverified check needs a concrete blocker, and synthesis discloses it unless
closed by retained exact evidence. Runtime final verification can close a
declared-command gap with identical argv, complete exit-0 execution and
untruncated runtime-owned output on the current reviewed revision, with a digest
matching the gap author's runtime-observed publication or review binding. Execution
order does not matter when those digests match. The runtime retains the command
reference as `closed_by` and rederives it on reads and synthesis; missing/different
author bindings, model claims, failed/incomplete evidence and stale revisions do
not close it.
A speculative run is eligible only after authoritative credit on the approved
digest; its `CHECK_EXECUTION_STARTED` marker alone is diagnostic and never closes
a gap. Discarded candidates remain ineligible.

A worker's failed check is closed only by an exact later passing rerun of the same argv and cwd; that exact rerun may cross between `command_run` and `outside_command` in either direction, preserving both receipts and their operation/profile provenance, while a same-operation profile change still never matches. A supported syntax, regex, or statically proven literal-expectation error in the worker's own bounded, self-contained standalone Python probe may instead be retracted by a later verified passing correction; the failure stays as `retracted` history and is never a pass. A supported standalone-Python probe mistake or a direct Python script's argparse startup error (exit code 2) may instead be superseded by an explicit `command_correction` (`superseded` history, never a pass). A complete whole-tree `git diff --check` whose whitespace diagnostics name only paths reserved by sibling writers of the same plan revision is `not_attributable` and does not fail the submitting check. Runtime verification failures and real project test/lint/build failures are never retracted (see [check history](../features/orchestration/index.md#check-history-and-current-outcome)). Tests for these rules: `tests/test_check_closure_fixes.py` and `tests/test_check_supersession.py`. The status-capture recognizer, explanatory `open_failed_check` corrections, `omitted_failed_check` retention of presented failures and the 64 KiB inline command-output bound are covered by `tests/test_check_submission_loop.py` (with `tests/test_check_history.py` and `tests/test_delegation_evidence.py`).
Executable Bash regressions in `tests/test_check_submission_loop.py` cover `%n`
and variable-expanded `printf -v` overwriting a captured failure: their exit-0
`command_run` receipts remain unverified on `result_submit`, while safe
`echo`/`tail` wrappers retain status propagation and exact-rerun coverage.
See [check closure](../features/orchestration/index.md#check-history-and-current-outcome)
and `tests/test_deferred_check_closure.py`.

## Targeted tests for a task

Workers run only the tests covering their change: the test files they added or
changed and the existing tests of the modules, contracts and documents they
touched, selected with the default suite's command and explicit paths:

```
timeout --kill-after=5 300 env PYTHONDONTWRITEBYTECODE=1 .venv/bin/python -B -m pytest -q -n 0 --timeout=60 --timeout-method=thread tests/test_<module>.py
```

Changes to a shared contract include its cross-cutting tests: advertised tool
schemas (`tests/test_advertised_tool_contracts.py`,
`tests/test_all_mcp_advertised_contracts.py`), profile sources
(`tests/test_portable_roles.py`, `tests/test_instruction_sources_portable.py`
and the profile-generation check below) and package payload
(`tests/test_package.py` after stamping). Workers do not run the whole suite:
it belongs in `plan.verification`, which the runtime runs once after review,
through the bounded default runner, never an unbounded whole-suite pytest command.

## Exact adapter qualification

Codex ACP 2.3.0 and Claude ACP 0.97.0 retain the outgoing ACP v1 contract.
Run the bounded registration, discovery and protocol checks without a model call:

```bash
env PYTHONDONTWRITEBYTECODE=1 .venv/bin/python -B -m pytest -q tests/test_engine_adapter_versions.py tests/test_engine_version_resolution.py tests/test_instruction_delivery_qualification.py tests/test_engine_codex_launch.py tests/test_engine_claude_evidence.py
env PYTHONDONTWRITEBYTECODE=1 .venv/bin/python -B -m pytest -q tests/test_engine_update.py tests/test_connections_discovery.py tests/test_route_availability.py tests/test_auto_route_delegation.py tests/test_engine_option_mapping_dispatch.py tests/test_generation_instruction_binding.py tests/test_context_generations.py
```

Installed-package checks skip explicitly when the named local distribution is
absent; a skip is not qualification. `tests/test_codex_developer_instructions.py`
additionally runs the actual private bundle against a fake App Server when
`CORTEX_CODEX_ACP_TEST_PACKAGE` names the inspected package directory.
Exercise both 2.3.0 and retained 2.1.1 separately. The real-adapter initialization
tests use empty isolated account directories and send no prompt. None of these
checks establishes paid CLI/desktop behavior or real inference cancellation.
See the [engine evidence](../features/codex-engine/index.md#current-support-230-2026-10-27)
and [Claude evidence](../features/claude-engine/index.md#current-support-0970-2026-10-27).

## Model hint notice checks

Run the no-paid routing/notice and native fake-engine regressions:

```bash
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_route_notices.py tests/test_auto_route_delegation.py tests/test_routing_visibility.py tests/test_explicit_effort_handoff.py
```

These exercise matching pins, real differences, explicit effort, unknown/conflicting
audit bindings, transaction rollback and repeated/recovered notifications. Wire
assertions use disposable fake engines, not paid native accounts. Neither notice
text nor a dispatch audit proves native live configuration. See
[notice semantics](../features/routing-and-fallback/index.md#ignored-model-hint-notices).

## Tool discovery and read batches

Run the changed-surface checks sequentially, not the entire suite:

```bash
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_tool_read_api.py
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_advertised_tool_contracts.py tests/test_all_mcp_advertised_contracts.py tests/test_codex_tool_cheat_sheet.py tests/test_codex_code_mode_visibility.py tests/test_delegation_contracts.py tests/test_codex_visible_tool_contracts.py
python3 -B scripts/cortex_read_api_benchmark.py
```

The benchmark reports deterministic JSON bytes and illustrative client round
trips, never paid token/cost measurements. Real dispatch/stdio tests cover
authority loss, schema parity, preflight, per-item failure, bounds, deferred
retrieval and conservative report observations; they do not prove live model
usage. Requested live CLI qualification follows the [dev guide](dev-environment.md)
and [read API scenarios](../features/tool-discovery-and-read-batch/index.md#live-cli-qualification).
Cancellation interleavings not established by bound events stay unverified.

The [executable public evidence matrix](../features/tool-discovery-and-read-batch/index.md#executable-public-evidence)
provides a no-paid real-stdio fixture and a separate current-task live assertion
reader. Run these focused regressions sequentially with other checks:

```bash
python3 -B -m pytest -q tests/test_read_api_evidence.py tests/test_qualification_assert.py tests/test_task_inspection.py
python3 -B -m pytest -q tests/test_qualification_observer.py
python3 -B scripts/cortex_read_api_fixture.py
```

Expected literal bytes, current effective schemas, native parity, receipt
identity and explicit completeness matter; a digest alone, preflight readiness
or `COMPLETED` does not prove API behavior. Valid unchanged-project review
waivers remain valid; `--require-review` explicitly tests a legitimate edit and
its runtime reviewer. Missing/forged/wrong-owner/task/stale/replayed/truncated/
pending evidence blocks assertions. The public live reader authenticates every
snapshot page and channel binding before selecting the worker's evidence and
role/stage catalogue. It checks every selected occurrence of each required read
shape; distinct valid repetitions are not replay. Other authenticated same-task
actors are counted separately and cannot replace a missing worker operation.
Counts and safe correlation identities remain in its result. Exercise its real CLI,
including paged disposable snapshots and contradictory/replayed controls:

```bash
python3 -B -m pytest -q tests/test_read_api_live_assert.py tests/test_read_api_actor_scope.py
```

No paid check is launched by these commands.

## Plan error recovery

Run this executable no-paid scenario in the checkout; every store it creates is
disposable, and it never starts an engine or replays an existing task:

```bash
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_plan_error_classification.py
```

It covers actual malformed/missing/unsupported routing policy through dispatch
and stdio, typed categories, conservative unknown-stage errors, controlled
precommit/commit/reply faults, atomic rollback and same-key exact reconciliation.
It also checks payload conflicts, changed-key rejection after acceptance and
concurrent identical submissions. Fault injection is test-only, never a live
qualification procedure. The fixture preserves exact-route plan structure without
retaining private objective bodies. It is not evidence of a historical failure's
inner cause or transaction disposition, nor of real-model recovery compliance.

## Development command verification

Focused default/provenance/gateway regressions include
`tests/test_development_commands.py` and `tests/test_access_mode.py`, alongside
permission, engine-floor, command-evidence, session recovery and format/split tests.
The test-only unjailed launch replacement covers real subprocess receipts and
cancellation, not real filesystem containment or Docker. Execute the marked real
bubblewrap cases where namespaces are supported; a nested-sandbox skip remains
unverified containment. Docker qualification uses only disposable project-local
inputs and uniquely named disposable images/containers/volumes, never production
workloads, daemon reconfiguration, privileged containers or host-root bind probes.
Daemon-side path visibility, cleanup and identity must be observed separately.
Paid CLI/desktop checks still require an explicit owner request naming the mode.

## Terminal resource regressions

`tests/test_terminal_resource_cleanup.py` uses disposable stores, real local fake
ACP subprocesses and the project advisory writer lock to check terminal/error/
disconnect teardown, another task's writer acquisition, repeated cleanup,
partial launch quarantine, unknown identity and surviving group descendants.
It does not qualify paid engine behavior or a host's rendered chat lifecycle.
Existing transport, recovery, completion and usage-settlement tests cover adjacent
EOF/SIGTERM, cancellation and transaction boundaries.

## No-paid default test suite

```
python3 -B scripts/run_tests_fast.py
```

- Install `pytest-xdist==3.8.0` and `pytest-timeout==2.4.0` into `.venv` before
  verification. The runner never installs dependencies; missing plugins fail.
- The runner uses `.venv/bin/python`, eight workers, `--dist=loadgroup`, at most
  32 explicit test files per batch, a 60-second per-test timeout (thread method),
  and GNU `timeout --kill-after=5 180` per batch. Every nonzero exit propagates.
  It disables bytecode and pytest cache writes and creates fresh account roots
  in temporary storage, not tracked paths. Read-only-sandbox execution still
  needs the local Node/Rust prerequisites used by selected tests.
- Before the first batch the runner rebuilds the service those tests launch with
  `cargo build --locked -p workspace-service`, into the git-ignored `target/` on the
  dev channel. A missing `cargo` or a failed build exits 2; an older binary is never
  reused. See [the default command](test-matrix.md#the-non-paid-default-command).
- No files are excluded. Any future slower-only files must have exact names and
  reasons in the runner's `SLOW_FILES` and the test matrix; invoke them with
  `python3 -B scripts/run_tests_fast.py --slow`. Selection completeness and actual
  failure/hang controls are in `tests/test_fast_suite.py`.
- For targeted serial debugging use the bounded command above; the legacy
  conftest auto-parallel policy is unchanged for direct pytest invocations.
  See [timings and limits](test-matrix.md#bounded-timing-observations-2026-11-05).
  After the connection-local Workspace hook repair, the measured bounded command
  `timeout 900 python3 -B scripts/run_tests_fast.py` completed all 400 files in
  **313.72 s**, exit **0**: **9708 passed, 143 skipped**, no failures. This is
  no-paid automated evidence, not live or full release qualification.
- `PYTHONDONTWRITEBYTECODE=1` is required; subprocesses do not inherit `-B`.
- `HOME` and `TMPDIR` must lie outside every git repository (ancestors
  included), or storage guards produce false failures.
- Run as a non-root user: `tests/test_artifact_revision.py`'s unreadable-entry
  check self-skips when the process can bypass directory permissions, so a
  root CI runner would silently never run it.
- Nothing in this suite can reach a paid engine (see
  `docs/project/test-matrix.md`): the paid probes refuse to run without
  both `--i-authorize-paid-call` and `--approval-ref`, `cortex_acp.py` tests
  use a fresh temporary `HOME`/`XDG_CONFIG_HOME`/`XDG_STATE_HOME` with no
  connections configured, and a dedicated test asserts zero engine launches
  during a full diagnostic run.
- Test totals vary with the checkout; retain the exact counts, skips and exit code from each run rather than treating a historical count as current. Remaining conditional guards are platform guards (POSIX FIFO, symlink support, `tmux` presence) that are not the sole coverage of any mandatory case.

## Running the suite inside a runtime sandbox

Lifecycle tests replace only verification launch preparation with a test-only
runner (`tests/sandbox_fakes.py`): literal commands still execute, and production
retains output, exit status, cancellation and revision checks. Git-context tests
likewise use a real fixed git command without a nested sandbox; receipt/recovery
tests opt into `unjailed_gateway_command` when containment is not their subject.
There is no production environment flag or unsandboxed fallback.

Real containment tests are marked `requires_bwrap`. A session-scoped probe tries
a user namespace once; a known namespace denial skips them with
`bwrap cannot create user namespace in this environment (nested sandbox)`.
Unexpected probe errors and failures after a successful probe still fail.
Use `python3 -B -m pytest -q -m requires_bwrap -rs` outside a nested sandbox to
exercise those cases; a nested run is not containment qualification. Do not
change host AppArmor or sysctl policy to make lifecycle tests pass.

## Release-grade run

For a release-grade run, verify in a throwaway clone rather than the working
checkout:

```
git clone --no-hardlinks
```

then overlay the working diff, stamp there, and run the no-paid suite above.
The stamp must reproduce byte-identically in the real checkout.

## Package stamping

```
python3 -B scripts/cortex_package.py stamp
```

Run after any package payload edit, before release-sensitive checks, to
regenerate the complete content hash.

## Profile generation

```
python3 -B scripts/generate_agent_profiles.py check
python3 -B scripts/generate_agent_profiles.py write
```

`check` is the source-only verification: it fails if the generated worker skills or
profiles differ from their sources, and it asserts the 23 profiles. `write`
regenerates the derived files. The script has no `--check` or `--dry-run` flag.
Preserve all 23 profiles (22 specialists and the consultant) and their shared
report protocol.

## Sequencing

Run package, profile-generation and test checks sequentially in the same
checkout. Use the smallest meaningful tests first. Before any Python checks that
spawn the local Paseo fake endpoint (including the default suite), prepare the
checkout-local plugin prerequisites in order:

```
npm --prefix integrations/paseo ci
npm --prefix integrations/paseo run typecheck
```

The endpoint imports `ws` and `@getpaseo/protocol/messages` from the locked local
dependencies; missing imports fail startup, not binding qualification. The Node
scripts also require a Node version supporting `--experimental-strip-types`.
These no-paid prerequisites do not install a production plugin or qualify a live
daemon. See [isolated startup](#fresh-process-isolated-targeted-launcher)
for interpreter-compatible dependencies and disposable HOME/TMPDIR requirements.

## Diagnostics (no model call)

```
python3 -B scripts/cortex_diagnose.py --format text
```

Reports engines, connection state, and storage/schema; never makes a model
call.

For final-default propagation and bound lifecycle metadata, run the complete
`tests/test_final_parameter_fidelity.py` and `tests/test_task_inspection.py`
with the targeted pytest command above. These use the real installer, ACP
selection and adapter with a fake downstream subprocess, plus executable
metadata-reader and refusal fixtures. They do not prove paid host behavior.
See [bound task inspection](../features/observability/index.md#bound-task-inspection)
for the exact read-only command, binding/pagination requirements and limits.

## Setup status (read-only)

```
python3 -B scripts/cortex_acp_setup.py status
python3 -B scripts/cortex_acp_setup.py add-connection --help
```

## Qualification evidence limits

A route assertion passes only with an exact task/current-owner binding and one
unambiguous provisioning, assignment, engine-session and native-confirmation
chain; missing, mismatched, superseded or replayed confirmation evidence fails.
A retained-output expectation establishes complete, uniquely bound execution
and output metadata, including success and non-truncation, but it does not read
or verify the retained body's bytes. Steering continuation remains pending until
the exact steering capture and same-attempt progress or terminal evidence appear;
an unchanged snapshot or a store-only observation does not prove a host
cancel-then-prompt transition. No-paid fixtures and tests are not live engine or
rendered Desktop evidence.

## Paid live verification on request

Default verification is no-paid. A paid live check runs only when the owner
explicitly requests it for the current work and names the mode, `cli` or
`desktop`; without a request, report each applicable live check as not run with
the blocker "paid live check not requested by the owner". Run a requested check
only through the isolated dev Paseo environment described in
[`docs/project/dev-environment.md`](dev-environment.md):
the permanent `~/.paseo-dev` daemon, separate `~/.config/Paseo-dev` Electron
profile, a disposable project, and the Cortex runtime launched by that daemon
from the explicit absolute checkout plugin path. Do not use production
`~/.paseo` or the production Cortex agent. Keep the existing production
Codex/Claude account and credential-isolation rules. Check native login status without printing credentials. Use an explicit owner-selected route supported by current settings and verified host/catalog capabilities; do not substitute or automatically escalate to the deep coordinator. Heavy models remain prohibited in routine live qualification.

The paid probes retain their safety guard and require both flags, with an
approval reference naming the owner's request:

```
scripts/cortex_codex_probe.py --i-authorize-paid-call --approval-ref owner-request-<date>-cli
scripts/cortex_claude_probe.py --i-authorize-paid-call --approval-ref owner-request-<date>-cli
```

The default automated suite remains no-paid. A `cli` check establishes runtime
and protocol behavior only; a probe or GUI observation supports only the exact
surface it exercises.

Before a requested `cli` run, the
[route preflight](dev-environment.md#route-preflight-before-a-paid-cli-run) must
exit 0 for each selected route. A refusal means some stage, usually the mandatory
reviewer, would run on another route, so the run does not start. Its no-paid
regressions are `tests/test_stage_route_preflight.py`. After the run, read the
receipts, check outcomes, route bases and question kinds with
[bound task inspection](../features/observability/index.md#bound-task-inspection);
its regressions are in `tests/test_task_inspection.py`. Assert the expected
outcome with `scripts/cortex_qualification_assert.py` (exit 0 only when every
expectation holds; see [assert the outcome](dev-environment.md#assert-the-outcome));
its regressions are in `tests/test_qualification_assert.py`. A case supervisor
observes, selects the running attempt, steers once and cleans up through
`scripts/cortex_qualification_observer.py`
([observe and steer within the case budget](dev-environment.md#observe-and-steer-within-the-case-budget));
its no-paid regressions are in `tests/test_qualification_observer.py`.

`scripts/cortex_acp_smoke.py` is the no-paid startup smoke harness
(schema-validated launch/protocol surface only, not GUI evidence).
Run both documented fixture forms from the checkout root after source/package checks:

```
python3 -B scripts/cortex_acp_smoke.py
python3 -B scripts/cortex_acp_smoke.py --fixture-connection
```

The second mode supplies only an isolated fake identity with explicit version-2 eligibility in temporary XDG settings; it verifies selector exposure, not migration of owner data or paid execution.

## Question and submission regressions

For changes to question continuation, recovery, or report submission, run the
focused runtime checks before package/profile checks:

```
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_question_recovery_continuation.py
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_steering_after_verification.py
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_delegation_mcp_wire.py tests/test_delegation_mcp_tools.py tests/test_delegation_evidence.py
```

Plan objective preflight and rejection boundaries use:

```
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_delegation_contracts.py tests/test_delegation_mcp_wire.py tests/test_advertised_tool_contracts.py tests/test_all_mcp_advertised_contracts.py
```

The supplemental historical `python3 -B scripts/check_docs.py` invocation failed
because that script does not exist; it was an invalid supplemental command,
not a failed product-doc validator and not a passing check. Use the existing
`tests/test_project_docs.py` and `tests/test_package.py` with the targeted command
above, plus source-backed link/command inspection for affected pages. The
catalog tests alone do not validate every documentation link or command.

The recovery test verifies exact option/custom answers, renewed bounded planning,
exactly-once answer consumption and continued rejection when no proposal arrives.
The submission tests cover required report forms, publication references,
argument validation and retained safe diagnostics. They cannot establish that a
live model serializes correctly or complies with the schema.

The earlier isolated Desktop question run is partial, not an overall pass: a
coordinator form rendered and its answer mapped, but the task then stopped with
`structured_proposal_missing`; batch and worker scenarios were not reached.
A later controller repair fixes a recovery-counter defect, but does not turn
that historical run into a pass. Repeat only under an explicitly requested,
isolated Desktop qualification and record actual rendered outcomes separately.
The actual Desktop owner-selected route was `codex-acp/gpt-6-luna`; do not
substitute another route. Package identity and the full executable scenario
must be handed to the Desktop QA owner after final stamping.

## Paseo plugin (`integrations/paseo/`)

Dependency security checks are separate from installation/test success. Run
`npm --prefix integrations/paseo audit --json` and retain its nonzero exit when
findings remain. The [dated dependency disposition](paseo-dependency-audit.md)
records the unpatched braces advisory and why a forced React Native upgrade is
not a verified fix. The client-bundle regression checks generated-artifact inputs,
not the installed host's dependency security.

**Before any testing:** Install dev dependencies and verify type-check:

```
npm --prefix integrations/paseo ci
npm --prefix integrations/paseo run typecheck
```

**Automated unit tests** (from `integrations/paseo/package.json`):

```
npm --prefix integrations/paseo run test:activation # server/plugin-activation.test.ts
npm --prefix integrations/paseo run test:steer      # steer, steered worker cards through the real installed ACP bridge, and scalar/batch question transport regressions
npm --prefix integrations/paseo run test:installed-questions -- /absolute/path/to/installed/Metro/index.js # installed question helper/component structure and state (no pixel rendering)
npm --prefix integrations/paseo run test:native-queue -- /absolute/path/to/installed/Metro/index.js # installed queue/edit/send/retry helpers and provider; no GUI
npm --prefix integrations/paseo run test:diff      # client/worker-panel-diff.test.ts
npm --prefix integrations/paseo run test:status    # client/worker-panel-status.test.ts
npm --prefix integrations/paseo run test:display   # display, assignment prompt and activity deduplication models
npm --prefix integrations/paseo run test:terminal-status # authoritative terminal outcome projection
npm --prefix integrations/paseo run test:command-stop # command list/stop RPC and client model
npm --prefix integrations/paseo run test:worker-card # card model, card contrast/Space activation, and one stable worker-card identity across turns/replay (cancel vs. real failure)
npm --prefix integrations/paseo run test:replay    # server/replay.test.ts
npm --prefix integrations/paseo run test:thinking  # per-model efforts; a stale thinking choice the model lacks is dropped through the real ACP bridge
npm --prefix integrations/paseo run test:coalesce  # client/coalesce.test.ts
npm --prefix integrations/paseo run test:worker-result-batch # client/worker-result-batch.test.ts
npm --prefix integrations/paseo run test:rpc-subscribe # client/rpc-subscribe.test.ts
npm --prefix integrations/paseo run test:python-runner # server/python-runner.test.ts
npm --prefix integrations/paseo run test:invoke-diagnostics # server/invoke-diagnostics.test.ts
npm --prefix integrations/paseo run test:worker-history # server/worker-history.test.ts
npm --prefix integrations/paseo run test:worker-history-concurrency # server/worker-history-concurrency.test.ts
npm --prefix integrations/paseo run test:worker-results # server/worker-results.test.ts
npm --prefix integrations/paseo run test:pipeline # server/pipeline.test.ts
npm --prefix integrations/paseo run test:error-boundary # client/plugin-error-boundary.test.ts
npm --prefix integrations/paseo run test:config         # server/config.test.ts
npm --prefix integrations/paseo run test:settings      # client/settings-model.test.ts + client/settings-panel.test.ts
npm --prefix integrations/paseo run test:client-bundle  # includes settings screen render-walk
```

For question transport or tab-label changes, run `test:installed-questions`
alongside `test:steer`, not just the mock provider tests. The installed helper
returns **header-keyed** answers; the tests include colliding topics and
all-member custom-answer submission. This is no-paid, read-only bundle
execution with component stand-ins, not a rendered Desktop qualification.
Both `test:installed-questions` and `test:native-queue` require an explicit
bundle path and do not discover an installation themselves. Run `test:native-queue`
for native queue/edit/send/retry or provider-delivery changes, `test:display` for
worker display/prompt/activity changes, `test:terminal-status` for terminal
outcome reconciliation, and `test:command-stop` for command-control changes.
The question harness binds stand-ins by inspected module export shape, not
import position or a fixed dependency count; missing or ambiguous shapes fail.
These installed-bundle checks do not launch or modify Paseo. On this Linux installation, discover the current file read-only with:

```
find /opt/Paseo/resources/app-dist/_expo/static/js/web -maxdepth 1 -name 'index-*.js' -print
npm --prefix integrations/paseo run test:installed-questions -- /absolute/path/from/discovery/index-HASH.js
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_question_batches.py tests/test_acp_steering_and_questions.py tests/test_question_recovery_continuation.py tests/test_worker_wait_continuation.py
```

Select the observed installed bundle explicitly; report an absent/ambiguous
bundle or incompatible helper as a blocker, not a skipped pass. The Python
regressions bind reverse-sorting random ids and tied timestamps to the displayed
batch order, blank-line separators, ACP `/cortex-answer` selection, persisted
answers and remaining-question renumbering after restart.

A local sibling import must use its real `.ts` extension (e.g. `./pipeline.ts`),
never a `.js` specifier standing in for it: `tsc`/esbuild remap `.js` to the
sibling `.ts` file, but `node --experimental-strip-types` does not, and a
`.js` specifier that resolves to nothing breaks every test that transitively
imports it.

**GUI testing** (verifies rendered user-facing behavior):

Paseo 0.10.0 desktop evidence (2026-09-29) is inconsistent about availability, so the screen must be treated as unverified rather than reliably reachable. One earlier report described the implemented settings screen and component tests; two isolated live desktop checks found the plugin enabled/running at this worktree but no Cortex entry in Settings or matching command-palette result. The later recheck also inspected the rendered screenshots directly. These negative findings apply to the tested host/session, not every host version or feature flag. Because those desktop runs could not reach the screen, they did not exercise GUI settings reads/updates, saving, revision-conflict/error display, secret redaction, or the OpenCode section; component/render tests are not substitutes. The limitations of those desktop runs are stated here. The settings UI code now uses typed controls rather than JSON text editors; component/unit tests and a client-bundle render-walk establish neither that the screen is reachable in Paseo Desktop nor rendered GUI behavior. Use the existing documented CLI fallback for supported setup operations, including OpenCode onboarding, where the UI is unavailable. Use the isolated dev home/profile and a disposable project as described in [the dev Paseo environment guide](dev-environment.md).

Always pass the plugin directory explicitly as a full absolute path. The plugin
is edited in isolated git worktrees and the path changes between sessions.
From the repository root:

```
npm --prefix integrations/paseo ci
npm --prefix integrations/paseo run typecheck
python3 -B scripts/cortex_paseo_dev.py -- start --timeout 60 --json
python3 -B scripts/cortex_paseo_dev.py -- plugin install "$PWD/integrations/paseo" --json
python3 -B scripts/cortex_paseo_dev.py gui
```

In Paseo Desktop, inspect the rendered behavior under test. If the Cortex Settings screen is available, check workspace selection, supported setting sections, validation/conflict behavior, redacted values, and explicit OpenCode onboarding; if it is absent, record that outcome and do not report those interactions as tested. The 2026-09-29 Paseo 0.10.0 checks found the plugin screen and command absent, with the later check also inspecting screenshot pixels. GUI form behavior remains unverified because those runs could not reach the screen. For Xvfb use `python3 -B scripts/cortex_paseo_dev.py gui --xvfb`.
After testing, stop only the isolated dev daemon:

```
python3 -B scripts/cortex_paseo_dev.py -- daemon stop --timeout 15 --json
```

CLI access is secondary and supports diagnostics such as daemon health, plugin
registration and logs. Keep the plugin path explicit when installing it:

```
python3 -B scripts/cortex_paseo_dev.py -- status --json
python3 -B scripts/cortex_paseo_dev.py -- plugin ls --json
python3 -B scripts/cortex_paseo_dev.py -- logs <agent-id> --json
```

For retained-history checks, omit `--tail` and retain the exit status, output
byte count, structural event count, safe first/last event identities and any
explicit error. The 0.9.2 observation that `--tail`, `--filter` and `--since` existed without pagination must not be assumed to apply to 0.10.0. Check the installed `logs --help`; if completeness cannot be independently established, mark that check unverified. Do not store message bodies or raw logs
in repository documents or reports.

## Installed desktop real-input gate

Before handing a desktop archive to an owner, run this check against that exact
archive, sequentially, never on the owner's session:

```sh
python3 -B packaging/container/check_installed.py --archive /absolute/path/to/candidate.deb --prior-archive /absolute/path/to/previous-owner.deb --evidence .workspace-build/installed-new
```

The build-integrated equivalent is
`python3 -B scripts/build_workspace_desktop.py --check-installed --prior-archive /absolute/path/to/previous-owner.deb`.
Docker must be available without host sudo. The command creates privileged
disposable PID-1 systemd containers, with a lingering desktop user and private
buses, HOME, Xvfb/Openbox and a real headless Mutter compositor. No host home,
display, bus, cgroup filesystem or daemon socket is mounted. Container privilege
is not hostile-code isolation. Image preparation downloads runtime libraries;
the installed runs have no network and no real engine accounts.

Each display is checked after plain apt clean install and plain apt upgrade from
the supplied prior archive (no forced downgrade or same-version reinstall).
The installed `.desktop` entry launches the actual packaged application.
Accessibility supplies text and geometry only: clicks use XTEST or Mutter's
observed RemoteDesktop pointer API, not DOM events, preload probes or accessible
actions. The saved-plan-with-unloaded-tasks regression must leave Inbox, Archive,
Settings and the sidebar usable. A separate fake-engine journey checks project
registration/activation, description-only creation, real keyboard typing and OS
clipboard text paste, an external GTK file drag through hit-testing, start, a
visible two-question batch, opening a synthetic worker, Stop/Resume (each
must show the exact opposite task button; matching text elsewhere never counts),
result/review, every current Settings navigation page and sidebar toggles.
The window must close through its own close button, its process must exit and
the installed user service must remain active and answer a read-only status
request. Installed regular-file SHA-256 values and symlink targets are compared
directly with the submitted archive, without relying on optional Debian
\`md5sums\` metadata. The image explicitly includes this package’s documentation;
Ubuntu container images otherwise omit its bundled font licence by default.
Test-only engine files and a user
unit override are confined to the container; shipped payload files are not patched.

Keep the new evidence directory: complete per-scenario output, fixture-only
application/compositor logs, failure accessibility tree and an archive-SHA-bound
`result.json`. Only a complete four-scenario `accepted: true` permits handoff. Do not combine
successful pieces of different runs, retry away an unexplained failure, or treat
the presence of this harness as package qualification.
The Linux task window leaves WebKit compositing available; it no longer forces
`WEBKIT_DISABLE_COMPOSITING_MODE`. After the unsaved-file guard and outstanding
service-request acknowledgements, the native close route calls WebKit’s public
[`terminate_web_process`](https://webkitgtk.org/reference/webkit2gtk/stable/method.WebView.terminate_web_process.html)
for this view and waits for `TerminatedByApi` before destroying the window. This
bypasses the child’s EGL/Skia exit-handler race
([WebKit 305909](https://bugs.webkit.org/show_bug.cgi?id=305909)), not native error detection.
It does not qualify unexpected process recycling or browser-storage durability.
Raw frontend window destruction is unavailable; failed closure keeps the UI open
with a reason. Add `--close-cycles 3` (1–10 supported) to the installed command
for repeated close/reopen checks in every clean/upgrade phase; evidence records
the cycle count. Any allocator/crash output still blocks acceptance.

Inside a disposable GTK display/container, never the owner’s session, run
`cargo test --locked --release -p cortex-workspace-desktop --lib native_renderer_keeps_compositing_available -- --ignored --test-threads=1`
and `cargo run --locked --release -p cortex-workspace-desktop --example rendering_close`.
The view-lifetime regression runs on the process main thread, not libtest’s worker
thread. Default acceleration policy is eligibility, not proof of hardware rendering.
Xvfb/headless software-display runs cannot qualify real-GPU scroll speed or
physical input-to-present latency.

Any failed deadline, error alert, missing response, unstable sampled control or
native allocator/crash diagnostic blocks acceptance, even if the interaction driver
returns zero. A vanished window alone does not prove a clean native process exit. Cleanup removes each owned container even after failure.
No screenshots are taken. Accessibility stability sampling does not establish
absence of paint-only flicker, aesthetic quality, physical GPU/input behavior or
unexercised controls. The worker is a test-only store fixture, not live delegation;
Settings coverage is navigation/rendering, not every possible configuration edit.
The upgrade checks plain apt replacement of a running service, not preservation
of an in-flight model workload. Fake engines do not qualify native accounts or
paid calls.
The current gate supports the dev Debian channel; release-channel handoff is not
qualified. Existing preload/DOM-driven audits remain supplemental, never a
substitute for this installed real-input gate.

## Cortex Workspace local slice checks (no-paid, plus container)

Focused task-start regressions use fake dispatch and disposable stores only:
distinct tasks in one project start independently, an already-started task stays
busy, and crashed starts stay unknown without replay or blocking other tasks.
They cover ordinary RECOVERING/WAITING_USER tasks, running Workspace executions,
ordinary ACP bootstrap beside pending/unknown/running Workspace starts with
source-event replay, independent ACP sessions/cancellation, the task-local busy message and unchanged
sprint queue pauses. Manual `tasks.start` has no dependency-order enforcement.
These are source/protocol checks, not rendered desktop qualification. Install the
checkout-local test dependencies first (including `pytest-timeout` in `.venv` and
`npm --prefix packages/workspace-ui ci` plus `npm --prefix packages/workspace-domain ci`).
The service integration test requires `cargo build --locked -p workspace-service`
before the focused command; it launches only a disposable, fake-backed service.

```
timeout --kill-after=5 300 env PYTHONDONTWRITEBYTECODE=1 .venv/bin/python -B -m pytest -q -n 0 --timeout=60 --timeout-method=thread tests/test_workspace_start.py tests/test_workspace_busy.py tests/test_workspace_api.py tests/test_workspace_scheduler.py tests/test_workspace_service_integration.py tests/test_package.py
```

The Cortex Workspace local slice is documented in [Cortex Workspace](../cortex-workspace/index.md). Its checks are no-paid: they use fake engines and the scripted protocol peer, and they do not use a production Paseo installation or a production Cortex store. Run them sequentially in the same checkout, after `python3 -B scripts/cortex_package.py stamp` when a package payload changed.

```
python3 -B scripts/check_workspace_pins.py
python3 -B scripts/generate_workspace_contract.py check
node packages/workspace-contract/test.mjs
timeout 180 .venv/bin/python -B -m pytest -q --timeout=60 tests/test_workspace_packaging.py tests/test_workspace_channels.py tests/test_workspace_container_build.py tests/test_workspace_service_integration.py
cargo test --locked
npm --prefix packages/workspace-domain run typecheck
npm --prefix packages/workspace-domain run test
npm --prefix packages/workspace-ui run typecheck
npm --prefix packages/workspace-ui run test
npm --prefix packages/workspace-ui run build
npm --prefix apps/cortex-workspace-desktop run typecheck
npm --prefix apps/cortex-workspace-desktop run test
npm --prefix prototypes/cortex-workspace/desktop run test
python3 -B scripts/cortex_package.py validate
python3 -B scripts/generate_agent_profiles.py check
```

- `cargo test --locked` covers the default member, the headless protocol crate. Focused service checks use `cargo test --locked -p workspace-service`; headless native bridge checks use `cargo test --locked -p cortex-workspace-desktop --no-default-features`. Full native-feature tests require the Docker builder, not host development packages.
- `python3 -B scripts/build_workspace_desktop.py --test-workspace` builds the `.deb` for the dev channel in the Docker builder image and runs the locked Cargo workspace tests there. It requires Docker usable without sudo and creates artifacts under `.workspace-build/`. `--release` requires a GitHub Actions manual workflow on clean `main`; `--ci-dev` requires the same on clean `dev`. Local builds default to dev. Explicit owner-run `./scripts/build-prod-deb.sh --check-install` selects `--local-release`, accepts `local-owner-test` at the Rust and packaging guards, and produces a non-official production-channel archive under `.workspace-build/local-prod/build-*/`. Its version ends in `.local` and cannot pass the GitHub publisher's version check. The install smoke is not release-channel installed desktop acceptance; that handoff remains unqualified.
- `python3 -B scripts/build_workspace_desktop.py --check-install` also runs `packaging/container/check_install.py` in a clean Ubuntu 26.04 container without build tools or a checkout. It installs runtime dependencies through apt, runs the bundled unit ExecStart as a normal user, checks activation and data-preserving removal, and rehearses upgrading the former archive shape. It attempts a user manager and app-driven start; an unavailable manager and denied namespaces are explicitly unverified, never a pass. Synthetic adapter discovery stubs installation/model refresh and is not live engine qualification.
- The native fake-engine journey and visual audit run in a disposable no-network container through `python3 -B packaging/container/check_rendered.py --container --artifacts <build>/artifacts`, after a Docker build. The default command runs the full common journey on X11 with Openbox plus a native Wayland frame/control stage. Add `--journey-wayland` for an independent full common journey on headless GNOME Mutter. That mode verifies the installed remote-keyboard API by introspection and creates the virtual keyboard before launching GTK; the same journey assertions run with native input, focus and resize. Both modes read computed styles and rectangles through a preload compiled in the container, so the product needs no inspector. Any regression exits 1. Each run records how the desktop process, its WebKit web process and the compositor ended: exit code or signal, stderr reduced to marker counts, and any core by name and size. A real death of any of them is a hard failure. The only tolerated case is an accessibility node the page replaced while every process is alive and the page answers; the action is repeated and logged as `accessibility_node_stale`. `python3 -B packaging/container/journey_series.py --artifacts <build>/artifacts --logs <directory> --runs 5` runs the Wayland journey one run at a time and prints each result on one line; add `--display x11` for the X11 journey. A series run is complete only with exit 0, a valid binary SHA-256, at least one passing screen audit, and exactly one consistent native-evidence/visual-summary/cleanup sequence. Failed or unfinished steps, missing observations, duplicate terminal records, contradictory native evidence and truncated JSON records block completion; `evidence_errors` explains rejected evidence. Add `--expect-sha256 <digest>` to bind the run to an expected desktop binary. Outcomes are text only; the harness captures no screenshot. It does not replace the owner's review of an installed window. Its workers are rows the harness writes into the disposable project's store, because the fake engine delegates to none, so it exercises reading and showing them, not real delegation.
- `npm --prefix prototypes/cortex-workspace/desktop test` also presses every enabled control of the richest fixture, with exactly the capabilities a real machine offers, and fails on one that changes nothing, sends nothing and goes nowhere. It leaves out menu items, what is already chosen, drag grips and copy buttons, which a test document cannot observe.
- Rendered layout geometry: `node packages/workspace-ui/test/layout-fit.mjs <label>` (from the checkout, with the `packages/workspace-ui` dependencies installed, Docker usable without sudo, and the `cortex-workspace-rendered:ubuntu26.04` image built) builds the workspace bundle with Vite, serves it with fake data, and runs `packages/workspace-ui/test/layout-fit.py` in that container (`--network host`) under a private Xvfb with WebKitGTK. Sizes come from the app: 1280×800 and 1920×1080 for the owner's screens, 960×540, 1024×768, 1280×800 and 1872×1100 for the screens held to the full rules, and the window minimum read from `min_inner_size` in `apps/cortex-workspace-desktop/src-tauri/src/desktop.rs` (960×540), with the sidebar dragged to `SIDEBAR_MAX` in `packages/workspace-ui/src/prefs.ts`. A run exits nonzero when a listed screen overflows a bar, wraps a row or label, clips or squeezes text, narrows a pane below its minimum, scrolls sideways, or moves a control when it becomes enabled or disabled while its reason slot does not change. It also fails if an injected fault goes unnoticed. Evidence goes to the git-ignored `.design-evidence/layout-rebuild/<label>`. The gate proves rendered geometry in WebKitGTK on fake data at those widths. It does not prove pointer or keyboard input (see the installed real-input gate), Wayland, colour or visual quality, or any screen outside its list.
- The prototype suite (`npm --prefix prototypes/cortex-workspace/desktop run test`) does not pass at the last commit (`8bb66c54`). On 2026-11-10 a clean export of that commit and the working tree both reported 67 failed and 475 passed of 542 tests, with 7 of 19 files failing. Of the 67, 65 also fail at the commit. The other two (the Inbox machine test and the machine settings shortcuts test in `phase4.test.tsx`) come from the uncommitted tabs work and are not yet fixed. Two layout-era expectations in `review.test.tsx` were updated to the approved review layout, and the mockup identity tests now assert token identity only. The command exits 1 until these failures are resolved, so it is not a passing check.
- With explicit authorization for a privileged disposable container only, `python3 -B packaging/container/check_systemd.py --artifacts <build>/artifacts --prior-archive <earlier-dev.deb>` exercises real PID-1 systemd, a lingering disposable user, default-target login startup, desktop-driven service start, active-unit restart on reinstall, removal stop and upgrade from the exact prior archive. With `--prior-archive`, a project registered and activated under the prior archive must report ready again after the plain apt upgrade, after a unit restart and after a user-manager restart (login), with only status reads and no new activation request; a refusal reason fails the check. It mounts no host home, bus, credentials, cgroups or Docker socket. It is opt-in, never part of ordinary no-paid pytest, and is not hostile-code isolation. The 2026-11-06 rerun passed against the fresh ordered-version dev archive and the earlier `1.0.6+sha256.f3eee9da173725e1` archive. The harness uses ordinary `apt install` for replacement and fails if it reports a downgrade or does not report one upgrade; only the separate same-version reinstall uses `--reinstall`.
- `python3 -B packaging/container/check_bundled_engines.py --archive <package.deb>` preinstalls only system runtime dependencies in its image, then installs the archive with `--network none`. It checks real bundled adapters/Node, honest absent versus synthetic login markers, ACP initialize, native versions and unchanged root-owned bundle hashes; it sends no model prompt and does not authenticate a real account.
- Namespace qualification must name its execution context. On 2026-11-06 the nested worker probe failed under enforcing `bwrap//&unpriv_bwrap`, while an authorized host-direct probe passed with Ubuntu's restriction switch still enabled. The host-direct focused `tests/test_workspace_discovery.py tests/test_runtime_verification_sandbox.py` run passed all ten tests with real namespaces. No policy change or package AppArmor exception was made.
- The earlier 2026-11-06 full journey failed at `field_focus_failed:Message backend-dev` with a Request changes dialog still open. After repair, a fresh dev Docker build with workspace tests and install smoke passed; its complete X11 and native Wayland common journeys passed with 118/114 screen audits and zero failures. Both reached Result v2, closed the review dialog, exercised worker guidance/history and command/worker stop, and completed offline/reopen screens. The independent terminal and attachment picker/paste/drop/queue-edit stages also passed. Real-systemd plain apt replacement, offline adapter/login-state initialization, and the ten focused MCP/sandbox fixtures against the extracted archive passed. This is not real model, physical-desktop or all-planned-functionality qualification. See [platform limits](../cortex-workspace/platforms.md).
- Native journey result, 2026-11-10: a fresh dev build ran the complete common journey on X11 and on Wayland. Every screen audit passed in both, including the task session result screen and its result tabs, the offline-at-launch home screen and the Wayland stage. Both runs still exit 1 for one cause that already exists at the last commit: each window close calls `terminate_web_process` (`apps/cortex-workspace-desktop/src-tauri/src/rendering.rs`) and logs one `terminated_by_api` (8 on X11, 7 on Wayland, one per launch). `packaging/container/check_rendered.py` counts any recorded web-process termination as a `web_process_terminated` failure, so the run fails although no process crashed, ran out of memory or left a core. The harness is not loosened to pass this. An owner decision is needed: for example, accept exactly one `terminated_by_api` per requested window close while keeping crashes and memory terminations fatal, or stop terminating the web process on close.
- Not run or not verified: the different-OS-user peer check of the service, a physical desktop/GPU or microphone baseline, real account/model workloads, the GitHub release workflow (manual runs only, production on `main` and dev prereleases on `dev`), and a direct host native build, which is not a supported route. Record each concrete blocker; do not install development packages on the host to run it.

## What automated evidence cannot close

The no-paid automated suite cannot reach a paid engine and does not establish rendered GUI behavior. The settings screen's client-bundle render-walk uses compiled UI plus host shims; it is not a visual Paseo Desktop result. The UI test coverage also includes `client/settings-model.test.ts` (run with `node --experimental-strip-types integrations/paseo/client/settings-model.test.ts`) and the config RPC end-to-end test. Use isolated dev Paseo for applicable live checks. A rendered desktop check must inspect the actual Settings screen and relevant behaviors (workspace selection, read/update, conflicts/redaction, and OpenCode opt-in) in the Electron app; CLI, logs, simulated render-walk, or CDP alone are not GUI evidence. Each probe or GUI observation supports only the exact surface exercised.

## Settings-only routing and profile deletion checks

For routing/settings changes, run applicable focused tests including tests/test_no_hardcoded_model_names.py, tests/test_model_settings_template.py, tests/test_settings_only_routing.py, tests/test_model_settings_integration.py, and the complete unfiltered routing/profile integration tests. For Paseo settings changes run documented typecheck and complete config/settings scripts, not a filtered pass. Automated tests do not prove production migration or installed GUI reachability. Production migration is a separately authorized config-only operation: inspect current revision, preview candidate, review preservation/diff and digests, obtain consent, apply against that exact revision, then verify resulting settings and backup/rollback evidence. Never mutate production during package verification. Paid qualification is not requested by default and, if requested, requires a named mode and explicit owner-selected route; do not silently select a substitute.


Quota settings changes additionally run the complete unfiltered `tests/test_quota_settings.py`, `tests/test_usage_accounting.py`, `tests/test_route_availability.py` and existing quota/routing/configuration/profile modules. They cover exact arbitrary identities/labels, conservative missing/unmapped mappings, ambiguity refusal before IO, project precedence, shuffled catalogs, menu/selection agreement, expiry, explicit candidate migration and snapshot round-trip. Keep existing authentication/CLI zero-cost/TTL/negative-cache tests; no global owner-template injection, skips or deselection. Run the documented plugin `ci`, `typecheck`, `test:config`, `test:settings` and `test:client-bundle` sequentially for typed quota controls. Package payload stamping and final whole-suite verification remain separate downstream steps; these source/component checks establish neither live quota evidence nor rendered host reachability.

## Fresh-process isolated targeted launcher

Use `scripts/cortex_test_isolated.py` when targeted tests need final HOME/XDG/TMPDIR
at Python startup. Obtain explicit permission first for NEW scratch outside every
Git repository and for reading the existing interpreter-compatible dependency
path; no installation or credential copying is needed. Create private owned
mode-0700 scratch and `home`, `config`, `state`, `cache`, `tmp` child directories
before invoking the launcher. Set `DEPENDENCIES` to the canonical existing
`site-packages` for the active Python major/minor (for example the checkout
virtualenv's matching directory). Verify interpreter and pytest locations before
changing HOME; do not borrow another Python version's packages or rely on
in-process conftest imports in a child. Both ordinary and isolated startup forms
are covered by `tests/test_cortex_test_isolated.py`.

```bash
python3 -B scripts/cortex_test_isolated.py --environment-root "$SCRATCH" \
  --dependency-path "$DEPENDENCIES" tests/test_package.py
```

The launcher rejects root, aliased/public/foreign/Git scratch, incompatible or
noncanonical dependency paths and anything other than explicit existing
`tests/test_*.py` modules. It removes inherited PYTHONHOME and starts import
preflight in a NEW Python process, then NEW `PYTHONDONTWRITEBYTECODE=1 python3 -B
-m pytest -q MODULE...` under identical final HOME/XDG_CONFIG_HOME/XDG_STATE_HOME/
XDG_CACHE_HOME/TMPDIR and explicit dependency-plus-checkout-src/scripts PYTHONPATH.
PYTEST_ADDOPTS is empty. Do not change HOME after interpreter startup or silently
fall back to ambient dependencies. Child failure exits propagate unchanged.

Binding regression commands use the same launcher with explicit
`tests/test_qualification_binding.py`, `tests/test_task_inspection.py` and
`tests/test_cortex_test_isolated.py`. The fake-wire matrix creates NEW fixtures
and supervises owned children; it is not a live daemon/paid result. The binding
regressions discriminate busy locks, WAL, legacy/unsupported/private storage and
missing ownership without accepting a binding, and verify allowlisted diagnostics
against malformed, duplicated, escaped, truncated and terminated child output.
They also exercise missing owned endpoint metadata, a stopped owned loopback
fixture, wrapper spawn/import-like process failures, the real TypeScript/Python
child boundary (including overflow and malicious diagnostics), unknown exception
code redaction, and zero-exit incomplete or mismatched binding envelopes.
Run `tests/test_cortex_paseo_dev.py` with the same launcher for neighboring helper
behavior. These checks cannot establish the cause of an earlier generic live refusal. Also use
`npm --prefix integrations/paseo run test:qualification-binding` for the TS contract.
See [binding provenance and limits](dev-environment.md#read-only-dev-binding).
After all edits, sequentially stamp, validate, check profiles, run isolated
`tests/test_package.py`, then validate again. Runtime independent review and
current-artifact final verification remain separate from these worker checks.


## Executable check declarations and Review

Use fake engines and disposable stores for this slice. Run focused commands sequentially:

```bash
timeout 120 .venv/bin/python -B -m pytest -q --timeout=30 -n 4 tests/test_check_preflight.py tests/test_check_blockers.py tests/test_codex_tool_cheat_sheet.py tests/test_deferred_check_closure.py
timeout 120 .venv/bin/python -B -m pytest -q --timeout=30 -n 4 tests/test_workspace_check_execution.py tests/test_workspace_review_recovery.py
npm --prefix packages/workspace-ui run typecheck
npm --prefix packages/workspace-ui test -- test/owner-review-flow.test.tsx test/real-port.test.ts
```

The service journeys need a current debug `workspace-service` binary. Availability probes use the same verification launch seam, not a host-side PATH guess; a probe is never a test result. Coverage includes missing programs, literal wrapper arguments, project-private tools, legacy manual items, policy exclusions, exact reruns, output, timeout, stale source and incomplete evidence. These fixture checks do not establish installed-desktop rendering, real screen-reader behaviour or paid engine qualification.

## Orchestration timing and developer wire-trace checks

For changes to orchestration timing or scheduler diagnostics, run the focused
measurement tests and inspect a fixture's read-only behavior:

```bash
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_orchestration_measurement.py
python3 -B scripts/cortex_timeline.py --project "$PROJECT" --task "$TASK" --json
```

The timeline command requires a checkpointed supported project store and an
exact task ID. It is observational only; missing/duplicate milestones are not
filled in, overlapping phases are not summed, and it refuses busy, uncheckpointed,
legacy, unsupported or over-limit inputs. Use an authorized disposable project
and do not treat the timeline as a live qualification.

For the observability signals and aggregates, run each focused file as its own
command, not the full suite:

```bash
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_gateway_observability_signals.py
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_external_proxy_signals.py
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_session_timing_signals.py
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_observability_stage2_metrics.py
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_cortex_timeline_aggregates.py
```

`tests/test_external_proxy_signals.py` has two groups. Three tests marked
`requires_bwrap` drive a real gateway subprocess: `test_same_arguments_different_meta_same_arguments_sha256`,
`test_denied_tool_records_one_refusal_row_without_arguments` and
`test_denied_method_records_one_transport_error_row`. Where bwrap cannot create a user
namespace, for example inside a nested sandbox, they skip. A skip is not evidence that
the assertions passed; rerun the file on a host that can run bwrap. Three further tests run
without bwrap: `test_unjailed_arguments_sha256_ignores_meta_and_follows_arguments`,
`test_unjailed_denied_tool_appends_one_refusal_row_without_arguments` and
`test_unjailed_transport_failure_appends_one_error_row_without_arguments`. They run the real
`external_proxy.main()` in-process against a fake MCP child, replacing only the final OS
launch through the `unjailed_gateway_command` fixture. They do not assert containment;
the bwrap tests remain the containment evidence. The multi-task aggregate command
(`python3 -B scripts/cortex_timeline.py --project "$PROJECT" --aggregate --json`) is
owner-run on a checkpointed store; inside a worker sandbox it refuses with
`unsafe_storage`.

For early review, digest caching and speculative verification, use the focused
tests covering the changed behavior, including:

```bash
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_early_review.py tests/test_artifact_revision.py tests/test_speculative_verification.py
```

For dev-only wire tracing, run the trace/redaction and wrapper gate tests:

```bash
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_dev_wire_trace.py
```

A requested trace session is separate from automated checks and must follow
[dev Paseo isolation policy](dev-environment.md). Do not use
production Paseo. Start tracing only through
`python3 -B scripts/cortex_paseo_dev.py --wire-trace -- daemon start ...`;
inspect existing traces with
`python3 -B scripts/cortex_trace_view.py --task "$TASK" --pretty`.
Trace files are private but may contain sensitive content; stop the writer before
exact-history analysis. Traces are diagnostic records only, may contain explicit
gaps, and never verify a command or prove a task outcome. Do not run a paid live
check unless the owner explicitly requested its mode for this work.

After these documentation edits, regenerate the package hash and run the focused
package test sequentially:

```bash
python3 -B scripts/cortex_package.py stamp
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_package.py
```
