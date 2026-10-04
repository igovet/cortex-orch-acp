# Verification

Commands below are copied exactly from `AGENTS.md`, `README.md`,
`docs/acp/test-matrix-and-ci.md` and `integrations/paseo/package.json`.

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
unverified check needs a concrete blocker, and synthesis discloses it.

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
  `docs/acp/test-matrix-and-ci.md`): the paid probes refuse to run without
  both `--i-authorize-paid-call` and `--approval-ref`, `cortex_acp.py` tests
  use a fresh temporary `HOME`/`XDG_CONFIG_HOME`/`XDG_STATE_HOME` with no
  connections configured, and a dedicated test asserts zero engine launches
  during a full diagnostic run.
- Test totals vary with the checkout; retain the exact counts, skips and exit code from each run rather than treating a historical count as current. Remaining conditional guards are platform guards (POSIX FIFO, symlink support, `tmux` presence) that are not the sole coverage of any mandatory case.

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

Source-only checks use `--check` or `--dry-run`. Preserve all 23 profiles (22
specialists and the consultant) and their shared report protocol.

## Sequencing

Run package, profile-generation and test checks sequentially in the same
checkout. Use the smallest meaningful tests first.

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
See [bound task inspection](../acp/observability.md#bound-task-inspection)
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
[`docs/acp/dev-paseo-environment.md`](../acp/dev-paseo-environment.md):
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
findings remain. The [dated dependency disposition](../acp/paseo-dependency-audit.md)
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
npm --prefix integrations/paseo run test:steer      # steer and scalar/batch question transport regressions
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

A local sibling import must use its real `.ts` extension (e.g. `./pipeline.ts`),
never a `.js` specifier standing in for it: `tsc`/esbuild remap `.js` to the
sibling `.ts` file, but `node --experimental-strip-types` does not, and a
`.js` specifier that resolves to nothing breaks every test that transitively
imports it.

**GUI testing** (verifies rendered user-facing behavior):

Paseo 0.10.0 desktop evidence (2026-09-29) is inconsistent about availability, so the screen must be treated as unverified rather than reliably reachable. One earlier report described the implemented settings screen and component tests; two isolated live desktop checks found the plugin enabled/running at this worktree but no Cortex entry in Settings or matching command-palette result. The later recheck also inspected the rendered screenshots directly. These negative findings apply to the tested host/session, not every host version or feature flag. Because those desktop runs could not reach the screen, they did not exercise GUI settings reads/updates, saving, revision-conflict/error display, secret redaction, or the OpenCode section; component/render tests are not substitutes. See desktop reports `r_24d9b22e87f5` and `r_a7f0d6da5ec2` for retained evidence and limitations. The settings UI code now uses typed controls rather than JSON text editors; component/unit tests and a client-bundle render-walk establish neither that the screen is reachable in Paseo Desktop nor rendered GUI behavior. Use the existing documented CLI fallback for supported setup operations, including OpenCode onboarding, where the UI is unavailable. Use the isolated dev home/profile and a disposable project as described in [the dev Paseo environment guide](../acp/dev-paseo-environment.md).

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

In Paseo Desktop, inspect the rendered behavior under test. If the Cortex Settings screen is available, check workspace selection, supported setting sections, validation/conflict behavior, redacted values, and explicit OpenCode onboarding; if it is absent, record that outcome and do not report those interactions as tested. The 2026-09-29 Paseo 0.10.0 checks found the plugin screen and command absent, with the later check also inspecting screenshot pixels; see `r_24d9b22e87f5` and `r_a7f0d6da5ec2`. GUI form behavior remains unverified because those runs could not reach the screen. For Xvfb use `python3 -B scripts/cortex_paseo_dev.py gui --xvfb`.
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
before invoking the launcher. For the current Python 3.12 toolchain, the explicit
existing dependency path is `/home/igovet/.local/lib/python3.12/site-packages`;
verify compatibility/existence rather than assuming it on another machine.

```bash
python3 -B scripts/cortex_test_isolated.py --environment-root "$SCRATCH" \
  --dependency-path /home/igovet/.local/lib/python3.12/site-packages tests/test_package.py
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
See [binding provenance and limits](../acp/dev-paseo-environment.md#read-only-dev-binding).
After all edits, sequentially stamp, validate, check profiles, run isolated
`tests/test_package.py`, then validate again. Runtime independent review and
current-artifact final verification remain separate from these worker checks.
