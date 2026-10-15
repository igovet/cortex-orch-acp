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

A worker's failed check is closed only by an exact later passing rerun of the same argv and cwd; that exact rerun may cross between `command_run` and `outside_command` in either direction, preserving both receipts and their operation/profile provenance, while a same-operation profile change still never matches. A supported syntax, regex, or statically proven literal-expectation error in the worker's own bounded, self-contained standalone Python probe may instead be retracted by a later verified passing correction; the failure stays as `retracted` history and is never a pass. Runtime verification failures and real project test/lint/build failures are never retracted (see [check history](../features/orchestration/index.md#check-history-and-current-outcome)). Tests for these rules: `tests/test_check_closure_fixes.py`. The status-capture recognizer, explanatory `open_failed_check` corrections, `omitted_failed_check` retention of presented failures and the 64 KiB inline command-output bound are covered by `tests/test_check_submission_loop.py` (with `tests/test_check_history.py` and `tests/test_delegation_evidence.py`).
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
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_<module>.py
```

Changes to a shared contract include its cross-cutting tests: advertised tool
schemas (`tests/test_advertised_tool_contracts.py`,
`tests/test_all_mcp_advertised_contracts.py`), profile sources
(`tests/test_portable_roles.py`, `tests/test_instruction_sources_portable.py`
and the profile-generation check below) and package payload
(`tests/test_package.py` after stamping). Workers do not run the whole suite:
it belongs in `plan.verification`, which the runtime runs once after review,
with no time limit.

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
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q
```

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
npm --prefix integrations/paseo run test:diff      # client/worker-panel-diff.test.ts
npm --prefix integrations/paseo run test:status    # client/worker-panel-status.test.ts
npm --prefix integrations/paseo run test:replay    # server/replay.test.ts
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
The script requires an explicit bundle path and does not discover an installation
itself. On this Linux installation, discover the current file read-only with:

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
