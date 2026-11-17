# Native incoming ACP

The `cortex-acp` binary in `crates/cortex-acp` is the Rust ACP-v1 NDJSON stdio
ingress over the window-independent native service host. It is the only Cortex
agent entry for Zed registration, the Paseo setup launch record and a Paseo
source-root launch. The former Python entry (`src/cortex_acp.py`) and its
runtime were removed on 2026-11-16; there is no Python opt-out, no Python
fallback and no interpreter requirement. Sections below that mention behavior
"carried from the former implementation" describe how the Rust runtime
preserves a contract that used to be implemented in Python; they are not claims
of a live comparison, and the Python oracle can no longer be re-run. Live Zed
qualification of the Rust entry was not run, and the differences listed under
[known limits](#known-limits) remain.

## Default launch

Build the binaries from this checkout. The entry is this checkout's
`target/debug/cortex-acp`, which this command produces:

```sh
timeout 180 cargo build --locked --offline -p cortex-acp
# Paseo only: the plugin's store readers, Settings and automatic setup for this launch (same target dir).
timeout 180 cargo build --locked --offline -p cortex-paseo-tools
# Preview only; no editor settings are read or written:
scripts/cortex-paseo-tools.sh print-client-recipes
# No-paid protocol smoke in disposable HOME/XDG and project roots:
timeout 90 scripts/cortex-paseo-tools.sh acp-smoke
timeout 90 scripts/cortex-paseo-tools.sh acp-smoke --rust-binary "$PWD/target/debug/cortex-acp"
```

The binary's channel is compiled in; an environment variable cannot turn a Dev
binary into Release. Keep its original public runtime payload available.
The helpers set `CORTEX_ACP_RUNTIME_ROOT` to their checkout/payload root;
a manual relocated launch must set that variable explicitly. Startup requires
no interpreter.

### Zed

`scripts/cortex-paseo-tools.sh print-client-recipes` emits the Zed entry: the
command `<checkout>/target/debug/cortex-acp` with no arguments and only the
public runtime-root variable in `env`. `--rust-binary /absolute/path/to/cortex-acp`
names another binary. Preview does not execute or require the binary; it prints a
note when the binary is not an executable file, and `--apply` refuses such an
entry. Writing still requires an explicit `--settings` path,
`--apply --i-consent-to-write`, and `--replace` when changing an existing entry.
`--python` and `--python3` are refused because the Python entry was removed.
Use only a throwaway Zed profile for qualification; never edit the owner's stable
settings to test this route. See [Zed setup](../features/zed-agent/index.md).

### Paseo

Three launch sources select the entry, in this order:

1. `CORTEX_ACP_RUST_BINARY=/absolute/path/to/cortex-acp` in the daemon's launch
   environment, not merely an unrelated shell, names an explicit binary. It must
   name an existing absolute target; an invalid target refuses rather than
   falling back. It takes precedence over `CORTEX_ACP_COMMAND` and the source-tree
   entry.
2. `CORTEX_PASEO_SOURCE_ROOT` (set by the guarded dev wrapper) launches that
   checkout's `target/debug/cortex-acp` with its public runtime root by default.
   An unbuilt binary refuses with a remedy; there is no other entry.
3. Otherwise the existing `CORTEX_ACP_COMMAND` JSON-argv override, then the
   profile-local launch record written by setup.

Without a source root, configure the shim through a profile-local launch record or
`CORTEX_PASEO_SHIM`, and retain the binary's compiled payload root or explicitly
set `CORTEX_ACP_RUNTIME_ROOT`. The shim accepts `CORTEX_ACP_RUNTIME_ROOT`
alongside HOME and the XDG config/state roots in the target environment; every
value must be absolute, and other target keys refuse.

The explicit setup operations record the Rust entry:

```sh
# Set HOME/XDG_CONFIG_HOME/XDG_STATE_HOME to disposable profile roots first.
# Default: the target/ directory beside cortex-paseo-tools (cortex-acp and cortex-paseo-tools must both be built there).
scripts/cortex-paseo-tools.sh configure-paseo-plugin-launch
# Another binary (cortex-paseo-tools must sit beside it) and an explicit runtime root:
scripts/cortex-paseo-tools.sh configure-paseo-plugin-launch \
  --rust-binary /absolute/path/to/cortex-acp --runtime-root /absolute/runtime-root
```

`install-paseo-plugin` takes `--rust-binary` and `--source-root` and writes the
same record. `configure-paseo-plugin-launch` also takes `--replace`. Both steps
require `cortex-acp` and `cortex-paseo-tools` to be executable files and
otherwise refuse before any record or daemon change, naming the build command.
The record stays private (0600), stores the existing storage roots plus the
public runtime root, and retains the Paseo load shim. Either step replaces an
existing Cortex-managed record for the same storage roots; unrelated records are
refused unless `--replace` explicitly authorizes replacing that exact record, and
a repeated identical selection is unchanged.

The Git-install build step (`integrations/paseo/paseo-plugin.json`) runs
`cargo build --release --locked -p cortex-acp -p cortex-paseo-tools` and then
`cortex-paseo-tools bootstrap-paseo-git-install`. That command stages an
owner-private runtime `<state>/paseo-runtime/<version>` (the non-code payload the
Rust entries read from `CORTEX_ACP_RUNTIME_ROOT`, the load shim and the two built
binaries) and writes the launch record; it requires no interpreter. Unset
`CORTEX_ACP_RUST_BINARY` to roll back an environment-selected launch. No
production Paseo installation or account migration is part of these commands.
Follow the [dev isolation guide](dev-environment.md); paid CLI/desktop checks
require their own explicit owner request.

Paseo's existing `modelEfforts` path invokes the selected binary directly with
`CORTEX_MODEL_EFFORTS=1`, its selected target environment and the existing
15-second deadline. It does not start an engine.

### Paseo Settings under the Rust launch

Build the adjacent agent and tools binaries for the same compiled channel.
Settings, automatic setup and the agent share a host-channel resolver: dev
appends `cortex-workspace-dev` to raw XDG config roots and
`cortex-workspace-dev/state` to raw XDG state roots, and uses `.cortex-dev`
for project configuration. Release retains ordinary XDG roots and `.cortex`.
Runtime channel environment values do not change this compiled identity.
HOME/native-account references and explicit `CORTEX_ROUTING` are unchanged.
The plugin asks `cortex-paseo-tools config --host` for read-free root metadata
before a Settings operation, so its displayed binding follows the actual writer;
unavailable metadata refuses before the operation. No existing files are moved.

The channel regression uses the actual setup, Settings and ACP binaries with
isolated roots and engine following/updating disabled. Build both binaries before
`cargo test --locked --offline -p cortex-paseo-tools --test channel_settings`.

`server/config.ts` sends each Settings request to `cortex-paseo-tools config`
beside `cortex-acp` (stdin/stdout JSON protocol, working directory `/`, and only
`HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME`, `CORTEX_ROUTING`,
`CORTEX_WORKSPACE_CHANNEL`, `PATH`, `CORTEX_ACP_RUNTIME_ROOT` and
`CORTEX_OPENCODE_BINARY` from the launch environment). Reads use
workspace-service's native `ConfigurationReader` producers; every other
operation uses its native management-API implementation
(`workspace_service::settings_api`):

- `user_defaults` and `routing` updates (user and project files), `mcp_servers`
  operations (`upsert` with `{"keep": true}` env values, `remove`, `set_enabled`),
  `connections` rename, session limit, `validate` and `remove`, and the
  `project_ceiling` read and update in the project store;
- every `routing_profiles` verb (`create`, `save`, `update`, `delete`, `activate`,
  `deactivate`), with the project only selecting which source shadows user routing;
- the default preset (`read`, `apply_default_preset` with per-file diagnostics for
  existing files, confirmed `reset_default_preset` with private backups and rollback);
- `route_catalog` `refresh`, `discover_connections`, `onboard_connection` (an
  existing connection is returned unchanged; otherwise the discovered, qualified
  adapter with a native login is registered and its models observed once),
  `install_engine` (the manifest pin only), `discover_opencode` and `onboard_opencode`.

Each takes the owner lock (`connections.lock`, with the private root and its
`.gitignore`), checks the caller's revision under it, writes atomically with a
fixed order and modes, and refuses with a structured payload (`field`, per-item
`errors` in the request's own key order, `current_revision`). A write that may have
completed reports `machine_outcome_unknown` and is never replayed.

Only shapes the native producers cannot answer refuse, with
`config_api_unavailable` and the reason, before any effect:

- validating a connection whose credential is an OS-keyring reference (no native
  keyring backend is observed);
- `discover_opencode`/`onboard_opencode`/`onboard_connection` for OpenCode when an
  OpenCode binary is installed or named: its version probe must run in the OpenCode
  network-less namespace jail. `discover_connections` (and the discovery an
  `install_engine` reply carries) then reports that engine's status as
  `discovery_unavailable` instead of probing it;
- a catalogue `refresh` whose registered engine has no qualified native catalogue
  producer (only verified Claude and Codex installs, including the exact-hash
  Codex 2.3.0 adapter, are admitted); records that launch nothing report their
  failure outcomes natively;
- `install_engine` when a login marker or the registry cannot be projected, and
  settings or profile values (non-finite or beyond 64-bit numbers, an unqualified
  activation shape) the native engines cannot reproduce exactly;
- `apply_default_preset` when an existing file is malformed in a way the native
  loader cannot describe (invalid JSON or a structural routing error).

An `onboard_connection` whose engine the native catalogue verifier does not admit
keeps the connection and reports `catalog_status: catalog_failed`
(`native_catalog_unavailable` in its first-run status), like the Rust first-run pass.
Plugin activation runs `cortex-paseo-tools setup` with the full launch environment:
cortex-acp's own native first-run pass (`cortex_acp::startup::first_run`), waiting
for it; a concurrent pass makes it skip. Its owner root is cortex-acp's own, so a
dev-channel build uses its channel subdirectory while `config` uses
`XDG_CONFIG_HOME/cortex`.

`cargo test --locked --offline -p cortex-paseo-tools` (`tests/config_parity.rs` and
`tests/config_parity/workflows.rs`) runs every read, the request-shape cases and
write workflows for each operation class above (successful changes, read-back,
stale revisions, every validation family and named loader refusal) through the
tool on identically reset fixture homes, project and project store, and compares
stdout bytes, exit status and the resulting files and modes with the golden
outcomes in `tests/config-parity-golden.json`. Those outcomes were recorded once
from the former Python management API and are now frozen; only wall-clock
timestamps, preset backup stamps and revisions derived from timestamped documents
are normalized. The test needs no Python and also checks that the explicit
refusals above change nothing. Known read differences from the reused producers:
two messages are fixed, value-free native text
(`Saved routing has an invalid shape.` / `Saved routing is not valid UTF-8 JSON.`
for an invalid routing file, and `The current section cannot be represented as a
profile; repair it first.` for a profile section problem). A native storage refusal
(for example an unsafe file) reports the producer's code with this tool's own
message. A relative project is refused rather than read against a working
directory. Integers beyond 64 bits in a settings file are not kept exact on reads;
the writers refuse preserved or requested integers beyond the exact range rather
than rounding them. Ceiling validation lists multiple problems in sorted key
order. An unsafe-storage refusal keeps the native message text. A project-ceiling
write opens a format-11 store without migrating it.

The store readers (project root, task binding, worker history, worker results,
running commands and pipeline) are Rust. `server/store-reader.ts` recognizes the
Rust target as a launch argv of exactly one absolute path named `cortex-acp` and
runs `cortex-paseo-tools` from that same directory (never `PATH`), with an empty
environment and working directory `/`. The tool opens the store only through
cortex-core's non-initializing live read-only mode, so a format-11 store refuses
(`legacy_storage_migration_required`) instead of being migrated; the Rust entry's
own initializing open migrates it first. A missing tool refuses each read with an
explicit message; there is no fallback. `cargo test --locked --offline -p
cortex-paseo-tools` compares 37 cases byte-for-byte with golden outcomes
(`tests/parity-golden.json`, recorded once from the former Python readers) on real
format-12 fixture stores, and checks that a full Rust pass leaves the database
bytes unchanged; `npm --prefix integrations/paseo run test:store-reader` runs the
plugin's store and Settings handlers on the Rust route with `PATH` emptied. Known
differences are limited to inputs a format-12 store cannot hold: lone UTF-16
surrogate escapes (replaced by U+FFFD), JSON nested deeper than 10,000 levels
(refused), and Unicode edge cases where Rust's character tables differ from the
former Python ones. Native session execution currently selects native-account
routes only; API-key catalogue/efforts visibility is not execution support.

### What the smoke check proves

`scripts/cortex-paseo-tools.sh acp-smoke` checks initialize, new session,
status/no task, missing-route refusal and clean EOF exit against the selected
binary. Reports name the selected argv. It replies to a missing route with
`-32001 / orchestration_unavailable` and `data.reason`
`no_configuration_revision`. With `--fixture-connection`, a fake API-key
connection is seeded; the fixture is asserted to be not routable (native session
routes require native-account auth) and refuses with the same reason. This is a
recorded limitation, not a successful native model-selection test. The harness
has no JSON-schema validator, so its `schema:*` conformance check is reported as
`skip`, never as a pass. Dev fixture and task counts use the compiled channel's
isolated roots. No real engines, account files, client GUI or installed package
are exercised.

## Supported native surface

- Initialize (protocol 1, no authentication methods); `authenticate` refuses as
  unsupported. Integer requested protocol versions
  negotiate protocol 1, including negative/unknown integers; non-integers refuse.
- New native sessions; exact retained native-session load with paged public
  message/tool-activity replay. Format-12 sessions created by the former Python runtime also load through
  a separate read-only facade, including message identities and attachment descriptors,
  and reply with the stored options (the task's latest live configuration revision,
  its language override and access mode; no inherited default for a task-less one).
  Load never replays a prompt, adopts a legacy task or starts a scheduler.
- Initial text, image, embedded-resource and resource-link admission through the native draft/Start transaction,
  coordinator/worker driver, independent review and contained verification.
  Parent end-turn messages do not certify task completion.
- Follow-up prompts to an admitted task with the same attachment kinds (see
  [follow-up attachments](#follow-up-attachments)).
- Busy-session answers: a message typed while a turn runs answers its open question (see [busy sessions](#busy-sessions)).
- Public streaming updates, persisted synthesis, cancellation as native Pause,
  process settlement and durable text-only steering through
  `cortex_queued_message`. A message ID reuses its retained control receipt;
  changed payloads conflict.
- One-shot engine permission requests with correlated responses. Core checks
  live channel/session authority both before and after the owner decision;
  unoffered approvals or replies after cancellation fail closed. Human responses
  have no 30-second transport deadline: correlation remains until response,
  card retirement, session cancellation or connection shutdown. Manual standing grants
  refuse; Auto may use an actually offered standing allow as described below.
- Native worker question cards and single/batch typed answers through
  `cortex_question_answer`; batches require every answer in one atomic submission.
  Question-card responses use the same non-expiring correlation lifecycle.
  Exact command stops through `cortex_stop_command`.
- Language and Development/Guarded/Auto access configuration, plus models/effort
  when the native configured route provider can truthfully offer them. Fresh native
  sessions start Development; missing or legacy access authority remains Guarded.
- Owner-configured stdio MCP attachment through the native host. The registry uses
  the same owner-only `$XDG_CONFIG_HOME/cortex/mcp-servers.json` source and explicit
  tool/path/network, role and engine filters as the retained runtime. Read-only
  workers receive only `read_only_tools`; writers receive `allowed_tools`;
  coordinators receive their read-only lookup subset and consultants receive none.
  Engines receive secret-free relay entries, never registry commands or environments.
- `/cortex-status`, `/cortex-setup` and validated `/cortex-model`, `/cortex-effort`,
  `/cortex-language` without dispatching an engine, recognized and answered with the
  documented text (see [local commands](#provider-model-and-thought-level-picker)).
- The task commands, advertised as one complete ordered list (see
  [task commands](#task-commands)).

### Task commands

Recognition, usage errors, replies and the recorded transcript are fixed. The typed command is recorded first, except `/cortex-kill`,
which is control input; `/cortex-new-task` usage is sent without being recorded.
Reply text for these commands is English.

- `/cortex-kill <command-ref> <reason>`: the command reference is the first
  whitespace-delimited token and the reason is the remainder, with refusals (`command_stop_invalid_ref`, `command_stop_invalid_reason`,
  `command_stop_no_task`). With a bound task it uses the native owner command-stop
  control; a native refusal reports its own code as `Command stop refused:`. It is handled even while a turn runs.
- `/cortex-stop` and `/cortex-finish`: no-task replies and `/cortex-finish`
  usage are fixed. With a bound task, stop pauses the task and revokes its channels, finish
  closes it uncertified. While a turn runs, either command first interrupts it,
  waits up to 30 seconds for it to release, then runs.
- `/cortex-answer <n> <text>`: fixed usage, no-open-question and unknown-number
  replies; a valid answer is delivered to that open question and the turn is
  followed.
- `/cortex-clear`: grammar `N`, `Nd`, `days N`, `[before] YYYY-MM-DD`,
  `force`, `confirm`, with its cutoff format and dry-run selection: tasks and task-less
  sessions of this project older than the cutoff, keeping the invoking session and
  task, this connection's other sessions and their tasks, and live work unless
  `force`. A confirmed request that deletes nothing replies with that result; one that
  would delete refuses without a write (this host does not delete stores yet). While
  a turn runs it refuses with the idle-session error.
- `/cortex-new-task <text>`: a missing route refuses with the orchestration-unavailable envelope before
  anything is created. Otherwise the task is bound to a separate internal session;
  its coordinator text streams under the command's message id, followed by a
  `Started a new, separate task (...)` line. The invoking session keeps its existing
  binding (or stays task-less). The internal session persists the invoking client's
  access mode and language before admission; route selection still uses the saved
  default. `session/cancel` cancels only the task executing that prompt, never an
  unrelated task bound to the invoking session. Idle cancellation has no task
  effect; `/cortex-stop` and `/cortex-finish` remain explicit bound-task controls.
  A Paseo session's cancel is held as described under Busy sessions.
- `/harvest`, `/harvest-refresh` and `/skill-management <request>`: fixed goal
  and instruction text (English or Russian from the session language), run as an
  ordinary prompt; `/skill-management` without a request replies with its usage.

### Busy sessions

A prompt sent while a turn of the same session runs follows these rules:

- `/cortex-kill` runs as control input; `/cortex-stop` and `/cortex-finish` interrupt
  the turn first; `/cortex-clear` refuses with the idle-session error.
- Plain text with exactly one open worker question answers it: the typed text is
  recorded in the session transcript, a number naming an offered option selects that
  option (`2` selects the second), other text is the answer itself, and the prompt
  ends with `end_turn` while the running turn continues. The question's open card is
  retired; a later selection on it is ignored.
- Plain text with several open questions refuses with `Multiple questions are
  open; use /cortex-answer <question-number> <answer text>.`; `/cortex-answer <n> <text>`
  answers question `n`, and a malformed or out-of-range number refuses with
  `Use /cortex-answer <question-number> <answer text>.`
- A coordinator question is answered by typed text only when the agent runs with
  Paseo's question cards (`CORTEX_PASEO_QUESTION_UI=1`, which the Paseo provider sets)
  and its card is open; the typed text is not recorded.
  Otherwise, and for every other command, the prompt refuses with `A prompt
  is already in flight for this session.`
- A member of a question batch cannot be answered alone: Rust refuses with
  `question_batch_requires_complete_answer`. Answer
  a batch with one complete `cortex_question_answer` submission.
- Paseo sends `session/cancel` before every message it sends in its default mode, and
  ACP cannot tell that cancel from a Stop click. For five seconds, a cancel of a running Paseo prompt ends that turn at once with
  `cancelled` without pausing its task. A prompt on the same session that arrives
  meanwhile waits for the cancelled turn to end and then runs: a message steers the
  task, and `/cortex-answer` still finds the task's open question. A lone cancel
  pauses the task once the five seconds pass. Other clients' cancels pause at once.

The Rust entry runs no background continuation in the transport (a late card
answer or queued-message drain): such a prompt is admitted at once as follow-up
input to the task.

### Follow-up attachments

A follow-up prompt to an admitted task is captured like a first prompt (same
validation, per-block and per-prompt 10 MiB caps and image refusals). Its
attachments become the task's next source revision in the same transaction as
the steering event and the user transcript row, which lists their descriptors (`[attachment: <name> (<availability>) source_ref=<ref>]`). Each
engine session receives every stored attachment body once, in revision order (see
[engine-facing attachment blocks](#engine-facing-attachment-blocks)), so their total across
revisions is also capped at 10 MiB: a follow-up that
would exceed it refuses with `task_attachment_total_exceeds_10mib_cap` and records
nothing. Queued messages
(`cortex_queued_message`) remain text-only.

The internal owner-control request includes the capture fingerprint before replay,
admission and settlement. Retrying the same request identity and capture returns
its settled receipt without another source revision, transcript row or steering
event. Changed capture bytes or metadata (including removing the capture) refuse
with `payload_conflict`; an admitted but unfinished control remains
`native_control_outcome_unknown`. This is internal control retry safety, not
cross-session deduplication: ordinary ACP prompts still receive fresh request IDs.

The facade owns no separate scheduler: it composes the same
`workspace-service::native_runtime`/NativeHost used by Desktop. It acquires Core's
project/channel owner fences before writable initialization. If another
participating service owns the project, session creation refuses with
`scheduler_owner_busy`; connecting to that daemon is not implemented.
Several `cortex-acp` processes (one per Paseo agent) may share a project: a
process hands the scheduler lease over once it has been idle for a second with
no unfinished task, and retakes it on its next write. A new process waits up to
five seconds for such a release; a process that cannot retake the lease refuses
with `scheduler_owner_busy`, and one that finds an unfinished task it never
recovered refuses with `scheduler_recovery_required`. Two schedulers never
drive one task, and a process with an unfinished (running, waiting or paused)
task keeps the lease. Concurrent active tasks in separate processes are not
supported.
Writable unadopted legacy projects and linked-worktree ingress still refuse here.
Workspace-service recovery can explicitly adopt legacy tasks under its scheduler fence
(see [native recovery](../cortex-workspace/local-task-execution.md#legacy-task-adoption));
loaded legacy sessions remain display-only and reject prompts/configuration.
A completed task requires a new session. EOF/SIGTERM settles owned native
processes before releasing the owner.

The service-only `_cortex/workspace` and `_cortex/workspace_control` routes are
not activated by an external ACP session, just as with standalone legacy ingress.
They retain explicit workspace-not-activated/project-inactive refusals; no
incompatible replacement envelope is invented under those method names.
Paseo's replay acknowledgements remain handled by its existing load shim.

## Startup gates and model-efforts mode

Ordinary Rust stdio startup now reads `pinned-dependencies.json` before
configuration or protocol output. Portable-contract mismatches exit 1 with stderr findings and repair text;
stdout stays empty. No interpreter floor exists or is enforced. Manifest schema and portable-contract validation
remain fail-closed in Rust. The default manifest root is the build
checkout; an operator relocating this binary must set
`CORTEX_ACP_RUNTIME_ROOT` to the exact public payload root. That variable is
host configuration, never an ACP/model argument. Missing or invalid manifests
refuse; JSON parser/OS diagnostic details are native.
No install, engine launch or network access occurs in this gate.

First-run startup seeds absent `routing.json`, `defaults.json` and
`mcp-servers.json` from the bundled public preset, under the compiled channel's
user configuration root. The root is prepared as follows: missing ancestors are created 0755 (before umask)
and left alone, the root itself is created or repaired to 0700 (an existing 0755
root included) and must then be owner-only and owned by the current user, a
symlink or non-directory component or a project marker in an ancestor other
than `$HOME` refuses, and `.gitignore` must contain exactly `*`. Core's
private-file checks and the shared `connections.lock` fence protect publication;
startup waits for that lock without a bound. Complete fsynced
temporary files are linked without replacing racing owner files. Existing bytes, malformed
files and symlinks are not overwritten. Failure is nonfatal and reported on
stderr. Startup does not rewrite an owner's settings.

Startup also creates an absent format-2 `routing-profiles.json` when the working
routing/defaults/MCP policies match the bundled preset. It reuses the shared native
profile capture generator, including existing connection concurrency limits and
revisions of the exact backing-file bytes. Creation holds the same owner lock and
uses private no-replacement publication. Custom working settings, an existing
profile registry (including malformed data) or a symlink are never replaced.
The generator is pure: it does not install, register, observe or launch engines.

Approved-adapter following is implemented in
`crates/cortex-acp/src/adapters.rs`. Unless `CORTEX_ENGINE_FOLLOW` is off, an
existing `codex-acp`/`claude-agent-acp` connection whose recorded launch command
resolves to a declared package bin advances to the newest exactly registered
adapter. Discovery: a packaged
`cortex-workspace[-dev]/runtime` payload's channel-local `engines` bundle is
preferred when its bundled `node/bin/node` is executable, its declared package
bin is qualified and it passes the install verification checks
(nearest `package.json`, canonical paths outside `~/.local/share/app.codeg` and
`~/.config/app.codeg`, executable node and, for codex, the hoisted
`@openai/codex` package and vendored platform binary, declared-bin entry).
Otherwise Cortex's owned root (`$XDG_DATA_HOME/cortex/engines`) is used. A
qualified version wins over a newer unqualified one; an older version is never
selected. An equal version moves only onto a different bundled launch, and only
`launch_command` and `approved_fingerprint` change. No adapter is executed. Planning reads happen first; the commit then repairs the owner root as
above, waits for the shared `connections.lock` without a bound, refuses if the registry bytes changed, writes the exact
pre-change bytes to private `connections.adapter-backup.json`, then atomically
replaces `connections.json` in its canonical sorted compact form.
An unsafe backup target, a concurrent change or any other failure leaves the
registry unchanged, prints `cortex: approved adapter selection deferred` and is
not retried. Each commit prints a `cortex: approved adapter <engine>
<old> -> <new>` line, in the connections' file order.

Startup reads the registry with a reader that is deliberately compatible with
registries written by the former Python runtime (`crates/cortex-acp/src/registry.rs`
over the lenient JSON reader `pyjson.rs`). It accepts `schema_version` values equal
to 1 (`1`, `true`, `1.0`), duplicate keys (last value at the first position),
keyring identifiers with one trailing newline, legacy records without
`max_concurrent_sessions` or `owned`, and any JSON in the opaque
`override_history`, including floats, `NaN`/`Infinity` and integers beyond 64 bits.
Those values are rewritten with the former runtime's exact number spelling and
sorted compact keys. Integers over a 4300-digit conversion limit and extreme
nesting still refuse. A lone UTF-16 surrogate can be read but not re-encoded:
such a registry is never rewritten, and the write fails.

The background first-run pass is implemented in
`crates/cortex-acp/src/automatic.rs`. Unless `CORTEX_ENGINE_UPDATE` is off,
startup detaches one thread that ends with the process. It takes the private non-blocking
`automatic-setup.lock`; a busy or unsafe lock skips the whole pass. It reseeds
absent presets and then handles `codex-acp` and `claude-agent-acp` in order,
writing allowlisted `first-run-<engine>.json` phases (`downloading`,
`connecting`, `refreshing_models`, then `ready`, `download_failed`,
`login_required`, `opted_out`, `catalog_failed` with a fixed reason code, or
`setup_failed`):

- Pinned install: only the qualified `pinned_package_version` from
  `pinned-dependencies.json`, never npm latest, and nothing when a verified
  bundle already has that version. npm comes from beside the resolved node, else
  `PATH`. It runs under the per-engine `.lock` with stdin closed, output
  discarded and a 600-second bound, in `.tmp-<version>` staging (the Claude
  `0.87.0` SDK override included). The staged package must declare the pinned
  version and a real bin before the rename to `<version>`. Pruning keeps the two
  newest and every qualified version; a qualified install is followed
  immediately. An existing install is a no-op. A failed or offline npm keeps the
  current version and leaves its staging for the next pass's cleanup.
- Registration: only when no connection of that engine exists, the owner has not
  opted out (`auto_connect_disabled`) and the native login marker
  (`~/.codex/auth.json`, `~/.claude/.credentials.json`) exists. All three checks
  repeat under `connections.lock`, which is awaited without a bound. The existing connection is the first of that
  engine in file order. The record is a referenced native-account record
  (`<engine> (discovered)`, unowned native directory, marker-derived state,
  ISO timestamps that omit a zero microsecond). The registry is rewritten in its
  canonical form; an unencodable registry fails that engine with `setup_failed`.
- Observation: a new connection, or an existing one without a genuine cached
  catalogue, is observed once through the shared native prompt-free producer
  (`native_routes::observe_first_run_catalog`, 20 seconds, no model prompt).
  Cached entries use the shared catalogue evidence/field validator and a
  recomputed route fingerprint; invalid cache data records `setup_failed`, never
  `ready`, without launching the engine or replacing the cache.
  Publication rechecks the unchanged observed record under
  `connections.lock` (30-second wait) and retains that lock until immutable
  publication finishes. Concurrent cooperative Settings changes/removal serialize
  with publication; a changed or removed record refuses without replay.
  A launch shape the native verifier does not admit is not launched: it records
  `catalog_failed` with `native_catalog_unavailable` and prints `cortex: native
  first-run catalog observation unavailable`. This is an explicit unavailable
  observation, not successful qualification; it publishes no catalogue. There is
  no fallback, including for unqualified Codex package/binary/patch shapes,
  unverified Claude packages, OS-keyring references, or unsafe native roots. These
  shapes remain unavailable. Owner-selected engine executables can themselves
  depend on an interpreter; this does not make an arbitrary configured engine
  interpreter-free. The producer admits a Codex adapter package whose
  `@openai/codex` dependency (or, on Linux x86_64, vendored binary) cannot be
  resolved: such an unverified launch is launched directly, without private
  artifacts or `CODEX_PATH`/`NO_BROWSER`. Native taskless
sessions inherit valid `en`/`ru` user defaults; absent/invalid language falls back
to `auto`, and a retained session choice wins. A pre-task language choice
(configuration option or `/cortex-language`) is also persisted as the user-wide
default: `defaults.json` is rewritten under `connections.lock` in
sorted compact form with its normalized ceiling, after the same registry
validation. The read happens inside the lock. Preserved integers
outside the exact native range refuse and change nothing. Task-bound language
changes stay task settings.

A pre-task coordinator `model` or `thought_level` choice (configuration option or
`/cortex-model`, `/cortex-effort`) is persisted as follows: the portable `coordinator` default is dropped and
`route_id`, `route_connection_id`, `route_model` and `parameters` are written. A
model change resets `parameters` (and the session effort) to empty, and the new
route is resolved and validated with empty dependent parameters, so a saved
effort the new model does not support is dropped instead of refused. An effort
change keeps the current route and its parameters with the new `thought_level`.
Re-sending the current model is not a write when a saved `route_id` (or a
portable coordinator) exists, without one, it is written. Later sessions inherit the saved route.

### Provider, model and thought-level picker

The route options are built in
`workspace-service/src/native_routes/presentation.rs` over
the enabled, non-denied, launchable owner routes in registry and snapshot order:

- Zed and other clients get `provider`, `model`, `thought_level`, `language`,
  `access_mode`, in that order. Provider values are connection ids labelled
  `<engine short name> · <account>` (or the engine alone), disambiguated with an
  id suffix on collision; model values are the chosen provider's model ids, named
  from the observed catalogue (`default` aliases and unnamed versioned ids get a
  derived name), described by the engine short name.
- Paseo gets one `model` option across every account: a model id names exactly
  one route, duplicates keep exact route ids. Its language is the
  "Coordinator language" mode-category option and access stays in `modes`, which
  carries fixed labels and descriptions.
- `thought_level` lists the selected route's observed efforts in strength
  order without `default`, current the saved effort when offered, else the first.
- `_meta._cortex.state` and the description suffix are: `default` for an
  inherited saved route (falling back to its saved connection/model when the route
  id changed), `pending_next_use` after a pre-task choice in this session,
  `default_unavailable`/`selection_unavailable` with a substitute when the saved
  route is gone, and a portable-coordinator resolution diagnostic. A session first
  seen by `session/load` has no inherited default.
- Selecting `provider` keeps the model name when the new account offers it,
  otherwise its first model; provider/model reset the effort and an effort keeps
  the route. The choice is an exact route id, so it binds across accounts even
  when the default is unresolvable. A portable-coordinator refusal carries
  `{code, remediation}` data; other refusals are followed by the unchanged options.

Claude provider labels are best effort: `claude auth status` has one five-second
deadline covering execution, stdout collection and cleanup, with a 1 MiB output
cap. The dedicated process group is killed even if its parent has exited; a
descendant retaining stdout cannot delay the reply beyond the lookup deadline.
Timeouts, invalid/oversized output and nonzero exits omit the account identity.
The no-paid `test_account_status_deadline_bounds_session_creation` regression
uses disposable executables to cover these failures, successful labels and group
cleanup through the real Rust `session/new` boundary.

After `session/new`, Rust refreshes each registered connection once without a
prompt through the shared native producer and sends the options again for every
published observation. The refresh is not
awaited at EOF and ends with the process.

Local commands are recognized from one text block whose trimmed
text is a trigger alone or followed by whitespace. Status lists every option with
its description, the connection summary and a truthful Rust execution line (no
engine jail); setup returns state-aware guidance; `/cortex-model` resolves
an exact route id, one label or one model id across accounts; `/cortex-effort`
and `/cortex-language` validate and reply with fixed wording. Typed commands and
replies are recorded for replay with stable message ids. Other text, including
unknown `/cortex-*` names, is an ordinary prompt.

`CORTEX_MODEL_EFFORTS=1` now prints one JSON object and exits, before pinned
verification or first-run setup's ordering. It maps
Paseo model selector values to ordered unique effort arrays, including exact route
IDs for duplicate model names, resolved default aliases, enabled/deny filtering
and empty arrays for models without efforts. It reuses the native configuration
reader and explicit filesystem/native-marker credential refresh; no engine is
started. OS-keyring connections explicitly refuse this mode as
`native_model_efforts_keyring_unavailable`. The native reader's stricter
snapshot identity, size and ambiguity refusals remain in force.

Ownership is a safety invariant: Rust refuses writable unadopted legacy projects
at `session/new` with `legacy_project_requires_retained_adapter` and does not
adopt legacy work through ordinary ingress.

Focused no-paid checks (build the binary first):

```sh
timeout 180 cargo test --locked --offline -p cortex-acp
timeout 180 cargo fmt --all --check
```

The Rust unit and binary tests exercise startup findings, selector values,
private absent-only seeding and efforts-mode ordering. The binary probe in
`crates/cortex-acp/tests/acp_binary_schema.rs` validates every frame against the
ACP schema. The remaining coverage is in `startup.rs` (no-account startup
boundary), `incoming.rs` (scripted external ACP clients and real no-paid
engine/relay child processes), `codex_runtime.rs` and `codex_task_trust.rs`.
Registry-shape behavior (historical `schema_version` spellings, duplicate keys,
lone surrogates, oversized integers, malformed JSON and a held
`connections.lock`) and protocol-envelope refusals (parameter validation order,
unknown-session/cwd/method messages, `-32600` before initialize, the `-32001
orchestration_unavailable` envelope with `data.reason`
`no_configuration_revision` for a missing route) are asserted in these Rust
tests. A refused prompt records its text row, and an image that fails capture
refuses with `-32602` and `data.reason` before any row, task or source exists.
HOME/XDG/project roots are disposable and separate; fake engine launch
sentinels must stay absent. These checks do not qualify paid engines, installed
Paseo/Zed clients or packaging. The earlier differential oracle against the
Python entry can no longer be run; its recorded outcomes survive only as the
frozen golden files described above.

## Known limits

Audio, attachment-bearing queued messages, unconfigured client-supplied stdio
MCP servers, the full worker-tool catalogue and confirmed `/cortex-clear`
deletion are not implemented. Native attachments remain unavailable on the
native host (a documented limit). The native routing implementation below is
tested against retained consent cases, but is not an installed-client gate.
Unsupported content refuses before task admission. Image and embedded-context
capabilities describe initial prompt admission, not full legacy attachment or
engine-capability coverage. A schema-valid exchange is not proof of smoke
capability, rendered client behavior, live accounts or release readiness.

### Launch status

The Rust entry is the only launch. The items under **Accepted fail-closed
limits** refuse explicitly. The items under **Known differences** are open to
accept or port.

#### Live qualification

- Paseo `cli` (owner-requested, paid, 2026-11-16) ran through the isolated dev
  daemon with `CORTEX_ACP_RUST_BINARY`, default Paseo settings and disposable Git
  projects, on Codex `gpt-6.1-sol` and Claude `claude-sonnet-5` native accounts.
  Every scenario passed on both engines: launch; a task with mandatory review
  and verification to completion; a question answered through the Paseo card;
  steering a running task; model-remark consent; stop; and `/cortex-status`.
  Some rows were carried over from earlier rounds on earlier binaries of the
  same day. The owner's Codex `config.toml` stayed byte-identical. An earlier
  round found Codex persisting project trust there; pinning the session launcher
  (see Accepted fail-closed limits) fixed it, and the rerun on fresh,
  never-trusted project paths confirmed byte identity. That evidence predates
  the removal of the Python runtime and the Rust-only packaging and was not
  repeated afterwards.
- Not run: live Zed qualification (a throwaway Zed profile written by
  `print-client-recipes`), rendered Paseo Desktop behavior, and the installed
  package/real-input gate for this entry. The Cortex Workspace desktop app does
  not launch this binary, so its live results do not qualify this entry.

Resolved (each has a no-paid Rust regression test): malformed, oversized and
unterminated client lines and duplicate request ids; the lone-surrogate registry
in the default-profile seed; NFKC label normalization; the per-session refresh's
`.refresh.lock`; the Paseo launch shim's runtime-root variable; question and
permission responses that arrive after 30 seconds; client `mcpServers` entries
no engine is handed, including the HTTP server Paseo injects into every session
(see External MCP boundary); the exact-hash Codex 2.3.0 install's model
catalogue; the native Codex launcher (`codex-acp-runtime`) written by
`cortex-acp` or `cortex-paseo-tools setup`/`config`, which names the writing
binary as its interpreter and which each of those binaries now runs
(`--native-codex-runtime`), so its Codex App Server starts and lists models; and
Paseo question cards and the cancel Paseo sends before a follow-up message (see
Busy sessions).

#### Known differences

- Client-supplied stdio MCP servers: the former Python runtime forwarded a
  client stdio entry (`command` set) that `mcp-servers.json` does not configure
  to worker sessions, with the grants the client declared. Rust refuses
  `session/new` and `session/load` with `external_mcp_server_unconfigured` for
  such an entry, so a client that sends one cannot open a session. Forwarding
  them needs a containment and tool-grant policy for unconfigured executables.
- The complete worker-tool surface of the former runtime is not implemented;
  native workers receive the native catalogue only.
- A confirmed `/cortex-clear` that would delete something refuses and writes
  nothing; dry-run previews work.
- Native-contract differences that are not host limits and still await an
  explicit owner decision (accept or port), described below: task-bound
  `_meta._cortex` revision state; first-prompt refusal without a task;
  command-stop refusal codes; a stopped task resuming through a later message;
  no queueing behind background continuations; atomic question batches; the
  10 MiB per-task attachment total; tool-card shape; Codex per-model effort
  observation; less strict snapshot-envelope deserialization; replayed option
  updates; steering that interrupts running workers.

#### Accepted fail-closed limits

- OS keyring: the Rust entry has no OS-keyring access. API-key and keyring
  catalogue entries are not offered, and OS-keyring efforts refuse.
  Native-account routes are unaffected.
- Launches the native producer cannot verify (verifier-dependent Codex installs,
  unresolvable Claude packages, other platforms): first-run observation records
  `native_catalog_unavailable` instead of launching them, and the per-session
  refresh skips them. The exact-hash Codex 2.3.0 install is observed natively;
  the bundle digest and every patched call site are checked before launch.
- No engine namespace jail: the former runtime answered non-deletion engine
  permission requests itself because its user-namespace engine jail bounded
  them. Rust does not run engines in that jail, so outside the cases listed
  under Notifications it forwards the request to the client, which asks the
  user. The jail also mounted the owner's Codex `config.toml` read-only; without
  it the native Codex launcher passes App Server
  `-c projects={"<cwd>"={trust_level="trusted"}}` for its own working directory
  (the project root, which codex-acp already trusts per session thread), so
  Codex never persists project trust there. The inline-table form is required:
  Codex keeps quote characters literally in a `-c` key path and splits it at the
  first `=`, so a quoted path key never matches. Every session launch
  (coordinator, workers, review, verification, sprint planning; this entry and
  the desktop service) pins the launcher as `CODEX_PATH` with `NO_BROWSER=1`
  through `SelectedRoute::session_launch`. The launcher is prepared only when
  the engine starts, so route selection and presentation write nothing. The
  session artifact is content-addressed (`codex-acp-runtime-<hash>` beside the
  catalogue's `codex-acp-runtime`), so another writer never replaces bytes a
  session is about to execute, and the binary is located like the catalogue's
  verified install without re-hashing it per launch. A launch that is not
  install-shaped (a test double) starts unchanged; an install-shaped one whose
  bundled Codex cannot be located refuses to start. Other engine state files
  stay writable.
- Legacy write ownership and adoption, linked-worktree ingress and concurrent
  daemon ownership refuse at their explicit boundaries (for example
  `legacy_project_requires_retained_adapter`); nothing falls back silently.

#### Differences in detail

- Startup: Codex packages the native verifier cannot resolve are admitted only
  through the native catalogue producer. Cached entry fields, evidence and route
  identity are checked, but snapshot-envelope deserialization remains less strict
  than the former runtime's. The default-profile seed reads a registry with lone
  UTF-16 surrogate escapes (it captures only identifiers and limits; the seed's
  parse replaces them with U+FFFD and never rewrites the registry).
- Configuration: Rust offers only routes it can launch. Task-bound native
  sessions show no `_meta._cortex` revision state (native tasks keep no
  configuration-revision history). Labels are NFKC-normalized before controls,
  `Cf` characters and stray combining marks are removed, using ICU4X 2.4 data. The
  per-session refresh holds `<state>/cortex/catalog/<connection>/.refresh.lock`
  without waiting and skips when another refresh of that connection holds it. Its
  Codex observation can record per-model efforts and it ends with the process.
- Sessions: task-bound sessions created by the former Python runtime reload
  with options read from their latest live revision. Rust also replays its
  recorded option updates. Writable legacy ownership/adoption, linked-worktree
  ingress and concurrent daemon ownership still require the explicit existing
  boundaries, not fallback.
- Prompts/tools: unconfigured client stdio MCP and the complete worker-tool
  surface of the former runtime are not implemented. A steering message
  (including one sent after a held Paseo cancel) interrupts the task's running
  workers and replans. Follow-up attachments are retained, but a task's
  attachments are capped at 10 MiB in total, and a batch member cannot be
  answered alone from a busy session. Confirmed `/cortex-clear` deletion
  refuses. A refused first prompt creates no task, so later commands on that
  session report no task. A stopped task does not resume through a later
  message, and there is no queueing behind background continuations. Native
  command-stop refusal codes are the native ones. Client lines are bounded at
  15 MiB per line, and a malformed, oversized or unterminated line gets an error
  (with the recovered request id where one can be found) while the connection
  continues. Every request id is tracked, but only a resent `session/prompt`
  refuses with `duplicate_request_id`.
- Notifications: Rust journeys pin the native contract. Rust shows each engine
  tool call as `<attempt>:<toolCallId>` titled `<role>: <title>`, created
  `in_progress` once and updated to `completed`, without inputs, outputs or
  content. Rust answers claude-agent-acp requests itself with an offered allow
  option for MCP tools, subagents, native questions and deletions (deletions are
  titled `Removal blocked pending confirmation: ...` and accept any offered allow
  kind), but without an engine jail Guarded approves only reads, searches,
  fetches and in-project unprotected edits and forwards other requests to the
  client. Codex and OpenCode requests outside Auto are forwarded unchanged
  (session id replaced) and accept only an offered `allow_once`/`reject_once`.
- Qualification: the no-paid binary tests run without a paid engine, so they do
  not exercise live active-engine cancellation/settlement, permission cards,
  busy-session answers, follow-up attachments or full execution/review/completion
  beyond what the fake engine drives. The local Cargo incoming harness runs only
  fail-closed command-profile assertions on a namespace-restricted host; its
  positive journeys (including the busy-session task commands and answers,
  cancel, permission, question and follow-up attachment journeys) require real
  namespaces: the documented disposable-container route or a host-direct run
  with `CORTEX_REQUIRE_NAMESPACE_FIXTURES=1`.
- Deployment: the binary is built separately from this checkout, and the Paseo
  Git-install build step builds and stages it (see above). Product
  settings/history sidecars are the Rust `cortex-paseo-tools` commands.

## Access authority and startup

Paseo's ACP `modes` and other clients' `access_mode` expose the same Development,
Guarded and Auto choices without duplicate presentation. The standard
`session/set_mode` remains available to both; Paseo cannot select the hidden
`access_mode` config option. Only trusted client/owner controls select access; model arguments, display
history and route parameters cannot grant it. Pre-task selections persist in
the existing session-default table and bind atomically with Start. Task changes
are append-only native runtime events, with origin and revision; no storage
format change or user-wide setting rewrite is involved.

Every engine session confirms its observed mode after the model switch.
Codex uses agent, Claude default and OpenCode cortex as the ordinary floor.
An Auto worker chooses the most permissive actually offered mode from the
retained engine-specific preference order; an unoffered or unconfirmed mode
refuses startup and settles the owned process. Existing sessions are not silently
reconfigured by a later access switch. Permission requests re-read access and
recheck channel/session authority after consent; a concurrent mode change refuses
the pending decision. Auto chooses an offered allow-once option, falling back
to an offered standing allow option. Read-only channels cannot approve a
non-read/search native request; Cortex gateway and attached external MCP tools
still pass because their own channel checks apply. See the
[permission contract](../../SECURITY.md#current-acp-boundary-106).

The new outside_write and outside_command routes require a current writing
worker, a nonempty scope and no pending assignment wait. Guarded outside writes
and all non-Auto outside commands require an exact one-shot client approval.
Development routine outside writes need no card; recognized control-plane
components and the selected engine's credential/install roots still ask.
Auto answers these cards itself. These are explicit same-user host routes,
not an escape retry after a sandbox refusal. Symlink aliases and inside-project
outside-write targets refuse. Writes use held no-follow parent descriptors,
private temporary files, fsync and atomic replacement. Request identities bind
complete payloads before effects; acknowledged retries return saved results,
conflicting keys refuse, denied requests do not rerun and unfinished effects
remain unknown across restart.

Outside commands supervise their process group and cancel on authority loss.
Their native output is bounded to 64 KiB; excess output reports incomplete and
truncated, never a verified pass. They do not yet provide the retained runtime's
large-output paging or worker passed-check receipt qualification. Ordinary
command_run/project-file tools and the retained engine jail/adapter patches
remain separate, unfinished native domains. Engine mode confirmation alone is
not filesystem containment, interception of unrequested native actions, or full
permission-policy parity.

Native session/load replays the exact saved session with message identities and
cortexReplay sequence metadata, restores access selection, and does not dispatch
or recover tasks. A subsequent explicit text prompt or queued owner message can
resume that paused native task: resumption and source admission commit together,
before a fresh coordinator starts. An acknowledged queued-message retry does not
resume again or append duplicate steering. Ordinary Desktop prompt controls still
require their separate explicit Resume action. Recovery runs once per newly
opened host when session/new requests startup. A settled planning checkpoint can start a fresh coordinator
with retained user messages, decisions, plan and current nodes, not a replayed
original prompt. Explicit context limits refuse rather than silently truncate.
Unknown processes, verification/control effects, outside effects and claimed
deliveries stay fenced; loading history is not authorization to replay them.
This does not qualify engine-session cross-process PID reconciliation, attachment
to an already owned daemon. Legacy-task adoption belongs to workspace-service recovery,
not this history-load path. Brokered-command
supervisor attachment is described separately below.

No-paid fake clients cover pre-task and active switches, reload, fresh-session
defaults, one-shot outside-write approval/refusal, paused reconnect with explicit
prompt/queued-message resume, and settled coordinator recovery.
Fake engine/relay tests cover all three engine-kind mode preferences, refusal
of unconfirmed modes, outside-route retries/conflicts, read-only roles, pending
waits, symlinks and inside-project refusal. These are disposable protocol tests,
not installed-client or hostile-code-isolation evidence.

## Context rollover and reconciliation

Native session prompts include current role instructions, the schema-derived
served-tool supplement for Codex, and a fresh retained-state capsule from
`cortex-core::recovery_context`. It includes original goal, all recorded user
messages/constraints, answered and pending questions, current plan/nodes/results,
source descriptors, access, owner epoch and attempt/capacity state. Attachment
bytes retain their existing separate hash-checked prompt path. The capsule is
data, never authority. Each collection is limited to 1,000 records and the JSON
snapshot to 100,000 characters; overflow refuses rather than silently dropping
constraints. It is not a compression policy.

Only a completed tool-call signal with `_meta.contextCompaction`, or Claude's
explicit `_meta.claudeCode.toolName=compact`, opens an observed generation.
Start-only and failed signals do not open one. Terminal duplicates share the
same evidence identity; display titles never trigger it. Generation identity
includes the native attempt as well as the wire session, so reused wire names
cannot alias different processes. Five completed compactions in fifteen minutes
stop another burst. Before/after token savings remain explicitly unmeasured;
there is no pre-compaction hook or newly qualified installed-adapter claim.

A continuation rebuilds state and redelivers instructions instead of trusting
engine summaries. A settled coordinator can be replaced with a new channel and
a recorded replacement generation; an unknown old process cannot be replayed.
Delivery records distinguish built, transmission attempted, exact transport
reply and a later continuation; settling a session supersedes its unfinished
delivery records. Neither a reply nor a subsequent prompt proves model recall.
Owner guidance arriving during capsule/attachment preparation keeps polling that
preparation and wire send before cancelling an active turn; it never poisons a
still-ready engine session by cancelling before its first prompt.

After verification, clean first-pass results may still use the native
retained-summary fast path. Open unverified checks, repair or replan history
instead dispatch a fresh coordinator in Reconciling. It can submit synthesis,
ask single/batch owner questions, or request a necessary replan/scope/escalation.
An answer returns a reconciliation question to Planning; answer_submit can close
that released work only through the same report-integrity, project-digest,
review, latest-verification, pending-input and process-settlement checks.
Uncertified released answers are retained but end Failed, never Completed.
Synthesis is refused in Planning, Waiting User and terminal states.

An authenticated coordinator's synthesis with missing current check names returns
a structured tool error with the current unresolved names and correction guidance,
rather than an opaque authority refusal. A name copied from a previous revision
does not disclose a renamed current check. Duplicate authored disclosure sections
also return correction guidance. Neither error records a synthesis or changes the
completion gates; a corrected proposal is still required. Channel, role and
task-state refusals remain refusals. This feedback is shared by Claude and Codex;
permission approval alone never proves that the synthesis was accepted.

The two final-text routes require every unresolved not_run/unverified check by
name with a concrete blocker, render grouped disclosures once in the selected
language, and refuse duplicate authored disclosure headings. A complete matching
runtime verification command on the current reviewed revision closes a
declared-command gap; unrelated passes do not. Ordinary native worker passed-check
claims require the same attempt's complete successful command receipt and intact
retained output. Ordinary and confirmed outside commands share the assessment
rules described below; failure history is retained, not fabricated evidence.

verification_submit repairs only a failed runtime verification in Planning,
with every current node already completed. It validates literal argv and probes
executable availability before acceptance, preserves the reviewed work and plan
revision, and runs verification again. Shortening away failed commands retains
the existing explicit dropped-check obligation. Exact keyed retries reuse their
receipt; changed payloads refuse.

Replan, scope and escalation share three accepted requests since the last owner
message/answer, surviving new plans. They cancel unfinished work and revoke
worker channels, then settle before new planning. Scope validation rejects
private/traversal paths and owner-denied grants; a scope request itself grants
nothing. Escalation injects the runtime-owned read-only consultant dependency
into the next plan, with recorded report hashes/edition identities and explicit
unavailable references. The model cannot plan that reserved role itself.

Disposable fake engines/relays and incoming clients exercise compaction
redelivery/deduplication, settled coordinator replacement, gap disclosure/refusal,
verification repair, reconciliation questions/answers, all three replan routes
and consultant dispatch. Question-card versions bind their own question/wait
event rather than unrelated engine activity; current channel, epoch and wait
status still fence late answers. Core tests cover budgets, stale channels/epochs,
overflow, disclosure formatting, conflicts and completion failures. These do not
qualify installed Paseo/Zed, live model recall, token savings or a release.

## Native document tools and selectors

Authenticated coordinator and eligible worker channels serve the existing
create_draft, draft_write, write_report, list_reports, read_report, read_draft
and tools_discover schemas. Drafts remain owned by the exact creating principal;
ordinary published reports can be read by other authorized channels in the same
task. A worker may submit its own published report identity instead of inline
Markdown; a sibling's report cannot stand in for its own result.

Publication reuses Core's private, hash-checked immutable documents and crash
journals. The complete mutation payload is bound before file effects; acknowledged
retries reuse their receipt, changed payloads refuse, and an ambiguous draft
replacement never overwrites a newer edit. Catalogue and content reads retain
their native bounded cursors. Source documents are excluded from list_reports
and use the separate native list_sources/read_source domain.
Responses with advertised output schemas are validated before returning.

Discovery returns complete effective schema records or bounded summary pages.
Its cursor binds the principal, attempt, epoch and catalogue revision; it cannot
discover a filtered-out tool. read_batch is not yet served; command/Git output is
retained separately for read_large_output. Oversized authorized schema/catalogue
JSON is retained whole with a deferred large_output_id, not clipped into a
purported complete response.
Nonempty redact_values also refuses until native user-input redaction exists.
Source-capture metadata reports not_attempted for these tools; prompt admission,
not a tool read, is the existing native capture boundary.

The model, effort and language slash commands use the same validation and
configuration serialization as ACP selectors and publish config_option_update.
Unsupported values leave settings unchanged. Status includes the persisted
selection. Other command-shaped slash input refuses before engine dispatch rather
than silently becoming an ordinary work request. Completed sessions still require
a new session: the retained store contract makes a bound ACP task immutable.

No-paid fake-engine/relay tests cover all seven document/discovery tools, retry
conflicts, Unicode pages, principal separation and published-result binding.
Fake-client tests cover selector commands and explicit completed-session refusal.
These checks do not establish complete authoring, permissions or installed-client parity.

Question cards use the Paseo card shape, which the Paseo plugin needs to show
its question form and send answers back: tool call `cortex-question:<question>`,
options `cortex-question-<n>` (a batch: `cortex-batch:<question>:<n>`), the
`__cortex_question` input marker and `_meta.cortexQuestion`. A question without
options offers an instructional action, which is never an answer. Question
cards bind to the prompt's cancellation generation. A card computed
before cancellation cannot open a new wait after pending permissions were cleared;
an in-flight question is interrupted, and a fresh prompt can ask again. Outside
Paseo's held cancel, cancellation still waits for the host's observed settlement
rather than certifying a stopped task from the transport notification alone.

## Native source, project and command tools

Authenticated channels serve list_sources/read_source, project_read and
project_git; eligible writing workers additionally receive project_write,
project_edit, command_run and access_request. Source pages retain captured
user text, attachment descriptors and answered questions. Text/base64 is paged;
stored images are returned whole as real MCP image blocks after same-task
size/hash checks. Native answer intake commits the real answering transport
session with the answer. Legacy runtime-authored answer provenance remains readable;
missing provenance is not guessed from a currently open session. Reference-only
file links are freshly observed without changing the original capture or upgrading
a reference into captured content. Main source text has no attachment-size cap.

Project text reads use bounded line pages and retain exact channel-local
coverage. Whole-file replacement requires complete non-truncated coverage of
the current hash; edits accept a partial text read but still require unchanged
whole-file bytes. Missing-path observations allow creation with parents.
No-follow descriptors, regular-file checks, owner denials, atomic replacement
and durable request identities protect ordinary cooperative edits. Successful
writes advance that channel's read evidence; runtime-augmented root .gitignore
writes leave it incomplete until reread. Overlapping edit matches count as
ambiguous unless replace_all is selected. Mutation acknowledgements retain
byte_count and edits_applied alongside hashes and sizes. Before replacement, an intent binds
old/new hashes and the exact bounded acknowledgement. Restart recovery observes
the no-follow path: new bytes recover that acknowledgement; old bytes return an
unchanged error without retrying the write. Diverged, aliased, unreadable or legacy
unbound intents remain unknown. Reconciliation itself grants no read coverage.
New-file installation uses an atomic no-replace rename, not a temporary hardlink. Text reads have no image-size cap;
images still refuse above 10 MiB. Delivery remains line-bounded. Text processing
still buffers the file, so this is not a bounded-memory large-file guarantee.

Command/Git output is spooled whole to private channel-owned files, with
64 KiB inline prefixes and byte paging through read_large_output. A clipped
prefix does not mean that retained bytes were lost. Exact retries return the
same observation; key conflicts refuse. Output read authority follows the
coordinator/assignment scope across channel replacement, not arbitrary channels.
Legacy-format output bodies retain encoding/status but explicitly lack native
hash certification. Exit, timeout, output integrity and settlement are distinct. Running handles join the existing owner-stop registry;
models receive no stop tool. Brokered command_run launches use an independent
Rust supervisor in the same shipped binary. Its private inherited pipe requires
current authority for preparation mutations and final launch; boot ID, PID, start
ticks, output identity and the exact request are committed before launch is granted.
The supervisor owns bubblewrap, pipe draining, deadline, Git guards and scratch
cleanup independently of the service. A service restart attaches to the same
private status/output files and restores the owner-stop registry. Stops use a
launch-specific marker consumed by that supervisor, not a recovered PID signal.
The supervisor retains its child leader until group cleanup, then publishes a
fsync'd atomic terminal status after output and sandbox cleanup. Recovery verifies
output hash/size and commits the original receipt/output/finish event together,
without inventing a project-revision digest. Missing, mismatched, dead-without-status
or corrupt handles remain unknown and fenced; legacy, outside-command and runtime
verification handles still use conservative unknown recovery. No branch relaunches
a payload. Engine sessions and detached unsandboxed descendants are not adopted.
Read-only Git disables hooks, fsmonitor, external
diff/text conversion and configured clean/smudge/process filters.

Ordinary worker commands use the [bubblewrap backend](#brokered-command-namespace-backend)
for read-only, Guarded and host-writable Development/Auto filesystem views.
Private descendants are overlaid after grants instead of refusing a writable
parent solely because it contains protected children. Writing roles receive the
whole-project grant independently of declared reservations (including absent
reserved paths). Development/Auto detach external writable hardlinks under
bounded, authority-checked preparation; other unsafe aliases stay read-only. They permit a
canonical outside cwd; Guarded requires outside_command for that cwd. Existing
literal and expanded glob owner denials are reapplied after grants.
Verification grants existing Git-ignored directories, excluding aliases/private
paths. Project virtualenv/bin paths are preferred, relocated explicit caches are
validated, and only an owner-owned ssh-agent socket may pass the auth filter.
Advanced Git/HOME/SSH/askpass/discarded-write policy is described in the
[command backend](#brokered-command-namespace-backend); engine containment
remains separate. Outside commands remain explicitly authorized unsandboxed
owner actions and retain full output and exact live handles. These boundaries
are not hostile-code isolation.

Worker results derive evidence strength: verified passed checks require intact
native output, a nontrivial argv, a unique receipt and an unchanged command
project digest matching submission. Confirmed outside commands share that rule.
Wrapper-stripped reads/searches, read-only Git and no-ops are trivial; script
files may verify checks. Trivial, duplicate or unknown passed references become
unverified disclosures with the original claim and a runtime evidence gap.
Verified failures retain their original outcome and evidence strength. A same-name
passing citation closes a failure only for the latest exact argv/cwd rerun,
including across ordinary/outside operations; same-operation profiles still match.
Later failing, unresolved or identity-less runs cannot be hidden by old passes.

An explicit exploratory withdrawal retains failed history with
`retracted_by=probe_withdrawn`. Supported standalone Python syntax/regex/literal
probe mistakes can bind a later passing correction through `invocation_error`;
`command_correction` additionally supports direct Python argparse startup errors.
The runtime derives retraction/supersession links, never passes the old command.
Both outputs, attempt/operation/cwd/profile, unique same-name citation, source
structure and later history must qualify. The pinned RustPython parser normalizes
ASTs entirely in Rust; no Python interpreter is launched. Literal-data proofs use
only the retained pure-builtin allowlist, bounded to 8,192 source characters and
256 AST nodes. Excessive parser nesting is a refused resource limit, never proof
of a syntax error. Static analysis never executes submitted source; unsupported
forms refuse. Real project test/lint/build
and runtime-verification failures cannot use these invocation corrections.
Presented failed citations survive refused submissions and reopening, so retries
cannot omit them. Completion rederives links and checks output integrity again.

General inline-shell status-flow proof and sibling-whitespace attribution remain
separate limitations, not silently accepted.

Native project-file preconditions follow the retained gateway: whole-file writes
check the expected current digest first, then complete same-channel non-truncated
read coverage. Partial read hashes identify content but never authorize overwrite.
Creation accepts the absent-content precondition without requiring a text read.
Missing, incomplete and stale reads have distinct structured correction responses
and rejected write state; they are not access refusals. Re-read and rebuild under
existing authority; keyless precondition failures are not replayed, while explicit
failed keys retain their full rejection and need a fresh key. Exact edits use a
current text read (a partial page suffices), reject ambiguous overlapping matches,
and validate every ordered replacement before changing the file.

No-paid fake-engine/relay scenarios cover source Unicode/image/question reads,
channel-local file preconditions, stale/partial reads, edits and creation,
Git context, access reservations, read-only commands, restricted writes,
full-output paging, retries/conflicts and timeouts. Existing incoming-client
tests retain outside-route consent/refusal coverage. Full check-assessment,
legacy read qualification, engine containment and installed-client equivalence
remain unqualified; this is not a full qualification.

## Coordinator questions and skill proposals

The authenticated coordinator serves the existing `user_question_request`
schema, including single questions, batches and `skill_proposal`. Questions
and the Waiting User transition commit before the client sees the card.
Only planning/reconciling can ask; worker channels cannot propose skills.
Evidence must name 2–5 distinct ordinary published reports in the project store;
the saved proposal includes bounded report titles and case descriptions.
Create/update checks inspect real skill directories without following symlinks.

The native implementation accepts the same eight English/Russian action-matched
approval/decline pairs as the retained runtime. Reversed, mixed and arbitrary
pairs refuse before persistence. Only the validated approval label or `1`
approves; the decline label or `2` declines; other text is unclear.
The decision and return to Planning commit together. Duplicate answers cannot
record another decision. The coordinator receives a claimed continuation, with
the approved `/skill-management` request and coordinator skill instruction,
or an explicit prohibition on creating/updating/linking the declined skill.
The consent instruction also remains available to a fresh planning turn after
reopen. No proposal decision writes project files or bypasses normal planning,
scope, review or verification. Native authoring/tool coverage is still partial;
this handoff is not qualification of a complete skill-authoring workflow.

No-paid fake-client scenarios cover approval, decline, unclear and reversed-pair
refusal; Core tests exercise label pairs, evidence, state and durable decisions.
These are protocol tests, not rendered installed-client evidence.

## Native routing and retained consent

The coordinator channel serves `list_route_options` using the existing empty
input schema. Workers do not receive it. The menu includes configured tiers,
observed permitted efforts, policy sources, role tiers/work kinds, eligible
affinity, unconditional and conditional preferences, stretch ladders, flagship
request names and known/unknown quota bands. `plan_submit` retains
`model_hints` previews with its idempotent receipt; dispatch re-reads current
settings and retained task evidence. Model hints and reasons never grant consent.

Routing uses exact role/alias/wildcard pins, owner preferences and node work kind.
A conditional preference uses an exact node hint as the coordinator's judgement
of owner prose; the runtime does not interpret the condition. Known usage of
95% or more is hot; otherwise usage breaks only the last tie, after purpose,
hint and affinity. Missing usage is unknown, not zero or exhausted. Codex usage
comes from bounded rollout tails; other engines use retained quota observations.
No CLI usage refresh or model call occurs on this native path.

Task-start file fingerprints fence root AGENTS.md/CLAUDE.md requirements.
Changed files produce `changed_during_task`, never new consent. Recorded owner
requests and actual answers precede pins and preferences; imported terminated
chat-history summaries, model reasons, reports and unanswered options do not.
Explicit-route or effort failures refuse without substitution. Ordinary automatic
fallback cannot introduce the flagship tier; explicit escalation or established
consent is required. Native flagship pins without consent also refuse.

The bounded Rust recognizer has static, source-labelled table tests in
`crates/workspace-service/tests/fixtures/routing_consent.json`, exercised by
`native_routes::consent_tests::legacy_consent_case_matrix`. They cover English
and Russian requests/refusals, restrictive selections, passive requirements,
role-purpose binding, selected versus merely mentioned models, opaque identity
punctuation and attached effort. A chosen answer may match its recorded option
with normalized whitespace, case and terminal `.`/`!`; unselected options and
question wording still grant nothing. Newer direct owner messages and answers
retain their own source identities; imported history cannot preserve an older
user grant or hide an earlier direct refusal of a project requirement.

Plan previews and dispatch share the recognizer. A project requirement carries
`pending_dispatch:project_instruction` with file/line/route provenance and a
note when replacing a hint. Preferred-over-hint previews list `owner_preferred`
and explain that this is not a model mismatch. Eligible hints stay pending
unless their known usage is hot beside a cooler alternative; explicit consent
still precedes usage until hard exhaustion. These deterministic no-paid tests
do not qualify live accounts, installed clients or a complete release.

## Brokered command namespace backend

Worker commands and runtime verification use `native_command_sandbox.rs`, a
Rust launcher for the Linux bubblewrap mount/process-view mechanism. It probes the exact view with
`/bin/true` before admitting a payload and refuses unavailable setup; there is
no unsandboxed or Landlock-only fallback. Availability is not cached across
launches or restart.

Read-only commands receive a private PID namespace/procfs and read-only host
mounts. Where the installed bubblewrap and filesystem support it, project writes
go to a private disk-backed overlay discarded at command end; otherwise they
remain read-only. Explicit existing Git-ignored verification directories remain
writable in place. Read-only/Guarded commands bind private scratch over literal
`/var/tmp`; host `/tmp` remains writable. Development/Auto makes the host writable
but remounts the project read-only before grants. Mount sources use retained
no-follow descriptors. Private/control-plane descendants and expanded owner
denials follow grants, including whole-project grants.

Whole-project writer grants reopen Git data (objects, refs, index, logs and
state), including the current linked worktree. Config, hooks and pointer files
remain read-only. Development also guards info and module/worktree containers,
reopens existing submodule data and disables hooks/fsmonitor through inherited
Git configuration. Absent Git controls are swept periodically and at settlement;
this is not an atomic barrier against hostile same-user code. Narrow grants
never reopen Git data. Inspecting read-only repositories redirects LFS storage
to private scratch.

Guarded HOME exposes existing non-sensitive tool-state directories, creating
`.cache` and `.npm` where absent. Read-only HOME exposes only explicit caches;
relocated environment caches cannot overlap sensitive/control-plane paths,
HOME ancestors or the project. Development retains its host-writable HOME.
External writable hardlinks are atomically detached only for Development/Auto;
internal writable aliases remain usable, denied/private aliases are not
detached, and hardlink-heavy disposable ignored directories stay read-only.
Preparation has byte/time bounds, preserves file metadata and rechecks authority
after a durable command intent. An interrupted preparation never replays.

Owner SSH-agent sockets are validated. Trusted root-owned SSH includes are shown
as owner-owned read-only copies. Opted-in askpass generates a sealed dialog script;
without an enabled owner setting, a display or an installed dialog it adds nothing.
Named control-plane targets, including absent relocated paths, cannot be treated
as routine outside writes; the existing consent/Auto policy still decides them.

The existing receipt, output-capture and cancellation paths remain in place.
Engine launch containment and external MCP parity remain separate. External MCP
retains the Landlock/seccomp boundary below; that boundary alone does not protect metadata.
These command fixtures do not qualify installed desktop rendering or a
complete release. See [namespace checks](verification.md#native-command-namespace-checks).

## External MCP boundary

A configured name in incoming `mcpServers` is accepted, but cannot override its
owner registry command, environment or grants. Entries no engine is
ever handed are accepted and ignored: an entry without a stdio `command` (an
`http` or `sse` server, such as the `paseo` server Paseo injects when
`daemon.mcp.injectIntoAgents` is on), an entry named `cortex`, `coordinator` or
`worker`, and a repeated name. Their URLs, headers and environment are neither
stored nor forwarded. An unconfigured stdio entry still refuses before session
admission with `external_mcp_server_unconfigured`. An empty client list still
attaches eligible configured servers.
No project-local registry is read. Configuration is snapshotted when a native
project host opens; a new host is needed to apply changed grants.

The native host uses mandatory Linux Landlock ABI 3+ and seccomp, not
the bubblewrap launcher. Project reads exclude private control directories; writing
workers additionally receive existing, non-root scoped paths. A scope path that
does not exist yet (a file the worker is about to create, or a pattern) is not
granted to the server: it is neither created nor widened to its parent.
Administrator grants must exist, contain no protected paths/symlink ancestry or
protected descendants, and cannot overlap the project. Missing administrator grant
paths are withheld. Invalid grants or scopes, unavailable containment, a failed
launch or relay, or a server process that exits before `session/new` fail closed
for that server only: the session starts without it and the runtime records
`NATIVE_EXTERNAL_MCP_WITHHELD` (`channel_id`, `server`, `reason`) plus a failed
timeline note naming the server and reason. Reason codes are
`executable_unavailable`, `runtime_directory_unavailable`, `grant_path_refused`,
`worker_scope_refused`, `protected_path_refused`, `containment_unavailable`,
`launch_failed`, `relay_unavailable` and `server_exited_before_session`. The exit
check is a single observation just before `session/new`; a server that dies later
is reported by the engine itself, not by this event. There is no uncontained
fallback: a contained server is never relaunched without containment. The
session start still fails if a launched server's exit cannot be
confirmed during cleanup.
Each contained child has private writable scratch/runtime storage. Socket creation is denied
unless `network: true` is explicitly configured (including local socket-based server
features); this is narrower than namespace-isolated Unix-socket support.
This is not hostile-code isolation or full external-server compatibility qualification.

An owner can instead choose, explicitly, to run a server uncontained:
`"containment": "none"` on its registry entry, or as the root registry default
(`"containment": "none"` beside `servers`), which an entry's own
`"containment": "contained"` overrides. Absent everywhere, a server is contained;
any other value makes the registry invalid (`invalid_mcp_registry`). An
uncontained server gets no Landlock policy, no seccomp filter and no
`no_new_privs`, so it runs with the owner's full user authority. Its
administrator path grants, `network`, `runtime_dir_env` and worker scopes are not
evaluated, the missing-grant-path filter does not apply, and the per-file
project walk that builds the Landlock policy is skipped entirely (on a checkout
of this repository that walk took about 9.5 s per contained server start). It
still launches only the resolved registry executable, with the registry
environment plus the owner's `HOME`, a built `PATH` (the executable's own
directory, `~/.local/bin`, `~/bin`, `/usr/local/bin`, `/usr/bin`, `/bin`),
`LANG=C.UTF-8` and the host process's `XDG_RUNTIME_DIR`; it gets no private
`TMPDIR` or runtime-directory variables and never a channel credential. It is
still reached only through the channel-bound relay, keeps tool-grant narrowing,
reservations and no-replay, and still runs in its own process group, killed at
session end. A process that leaves that group (for example a daemon that calls
`setsid()`) is outside this kill guarantee. A genuine launch failure is still
withheld per server with the reasons above.

Core reserves every tool call transactionally before dispatch, binding the channel,
server, typed request ID and parameter digest. Complete replies are retained before
return; identical calls replay the retained reply, conflicting calls refuse, and
pending/unknown effects never dispatch again. Channel revocation stops the proxy.
The configured environment stays in memory; private call receipts can contain tool
results, but public history does not expose those bodies.

## Attachment retention

Bodies are stored in private task source-block files, not SQLite or base64 prose.
The existing source/revision/report tables retain hashes, user_direct trust labels
and capture-time availability/reasons. Local resources require fd-verified
containment, regular-file/owner/single-hardlink checks. The launch environment may
supply a Paseo uploads root; prompt metadata cannot. Remote links remain references,
never network fetches. Per-block and total attachment caps are both 10 MiB, and
a task's retained attachments across all source revisions stay within 10 MiB.
Rejected images never create a task or a source revision. Outgoing engine prompts verify retained hashes
before sending actual ACP content blocks.

### Engine-facing attachment blocks

The blocks follow the engine's `agentCapabilities.promptCapabilities` (observed at
`initialize`; an absent field means no image and no embedded context) and what
the bundled adapters read:

| Attachment | Sent as |
| --- | --- |
| Text (`text/*`, JSON, XML, YAML and similar, `+json`/`+xml`, or valid UTF-8 without a MIME type) | `{"type":"resource","resource":{"uri","mimeType","text"}}` when `embeddedContext` is advertised; otherwise a text block with a header (name, MIME type, size, sha256) |
| PNG, JPEG, GIF, WebP | an image block when `image` is advertised; otherwise a marker |
| SVG, BMP and other image types | a marker. At capture they are not retained at all (`unsupported_image_type`) and the prompt text says so |
| PDF | a blob resource for claude-agent-acp 0.107.0, which decodes it; for codex-acp 2.6.0, which would put base64 text into the model context, a marker that the PDF is attached but not readable. No text is extracted |
| Any other binary | a marker with name, MIME type, size and sha256, never base64 |

Each attachment is sent once per engine session. Later prompts of that session
list the already-provided attachments by one marker instead of re-sending them;
a replacement session starts again with the full set. The 10 MiB per-task cap is
unchanged.

Ingress notes: a resource link without retained bytes (remote, missing, private or
unreadable) is named in the coordinator prompt text with its URI and availability, and
the recovery snapshot's source list carries each block's name, URI and MIME type.
`resource_link` URIs under `.git`, `.cortex` or `.cortex-dev` are never read or retained,
as for re-resolution. An answer typed while a turn runs (`steer_busy`) keeps its
attached images and resources: they are retained as the task's next source revision
(`USER_ANSWER_ATTACHMENTS`, no steering and no interruption) and reach the engine in the
prompt that delivers the answer; the answer itself is the typed text only. A failed admission can leave private,
unreferenced files; these are not successful input or permission evidence.
Incoming client lines are bounded at 15 MiB; engine-side native NDJSON
frames keep a 16 MiB budget. Inbound buffering is four frames.

## No-paid checks

Run sequentially from the repository root:

```sh
cargo fmt --check -p cortex-acp -p workspace-service -p cortex-core -p cortex-engine
cargo test --locked --offline -p cortex-acp -p workspace-service -p workspace-protocol -p cortex-core -p cortex-engine
cargo build --locked --offline -p cortex-acp
```

The Rust integration executable is its own fake ACP engine/relay peer; all
projects and stores are disposable. Where the shell itself has unprivileged
namespaces (a host-direct run, not a nested sandbox),
`CORTEX_REQUIRE_NAMESPACE_FIXTURES=1 cargo test --locked --offline -p cortex-acp --test incoming`
runs its positive journeys instead of skipping them. Add `-- separate-task` to
run only separate-task Guarded authority (including an executed worker command
receipt), cancellation with a running parent, ordinary cancel and explicit-stop
journeys, or `-- steering` for the tool-activity contract and its replay, permission
request shape and allow/reject mapping, busy-session answers (worker, batch and
Paseo coordinator cards) and follow-up attachments. The same argument works with the disposable-container Cargo runner in
[verification](verification.md#native-command-namespace-checks).
`cargo test --locked --offline -p cortex-acp --test codex_task_trust` runs a
task through the real `cortex-acp` binary against a fake codex-acp and App
Server that persist project trust for any thread without the launcher's layer,
and asserts the fixture owner's Codex `config.toml` stays byte-identical. Plan
admission preflights verification in the command sandbox, so its worker phase
runs only with real namespaces (required under
`CORTEX_REQUIRE_NAMESPACE_FIXTURES=1`); elsewhere it asserts the coordinator
sessions. The binary startup probe
(`acp_binary_schema.rs`) validates replies against the ACP schema, checks status
without a task and verifies no-route prompt refusal without an engine call.
It checks the advertised initial content capabilities and read-only history of
sessions created by the former Python runtime, without treating these as a
complete qualification. Container staging discovers Cargo members and
checks this crate with the rest of the workspace.

Paid client checks require a separate explicit owner request for the named mode.
See [verification](verification.md) and [architecture](architecture.md).

Several `cortex-acp` processes on one project share the scheduler lease: a process
releases it once idle (every task finished) and the next job retakes it, waiting up
to five seconds for another holder. A live process never releases the lease while a
task is unfinished, so a process that retakes it and finds an unfinished task adopts
that task left by a vanished peer and runs recovery before new work. Each streamed
engine message keeps one message id per turn; a later turn starts a new message.
