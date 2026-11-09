# Automated test matrix and CI commands

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed.


This page documents the no-paid automated suite and its limits. Paid live checks run only on an explicit owner request naming `cli` or `desktop`, through the isolated dev Paseo environment documented separately.

Current candidate coverage includes settings-only routing, exact model-policy validation and profile management alongside deterministic DAG scheduling, authenticated
file/command/external MCP gateways, mandatory review, exact revision verification,
steering, bounded recovery and unfiltered transcript replay. Historical paid qualification used the then-configured routes in an isolated test project; it is not evidence for current settings or a production route. See the retained observations.
The standalone paid probe scripts were not used in this campaign. See [verification status](verification.md).
The broker suite forces a random token whose first byte is `-`, then checks that the stdio relay still connects and advertises tools. It also checks an MCP child that exits before `tools/list` and prompt-level error classification. These tests cover the runtime broker behavior exercised by the fixtures.

Focused no-paid routing checks are `tests/test_routing_profiles.py`
(revisioned user profile operations, write-through, divergence, shadowing and
storage refusals), `tests/test_model_policy.py` (purpose-selection validation),
`tests/test_auto_route.py` (eligible preferences and affinity source),
`tests/test_configuration_management.py` (management API/catalog contract),
and `tests/test_no_hardcoded_model_names.py` (current template-derived prompt/schema guards). Also run `tests/test_model_settings_template.py`, `tests/test_settings_only_routing.py` and `tests/test_model_settings_integration.py` for version-2 installation, routing and migration behavior. These do not establish rendered settings UI behavior, production migration or a live paid engine result.

Positive mode/permission/session/fallback fixtures must write the minimum exact version-2 owner policy into disposable project or HOME/XDG roots. `tests/model_policy_fixtures.py` supplies opt-in helpers; no global autouse production template or unknown-model enablement is permitted. Keep missing/malformed/denied policy cases separate. `tests/test_delegation_mcp_tools.py`, `tests/test_stdio.py` and `tests/test_delegation_worker_mode_floor.py` cover actionable route-menu diagnostics and refusal before capacity/engine launch.

Focused no-paid delegation/capability checks:

```
python3 -B -m pytest -q tests/test_worker_capability_discovery.py tests/test_host_mcp_forwarding.py tests/test_truthful_tool_advertising.py tests/test_unified_system_prompt.py tests/test_prompt_drift.py tests/test_runtime_task_machine.py::test_project_overview_reads_and_answers_once_without_workers
```

They cover configured grants versus session attachments, arbitrary multi-server
names, role/engine filtering, bounded sanitized summaries, missing inventory,
composed prompts, native discovery with no external MCP, unknown source
locations, Paseo UUID versus local reports, real outside-cwd validation, command
refusal, scripted specialist handoff and direct informational answers. Scripted reasoning does not establish real-model
delegation choices or rendered Desktop behavior.

Additional focused host-environment and outcome regressions:

```
python3 -B -m pytest -q -rs tests/test_external_host_read_route.py tests/test_worker_outcome_projection.py tests/test_worker_capability_discovery.py tests/test_paseo_worker_history_boundary.py
npm --prefix integrations/paseo run test:steer
npm --prefix integrations/paseo run test:worker-card
```

These run real consent-backed host subprocess reads of temporary `/var/tmp`
fixtures (allowed, denied, missing), plus actual runtime output through the
installed ACP bridge, compact provider and client status projection, live and
replayed. Outcome cases separate genuine user cancellation from failed work
before cleanup, replan/Stop after a committed failure, stale same-status host
updates, and old Cancelled rows without verdict metadata. They do not prove
model tool selection, GUI rendering or containment.
The separate namespace visibility cases require working user namespaces and
remain explicit skips when unavailable; fixtures are not relocated to pass.

Focused no-paid effort preservation checks:

```
python3 -B -m pytest -q tests/test_explicit_effort_handoff.py tests/test_auto_route_delegation.py tests/test_stage_route_preflight.py tests/test_routing_quota_fixes.py tests/test_engine_auth_recovery.py tests/test_delegation_worker_mode_floor.py
```

These cover source-bound exact model+effort, automatic defaults, owner pins,
policy/support conflicts before worker prompts, real fake-engine wire
confirmation, plan/retry/fix propagation and exact recovery identity. They do not
establish paid-engine or rendered Desktop behavior.

## Local plugin prerequisites

Before the default suite or `tests/test_qualification_binding.py` and
`tests/test_cortex_test_isolated.py`, run sequentially:

