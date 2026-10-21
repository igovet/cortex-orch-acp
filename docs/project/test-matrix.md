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

## The non-paid default command

The ordinary command automatically uses `-n auto --dist loadgroup` when the
optional pytest-xdist plugin is loaded and no worker/distribution choice was given.
Without xdist it runs serially, without unknown-option errors:

```
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q
```

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

With [pytest-xdist](https://pypi.org/project/pytest-xdist/) installed
(`pip install --user pytest-xdist`), the default suite runs across all cores.
The equivalent explicit invocation is:

```
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q -n auto --dist loadgroup
```

Use `--dist loadgroup`. It spreads tests one by one and keeps tests marked
`xdist_group` on one worker. `tests/test_emission_scale.py` uses those marks
so each module-scoped scale run happens once per session, not once per
worker. The default and explicit commands select the same tests.

For serial debugging, use `CORTEX_PYTEST_SERIAL=1` (works with or without xdist):

```
CORTEX_PYTEST_SERIAL=1 PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q
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

Without pytest-xdist, the serial command still works and the `xdist_group`
marker has no effect.

For a release-grade run, verify in a throwaway clone rather than the working
checkout: `git clone --no-hardlinks`, overlay the working diff, stamp there
(`python3 -B scripts/cortex_package.py stamp`), then run the command above.
The stamp must reproduce byte-identically in the real checkout.

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
