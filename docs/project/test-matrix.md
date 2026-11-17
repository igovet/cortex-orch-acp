# Automated test matrix and CI commands

> Status: migrated from the former ACP reference and rewritten for the Rust-only product on 2026-11-16. The Python test tree (`tests/test_*.py`, `conftest.py`, `pytest.ini`, `.venv`) was removed; Rust, Node and shell checks remain. Claims without a source path are unverified until reviewed.

This page documents the no-paid automated suite and its limits. Paid live checks run only on an explicit owner request naming `cli` or `desktop`, through the isolated dev Paseo environment documented separately. See [verification](verification.md) for per-surface commands.

The suites cover settings-only routing, exact model-policy validation and profile management alongside deterministic DAG scheduling, authenticated file/command/external MCP gateways, mandatory review, exact revision verification, steering, bounded recovery and unfiltered transcript replay. Historical paid qualification used the then-configured routes in an isolated test project; it is not evidence for current settings or a production route. The paid probes were not used in that campaign.

## Where the tests are

| Surface | Location | Command |
| --- | --- | --- |
| Store, lifecycle, scheduler, routing, recovery, publication | `crates/cortex-core/tests`, unit tests in `crates/cortex-core/src` | `cargo test --locked --offline -p cortex-core` |
| Engine sessions, MCP gateway, permissions, tool schemas, instruction catalogue | `crates/cortex-engine/tests` | `cargo test --locked --offline -p cortex-engine` |
| ACP ingress: startup, schema, incoming journeys, Codex runtime and trust | `crates/cortex-acp/tests` | `cargo test --locked --offline -p cortex-acp` |
| Native host, settings, catalogue, routing, desktop journeys | `crates/workspace-service/tests` | `cargo test --locked --offline -p workspace-service` |
| Workspace wire contract | `crates/workspace-protocol/tests`, `packages/workspace-contract/test.mjs` | `cargo test --locked --offline -p workspace-protocol`, `node packages/workspace-contract/test.mjs` |
| Paseo store readers, Settings API, setup, channel settings | `crates/cortex-paseo-tools/tests` | `cargo test --locked --offline -p cortex-paseo-tools` |
| Generators, package identity, runtime payload, markdown links | `crates/cortex-xtask/tests/repo.rs` | `cargo test --locked --offline -p cortex-xtask` |
| Paseo plugin | `integrations/paseo/server`, `integrations/paseo/client` | `npm --prefix integrations/paseo run typecheck` and the `test:*` scripts |
| Workspace UI and domain | `packages/workspace-ui/test`, `packages/workspace-domain/test`, `apps/cortex-workspace-desktop/test` | `npm --prefix <dir> run test` |
| Workspace busy state | `tests/workspace-busy.test.ts` | its Vitest config `tests/workspace-busy.vitest.config.ts` |

Run Rust tests under `umask 022` with `~/.cargo/bin/cargo`, `--locked --offline`, GNU `timeout` and sequentially.

The broker suite forces a random token whose first byte is `-`, then checks that the stdio relay still connects and advertises tools. It also checks an MCP child that exits before `tools/list` and prompt-level error classification.

Focused no-paid checks for routing and delegation are the `cortex-core` routing/scheduler tests, the `workspace-service` `routing_management`, `native_routes`, `settings_view`, `defaults_profiles` and `mcp_management` tests, and the `cortex-engine` `instruction_catalogue` and `gateway` tests. They cover configured grants versus session attachments, role/engine filtering, bounded sanitized summaries, composed prompts, source-bound exact model and effort, owner pins, policy/support conflicts before worker prompts, and plan/retry/fix propagation, using fake engines. Scripted reasoning does not establish real-model delegation choices, and none of these establishes rendered settings UI behavior, production migration or a paid engine result. Positive fixtures must write the minimum exact owner policy into disposable project or HOME/XDG roots; no global production template or unknown-model enablement is permitted.