```
npm --prefix integrations/paseo ci
npm --prefix integrations/paseo run typecheck
```

The local fake endpoint needs the locked `ws` and `@getpaseo/protocol/messages`
imports. Plugin test scripts require Node support for `--experimental-strip-types`.
Missing startup output is a failure, never a successful binding or a skip.
Ordinary and isolated Python startup forms both exercise the complete no-paid
binding matrix with fresh scratch outside every repository; see
[verification sequencing](verification.md#sequencing).

## Focused Paseo plugin checks

Run these no-paid scripts after the local prerequisites above:

```
npm --prefix integrations/paseo run test:display
npm --prefix integrations/paseo run test:terminal-status
npm --prefix integrations/paseo run test:command-stop
```

Select `test:display` for worker display, assignment-prompt and activity models;
`test:terminal-status` for terminal outcome reconciliation; and `test:command-stop`
for command list/stop RPC and client state. They are not rendered GUI checks.

For native queue/edit/send/retry or provider-delivery changes, discover the
installed Metro bundle read-only, then supply that exact path:

```
find /opt/Paseo/resources/app-dist/_expo/static/js/web -maxdepth 1 -name 'index-*.js' -print
npm --prefix integrations/paseo run test:native-queue -- /absolute/path/from/discovery/index-HASH.js
```

An absent, ambiguous or incompatible bundle is a blocker, not a skipped pass.
This executes selected installed helpers with stand-ins and the Cortex provider;
it does not launch or modify Paseo, prove pixels/focus, or make a paid call.
See the [complete plugin inventory](verification.md#paseo-plugin-integrationspaseo)
for the other scripts and the installed-question component check.

## The non-paid default command

Run the fixed, no-argument entry point from the checkout (also supported when
tracked files are read-only):

```
python3 -B scripts/run_tests_fast.py
```

It uses `.venv/bin/python`, eight xdist workers, `--dist=loadgroup`, explicit
batches of at most 32 files, a 60-second per-test timeout (including setup and
teardown), and GNU `timeout --kill-after=5 180` for each batch. A timed-out test
names the test and fails its worker; workers are not restarted to hide a crash.
Any nonzero batch exit makes the final exit nonzero, while later batches still
run. Bytecode and pytest cache writes are disabled. Fresh HOME/XDG roots are
created outside Git; no dependencies are installed by the runner.

Before the first batch the runner runs `cargo build --locked -p workspace-service`
into `target/` on the dev channel, bounded by GNU `timeout --kill-after=5 300`, and
points `WORKSPACE_TEST_SERVICE_BINARY` at the result. Service tests launch that real
binary, so an older build would test older code: a missing `cargo` or a failed build
stops the run with exit 2 instead of reusing it. `cargo` comes from `PATH`, then
`$CARGO_HOME/bin` or `~/.cargo/bin`. `target/` is git-ignored, so this works when
tracked files are read-only. A direct pytest run does not build; those tests refuse
a binary older than any input in its `target/debug/workspace-service.d` and name
the build command. A binary chosen through `WORKSPACE_TEST_SERVICE_BINARY` with no
`.d` file beside it is not compared.

Install the local prerequisites before entering a read-only verification sandbox:

```
uv pip install --python .venv/bin/python pytest-xdist==3.8.0 pytest-timeout==2.4.0
```

Missing dependencies are an error, not a serial fallback. `tests/test_fast_suite.py`
independently inventories test files and checks the batch selection, stale
exclusions, the service build, successful execution, real failure, and a
deliberately hanging test.
No files are currently excluded. Every future exclusion must appear by exact
name and reason in `SLOW_FILES` in the runner and here; the explicit slower
command is `python3 -B scripts/run_tests_fast.py --slow` (currently an empty list).
Do not use the old unbounded whole-suite pytest command.

Two environment requirements are not cosmetic; both were learned by losing
runs to them:

- `PYTHONDONTWRITEBYTECODE=1` remains required for candidate checks and is
  recommended for the suite. Subprocesses do not inherit `python3 -B`, only
  the environment variable. Package enumeration also excludes local
  `__pycache__`, `.pyc`, and `.pyo` residue from the shipped payload and
  isolated candidate copy; these execution artifacts are never package
  inputs, so stamping does not require destructive cleanup of ignored files.
- `HOME` and `TMPDIR` must lie outside **every** git repository, ancestors
  included. The storage guards refuse to treat a directory inside a checkout
  as a user home, and a scratch home under a user directory that is itself a
  dotfiles repository produces false failures far from their cause.

### Nested-sandbox portability

The default suite may itself run inside Cortex's runtime verification sandbox.
Lifecycle verification uses a test-only launch seam, not nested bubblewrap;
real subprocess outcomes, retention, cancellation and revision checks remain
exercised. Production sandbox preparation remains fail-closed and has no bypass.
Fixed git-context commands use a separate test-only preparation seam.

Tests executing real bubblewrap use `requires_bwrap` and one namespace probe per
pytest session. Known namespace denial skips explicitly with
`bwrap cannot create user namespace in this environment (nested sandbox)`;
unexpected probe failures are errors, never inferred passes. Run
`python3 -B -m pytest -q -m requires_bwrap -rs` outside a nested sandbox to retain
real containment evidence. See [verification](verification.md#running-the-suite-inside-a-runtime-sandbox).

### Parallel run

The default runner fixes the worker count at eight instead of depending on
host CPU count or inherited pytest arguments. For a small targeted group use:

```
timeout --kill-after=5 300 env PYTHONDONTWRITEBYTECODE=1 .venv/bin/python -B -m pytest -q -n 8 --dist=loadgroup --timeout=60 --timeout-method=thread tests/test_fast_suite.py tests/test_pytest_parallel.py
```

Use `--dist loadgroup`. It spreads tests one by one and keeps tests marked
`xdist_group` on one worker. `tests/test_emission_scale.py` uses those marks
so each module-scoped scale run happens once per session, not once per
worker. The default and explicit commands select the same tests.

For serial debugging, use `CORTEX_PYTEST_SERIAL=1` (works with or without xdist):

```
timeout --kill-after=5 300 env CORTEX_PYTEST_SERIAL=1 PYTHONDONTWRITEBYTECODE=1 .venv/bin/python -B -m pytest -q -n 0 --timeout=60 --timeout-method=thread tests/test_fast_suite.py
```

With xdist installed, `-n 0` is also an explicit serial escape. Explicit `-n`
choices, distribution options and `-p no:xdist` are preserved, including options
from ini `addopts` and `PYTEST_ADDOPTS`. Collection-only runs remain serial.
`tests/conftest.py` marks the running parent with `CORTEX_PYTEST_CHILD=1`;
inherited nested pytest invocations and xdist workers never auto-spawn workers.
An explicit child `-n` remains an explicit choice; the existing launcher children
use no such flags. The parent marker is restored after pytest returns.

A 2026-10-21 baseline on a 20-CPU workstation (Python 3.12.13, pytest 9.0.2,
pytest-xdist 3.8.0) measured 560.120 s serially and 100.026 s in parallel,
including pytest shutdown and temporary-directory cleanup. Both runs reported
8,057 passed and 133 skipped (8,190 collected). The measurement used fresh
HOME/XDG roots outside Git repositories, `-p no:cacheprovider --durations=20`,
and the repository's `.venv/bin/python -B` with `PYTHONDONTWRITEBYTECODE=1`.
On 2026-10-22, after the URL-redaction prefilter, the exit-aware command wait
and the concurrent launcher matrices, the plain default command took about
62 s on the same workstation (four runs, 61.2–62.2 s, 20 or 16 workers alike).
These are development measurements, not release qualification or a timing SLA.
Benchmark before/after runs only on a stable checkout: concurrent source edits
can change test selection and invalidate package stamps as well as comparisons.

The isolated-launcher regression runs its two complete qualification-binding
matrices concurrently, each with separate private HOME/XDG/TMPDIR directories.
It still checks both startup forms, the pass/fail exit-propagation canaries, and
both real child exit codes; each child remains serial with its existing timeout.
Do not put unconditional xdist flags in ini `addopts`: that breaks the optional
plugin fallback and can multiply workers inside nested pytest subprocesses.

For targeted pytest only, without pytest-xdist the legacy conftest auto-parallel
hook remains inert. The default runner deliberately requires xdist and timeout.

For a release-grade run, verify in a throwaway clone rather than the working
checkout: `git clone --no-hardlinks`, overlay the working diff, stamp there
(`python3 -B scripts/cortex_package.py stamp`), then run the command above.
The stamp must reproduce byte-identically in the real checkout.

### Bounded timing observations (2026-11-05)

On this 20-CPU workstation, Python 3.14.4 / pytest 9.2.0 / xdist 3.8.0:

- Diagnostic baseline: 399 files in 34 groups of at most 12, eight workers,
  45-second per-test and 180-second per-group bounds: **422.53 s**, exit 1.
- Repaired default: `timeout 900 python3 -B scripts/run_tests_fast.py`,
  400 files in 13 groups of at most 32, eight workers, 60-second per-test and
  180-second per-group bounds: **313.72 s**, exit **0**, **9708 passed,
  143 skipped**, no failures. No test files were removed or excluded; five
  additional cases cover independent SQLite writers, missing Workspace tables,
  legacy-hook repair and immutable inspection.
- No test hit the 45-second diagnostic deadline; no real hang was reproduced.
  The baseline summed test-case time was 2108.52 s (parallel time, not a measured
  serial wall clock). Slowest file totals were emission scale 327.57 s,
  composed synthesis 128.66 s, and runtime task machine 66.28 s. The slowest
  individual test took 33.16 s, in emission scale.
- Separate eight-worker, one-file wall-clock checks measured emission scale
  **45.17 s** (41 passed), composed synthesis **15.50 s** (112 passed), and
  runtime task machine **10.45 s** (153 passed); all exited 0.

The baseline also found missing local plugin dependencies, a fixed-port collision,
and incomplete stat test doubles under Python 3.14. The SQLite writer failures
are fixed by connection-local outbox hooks; the original orchestration-overhead,
read-API actor-scope and retention regressions pass unchanged. The passing default
is no-paid automated evidence, not live or full release qualification; conditional
skips remain unqualified. The original hours-long process was absent at inspection;
its exact cause cannot be reconstructed from these measurements. These timings
are not an SLA, a controlled before/after performance comparison, or a claim that
the historical Python 3.12 measurements above were reproduced.

## Opt-in installed-binary checks (no paid call)

Two tests exercise a real installed engine binary against a loopback fake
model provider. They skip unless their variable names the binary:

- **Codex.** `CORTEX_PINNED_CODEX_BINARY=<codex binary>` enables
  `tests/test_engine_native_gateway.py`.
  `CORTEX_CODEX_ACP_TEST_PACKAGE=<installed codex-acp package directory>`
  enables `tests/test_codex_developer_instructions.py`, which uses a fake
  App Server and sends no paid model request. On 2026-09-30 its 13 tests
  passed against 2.0.1 and the retained 1.13.0; the installed Codex CLI
  0.159.1 wrapper/jail checks also passed. A separate prompt-free dev catalog
  refresh observed `gpt-6.1-sol` on 2.0.1 and refreshed Claude 0.84.0.
  On 2026-10-07 the 13 bundle tests passed for 2.1.1 and retained 2.0.1;
  three installed CLI 0.159.3 wrapper/jail tests passed. The fake App Server
  frame reader now retains raw buffered frames before waiting on the fd.
  This does not qualify paid turns, retained raw custom-tool history or
  rendered client behavior (`paid live check not requested by the owner`).
- **OpenCode.** `CORTEX_PINNED_OPENCODE_BINARY=<opencode 2.x.x binary>`
  enables `tests/test_engine_opencode_qualification.py`. It runs the binary
  through `EngineSessionManager`, the engine jail with process isolation, the
  brokered Cortex MCP server and the permission bridge, in a disposable HOME
  and project:

```
CORTEX_PINNED_OPENCODE_BINARY=$HOME/.opencode/bin/opencode PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_engine_opencode_qualification.py
```

`tests/test_engine_opencode.py` needs no OpenCode binary and is part of the
default suite. Its jail, version-probe and onboarding cases skip without
`bwrap`.

## Why a default run cannot start a paid campaign

Nothing in the default suite can reach a paid engine, and this is enforced in
the product, not by convention:

- The paid probe (`scripts/cortex_codex_probe.py`, and its Claude
  counterpart) refuses to do anything unless **both** `--i-authorize-paid-call`
  and `--approval-ref` are supplied. Either flag alone exits non-zero before
  any engine is loaded. The probe test suite never passes them.
- Tests that spawn the real `cortex_acp.py` entry point run against a fresh
  temporary `HOME`/`XDG_CONFIG_HOME`/`XDG_STATE_HOME` with no connections
  configured, so `session/prompt` fails closed at route resolution before any
  engine process could be launched — regardless of which engine binaries
  happen to be on `PATH`.
- Diagnostics are covered by a test that instruments the real engine-launch
  constructor and asserts zero launches during a full diagnostic run.

A live campaign therefore requires an explicit, separately authorized
invocation. It cannot be reached by running the test suite.

## CI note: run as a non-root user

One test (`tests/test_artifact_revision.py`, unreadable-entry handling) skips
itself when the process can bypass directory permissions. A containerized CI
runner executing as root would silently never run it. Configure CI with a
non-root user. Current totals depend on the checkout and environment; record
the exact passed, skipped and failed counts from each run rather than relying
on earlier recorded counts.

Remaining conditional guards are platform guards (POSIX FIFO support,
symlink support, `tmux` presence) whose logic is also covered by unconditional
tests; they are not the sole coverage of any mandatory case.

Settings-only routing/install behavior is covered by tests/test_no_hardcoded_model_names.py, tests/test_model_settings_template.py, tests/test_settings_only_routing.py and tests/test_model_settings_integration.py; profile deletion is covered by integration and typed form/API tests. No-paid tests do not establish production migration, installed desktop reachability or a paid route. Migration tests use disposable fixtures; live checks remain gated by explicit owner consent and an owner-selected supported route.

### Project runtime store ignore invariant

`python3 -B -m pytest -q tests/test_gitignore_guard.py tests/test_situational_skill_gate.py tests/test_command_run_arguments.py`
checks Git effective ignore semantics, digest/replay/no-follow and authority
protections, native result boundaries, consented deletion, and command completion.
These are isolated no-paid runtime tests, not Desktop or native-model evidence.
See [ignore boundary limits](../features/permissions-and-containment/index.md#runtime-store-ignore-protection).

After changes to the observed-change/submission boundary, also run
`python3 -B -m pytest -q tests/test_artifact_revision.py` and
`python3 -B -m pytest -q tests/test_runtime_task_machine.py` to verify revision
coverage and unchanged-task review behavior.

## Cortex Workspace local slice

The Workspace checks are listed with their commands in [verification](verification.md#cortex-workspace-local-slice-checks-no-paid-plus-container). The test files, grouped by area:

- Wire contract and generation: `tests/test_workspace_contract.py`, with the Node vectors in `packages/workspace-contract/test.mjs`.
- Pins and prerequisites: `tests/test_workspace_pins.py`.
- Adapter, identity, projections, outbox, decisions, starts and results: `tests/test_workspace_api.py`, `tests/test_workspace_extension.py`, `tests/test_workspace_outbox.py`, `tests/test_workspace_decisions.py`, `tests/test_workspace_start.py` and `tests/test_workspace_results.py`.
- Service, journey and integration: `tests/test_workspace_service_integration.py` and `tests/test_workspace_journey.py`.
- Machine helper against a real repository and a private configuration root: `tests/test_workspace_machine_tools.py`. Weekly limits, source fallback without a global CLI, scoped-window aggregation and exclusion of session quotas: `tests/test_workspace_weekly_usage.py`.
- Channel stores: `tests/test_workspace_channels.py` exercises release-canary preservation, dev-only format-11 migration/inspection, both private paths and effective Git ignores. Rust service tests exercise coexistence and unowned-dev-store refusal.
- Question notifications: `packages/workspace-ui/test/notifications.test.tsx` checks off-task delivery, deduplication, disabled/open-task suppression and delivery failure. The native `notification::tests` checks payload shape; `delivery_to_private_bus` is explicitly ignored unless run in a disposable container under `dbus-run-session` with `CORTEX_NOTIFICATION_TEST_BUS=private-container`, where it verifies a real Notify exchange and missing-service failure, not rendered desktop behavior.
- Packaging and channels: `tests/test_workspace_packaging.py`, `tests/test_workspace_container_build.py`, `tests/test_workspace_rendered_harness.py`, `tests/test_workspace_channels.py` and `tests/test_workspace_clone_dev_settings.py`. The rendered-harness tests cover stale-node versus process-death handling and series rejection of failed, missing, duplicated or contradictory evidence, including the CLI exit/count verdict. Archive checks compare the complete stamped runtime payload and normal-user file modes. `python3 -B scripts/build_workspace_desktop.py --check-install` adds the clean Ubuntu apt install, bundled-runtime activation, legacy-shape upgrade and data-preserving removal smoke; unavailable systemd managers and denied sandbox namespaces remain unverified.
- Domain and presentation packages: `npm` test scripts in `packages/workspace-domain`, `packages/workspace-ui`, `apps/cortex-workspace-desktop` and `prototypes/cortex-workspace/desktop`.

The glob `tests/test_workspace_*.py` also runs any other Workspace test file. Passing counts change with the suite and are not evidence on their own; the retained command receipts are.
