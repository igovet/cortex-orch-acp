# Verification

Commands below are taken from `AGENTS.md`, `README.md`,
`docs/project/test-matrix.md`, `integrations/paseo/package.json`, the `scripts/*.sh`
wrappers and the `cortex-xtask` / `cortex-paseo-tools` usage text. The product contains no
Python; every check is a Rust, Node or shell command. `scripts/cortex-xtask.sh`
and `scripts/cortex-paseo-tools.sh` build the tool offline with the checkout lockfile
and run it. Run Rust builds and tests under `umask 022` with `~/.cargo/bin/cargo`.

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

A worker's failed check is closed only by an exact later passing rerun of the same argv and cwd; that exact rerun may cross between `command_run` and `outside_command` in either direction, preserving both receipts and their operation/profile provenance, while a same-operation profile change still never matches. A supported syntax, regex, or statically proven literal-expectation error in the worker's own bounded, self-contained standalone Python probe may instead be retracted by a later verified passing correction; the failure stays as `retracted` history and is never a pass. A supported standalone-Python probe mistake or a direct Python script's argparse startup error (exit code 2) may instead be superseded by an explicit `command_correction` (`superseded` history, never a pass). A complete whole-tree `git diff --check` whose whitespace diagnostics name only paths reserved by sibling writers of the same plan revision is `not_attributable` and does not fail the submitting check. Runtime verification failures and real project test/lint/build failures are never retracted (see [check history](../features/orchestration/index.md#check-history-and-current-outcome)). The Rust check-history, correction, retraction and evidence-strength cases are in `cargo test -p cortex-core --lib worker_checks` and `cargo test -p cortex-core --test lifecycle_parity resolved_check_history_is_rederived_at_completion`.
Executable Bash receipts that overwrite a captured failure (`%n`, variable-expanded `printf -v`)
remain unverified on `result_submit`, while safe `echo`/`tail` wrappers retain status
propagation and exact-rerun coverage.
See [check closure](../features/orchestration/index.md#check-history-and-current-outcome).

A writing execution worker that submits a failed assignment with an open, verified
failed check receives one repair opportunity in that attempt. Read-only workers retain the failure and
evidence for the coordinator, which schedules a writing repair or changes the
plan. A retry reply does not close the assignment. Ordinary repair plans carry
every unresolved final-verification argv into the next revision. Only an exact
later passing runtime receipt closes that argv, or an explicit removal through
`verification_submit` followed by an owner answer to a task-global question opened after the removal. A worker-local answer or ordinary retry never
waives the obligation. Automatic continuation follows stricter rules and never
waives a failed, missing, truncated or unobservable check.


## Targeted tests for a task

Workers run only the tests covering their change: the test files they added or
changed and the existing tests of the crates, contracts and documents they
touched, selected with the narrowest `cargo test` filter and a bounded timeout:

```
umask 022 && timeout --kill-after=5 600 ~/.cargo/bin/cargo test --locked --offline -p <crate> [--test <file>] [<name filter>]
```

Changes to a shared contract include its cross-cutting checks: advertised tool
schemas and the Codex cheat sheet (`cargo test -p cortex-engine` and
`cargo test -p workspace-service --lib native_instructions`), profile and
instruction sources (`scripts/cortex-xtask.sh profiles check` and
`scripts/cortex-xtask.sh generate-instructions check`; `check-generated` runs
the contract, instruction and profile checks together) and package payload
(`scripts/cortex-xtask.sh package check` after stamping). Workers do not run the
whole suite: it belongs in `plan.verification`, which the runtime runs once after
review, through the bounded default runner (`scripts/cortex-xtask.sh test-fast`),
never an unbounded whole-workspace command.

## Exact adapter qualification

Codex ACP 2.6.0 and Claude ACP 0.107.0 retain the outgoing ACP v1 contract.
Run the bounded registration, discovery and protocol checks without a model call:

```bash
umask 022 && timeout 1500 cargo test --locked --offline -p cortex-engine -p cortex-acp -p workspace-service
```

Installed-package checks skip explicitly when the named local distribution is
absent; a skip is not qualification. Exercise 2.6.0 and the retained 2.3.0 and 2.1.1
separately where the installed package is available. The real-adapter
initialization tests use empty isolated account directories and send no prompt.
None of these checks establishes paid CLI/desktop behavior or real inference
cancellation.
See the [engine evidence](../features/codex-engine/index.md#current-support-260-2026-11-16)
and [Claude evidence](../features/claude-engine/index.md#current-support-01070-2026-11-16).

## Model hint notice checks

Run the no-paid routing/notice and native fake-engine regressions:

```bash
umask 022 && timeout 900 cargo test --locked --offline -p cortex-core -p workspace-service
```

These exercise matching pins, real differences, explicit effort, unknown/conflicting
audit bindings, transaction rollback and repeated/recovered notifications. Wire
assertions use disposable fake engines, not paid native accounts. Neither notice
text nor a dispatch audit proves native live configuration. See
[notice semantics](../features/routing-and-fallback/index.md#ignored-model-hint-notices).

## Tool discovery and read batches

Run the changed-surface checks sequentially, not the entire suite:

```bash
umask 022 && timeout 900 cargo test --locked --offline -p cortex-engine
umask 022 && timeout 900 cargo test --locked --offline -p workspace-service --lib native_instructions
```

The read-API benchmark and the public-evidence fixture and live-assertion scripts
(`cortex_read_api_benchmark`, `cortex_read_api_fixture`, `cortex_qualification_assert`,
`cortex_inspect_task`) were Python, were removed on 2026-11-16 and have no
replacement. Real dispatch/stdio tests cover authority loss, schema parity,
preflight, per-item failure, bounds, deferred retrieval and conservative report
observations; they do not prove live model usage. Requested live CLI
qualification follows the [dev guide](dev-environment.md) and
[read API scenarios](../features/tool-discovery-and-read-batch/index.md#live-cli-qualification).
Cancellation interleavings not established by bound events stay unverified.
Expected literal bytes, current effective schemas, receipt identity and explicit
completeness matter; a digest alone, preflight readiness or `COMPLETED` does not
prove API behavior. Missing/forged/wrong-owner/task/stale/replayed/truncated/
pending evidence blocks acceptance, and distinct valid repetitions are not replay.
No paid check is launched by these commands.

## Plan error recovery

Run this no-paid scenario in the checkout; every store it creates is
disposable, and it never starts an engine or replays an existing task:

```bash
umask 022 && timeout 900 cargo test --locked --offline -p cortex-core --test plan --test lifecycle
```

The Rust plan and lifecycle suites cover plan intake, atomic rollback and same-key exact
reconciliation. The former Python scenario also covered malformed/missing/unsupported
routing policy, typed error categories and concurrent identical submissions; those
cases were not re-verified in the Rust suites for this page. Fault injection is
test-only, never a live qualification procedure. It is not evidence of a historical failure's inner cause
or transaction disposition, nor of real-model recovery compliance.

## Development command verification

Focused default/provenance/gateway regressions are in the `workspace-service`, `cortex-engine` and `cortex-core` Rust test suites (`cargo test --locked --offline -p workspace-service -p cortex-engine -p cortex-core`), alongside permission, command-evidence, session recovery and store-format tests.
A test-only unjailed launch replacement covers real subprocess receipts and
cancellation, not real filesystem containment or Docker. Execute the real
bubblewrap cases where namespaces are supported; a nested-sandbox run that
cannot create them remains unverified containment. Docker qualification uses only disposable project-local
inputs and uniquely named disposable images/containers/volumes, never production
workloads, daemon reconfiguration, privileged containers or host-root bind probes.
Daemon-side path visibility, cleanup and identity must be observed separately.
Paid CLI/desktop checks still require an explicit owner request naming the mode.


## Native command namespace checks

The Rust brokered command/verification backend uses bubblewrap, the Linux host
mechanism for brokered commands. Its focused real-fixture check is:

```sh
cargo test --locked --offline -p workspace-service --lib native_command_sandbox -- --nocapture
```

On a namespace-restricted host it asserts explicit fail-closed preparation for
all three profiles, rather than ignoring the test. That outcome does not prove
positive containment. Where namespaces work, the same test exercises literal
private `/var/tmp`, read-only chmod/timestamp/data denial, writable scratch and
HOME-cache metadata, protected descendants beneath writable parents, private
procfs and a fresh view after reopening the backend. Advanced fixtures exercise
Git commits and linked/submodule data with control guards, absent-entry sweeps,
Guarded HOME state versus read-only caches, sensitive relocations, owner SSH-agent
sockets/trusted config/opt-in askpass, discarded overlays, hardlink detachment and
limits, glob denials and deterministic source/parent/authority races. The positive-only incoming
and desktop integration harnesses assert the same production fail-closed path
when namespaces are unavailable; their positive journeys must run in the
namespace-capable container and are not established by the restricted-host run.

The positive namespace journeys need real unprivileged namespaces. Where the shell
itself has them (a host-direct run, not a nested sandbox), require them explicitly:

```sh
umask 022 && CORTEX_REQUIRE_NAMESPACE_FIXTURES=1 timeout 1500 cargo test --locked --offline -p cortex-acp -p workspace-service -p workspace-protocol -p cortex-core -p cortex-engine
```

With that variable set, unavailable namespaces are a hard failure, not a skipped
positive check. A disposable-container Cargo runner that wrapped the test binaries
(`tests/workspace_support/run_native_sandbox_container.py` and its
`CORTEX_SANDBOX_TEST_IMAGE` image selection) was Python and has no replacement; the
workspace tests also run in the Docker builder image through
`build-deb --test-workspace` (see [packaging](packaging.md)). Privilege in a
container is not hostile-code isolation.

Focused native recovery/evidence checks are included in the same crate run:

```sh
cargo test --locked --offline -p workspace-service --lib recovery_tests
cargo test --locked --offline -p workspace-service --lib evidence_tests
cargo test --locked --offline -p cortex-core --lib evidence_policy_tests
cargo test --locked --offline -p cortex-core --lib worker_checks
cargo test --locked --offline -p cortex-core --test lifecycle_parity resolved_check_history_is_rederived_at_completion
```

The recovery fixtures really close/reopen their disposable stores, compare
before/after/diverged hashes, refuse aliases/legacy intents, and retain boot/PID/
start identities without signalling or relaunching a lost handle. The extended
native_sessions fake-client journey also injects an acknowledgement-transaction
failure after a real file replacement and checks exact no-replay reconciliation,
plus actual sandbox-command process identities and project-digest bindings.
The worker-check tables carry the check-history, correction,
retraction and evidence-strength cases. Real-store completion fixtures rederive
links and reject forged links, lost output and later failures; the service
fixture reopens a refused failure and proves it cannot be omitted.
The native_desktop executable additionally kills and restarts the actual service
binary mid-brokered-command, with isolated HOME/XDG and disposable projects. It
checks continued execution, completion while the service is offline, a recovered
owner stop, and supervisor loss. Exact command/output identities, binary output
beyond the inline limit, nonzero exit status, hash/size, one receipt after another
restart and a single launch marker are asserted. A lost supervisor retains an
unknown outcome and refuses a stop rather than signalling its old PID; once its
recorded payload process group is empty its receipt is settled as
`outcome_not_recorded`, never as a success. Run just
these cases with real namespaces (see above) with:

```sh
umask 022 && CORTEX_REQUIRE_NAMESPACE_FIXTURES=1 timeout 1500 cargo test --locked --offline -p workspace-service --test native_desktop -- command-restart-probe
```

These are not exhaustive crash-phase/disk-pressure, outside-command descendant
settlement or engine-session recovery gates.

Run formatting (`cargo fmt --all --check`), package stamping, profile generation and tests sequentially.
These checks do not qualify a desktop archive, live models, engine-jail parity
or the complete Rust migration.

## Terminal resource regressions

The Rust `workspace-service` and `cortex-engine` suites use disposable stores and real
local fake ACP subprocesses to check terminal/error/disconnect teardown, another task's
writer acquisition, repeated cleanup, partial launch quarantine, unknown identity and
surviving group descendants (for example `cargo test -p workspace-service --test
terminal`). They do not qualify paid engine behavior or a host's rendered chat
lifecycle. Existing transport, recovery, completion and usage-settlement tests cover
adjacent EOF/SIGTERM, cancellation and transaction boundaries.

## No-paid default test suite

```
scripts/cortex-xtask.sh test-fast
```

- The runner is the Rust `cortex-xtask test-fast` command. It runs its steps one at
  a time, never in parallel, in the same checkout, each under GNU
  `timeout --kill-after=5`: `build` (900 s: `cargo build --locked --offline -p
  cortex-acp -p cortex-paseo-tools -p workspace-service`), `cargo` (900 s: the
  headless crates `workspace-protocol`, `cortex-core`, `cortex-engine`,
  `workspace-service`, `cortex-acp`, `cortex-paseo-tools` and `cortex-xtask`),
  `desktop` (900 s: `cortex-workspace-desktop --no-default-features`), `remote`
  (900 s: `workspace-remote`, `workspace-client` and `cortex-workspace-android`), `ui`,
  `desktop-ui` and `phone-ui` (Vitest with `--testTimeout=60000`, 600 s each), `paseo-typecheck`
  (600 s) and one `paseo-<name>` step per `test:*` script in
  `integrations/paseo/package.json` (180 s each).
- `--list` prints the steps without running them; `--only NAMES` and `--skip NAMES`
  select steps by name (comma-separated; an unknown name fails). Every nonzero exit
  propagates.
- The runner points `HOME` and `XDG_CONFIG_HOME`/`XDG_STATE_HOME`/`XDG_CACHE_HOME` at a
  fresh private scratch directory outside Git, keeps `CARGO_HOME`/`RUSTUP_HOME`, runs
  cargo `--locked --offline`, sets `CORTEX_WORKSPACE_CHANNEL=dev` and removes
  `CORTEX_WORKSPACE_RELEASE_AUTHORIZED`. It never installs dependencies.
- A missing prerequisite (`cargo`, `npm`/`npx`, installed `node_modules` in a step's
  directory, GNU `timeout`) is an explicit failure with exit 2, never a skip: run
  `npm ci` in the named directory first. See
  [the default command](test-matrix.md#the-non-paid-default-command).
- `HOME` and `TMPDIR` of the invoking shell must lie outside every git repository
  (ancestors included), or storage guards produce false failures.
- Nothing in this suite can reach a paid engine (see
  `docs/project/test-matrix.md`): the paid probes refuse to run without
  both `--i-authorize-paid-call` and `--approval-ref`, and the `cortex-acp` tests use
  a fresh temporary `HOME`/`XDG_CONFIG_HOME`/`XDG_STATE_HOME` with no connections
  configured.
- Test totals vary with the checkout; retain the exact counts, skips and exit code
  from each run rather than treating a historical count as current. Platform guards
  (user namespaces, `tmux` presence) are not the sole coverage of any mandatory case.
  The earlier Python-era timing and count observations were removed with that suite;
  no Rust-era whole-suite timing has been recorded for this page.

## Running the suite inside a runtime sandbox

The runtime's final verifier allows 20 minutes of child execution and retains up to
1 MiB per stream for each command. The aggregate suite's sequential deadlines exceed
that budget. For full default-suite coverage, use `scripts/cortex-xtask.sh test-fast
--list` to enumerate steps (this does not run tests), then declare independent
verifier commands using `--only`. Keep the generated order: one command each for
`build`, `cargo`, `desktop`, `ui`, `remote`, `desktop-ui`, `phone-ui` and
`paseo-typecheck`, then up to five `paseo-…` steps per command. For example:

```text
["scripts/cortex-xtask.sh", "test-fast", "--only", "build"]
["scripts/cortex-xtask.sh", "test-fast", "--only", "cargo"]
```

Use comma-separated names for a group; cover each listed step exactly once. This
checkout's 21 default Paseo test scripts need five groups, leaving room for
`check-generated` and `package check` within the 16-command plan limit. A group of
five uses at most 15 minutes 25 seconds for the test processes, plus wrapper and
startup time. Keep all commands sequential, in the same checkout, with `build`
before suites that launch its service binary. Confirm the ignored `target/`
directory exists before verification so the sandbox can preserve build artifacts
between commands. Install the locked dependencies in each Node step directory and
confirm that `timeout --version` resolves to GNU coreutils on the verifier's
owner tool search path (some distributions expose GNU as `gnutimeout` while
`timeout` resolves to uutils). Do not substitute shortened or redirected output
for complete receipts. A namespace, time, output, or prerequisite failure still
blocks acceptance. The grouped budget requires a runtime built with that budget;
editing a checkout does not change an already-running verifier.

Outside the runtime, `scripts/cortex-xtask.sh test-fast` remains the standalone
bounded owner command, with the same prerequisites. A standalone result does not
replace exact reviewed runtime receipts.

Lifecycle tests replace only verification launch preparation with a test-only
runner: literal commands still execute, and production retains output, exit status,
cancellation and revision checks. Git-context tests likewise use a real fixed git
command without a nested sandbox. There is no production environment flag or
unsandboxed fallback.

Real containment tests need a user namespace. A run in an environment that cannot
create one (a nested sandbox) asserts explicit fail-closed preparation instead; set
`CORTEX_REQUIRE_NAMESPACE_FIXTURES=1` on a host that has unprivileged namespaces to
run the positive journeys. A nested run is not containment qualification. Do not
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
scripts/cortex-xtask.sh package stamp
scripts/cortex-xtask.sh package check
```

Run `stamp` after any package payload edit, before release-sensitive checks, to
regenerate the complete content hash; `check` validates the stamp. `runtime-payload
list` prints the exact shipped runtime file set (no Python, no `src/`).

## Generation checks

```
scripts/cortex-xtask.sh profiles check
scripts/cortex-xtask.sh profiles write
scripts/cortex-xtask.sh generate-instructions check
scripts/cortex-xtask.sh generate-contract check
scripts/cortex-xtask.sh check-generated
```

`profiles check` is the source-only verification: it fails if the generated worker
skills or profiles differ from their `agent-sources/`, and it asserts the 23 profiles.
`write` regenerates the derived files. `generate-instructions` covers the native
instruction data derived from `skills/`, `generate-contract` the Workspace wire
contract (schema `crates/workspace-protocol/schema/workspace.json`), and
`check-generated` runs all three checks. Each takes one positional action, `check` or
`write`; `check` never writes. Preserve all 23 profiles (22 specialists and the
consultant) and their shared report protocol. `scripts/cortex-xtask.sh check-pins`
verifies the reviewed Rust/CLI dependency pins without a build or network.

## Sequencing

Run package, profile-generation and test checks sequentially in the same
checkout. Use the smallest meaningful tests first. Before any check that spawns
the local Paseo fake endpoint (including `test-fast`), prepare the checkout-local
plugin prerequisites in order:

```
npm --prefix integrations/paseo ci
npm --prefix integrations/paseo run typecheck
```

The endpoint imports `ws` and `@getpaseo/protocol/messages` from the locked local
dependencies; missing imports fail startup, not binding qualification. The Node
scripts also require a Node version supporting `--experimental-strip-types`.
These no-paid prerequisites do not install a production plugin or qualify a live
daemon.

## Diagnostics (no model call)

`scripts/cortex-xtask.sh prerequisite-probe` prints a read-only inventory of the
native-build prerequisites (inventory only, never an install). The Python
diagnostics report (`cortex_diagnose`) was removed without replacement. The
read-only `cortex-paseo-tools` readers (`project-root`, `task-id`, `worker-history`,
`worker-results`, `running-commands`, `pipeline`) report the state of a project store
without a model call. For final-default propagation and bound lifecycle metadata, use
the Rust lifecycle and `cortex-paseo-tools` suites; they use a fake downstream
subprocess and do not prove paid host behavior.

## Setup status (read-only)

```
scripts/cortex-paseo-tools.sh status
```

`status` prints one JSON document (sorted keys); a refusal prints `error: ...` on
stderr and exits 1. The setup commands are `status`, `init-storage`,
`bootstrap-paseo-git-install`, `configure-paseo-plugin-launch`,
`install-paseo-plugin`, `validate-connection --connection ID`, `add-connection --engine ENGINE`
(native-account registration only; API-key and install-record options are refused),
`install-engine --engine ENGINE`,
`add-opencode-connection [--binary PATH] [--name NAME]`, `refresh-catalog [--workspace PROJECT]` and
`pin-route --connection ID --model MODEL`; all but `status` write owner configuration, so
run only the read-only `status` against an owner's real roots.

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

Default verification is no-paid. Run a paid live check only on an explicit
owner request for the current work naming the mode (`cli` or `desktop`) and
the surface. Record an approval reference naming that request. Otherwise,
report each applicable live check as not run with the blocker "paid live check
not requested by the owner". Authorization is not transferable between
surfaces or modes. Use only the owner-selected engine/model route supported
by current settings and observed host/catalog capabilities; do not substitute
or automatically escalate. Heavy models remain prohibited in routine live
qualification. Use existing production native accounts, check native login
status without printing credentials, and do not copy credentials, create test
accounts or log in.

### Paseo `cli` and `desktop`

Run Paseo checks only through the [isolated dev Paseo environment](dev-environment.md):
the permanent `~/.paseo-dev` daemon, separate `~/.config/Paseo-dev` Electron
profile, a disposable project, and the Cortex runtime launched by that daemon
from the explicit absolute checkout plugin path. Do not use production
`~/.paseo` or the production Cortex agent. `cli` is headless; Paseo `desktop`
uses the isolated Electron app.

The paid probes retain their safety guard and require both flags, with an
approval reference naming the owner's request:

```
scripts/cortex-paseo-tools.sh probe-codex --i-authorize-paid-call --approval-ref owner-request-<date>-cli
scripts/cortex-paseo-tools.sh probe-claude --i-authorize-paid-call --approval-ref owner-request-<date>-cli
```

The default automated suite remains no-paid. A `cli` check establishes runtime
and protocol behavior only; a probe or GUI observation supports only the exact
surface it exercises.

The Python route preflight, task-inspection, qualification-assertion and
case-observer scripts (`cortex_inspect_task`, `cortex_qualification_assert`,
`cortex_qualification_observer`, `cortex_timeline`, `cortex_trace_view`) were removed
without replacement on 2026-11-16, as were the `--wire-trace`, `route-preflight` and
`binding` modes of the dev wrapper. A requested `cli` run therefore reads its outcome
from the disposable project's `.cortex` store through the read-only
`cortex-paseo-tools` readers (`project-root`, `task-id`, `worker-history`,
`worker-results`, `running-commands`, `pipeline`) and the Paseo timeline
(see [the dev guide](dev-environment.md)); the owner-selected route is recorded with
the run, and an unrun assertion is reported as unrun, never inferred as passed.

`scripts/cortex-paseo-tools.sh acp-smoke` is the no-paid startup smoke harness
(launch/protocol surface only, not GUI evidence).
Run both documented fixture forms from the checkout root after source/package checks:

```
scripts/cortex-paseo-tools.sh acp-smoke
scripts/cortex-paseo-tools.sh acp-smoke --fixture-connection
```

The second mode supplies only an isolated fake API-key identity in temporary XDG settings; the harness asserts that it is not routable on the native host (and refuses with the same missing-route reason). It verifies no migration of owner data and no paid execution.


## Question and submission regressions

For changes to question continuation, recovery, or report submission, run the
focused runtime checks before package/profile checks:

```
umask 022 && timeout 900 cargo test --locked --offline -p cortex-core -p cortex-engine
umask 022 && timeout 900 cargo test --locked --offline -p workspace-service --test native_sessions
umask 022 && timeout 900 cargo test --locked --offline -p cortex-acp --test incoming
```

Documentation changes have no catalog test: the markdown-link check is
`cargo test -p cortex-xtask --test repo markdown_local_links_resolve`, plus
source-backed link/command inspection for affected pages. It does not validate
every documentation command.

The recovery tests verify exact option/custom answers, renewed bounded planning,
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

### Standalone Cortex Workspace desktop

This is the explicit-owner-approved `desktop` route for the 1.0.6 app. It runs
the **Dev channel** package in the owner's real environment, as the owner: real
HOME, session bus and keyring, the existing native Codex/Claude logins and the
existing GitHub CLI login. There is no nested sandbox lab, no RootlessKit or
namespace prerequisite and no owner preparation. Never install, modify or
restart the release package or service, never touch production `~/.paseo`, and
never use or change the owner's projects, settings, registrations or stores.
Use a separate disposable Git project.

From the checkout, run via the host outside-command route:

```bash
bash scripts/qualify-dev-deb.sh probe
bash scripts/qualify-dev-deb.sh smoke
bash scripts/qualify-dev-deb.sh smoke --archive newest
bash scripts/qualify-dev-deb.sh smoke --archive /absolute/path/to/cortex-workspace-dev_...deb
bash scripts/qualify-dev-deb.sh drive --archive newest -- /absolute/path/to/foreground-scenario-driver
bash scripts/qualify-dev-deb.sh install --archive newest
bash scripts/qualify-dev-deb.sh cleanup build-XXXXXXXX
```

- `probe` prints host facts and never fails on a gap: tools, display and
  session variables, exit codes of `gh auth status`, `codex login status`,
  `claude auth status`, `sudo -n true` and `docker info`, free space of the
  checkout and `/tmp`, and the newest local Dev archive. The runtime records the
  same kind of facts for every task at start (see the
  [environment section of the capsule](../features/recovery-capsules/index.md#native-retained-state-capsule-current-rust-runtime)),
  so plans start from them instead of discovering them hours later.
- `smoke` and `drive` use the exact archive: a fresh build without `--archive`,
  the newest local Dev archive with `--archive newest`, or a given path. Reuse an
  archive instead of rebuilding: every build is retained (the newest two per
  channel; see [build hygiene](packaging.md#build-output-hygiene)). The launcher
  checks that the package is `cortex-workspace-dev` and that its SHA-256 matches
  its build manifest, runs the [no-paid bundled-engine check](packaging.md#offline-workspace-engines)
  and extracts it with `dpkg-deb -x` (no maintainer scripts, no dpkg
  registration). The [extracted Dev startup contract](../configuration.md#workspace-dev-extracted-package)
  gives the run five private XDG roots and its own
  `run/cortex-workspace-dev/service.sock`; the extracted app cannot fall back to
  an installed unit, and `CORTEX_ROUTING` is unset. `GH_CONFIG_DIR` and
  `DBUS_SESSION_BUS_ADDRESS` point at the owner's real gh configuration and
  session bus, so logins and the keyring stay visible without copying anything.
- The display is a private Xvfb by default (authenticated with an X cookie when
  `xauth` exists), or the owner's display with `--real-display`. The service, Xvfb
  and desktop each run in their own process group, which teardown stops; the
  per-run scratch directory is removed and the archive and evidence are retained.
- `smoke` writes private smoke settings (Codex `gpt-6-luna` as the only enabled
  route and a matching portable coordinator default with `high` effort), waits for
  the real adapter's observed catalog and reads back the route, effort and
  default over the run's own socket, then clicks the window with XTEST and keeps a
  screenshot. It sends no prompt and makes no paid call. The GitHub CLI login is
  recorded as a fact, not a smoke requirement.
- `drive` additionally runs a trusted absolute foreground executable (an absolute
  interpreter and script are fine) for an explicitly authorized paid scenario,
  bounded to 40 minutes. It receives HOME, the private XDG values, DISPLAY,
  XAUTHORITY and `CWQ_PROJECTS` (allowed root), `CWQ_SOCKET`, `CWQ_WINDOW` (X11
  window ID) and `CWQ_EVIDENCE` (write sanitized artifacts here). Register only a
  disposable Git project under `CWQ_PROJECTS`, keep remote access off, pass no
  secrets in argv or evidence and leave no background processes. Before paying,
  the driver reads machine `config.read` on its own socket, checks the owner-selected
  light route and default (also in the rendered app) and checks native login by
  exit code. `--capture-opener` puts an `xdg-open` recorder first on the desktop's
  PATH: it records one SHA-256 per HTTPS argument in `opener.sha256` (compare them
  with an independent GitHub URL readback) and proves handoff, not navigation; an
  opener that bypasses PATH is not captured. Without it, external openers may open
  in the owner's browser.
- `install` installs the exact Dev archive with `sudo apt-get install` when a
  scenario needs installed-package behavior (desktop entry, the Dev user unit). It
  refuses any package other than `cortex-workspace-dev`. With `SUDO_ASKPASS` set
  (the owner password prompt) it uses `sudo -A`; otherwise `sudo -n`. Package
  installation never touches Dev or release user data.
- `cleanup` removes explicitly named local Dev build pairs; `cortex-xtask
  clean-builds` keeps the newest per channel.

**Environment problems are facts, not stops.** A missing display, tool or login
does not end the run: the launcher records it as an unobserved step, continues
with what it can observe (for example a service-only run without a display) and
exits 3 with the list. The task then adapts: use a private Xvfb or the owner's
display, install a missing tool (through the owner's password prompt when it needs
`sudo`), ask the owner to log in through the question route while other work
continues, or report the step as not run. Exit 1 is reserved for product or
archive failures (wrong package, hash mismatch, socket or window never appearing,
settings readback failing, the driver failing). An unobserved step is never a
pass: apply the [qualification evidence rules](../../AGENTS.md#qualification-evidence-discipline)
and bind results to the exact archive and session. A screenshot, rendered input,
socket evidence, native auth and scenario completion are separate checks.

The evidence directory under `.workspace-build/dev-evidence.*` holds the ledger
(commands, exit codes and output digests, never output bodies), the host facts,
the archive digest, the readiness readback and the screenshot. A desktop window
does not qualify phone-width behavior. This route is not the
[installed desktop real-input gate](#installed-desktop-real-input-gate), a
physical-device check or release acceptance.

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
npm --prefix integrations/paseo run test:process-runner # server/process-runner.test.ts
cargo test --locked --offline -p cortex-paseo-tools # Rust store readers and settings API: byte parity with the recorded outcomes of the former Python readers and management API (tests/parity-golden.json, tests/config-parity-golden.json; needs no Python)
npm --prefix integrations/paseo run test:store-reader # server/store-reader.test.ts: Rust-launch store reads and every Settings operation class (and native automatic setup) with PATH emptied; needs the cortex-paseo-tools build above
npm --prefix integrations/paseo run test:invoke-diagnostics # server/invoke-diagnostics.test.ts
npm --prefix integrations/paseo run test:worker-history # server/worker-history.test.ts
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
```

Select the observed installed bundle explicitly; report an absent/ambiguous
bundle or incompatible helper as a blocker, not a skipped pass.

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
scripts/cortex-paseo-tools.sh paseo-dev -- start --timeout 60 --json
scripts/cortex-paseo-tools.sh paseo-dev -- plugin install "$PWD/integrations/paseo" --json
scripts/cortex-paseo-tools.sh paseo-dev gui
```

In Paseo Desktop, inspect the rendered behavior under test. If the Cortex Settings screen is available, check workspace selection, supported setting sections, validation/conflict behavior, redacted values, and explicit OpenCode onboarding; if it is absent, record that outcome and do not report those interactions as tested. The 2026-09-29 Paseo 0.10.0 checks found the plugin screen and command absent, with the later check also inspecting screenshot pixels. GUI form behavior remains unverified because those runs could not reach the screen. For Xvfb use `scripts/cortex-paseo-tools.sh paseo-dev gui --xvfb`.
After testing, stop only the isolated dev daemon:

```
scripts/cortex-paseo-tools.sh paseo-dev -- daemon stop --timeout 15 --json
```

CLI access is secondary and supports diagnostics such as daemon health, plugin
registration and logs. Keep the plugin path explicit when installing it:

```
scripts/cortex-paseo-tools.sh paseo-dev -- status --json
scripts/cortex-paseo-tools.sh paseo-dev -- plugin ls --json
scripts/cortex-paseo-tools.sh paseo-dev -- logs <agent-id> --json
```

For retained-history checks, omit `--tail` and retain the exit status, output
byte count, structural event count, safe first/last event identities and any
explicit error. The 0.9.2 observation that `--tail`, `--filter` and `--since` existed without pagination must not be assumed to apply to 0.10.0. Check the installed `logs --help`; if completeness cannot be independently established, mark that check unverified. Do not store message bodies or raw logs
in repository documents or reports.

## Desktop build diagnostics

The desktop, production UI and prototype share bounded vendor/application chunks;
the normal Vite 500 kB threshold stays enabled, with source execution order preserved.
The pre-paint theme script remains a synchronous classic public asset; Vite resolves
its public-root URL to the configured relative production base.

The only npm output exception is the three exact locked React Native 0.81.5 toolchain deprecations (inflight 1.0.6, rimraf 3.0.2, glob 7.2.3): compatible replacements are unavailable in the offline cache.
`cortex-xtask npm-ci` matches complete notices against the lockfile,
prints an exception count, and preserves every other diagnostic and npm exit status;
it does not change dependencies, disable audits beyond the existing build flags,
or filter peer/version notices. Revisit the exception when upgrading the SDK/toolchain.

Dependency scanning stages both actual ELF files under a Debian package root
with its `DEBIAN` directory, rather than analyzing unstaged build outputs.
The only dpkg output exception is Ubuntu libc6's two merged-/usr loader-alias notices: the pinned dpkg emits them unconditionally and has no offline fix.
`cortex-xtask shlibdeps` applies that exact exception only after a
successful scan and only when the loader symlink resolves to the expected
canonical library; unknown notices, failed scans and derived dependencies remain intact.
Binary package metadata includes Section and Priority.

The shared dev/release builder restores package manpages before apt installation
by removing only Ubuntu's `/usr/share/man/*` dpkg exclusion. This supplies the real
xz and g++ alternatives targets instead of suppressing missing-target notices or
creating placeholders; the image build checks both targets are nonempty.

Retain the full build output and inspect it, not only an excerpt:
`grep -niE 'warn|warning' /absolute/path/to/build.log` must return no matches.
A zero-match build log is not an installed-package gate or live-engine qualification.

## Installed desktop real-input gate

Before handing a desktop archive to an owner, run this check against that exact
archive, sequentially, never on the owner's session:

```sh
scripts/cortex-xtask.sh installed-gate --archive /absolute/path/to/candidate.deb --prior-archive /absolute/path/to/previous-owner.deb --evidence .workspace-build/installed-new
# For a release-channel archive and the exact previous release archive:
scripts/cortex-xtask.sh installed-gate --channel release --archive /absolute/path/to/candidate-release.deb --prior-archive /absolute/path/to/previous-release.deb --evidence .workspace-build/installed-release-new
```

The gate options are `--channel dev|release` (default `dev`), `--display both|x11|wayland` (default `both`), `--close-cycles N` (1-10), `--native-engine FILE` and `--evidence DIR` (required, with `--archive` and `--prior-archive`); it runs through Docker only, and `scripts/cortex-xtask.sh check-real-input`, `check-install`, `check-systemd` and `check-bundled-engines` are its individually runnable container-side stages. The gate container has no `python3`, and the gate fails if one is present (after the image and after the install) unless the archive's `Depends` names `python3`; the evidence records `python_required_by_archive_depends`. The shipped archive depends on no Python. The same container build emits `native-fake-engine` beside the archive. The
gate requires that no-paid Rust peer (or an explicit `--native-engine` path),
retains its hash, and adds a native task journey in each clean/upgrade display
scenario: real-input create/Start to runtime-certified completion, then a second
task's Stop/Resume to completion. Native execution, settled processes and
contained checks are observed in that disposable project's store; the result
must also render. Exactly two distinct completed native task identities per
scenario are required. The peer is test-only and never part of the Debian payload.

The build-integrated equivalent is
`scripts/cortex-xtask.sh build-deb --check-installed --prior-archive /absolute/path/to/previous-owner.deb` (the `scripts/build-dev-deb.sh` and `scripts/build-prod-deb.sh` wrappers pass the same options).
It forwards the selected build channel, including official `--release` and local
`--local-release`; the latter remains non-official and does not gain publication
eligibility by passing the gate. Docker must be available without host sudo. The command creates privileged
disposable PID-1 systemd containers, with a lingering desktop user and private
buses, HOME, Xvfb/Openbox and a real headless Mutter compositor. No host home,
display, bus, cgroup filesystem or daemon socket is mounted. Container privilege
is not hostile-code isolation. Image preparation downloads runtime libraries;
the installed runs have no network and no real engine accounts.

Each display is checked after plain apt clean install and plain apt upgrade from
the supplied prior archive (no forced downgrade or same-version reinstall).
Three project identities, including an unreadable-store fixture, are seeded under
the prior archive before its user-manager restart; upgrade must preserve their registry bytes and display
all three without re-registration. Clean install seeds the same visibility fixture.

A separate synthetic orchestration fixture is seeded **before** the prior install
(or before the candidate clean install): `routing.json`, `defaults.json` and
`routing-profiles.json`. It has six saved profiles, `default` active, Russian message
language and a synthetic exact coordinator route with `thought_level: medium`.
There are no real credentials, native accounts or model calls. Byte equality and
parsed JSON equality are required after the prior install, before upgrade, after
the candidate install/upgrade, after the first GUI launch, and after launching with
the unreadable-project fixture (prior-only stages do not apply to clean installs).
Per-file byte and canonical-JSON SHA-256 values and stage identities are retained;
missing, duplicate, wrong-channel or changed evidence fails acceptance.
Through real pointer input, Settings must show all six profiles with `default`
active and the saved coordinator model and effort, both on first launch and with
the unreadable project present. `default` is active when the machine's profile
picker reads `Profile: default`, every profile row has its own actions button, and
exactly the five other rows offer `Activate…`. WebKitGTK exposes the plain `Active`
row label only as text of a non-showing ancestor, never as its own accessible
object, so the gate does not look for it. These are rendered accessibility checks,
not merely successful config RPCs or preserved files. After these checks only the
synthetic files are removed to restore the empty test account for unrelated
fake-engine journeys; that deliberate fixture transition is retained in output.

Before the interaction journey, the installed desktop starts from a fresh GUI
profile with its service stopped, then starts again with a container-only
`ExecStartPre=/usr/bin/sleep 10` override. The latter must observe systemd's
`start-pre` state. Sampled accessibility must show neither a connection alert nor
Retry before automatic readiness; this is not proof against sub-sample paint flicker.
A separate populated-store launch uses 68 saved tasks, a 31-member sprint and
100,000 retained history rows. Fresh and cached-profile launches must show sprint
task cards without loading/unavailable banners; the cached launch must show saved
tasks while a ten-second service-start delay is demonstrably active. The observed
visible task-link count is retained separately from the fixture\'s 31-member
membership; fixture size alone is not evidence of rendered task cards. Both
startup cases reject the four loading/missing-detail/queue messages as well as
connection alerts.
Sidebar assertions use its named Workspace navigation landmark, not Home's separate
Inbox link, and require it visible before hiding and restored after the next click.
The installed `.desktop` entry launches the actual packaged application.
Accessibility supplies text and geometry only: clicks use XTEST or Mutter's
observed RemoteDesktop pointer API, not DOM events, preload probes or accessible
actions. The saved-plan-with-unloaded-tasks regression must leave Inbox, Archive,
Settings and the sidebar usable. A separate fake-engine journey checks project
registration/activation, description-only creation, real keyboard typing and OS
clipboard text paste, an external GTK file drag through hit-testing, start, the
visible coordinator question, opening a synthetic worker, Stop/Resume (Start,
Stop and Resume are the action button of the group named Task composer; each
press must show the exact opposite button there, the only button of that name in
the window, so matching text or a stale control elsewhere never counts),
result/review, every current Settings navigation page and sidebar toggles.
Choosing an answer only selects it; the journey submits the answer form with its
one Send answer control and requires that control to leave (the form hides it
from the press) and the answer to be recorded in the store, not question text
also present in the transcript. Its fixture asks
one question; the one-form batch with its single Send answers control is covered
by the Workspace UI tests, not by this gate.
The Activate control and a directly inserted worker's Work details target must
retain exact text and rectangles throughout stationary-pointer sampling, including
a question-answer reconciliation. Samples retain baseline/current rectangles and
monotonic timestamps; a missing target or any movement fails immediately. The
worker must reach the window before navigation: the task bar's Work details link
must count every worker the store records for the task (one per runtime node
with a delegation provisioning record, as the service lists them), so the
bare label that stands before any worker arrives never satisfies it. Opening a task must expose its
Task progress and details panel, not merely a Start button on the board. Before replacing a fixture after a deliberate stop,
the harness requires an inactive unit with `Result=success`, then clears only that
disposable unit\'s start counter. This separates fixture restarts from the shipped
five-starts-per-minute protection; a failed stop is never cleared or accepted.
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
The gate supports Docker and both Debian channels. `--channel dev` is the default
and requires both archives to be `cortex-workspace-dev`; `--channel release` requires
both to be `cortex-workspace`. Mixing channels fails before container launch.
The release selection uses `.cortex` project stores, the `cortex-workspace` service
and socket namespace, `~/.config/cortex` configuration and `~/.local/state/cortex`
engine/catalog state; the project registry remains under
`~/.local/state/cortex-workspace`. Dev retains `.cortex-dev`, its `-dev` service/socket,
`~/.config/cortex-workspace-dev/cortex` configuration and
`~/.local/state/cortex-workspace-dev/state/cortex` engine/catalog state.
All paths are relative to the disposable container user, never the owner.
Supporting a channel is not evidence that an archive passed: the exact candidate
and prior hashes, channel and complete four-scenario result must still be retained.
Existing preload/DOM-driven audits remain supplemental, never a substitute for this
installed real-input gate.


## Cortex Workspace local slice checks (no-paid, plus container)

Focused task-start, busy-state, API, scheduler and service-integration regressions use fake dispatch, fake engines and disposable stores only: distinct tasks in one project start independently, an already-started task stays busy, and crashed starts stay unknown without replay or blocking other tasks. They are Rust tests in `workspace-service` (for example `tests/service.rs`, `tests/native_desktop.rs`, `tests/native_sessions.rs`, `tests/task_actions.rs`) and `workspace-protocol`, plus the Vitest suites of `packages/workspace-domain` and `packages/workspace-ui` and `tests/workspace-busy.test.ts`. They are source/protocol checks, not rendered desktop qualification. Install the checkout-local test dependencies first (`npm --prefix packages/workspace-ui ci` plus `npm --prefix packages/workspace-domain ci`). The service integration tests launch the debug `workspace-service` binary, so build it first:

```
umask 022
timeout 900 cargo build --locked --offline -p workspace-service
timeout 1500 cargo test --locked --offline -p workspace-service
```

The Cortex Workspace local slice is documented in [Cortex Workspace](../cortex-workspace/index.md). Its checks are no-paid: they use fake engines and the scripted protocol peer, and they do not use a production Paseo installation or a production Cortex store. Run them sequentially in the same checkout, after `scripts/cortex-xtask.sh package stamp` when a package payload changed.

```
scripts/cortex-xtask.sh check-pins
scripts/cortex-xtask.sh generate-contract check
node packages/workspace-contract/test.mjs
cargo test --locked
npm --prefix packages/workspace-domain run typecheck
npm --prefix packages/workspace-domain run test
npm --prefix packages/workspace-ui run typecheck
npm --prefix packages/workspace-ui run test
npm --prefix packages/workspace-ui run build
npm --prefix apps/cortex-workspace-desktop run typecheck
npm --prefix apps/cortex-workspace-desktop run test
npm --prefix prototypes/cortex-workspace/desktop run test
scripts/cortex-xtask.sh package check
scripts/cortex-xtask.sh profiles check
```

Remote access and the Android app add these no-paid targeted checks (loopback only: no relay, no device, no emulator):

```
umask 022 && timeout 900 cargo test --locked --offline -p workspace-remote -p workspace-client -p cortex-workspace-android
umask 022 && timeout 900 cargo test --locked --offline -p workspace-service --test remote
umask 022 && timeout 900 cargo test --locked --offline -p workspace-service --lib remote_policy
npm --prefix apps/cortex-workspace-android run typecheck
npm --prefix apps/cortex-workspace-android run test
```

They cover pairing, the tunnel, refusals, revoke, restart, private-file permissions and the link format; the closed remote method policy and one end-to-end run through a real service on a temporary socket; both request-policy profiles; the phone and Remote access interface tests (`npm --prefix packages/workspace-ui run test`) and the app's shell test. `workspace-remote` and `workspace-client` also cross-compile for `aarch64-linux-android`, and a debug APK of the app was built on the development machine. Not verified: anything on a physical Android device or emulator (install, launch, camera and QR scan, touch, keyboard, Back, lifecycle, network changes), a connection through a relay, pairing between a real phone and a real desktop, the rendered layout gates for the phone markup, the Android job of the release workflow (never run) and the Remote access page in the native desktop window. These are not release gates.

`scripts/cortex-xtask.sh test-fast` (see [test matrix](test-matrix.md)) runs the bounded no-paid cargo, Vitest and Paseo suites sequentially in one command.

- `cargo test --locked` covers the default member, the headless protocol crate. Focused service checks use `cargo test --locked -p workspace-service`; headless native bridge checks use `cargo test --locked -p cortex-workspace-desktop --no-default-features`. Full native-feature tests require the Docker builder, not host development packages.
- `scripts/cortex-xtask.sh build-deb --test-workspace` (or the `scripts/build-dev-deb.sh` wrapper with the same option) builds the `.deb` for the dev channel in the Docker builder image and runs the locked Cargo workspace tests there. It requires Docker usable without sudo and creates artifacts under `.workspace-build/`. `--release` requires a GitHub Actions manual workflow on clean `main`; `--ci-dev` requires the same on clean `dev`. Local builds default to dev. Explicit owner-run `./scripts/build-prod-deb.sh --check-install` selects `--local-release`, accepts `local-owner-test` at the Rust and packaging guards, and produces a non-official production-channel archive under `.workspace-build/local-prod/build-*/`. Its version ends in `.local` and cannot pass the GitHub publisher's version check. The install smoke is not release-channel installed desktop acceptance; that handoff remains unqualified.
- `scripts/cortex-xtask.sh build-deb --check-install` also runs `scripts/cortex-xtask.sh check-install --artifacts <build>/artifacts` in a clean Ubuntu 26.04 container without build tools or a checkout. It installs runtime dependencies through apt, runs the bundled unit ExecStart as a normal user, checks activation and data-preserving removal, and rehearses upgrading the former archive shape. It attempts a user manager and app-driven start; an unavailable manager and denied namespaces are explicitly unverified, never a pass. Synthetic adapter discovery stubs installation/model refresh and is not live engine qualification.
- The native fake-engine journey and visual audit run in a disposable no-network Docker container through `scripts/cortex-xtask.sh check-rendered --artifacts <build>/artifacts`, after a Docker build. `<build>/artifacts` is the build's artifact directory: it holds `cortex-workspace-dev`, `cortex-workspace-service-dev` and `native-fake-engine` (the build container emits the last beside the archive) and optionally `voice-model/` and `voice-fixture.wav`. The default `--display x11` runs the whole journey on a private Xvfb with Openbox and then a separate native Wayland frame and control stage; `--display wayland` runs the whole journey on headless GNOME Mutter instead (the `--journey-wayland yes` spelling is also accepted). `--only common|lifecycle|controls|wayland|terminal|voice|attachments` runs one stage, which is for iteration and is never a complete series run; `--build-image no` reuses the image and `--image` names another tag. The harness is the `cortex-xtask` binary itself, copied into the container, so the container needs no interpreter; it runs `--cap-drop=ALL` with seccomp and AppArmor unconfined and `--security-opt=systempaths=unconfined` (the worker sandbox mounts `--proc /proc`), mounts the artifacts and the checkout read-only, and has no network.
- Inside, the harness makes a disposable account (home, config, state and runtime directories under a scratch tree, a private session bus), writes the same engine connection, defaults, routing and catalog files the installed gate writes so the no-paid peer is the only engine, starts the native service with the two-argument form (`<service> <home> <checkout>`) and launches the unmodified desktop binary with a preload compiled in the image, which adds a request file to its web view: computed styles and rectangles are read from the real page and the product carries no inspector. Keyboard and pointer input is XTEST through xdotool on X11 and Mutter's remote-desktop session on Wayland, and controls are found in the accessibility tree of the exact process group the harness spawned. The stages are: the common screens with no project in dark and light, window controls and edges (X11 pointer; Wayland through its own stage), project registration through the add-project dialog and its inertness (only `.cortex-dev/owner.json`), activation, task form and draft, a second project, task start, the coordinator question and a multi-line answer, reopening during the question, the result, requesting changes and the second result, a held worker (streaming survives a window close, queue, edit and steer), worker, command and guidance controls, the project views, a Git refusal that stays readable in its form, the offline and disconnected screens, a terminal on a real PTY (resize, theme, retained across a window close, reaped), attachments and the voice round trip. Every screen is audited in the design font (`Noto Sans Variable` for the interface, `Cousine` for code), against the theme surface and the title-band geometry, for unstyled native controls, strong shadows, dashes standing for data, clipped control text and plain icons. Any regression exits 1.
- The attachment stage runs on its own `attachments` project and window and drives the New task composer with real OS transfer on either display. File drop: a separate GTK window (`cortex-xtask installed-drag-source`, the installed gate's drag source) offers a fixture file as `text/uri-list`, and the real pointer drags it onto the composer; on Wayland Mutter's own Super+Right and Super+Left tiling places the app and the source side by side, because Wayland has no client positioning. The drop target is calibrated by where the page reports the real pointer landed. Clipboard: an independent owner (`wl-copy --type image/png` on Wayland, `xclip -target image/png` on X11) holds a generated PNG, which is read back byte for byte before the real Ctrl+V; the composer must show an image chip. File picker: a real pointer press on Attach files must open the system file chooser in the owned app's accessibility tree, driven with the real keyboard (Ctrl+L, the path, Return); an enabled Attach files that opens no chooser is a failure. Each surface must leave the typed description unchanged. The draft is then saved, the task page must read every file back as an openable chip, and Start must deliver them: the no-paid peer records each prompt it receives while the project holds `record-prompts`, and the coordinator's prompt must carry the exact PNG as an `image/png` block and each text file's body. The project store must retain each file in `source_blocks` by name, SHA-256 and size, and no runtime event may carry the image's bytes. The peer's recorder writes into the project folder and treats a failed write as fatal, while the reviewer works there read-only, so the harness removes `record-prompts` once the coordinator's prompt is in hand; the task must then reach `COMPLETED`. A page that offers attachments where any of this fails is a failure.
- A host capability that is absent is `not_applicable` with the page's own words, never a pass: task worktrees ("A separate worktree per task is not available yet. Tasks work in the project folder."), attachments when the host reports them unavailable ("Attachments are not available on this machine."), and the voice round trip when the artifacts carry no `voice-model/` or `voice-fixture.wav`. Each run records how the desktop process, its WebKit web process and the compositor ended: exit code or signal, stderr reduced to marker counts, and any core by name and size. A real death of any of them is a hard failure. Each requested window close logs exactly one `terminated_by_api` (the product ends the web process on close): the harness counts its requested closes, accepts exactly that many `terminated_by_api` records and nothing else, and reports a missing, extra or differently-caused termination as `web_process_terminated`. The only tolerated accessibility condition is a node the page replaced while every process is alive and the page answers; the action is repeated and logged as `accessibility_node_stale`.
- `scripts/cortex-xtask.sh journey-series --artifacts <build>/artifacts --logs <directory> --runs 5` runs the Wayland journey one run at a time and prints each result on one line; `--display x11` runs the X11 journey. A series run is complete only with exit 0, a valid binary SHA-256, at least one passing screen audit, and exactly one consistent native-evidence/visual-summary/cleanup sequence. Failed or unfinished steps, missing observations, duplicate terminal records, contradictory native evidence and truncated JSON records block completion; `evidence_errors` explains rejected evidence and `not_applicable` lists the steps the host could not offer. Add `--expect-sha256 <digest>` to bind the run to an expected desktop binary. Outcomes are text only; the harness captures no screenshot. It does not replace the owner's review of an installed window. Its workers come from the no-paid native peer's own scenarios (`hold`, `guidance`, `stop-siblings`, `command-restart-1`, `reconcile-question`) through the real service, so it exercises the screens and the owner's controls over real worker processes, not a real engine's delegation.
- `npm --prefix prototypes/cortex-workspace/desktop test` also presses every enabled control of the richest fixture, with exactly the capabilities a real machine offers, and fails on one that changes nothing, sends nothing and goes nowhere. It leaves out menu items, what is already chosen, drag grips and copy buttons, which a test document cannot observe.
- Rendered visual QA tools (no Python): the container image `cortex-workspace-rendered:ubuntu26.04` is built by `cortex-xtask rendered-image` (builder stage compiles the C window host and the request-file probe; the final stage is the plain Ubuntu base with WebKitGTK runtime libraries, Xvfb, Openbox, Mutter, xdotool, the AT-SPI registry, bubblewrap, git, PulseAudio, xclip, wl-clipboard, sqlite3, a session bus and the builder's Node binary; no interpreter). Drivers: `layout-fit`, `feed-scroll`, `feed-visual`, `pages-shots`, `conversation-shots`, `tabs-visual`, `sprint-graph-render`, `scroll-perf`, `webkit-cpu` and `webkit-performance` run through their `<name>.mjs` launcher and `<name>.driver.mjs`; `flow.measure.mjs`, `focus.measure.mjs` and `sweep.measure.mjs` (prototype bundle, `--network none`) and `timeline-clarity.render.mjs` are self-contained. `cortex-xtask rendered-scrollbar <label>` (inside the rendered image, with the built artifacts) grabs the scrollbar and window edge of the real desktop window with real pointer input; `cortex-xtask check-rendered --artifacts <build>/artifacts` runs the complete owner journey and visual audit against the built binary and `cortex-xtask journey-series` repeats it with the series-evidence rules above. Set `CW_RENDERED_IMAGE` to run a launcher in another image tag.- Rendered layout geometry: `node packages/workspace-ui/test/layout-fit.mjs <label>` (from the checkout, with the `packages/workspace-ui` dependencies installed, Docker usable without sudo, and the `cortex-workspace-rendered:ubuntu26.04` image built) builds the workspace bundle with Vite, serves it with fake data, and runs `packages/workspace-ui/test/layout-fit.driver.mjs` (Node, over the C WebKitGTK window `packages/workspace-ui/test/rendered/webkit-host.c`) in that container (`--network host`) under a private Xvfb with WebKitGTK. Sizes come from the app: 1280×800 and 1920×1080 for the owner's screens, 960×540, 1024×768, 1280×800 and 1872×1100 for the screens held to the full rules, and the window minimum read from `min_inner_size` in `apps/cortex-workspace-desktop/src-tauri/src/desktop.rs` (960×540), with the sidebar dragged to `SIDEBAR_MAX` in `packages/workspace-ui/src/prefs.ts`. A run exits nonzero when a listed screen overflows a bar, wraps a row or label, clips or squeezes text, narrows a pane below its minimum, scrolls sideways, or moves a control when it becomes enabled or disabled while its reason slot does not change. It also fails if an injected fault goes unnoticed. Evidence goes to the git-ignored `.design-evidence/layout-rebuild/<label>`. The gate proves rendered geometry in WebKitGTK on fake data at those widths. It does not prove pointer or keyboard input (see the installed real-input gate), Wayland, colour or visual quality, or any screen outside its list.
- The prototype suite (`npm --prefix prototypes/cortex-workspace/desktop run test`) does not pass. On 2026-11-10 a clean export of commit `8bb66c54` and the working tree both reported 67 failed and 475 passed of 542 tests, with 7 of 19 files failing. On 2026-11-16 the working tree (last commit `2d9c1d87`; a clean export was not re-measured) reported 124 failed and 422 passed of 546 tests, with 7 of 20 files failing: `capability-sweep` (35), `session` (32), `composer-fit` (14), `ui` (13), `phase4` (13), `review` (11) and `orchestration` (6). The added failures include the mockup token identity test, because the shared `packages/workspace-ui` tokens now lead with the bundled Noto Sans and Cousine faces while the prototype mockups do not. Two layout-era expectations in `review.test.tsx` were updated to the approved review layout, and the mockup identity tests assert token identity only. The command exits 1 until these failures are resolved, so it is not a passing check.
- With explicit authorization for a privileged disposable container only, `scripts/cortex-xtask.sh check-systemd --artifacts <build>/artifacts --prior-archive <earlier-dev.deb>` exercises real PID-1 systemd, a lingering disposable user, default-target login startup, desktop-driven service start, active-unit restart on reinstall, removal stop and upgrade from the exact prior archive. With `--prior-archive`, a project registered and activated under the prior archive must report ready again after the plain apt upgrade, after a unit restart and after a user-manager restart (login), with only status reads and no new activation request; a refusal reason fails the check. It mounts no host home, bus, credentials, cgroups or Docker socket. It is opt-in, never part of the ordinary no-paid `test-fast` run, and is not hostile-code isolation. The 2026-11-06 rerun passed against the fresh ordered-version dev archive and the earlier `1.0.6+sha256.f3eee9da173725e1` archive. The harness uses ordinary `apt install` for replacement and fails if it reports a downgrade or does not report one upgrade; only the separate same-version reinstall uses `--reinstall`.
- `scripts/cortex-xtask.sh check-bundled-engines --archive <package.deb>` preinstalls only system runtime dependencies in its image, then installs the archive with `--network none`. It checks real bundled adapters/Node, honest absent versus synthetic login markers, ACP initialize, native versions and unchanged root-owned bundle hashes; it sends no model prompt and does not authenticate a real account.
- Namespace qualification must name its execution context. On 2026-11-06 the nested worker probe failed under enforcing `bwrap//&unpriv_bwrap`, while an authorized host-direct probe passed with Ubuntu's restriction switch still enabled. The host-direct focused run of the then-current Python discovery and sandbox tests (since removed) passed all ten tests with real namespaces; the Rust namespace checks are in "Native command namespace checks" above. No policy change or package AppArmor exception was made.
- The earlier 2026-11-06 full journey failed at `field_focus_failed:Message backend-dev` with a Request changes dialog still open. After repair, a fresh dev Docker build with workspace tests and install smoke passed; its complete X11 and native Wayland common journeys passed with 118/114 screen audits and zero failures. Both reached Result v2, closed the review dialog, exercised worker guidance/history and command/worker stop, and completed offline/reopen screens. The independent terminal and attachment picker/paste/drop/queue-edit stages also passed. Real-systemd plain apt replacement, offline adapter/login-state initialization, and the ten focused MCP/sandbox fixtures against the extracted archive passed. This is not real model, physical-desktop or all-planned-functionality qualification. See [platform limits](../cortex-workspace/platforms.md).
- Native journey result, 2026-11-16: against a fresh dev build (desktop binary SHA-256 `123c7a2e317d210d9f1afabcfd88cefb15b8bda1d6db561648b89b75cefab0ea`) the complete journey ran to its end on X11 (120 screen audits) and on headless Mutter (116 screen audits). No screen audit failed, every process ended as expected, no core was left, and every requested window close (11 on X11, 10 on Wayland) logged exactly one `terminated_by_api`. Both runs exit 1 for the same three failing steps, which are product behavior and are not loosened: `review_dialog_closed` (after Request changes and Send and reopen the focus lands on the page body, and the button that opened the dialog is no longer in the page), `worker_stop_did_not_complete` (the Stop worker dialog stays open for 15 seconds with no refusal shown, no request recorded in the store and the worker still running, with a real pointer click and with the accessibility action, on a held worker and on `stop-siblings`) and `worker_command_stop_not_offered` (a command a worker runs through the gateway is alive, with its start recorded in the store, while the worker page says "No activity recorded yet." and offers no Stop command). `not_applicable`: task worktrees, attachments, and the voice round trip (the build's artifact directory has no `voice-model/` and no `voice-fixture.wav`). The harness previously reported the same recorded `terminated_by_api` per close as a failure; that count is now accepted only per requested close. The page's interface face is Noto Sans Variable and its code face Cousine; the audit that found the check-row command text in the system's bare `monospace` led to the CSS fix in `packages/workspace-ui/src/workspace.css`.
- Not run or not verified: the different-OS-user peer check of the service, a physical desktop/GPU or microphone baseline, real account/model workloads, the GitHub release workflow (manual runs only, production on `main` and dev prereleases on `dev`), and a direct host native build, which is not a supported route. Record each concrete blocker; do not install development packages on the host to run it.


## What automated evidence cannot close

The no-paid automated suite cannot reach a paid engine and does not establish rendered GUI behavior. The settings screen's client-bundle render-walk uses compiled UI plus host shims; it is not a visual Paseo Desktop result. The UI test coverage also includes `client/settings-model.test.ts` (run with `node --experimental-strip-types integrations/paseo/client/settings-model.test.ts`) and the config RPC end-to-end test. Use isolated dev Paseo for applicable live checks. A rendered desktop check must inspect the actual Settings screen and relevant behaviors (workspace selection, read/update, conflicts/redaction, and OpenCode opt-in) in the Electron app; CLI, logs, simulated render-walk, or CDP alone are not GUI evidence. Each probe or GUI observation supports only the exact surface exercised.


## Settings-only routing and profile deletion checks

For routing/settings changes, run the applicable focused Rust tests (`cargo test -p workspace-service --test routing_management --test settings_view --test defaults_profiles`, `cargo test -p cortex-paseo-tools`) and the complete unfiltered routing/profile integration tests. For Paseo settings changes run the documented typecheck and the complete config/settings scripts, not a filtered pass. Automated tests do not prove production migration or installed GUI reachability. Production migration is a separately authorized config-only operation: inspect current revision, preview candidate, review preservation/diff and digests, obtain consent, apply against that exact revision, then verify resulting settings and backup/rollback evidence. Never mutate production during package verification. Paid qualification is not requested by default and, if requested, requires a named mode and explicit owner-selected route; do not silently select a substitute.

Quota settings changes additionally run the complete unfiltered `workspace-service` and `cortex-core` suites. They cover exact arbitrary identities/labels, conservative missing/unmapped mappings, ambiguity refusal before IO, project precedence, shuffled catalogs, menu/selection agreement, expiry, explicit candidate migration and snapshot round-trip. Keep existing authentication/CLI zero-cost/TTL/negative-cache tests; no global owner-template injection, skips or deselection. Run the documented plugin `ci`, `typecheck`, `test:config`, `test:settings` and `test:client-bundle` sequentially for typed quota controls. Package payload stamping and final whole-suite verification remain separate downstream steps; these source/component checks establish neither live quota evidence nor rendered host reachability.

## Isolated test environment

`test-fast` already runs every step under a private scratch `HOME`/`XDG_*`. For a
targeted command, create private owned mode-0700 scratch `home`, `config`, `state`,
`cache` and `tmp` directories outside every Git repository, set `HOME`,
`XDG_CONFIG_HOME`, `XDG_STATE_HOME`, `XDG_CACHE_HOME` and `TMPDIR` to them before
starting the process, and run it under `timeout`. The former Python launcher
(`cortex_test_isolated`) and the qualification-binding checks that depended on it
were removed. Do not change `HOME` after a process has started. After all edits,
sequentially stamp, check the package (`scripts/cortex-xtask.sh package check`), check
profiles and run the affected suite. Runtime independent review and current-artifact
final verification remain separate from these worker checks.

## Executable check declarations and Review

Use fake engines and disposable stores for this slice. Run focused commands sequentially:

```bash
umask 022 && timeout 900 cargo test --locked --offline -p cortex-core --lib worker_checks
umask 022 && timeout 900 cargo test --locked --offline -p workspace-service --lib recovery_tests
umask 022 && timeout 900 cargo test --locked --offline -p workspace-service --lib evidence_tests
npm --prefix packages/workspace-ui run typecheck
npm --prefix packages/workspace-ui test -- test/owner-review-flow.test.tsx test/real-port.test.ts
```

The service journeys need a current debug `workspace-service` binary. Availability probes use the same verification launch seam, not a host-side PATH guess; a probe is never a test result. Coverage includes missing programs, literal wrapper arguments, project-private tools, legacy manual items, policy exclusions, exact reruns, output, timeout, stale source and incomplete evidence. These fixture checks do not establish installed-desktop rendering, real screen-reader behaviour or paid engine qualification.

## Orchestration timing and developer wire-trace checks

The timeline (`cortex_timeline`), trace viewer (`cortex_trace_view`) and developer
wire-trace (`paseo-dev --wire-trace`) tools were Python-era diagnostics. They were
removed on 2026-11-16 without replacement, so orchestration timing and wire traces
are not observable through a supported tool today; report such a check as not run
with that blocker instead of inferring a result. The Rust scheduler, completion and
gateway suites (`cargo test -p cortex-core`, `-p cortex-engine`, `-p workspace-service`)
remain the no-paid regressions for early review, digest caching and speculative
verification.

After documentation edits, regenerate the package hash and check it sequentially:

```bash
scripts/cortex-xtask.sh package stamp
scripts/cortex-xtask.sh package check
umask 022 && timeout 1500 cargo test --locked --offline -p cortex-xtask
```