The separate namespace visibility cases require working user namespaces and remain explicit fail-closed outcomes when unavailable; fixtures are not relocated to pass (see [verification](verification.md#native-command-namespace-checks)).

## Local plugin prerequisites

Before the default suite, run sequentially:

```
npm --prefix integrations/paseo ci
npm --prefix integrations/paseo run typecheck
```

The local fake endpoint needs the locked `ws` and `@getpaseo/protocol/messages` imports. Plugin test scripts require Node support for `--experimental-strip-types`. Missing startup output is a failure, never a successful binding or a skip; see [verification sequencing](verification.md#sequencing).

## Focused Paseo plugin checks

Run these no-paid scripts after the local prerequisites above:

```
npm --prefix integrations/paseo run test:display
npm --prefix integrations/paseo run test:terminal-status
npm --prefix integrations/paseo run test:command-stop
```

Select `test:display` for worker display, assignment-prompt and activity models; `test:terminal-status` for terminal outcome reconciliation; and `test:command-stop` for command list/stop RPC and client state. They are not rendered GUI checks.

The Paseo store readers (worker history, worker results, command-stop, pipeline, project root, task binding) are Rust: `crates/cortex-paseo-tools` serves them for the `cortex-acp` launch, and `server/store-reader.ts` runs it. After changing either side, run both, sequentially:

```
timeout 900 cargo test --locked --offline -p cortex-paseo-tools
npm --prefix integrations/paseo run test:store-reader
```

The cargo parity tests compare the Rust binary byte-for-byte with golden outcomes recorded once from the former Python readers and Python management API (`tests/parity-golden.json`, `tests/config-parity-golden.json`) on fixture format-12 stores and fixture homes. The test needs no Python, and the oracle can no longer be re-run. `tests/config_parity.rs` (with `tests/config_parity/workflows.rs`) covers each read, request-shape refusal and write workflow of `cortex-paseo-tools config` (defaults, routing, MCP servers, connections, profiles, default preset, project ceiling, catalogue refresh, discovery and onboarding: successes, read-back, stale revisions and validation refusals); only the two native message texts declared by the reused producers' fixtures are accepted as differences, and only timestamps (plus revisions derived from timestamped documents) are normalized. It also checks that the documented native limits refuse with `config_api_unavailable` and change no file. `test:store-reader` needs the `cortex-paseo-tools` debug build the cargo test leaves in `target/debug`, and additionally drives each operation class through the real plugin handlers with `PATH` emptied.

For native queue/edit/send/retry or provider-delivery changes, discover the installed Metro bundle read-only, then supply that exact path:

```
find /opt/Paseo/resources/app-dist/_expo/static/js/web -maxdepth 1 -name 'index-*.js' -print
npm --prefix integrations/paseo run test:native-queue -- /absolute/path/from/discovery/index-HASH.js
```

An absent, ambiguous or incompatible bundle is a blocker, not a skipped pass. This executes selected installed helpers with stand-ins and the Cortex provider; it does not launch or modify Paseo, prove pixels/focus, or make a paid call. See the [complete plugin inventory](verification.md#paseo-plugin-integrationspaseo) for the other scripts and the installed-question component check.

## The non-paid default command

Run the fixed entry point from the checkout (also supported when tracked files are read-only):

```
scripts/cortex-xtask.sh test-fast
scripts/cortex-xtask.sh test-fast --list
```

`test-fast` is the Rust `cortex-xtask` command. It runs its steps one at a time under GNU `timeout --kill-after=5`; no step runs in parallel with another:

| Step | Directory | Command | Bound |
| --- | --- | --- | --- |
| `build` | checkout | `cargo build --locked --offline -p cortex-acp -p cortex-paseo-tools -p workspace-service` | 900 s |
| `cargo` | checkout | `cargo test --locked --offline` for `workspace-protocol`, `cortex-core`, `cortex-engine`, `workspace-service`, `cortex-acp`, `cortex-paseo-tools`, `cortex-xtask` | 900 s |
| `desktop` | checkout | `cargo test --locked --offline -p cortex-workspace-desktop --no-default-features` | 900 s |
| `ui` | `packages/workspace-ui` | `npx --no-install vitest run --testTimeout=60000` | 600 s |
| `desktop-ui` | `apps/cortex-workspace-desktop` | `npx --no-install vitest run --testTimeout=60000` | 600 s |
| `paseo-typecheck` | `integrations/paseo` | `npm run typecheck` | 600 s |
| `paseo-<name>` | `integrations/paseo` | one per `test:*` script in `package.json`, except `test:installed-questions` and `test:native-queue`, which need the explicitly supplied installed Metro bundle path described above | 180 s each |

`--only NAMES` and `--skip NAMES` take comma-separated step names; an unknown name fails. Any nonzero step makes the final exit nonzero, while later steps still run. The runner points `HOME` and `XDG_CONFIG_HOME`/`XDG_STATE_HOME`/`XDG_CACHE_HOME` at a fresh private scratch directory outside Git, keeps `CARGO_HOME`/`RUSTUP_HOME`, sets `CORTEX_WORKSPACE_CHANNEL=dev`, removes `CORTEX_WORKSPACE_RELEASE_AUTHORIZED` and points `WORKSPACE_TEST_SERVICE_BINARY` at `target/debug/workspace-service` for the `cargo` and `desktop` steps; no dependencies are installed by the runner. The `build` step rebuilds the service those tests launch, so an older build is never tested. A missing prerequisite (`cargo`, `npm`/`npx`, installed `node_modules`, GNU `timeout`) exits 2 with an explicit message, never a silent skip. Keep `HOME` and `TMPDIR` of the invoking shell outside **every** git repository, ancestors included: the storage guards refuse to treat a directory inside a checkout as a user home, and a scratch home under a dotfiles repository produces false failures far from their cause.

The earlier Python runner's timing observations (eight pytest workers, batch grouping and the 2026-10 and 2026-11 baselines) described a suite that no longer exists and are not carried forward; no Rust-era whole-suite timing is recorded here, so retain exact counts, skips and exit codes from each run instead.

### Nested-sandbox portability

The default suite may itself run inside Cortex's runtime verification sandbox. Lifecycle verification uses a test-only launch seam, not nested bubblewrap; real subprocess outcomes, retention, cancellation and revision checks remain exercised. Production sandbox preparation remains fail-closed and has no bypass. Fixed git-context commands use a separate test-only preparation seam. Tests that execute real bubblewrap assert explicit fail-closed preparation where a user namespace cannot be created, never an inferred pass; set `CORTEX_REQUIRE_NAMESPACE_FIXTURES=1` on a host with unprivileged namespaces to run the positive journeys. See [verification](verification.md#running-the-suite-inside-a-runtime-sandbox).

### Release-grade run

For a release-grade run, verify in a throwaway clone rather than the working checkout: `git clone --no-hardlinks`, overlay the working diff, stamp there (`scripts/cortex-xtask.sh package stamp`), then run the command above. The stamp must reproduce byte-identically in the real checkout.

## Opt-in installed-binary checks (no paid call)

The Python tests that exercised a real installed Codex or OpenCode binary against a loopback fake model provider (`CORTEX_PINNED_CODEX_BINARY`, `CORTEX_PINNED_OPENCODE_BINARY`, `CORTEX_CODEX_ACP_TEST_PACKAGE`) were removed with the Python suite and have no Rust replacement; the Rust fake-engine tests and the no-paid installed gate in [verification](verification.md#installed-desktop-real-input-gate) do not qualify a real Codex or OpenCode binary. Report those checks as not run. Dated 2026-09 and 2026-10 observations of the former tests are retained in the engine pages as history only.

Two Rust tests run the installed engine bundle without a prompt, a synthetic key, a disposable engine home and the network cut off by `unshare -rn`. Both are explicitly ignored unless the bundle is named, and require `unshare`:

- `CORTEX_REAL_CODEX_BUNDLE=/usr/lib/cortex-workspace/engines timeout --kill-after=5 900 ~/.cargo/bin/cargo test --locked --offline -p workspace-service --test codex_real_chain -- --ignored` runs codex-acp and Codex App Server behind the native launcher with a fixture plugin and owner MCP server, and requires that a Cortex session starts only its relay (a control launch without the thread feature config must start the plugin's server).
- `CORTEX_REAL_CLAUDE_BUNDLE=/usr/lib/cortex-workspace/engines timeout --kill-after=5 900 ~/.cargo/bin/cargo test --locked --offline -p cortex-engine --test engine real_claude -- --ignored` starts four claude-agent-acp sessions at once, twice, and requires `session/new` and the routes' configure calls to keep the transport.

They establish start-up and configuration behaviour of those exact installed versions only, not model, tool or desktop behaviour.

## Why a default run cannot start a paid campaign

Nothing in the default suite can reach a paid engine, and this is enforced in the product, not by convention:

- The paid probes (`cortex-paseo-tools probe-codex` and `probe-claude`) refuse to do anything unless **both** `--i-authorize-paid-call` and a non-empty `--approval-ref` are supplied (they also require `--adapter` as an absolute executable path). Either flag alone exits non-zero before any engine is loaded. The default suite never passes them.
- Tests that spawn the real `cortex-acp` run against a fresh temporary `HOME`/`XDG_CONFIG_HOME`/`XDG_STATE_HOME` with no connections configured, so `session/prompt` fails closed at route resolution before any engine process could be launched, regardless of which engine binaries happen to be on `PATH`.

A live campaign therefore requires an explicit, separately authorized invocation. It cannot be reached by running the test suite.

## CI note

CI jobs should run as a non-root user so permission-denied cases are not bypassed. Current totals depend on the checkout and environment; record the exact passed, skipped and failed counts from each run rather than relying on earlier recorded counts. Remaining conditional guards are platform guards (user namespaces, POSIX FIFO support, symlink support, `tmux` presence) whose logic is also covered by unconditional tests; they are not the sole coverage of any mandatory case.

Settings-only routing and install behavior and profile deletion are covered by the `workspace-service` and `cortex-paseo-tools` suites and the Paseo `test:config`/`test:settings` scripts. No-paid tests do not establish production migration, installed desktop reachability or a paid route. Migration tests use disposable fixtures.

### Project runtime store ignore invariant

The `workspace-service` and `cortex-core` suites check Git effective ignore semantics, digest/replay/no-follow and authority protections, native result boundaries, consented deletion and command completion. These are isolated no-paid runtime tests, not Desktop or native-model evidence. See [ignore boundary limits](../features/permissions-and-containment/index.md#runtime-store-ignore-protection).

## Cortex Workspace local slice

The Workspace checks are listed with their commands in [verification](verification.md#cortex-workspace-local-slice-checks-no-paid-plus-container). The test files, grouped by area:

- Wire contract and generation: `crates/workspace-protocol/tests/contract.rs`, the Node vectors in `packages/workspace-contract/test.mjs`, and `scripts/cortex-xtask.sh generate-contract check`.
- Pins and prerequisites: `scripts/cortex-xtask.sh check-pins`.
- Adapter, identity, projections, outbox, decisions, starts and results: the `workspace-service` tests (`task_actions`, `task_facts`, `service`, `native_sessions`, `native_desktop`) and `crates/cortex-core/tests/outbox.rs`.
- Rendered layout geometry: `packages/workspace-ui/test/layout-fit.mjs` with `layout-fit.driver.mjs` (a Node driver over the C WebKitGTK window in `test/rendered/`; no Python). The Docker run, its widths and its limits are in [verification](verification.md#cortex-workspace-local-slice-checks-no-paid-plus-container). The source-level layout rules are pinned by `packages/workspace-ui/test/layout.test.tsx` and `packages/workspace-ui/test/interface-rules.test.ts`, which also forbid a stylesheet from styling by `data-why` or printing it with `content: attr()`.
- Service, journey and integration: `crates/workspace-service/tests/native_desktop.rs`, `service.rs` and `cold_start.rs`.
- Channel stores: `crates/cortex-core/tests/storage_location.rs` and `store_format12.rs` (both private paths, release/dev separation and format-11 migration), plus the `workspace-service` coexistence and unowned-dev-store refusal tests.
- Question notifications: `packages/workspace-ui/test/notifications.test.tsx` checks off-task delivery, deduplication, disabled/open-task suppression and delivery failure. The native `notification::tests` checks payload shape; `delivery_to_private_bus` is explicitly ignored unless run in a disposable container under `dbus-run-session` with `CORTEX_NOTIFICATION_TEST_BUS=private-container`.
- Packaging and channels: `crates/cortex-xtask/tests/repo.rs` (identity, runtime payload with no Python and no `src/`, 23 worker skills) and the `cortex-xtask` unit tests for the Debian version, channel overlay and installed gate. The rendered journey, its series runner and its image recipe are `cortex-xtask` commands (`check-rendered`, `journey-series`, `rendered-image`, `rendered-scrollbar`); their unit tests cover the visual judge, the series evidence rules, web-process termination accounting, the stale accessibility-node rule and the attachment stage's fixture PNG, base64 and prompt-delivery rule, and the journey itself needs Docker and a built archive directory (see `docs/project/verification.md`).
- Domain and presentation packages: `npm` test scripts in `packages/workspace-domain`, `packages/workspace-ui`, `apps/cortex-workspace-desktop` and `prototypes/cortex-workspace/desktop`.

Passing counts change with the suite and are not evidence on their own; the retained command receipts are.
