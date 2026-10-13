# Persistent isolated dev Paseo environment

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed.


## Installed Paseo version boundary

> On 2026-10-13 the installed CLI reported 0.11.2. Its CLI help still lists
> `--home`, and its bundle reads `PASEO_HOME` for the daemon home and
> `PASEO_ELECTRON_USER_DATA_DIR` for the Electron profile; the desktop daemon
> manager resolves its daemon from `PASEO_HOME`. A requested `desktop` check
> that day ran the guarded wrapper's dev daemon on `127.0.0.1:16767` with the
> app on the dev server id, while production's daemon on `127.0.0.1:6767`
> kept running untouched. `paseo workspace create --isolation local --path`
> registers a disposable project without starting an agent.

> On 2026-09-30, read-only package/bundle inspection reported Paseo 0.10.2. The 0.10.0 CLI/isolation observations below are historical, not qualification of 0.10.2. Recheck the installed CLI, bridge and guarded wrapper before a requested live launch; do not infer unchanged capabilities from the version number.

The installed Linux CLI is `/opt/Paseo/resources/bin/paseo`; a read-only `--version` check on 2026-09-29 returned 0.10.0. Do not assume older 0.9.2 command or isolation observations still apply without checking the installed CLI and bridge. The guarded wrapper's current `--check` was run on 2026-09-29: it selected `/home/igovet/.paseo-dev` and reported `/home/igovet/.paseo` as the protected production home (exit 0). This validates wrapper selection only, not daemon endpoint or GUI isolation.

The permanent development home is `~/.paseo-dev`; use the separate Electron
profile directory `~/.config/Paseo-dev` for desktop validation. The host version
change means historical endpoint values must not be assumed current: check the
active dev daemon configuration and endpoint before any live run. Do not use the
normal Paseo desktop profile for development testing.

Paseo 0.10.0's CLI help (checked 2026-09-29) lists CLI commands and options but no web-client launch command. That alone does not prove whether a daemon-served UI exists; use the Electron app for rendered GUI evidence unless a supported alternative is independently verified.

## Mandatory safety rule

Never update, reinstall, stop, restart, lock, or otherwise modify the production
Paseo home, daemon, or cortex-acp plugin. All Paseo and plugin work must use
`~/.paseo-dev`, `~/.config/Paseo-dev`, and a disposable project. Do not use
`scripts/update-paseo-plugin.sh`: it does not establish a dev-only daemon boundary.

Run Paseo through [scripts/cortex_paseo_dev.py](../../scripts/cortex_paseo_dev.py).
A read-only `--check` on 2026-09-29 selected `/home/igovet/.paseo-dev`, reported `/home/igovet/.paseo` as the protected production home, and exited 0. The wrapper source defaults to `~/.paseo-dev`, refuses the resolved production home, and blocks forwarded `--home` and `--host` overrides. It sets Cortex XDG config/state below the selected dev home and passes the checkout root for the local plugin; CLI calls also discard inherited `PASEO_AGENT_ID` and `PASEO_AGENT_CWD`. The check validates home selection only; it does not launch Paseo or verify the daemon endpoint, daemon state, Electron profile, or GUI isolation.

Until plugin-reload recovery is qualified for the deployed version, apply plugin updates between active tasks. Recovery uses persisted Cortex state and does not automatically repeat actions with an unknown outcome. A host SIGKILL without SIGTERM leaves no disconnect record (a later reader identifies the interrupted worker from its provably gone owning process), and native engine actions have no retained command receipt.

Historical 0.9.2 observation only: a plugin reload with an active worker closed the provider connection, the shim forwarded SIGTERM, Cortex recorded `host_disconnected` with `trigger: sigterm`, and Paseo did not reload the session until the next prompt. This recovery behavior has not been requalified for 0.10.0; do not reproduce it against active work without a separately authorized, isolated check. Start the daemon and the `gui` helper outside the Cortex command sandbox (a Cortex worker uses `outside_command`). A daemon started through `command_run` keeps that command's mount namespace and `TMPDIR`; Cortex deletes the command's scratch, including the directory mounted over `/var/tmp`, when the command ends, so every engine jail the daemon later launches fails in bubblewrap (`Can't bind mount ... on /newroot/var/tmp: ... No such file or directory` in the engine's `stderr.log`) and each agent reports `engine_launch` (live, 2026-10-06). The guarded wrapper therefore refuses `start`, `onboard`, `daemon start`, `daemon run` and `gui` when `/var/tmp` is a Cortex sandbox scratch mount; stop such a daemon and start it again from `outside_command`. The `gui` helper blocks until Electron exits, so run it detached; SIGTERM to the helper stops its Electron and Xvfb.

Development command mounts use the real host `/var/tmp`, unlike restricted
commands and engine jails, which retain their per-run scratch view. Do not infer
that this alone qualifies a long-lived daemon launch or grants paid/live authority:
use the guarded dev wrapper and the explicit owner-selected mode below. Switching
access modes or stopping a CLI does not clean up a daemon, Docker resources or
detached children. See [command profiles](../features/permissions-and-containment/index.md#access-mode-development-guarded-and-auto).

## Paid live testing only on request

Default verification is no-paid. The 2026-09-24 standing approval was revoked
on 2026-09-28. Run a paid live check only when the owner explicitly requests it
for the current work and names the mode: `cli` or `desktop` (see below).
Without a request, report each applicable live check as not run with the
blocker "paid live check not requested by the owner". Run requested checks
through the isolated dev Paseo instance below: the `~/.paseo-dev` daemon, its
separate Electron profile, a disposable project, and the Cortex runtime launched
by that daemon from the explicit absolute checkout plugin path. Never send paid
prompts through production `~/.paseo` or the production Cortex agent.

The existing production Codex and Claude native accounts remain the approved
engine accounts. Do not create test accounts, copy credentials, or run isolated
engine login. Before relying on an engine, check its native login status without
printing credentials. Use only the owner's explicitly selected, currently observed economical settings
route; resolve its exact model and effort from settings and supported CLI options.
Do not substitute a fixed example model or automatically escalate the coordinator.
Heavy models remain prohibited in routine live qualification.

For a requested guarded paid probe, supply both required flags and a reference
naming the owner's request:

~~~bash
scripts/cortex_codex_probe.py --i-authorize-paid-call --approval-ref owner-request-<date>-cli
scripts/cortex_claude_probe.py --i-authorize-paid-call --approval-ref owner-request-<date>-cli
~~~

The default automated test suite remains no-paid. A request does not change the
evidence standard: every live result must be observed and retained for the
exact surface being checked.

## Requested `cli` mode

`cli` mode drives a Cortex agent headlessly through the guarded wrapper, without
Electron. Verify that the installed Paseo CLI help supports the commands and flags used here, and check the dev daemon's actual provider/model configuration before use.
For a later freshly authorized CLI qualification, include startup, exercise,
waits and cleanup within ten minutes as described below; individual waits must
not exceed 60 seconds. This recipe does not authorize a paid launch now. The variables below are placeholders for the owner's
explicitly selected supported settings route, not defaults. Start the daemon
and check the plugin as in the setup section, then, from a
disposable project directory outside this checkout:

~~~bash
W="python3 -B $CHECKOUT/scripts/cortex_paseo_dev.py --"
$W run --provider cortex --model "$MODEL_FROM_SETTINGS" --thinking "$EFFORT_FROM_SETTINGS" \
  --cwd "$PROJECT" --background --json "<scenario prompt>"
$W permit ls --json                      # pending permission requests
$W permit allow <agent-id> <request-id>  # or: permit deny
$W send <agent-id> --prompt "<steering message>" --no-wait
$W wait <agent-id> --timeout 60 --json
$W inspect <agent-id> --json
~~~

Check outcomes against the disposable project's `.cortex/cortex.sqlite3`
store, opened read-only: tasks, reports, review and verification records.
The [bound inspector](../features/observability/index.md#bound-task-inspection) provides a
metadata-only paged route once an exact owned host-to-ACP/task/epoch binding
has actually been observed. It refuses busy/uncheckpointed storage and never
initializes or repairs it. An opaque host agent ID or daemon stop alone is not
binding, engine confirmation or a resource-release receipt.
On 2026-09-28, `logs <agent-id> --json` on 0.9.2 printed a text timeline, not
JSON, so do not parse it as structured evidence; use it only under the
retained-history rules below. A `cli` check establishes runtime and protocol
behavior only; it is never evidence of rendered GUI behavior.

## Create or refresh the persistent clone

Refresh only while the dev daemon is stopped. The clone reads production and writes
only the dev home:

~~~bash
python3 -B scripts/cortex_paseo_clone.py
~~~

The clone utility permits only `~/.paseo` as source and `~/.paseo-dev` as target,
skips sockets, PID files, and locks, never copies production's daemon identity
(top-level `server-id` and `daemon-keypair.json`; it keeps the dev home's own
copies when they differ from production's, otherwise installed Paseo 0.10.2
creates new ones on start), copies normal files to a staging directory, and
uses SQLite's backup API for databases. It atomically replaces the dev home and
preserves the previous dev home as a timestamped rollback directory. It never
writes to production.

After each refresh, set the distinct endpoint and enable plugins before starting:

~~~bash
python3 -B scripts/cortex_paseo_dev.py -- \
  daemon config set daemon.listen 127.0.0.1:16767 --string
python3 -B scripts/cortex_paseo_dev.py -- daemon config set pluginsEnabled true
~~~

## Cortex settings preflight

The wrapper isolates Cortex configuration as well as Paseo. Cloning the Paseo
home does not import the owner's normal Cortex XDG settings. Before starting a
fresh or older dev home, explicitly install missing model settings in **that dev
profile only**:

~~~bash
XDG_CONFIG_HOME="$HOME/.paseo-dev/cortex-xdg/config" \
XDG_STATE_HOME="$HOME/.paseo-dev/cortex-xdg/state" \
python3 -B scripts/cortex_acp_setup.py apply-routing-defaults
~~~

This validated install-only command creates absent routing/default files and
preserves existing settings. It does not repair an existing malformed or legacy
policy; diagnose that policy explicitly instead. Nor does it update a valid policy
written from an older preset: on 2026-10-08 the dev profile still denied bare
`haiku` after the preset had enabled Haiku 5.5, so the dev picker could not show
it. Compare the dev `routing.json` with the `routing` object of
`policy/model-settings-default.json`; to adopt the preset, back the dev file up
privately and write it with
`cortex_runtime.configuration.management.update_routing` under the dev XDG
variables above. Check the dev profile's saved
coordinator model against its enabled models and current connection catalogue
before a paid prompt; do not substitute a different route.

A dev profile can have valid connections and a saved default but no
`routing.json`. Runtime policy loading then fails closed with
`model_policy_migration_required`, the selector offers no routes, and the first
prompt cannot acquire a configuration revision. Paseo may render only the ACP
envelope `[System Error] orchestration_unavailable`; plugin registration and
captured source inputs do not prove coordinator startup. Do not work around this
by using production configuration/state directories.

## Dev instance setup and verification

Before testing, install plugin dev dependencies and pass type-check:

~~~bash
npm --prefix integrations/paseo ci
npm --prefix integrations/paseo run typecheck
~~~

Type-check failures block GUI testing. Always give the plugin directory as a full absolute path when installing it. The plugin is edited in isolated git worktrees,
so the path changes between sessions. Resolve it from the current worktree root.
The guarded wrapper passes that root as `CORTEX_PASEO_SOURCE_ROOT` to the isolated
daemon for local directory-plugin loading. It also sets `XDG_CONFIG_HOME` and
`XDG_STATE_HOME` to `~/.paseo-dev/cortex-xdg/config` and
`~/.paseo-dev/cortex-xdg/state` (or equivalent paths below an explicit dev home).
This prevents a user-level Cortex launch record from selecting a packaged runtime;
the explicit source root takes precedence for this dev-only path.

Start the isolated daemon, install or inspect the explicitly addressed local plugin,
then launch Electron Desktop:

~~~bash
python3 -B scripts/cortex_paseo_dev.py -- start --timeout 60 --json
python3 -B scripts/cortex_paseo_dev.py -- plugin install "$PWD/integrations/paseo" --json
python3 -B scripts/cortex_paseo_dev.py gui
~~~

If the cloned dev home already contains the local plugin record, installation may
report that it is already configured. Inspect `plugin ls --json`; continue only
when the record identifies the current absolute worktree path, is enabled, and
has status `running`. Never install into `~/.paseo`.

For Xvfb use:

~~~bash
python3 -B scripts/cortex_paseo_dev.py gui --xvfb
~~~

The `gui` helper selects `~/.paseo-dev`, the separate
`~/.config/Paseo-dev` Electron profile, binds remote debugging to
`127.0.0.1`, and removes `ELECTRON_RUN_AS_NODE` before launch. By default it
asks the operating system for a free loopback port; pass `--port <port>` to
choose one explicitly. An explicit port already in use fails before launch.
The selected DevTools URL and, with `--xvfb`, the private `DISPLAY` are printed
so a caller can attach. Do not pass direct
Chromium arguments to the Paseo executable; the app daemon manager rejects them.
With `--xvfb`, it starts its own Xvfb using its supported `-displayfd`, `-screen`,
and `-nolisten` flags, gives only the Electron child that new `DISPLAY`, and stops
that Xvfb when the GUI exits. It never inherits a desktop `DISPLAY` in this mode.
The command fails before launching Paseo when Xvfb is absent or cannot provide a
private display.

The installed Paseo calls Electron's `requestSingleInstanceLock()` per `userData`
directory; a second launch on a profile already held by a live instance quits
with status 0 after briefly opening its DevTools port, and Paseo forwards the
launch to the holder as an extra window. The helper therefore reads the
profile's `SingletonLock` before launch and exits 3, naming the holder PID,
when a live local process holds it; it also exits 3 instead of 0 if the lock
was taken by another instance while Electron started. Reuse that instance
through the DevTools URL its own launch printed (a failed second launch can
overwrite the profile's `DevToolsActivePort` file with a dead port) or stop it
by sending SIGTERM to its `gui` helper. `--user-data-dir <absolute path>`
selects another isolated profile; the production `~/.config/Paseo` profile is
refused.

After a GUI check, stop the isolated daemon:

~~~bash
python3 -B scripts/cortex_paseo_dev.py -- daemon stop --timeout 15 --json
~~~

In Paseo Desktop inspect the rendered timeline and the specific behavior under
test. CLI output, logs and CDP inspection alone do not establish a rendered GUI
result. Run the GUI only for a requested `desktop` check.

## Secondary CLI diagnostics

Use the guarded wrapper for daemon health, plugin registration and logs. When
installing, always give the full absolute plugin path:

~~~bash
python3 -B scripts/cortex_paseo_dev.py -- status --json
python3 -B scripts/cortex_paseo_dev.py -- plugin ls --json
python3 -B scripts/cortex_paseo_dev.py -- logs <agent-id> --json
~~~

For retained-history checks, omit `--tail` and retain exit status, output byte
count, structural event count, safe first/last event identities and any explicit
error. The 0.9.2 observation that `--tail`, `--filter` and `--since` existed without pagination must not be carried forward to 0.10.0 without checking its logs command help. If completeness cannot be independently established, mark the check unverified. Do not store message bodies or raw logs
in repository documents or reports.


## Native queue workaround and qualification limit

The owner-selected workflow uses default `sendBehavior: "steer"` plus
Ctrl/Cmd+Enter to enter Paseo's native pre-delivery queue. Native **Edit** restores
a queued item into the existing composer without sending; cancellation is
**Edit**, then clear text and restored attachments. Save any existing draft
first: Edit overwrites it without confirmation, native enqueue trims surrounding
whitespace, and exact user-attachment filtering is unverified. Native queue
persistence on reload is inferred only. Do not change the owner's send behavior
or drafts to test this.

Inspected 0.10.2 `queue` send behavior selects explicit interruption for Send
now and question-card sending; keep `steer`. Cortex's normal prompt/continuation
handoff no longer interrupts unrelated active workers, but explicit Stop/cancel
still must do so. See [the delivery ownership contract](../features/steering-and-questions/index.md#continuation-delivery-ownership).

No Cortex Cancel/Edit UI or composer hook was added. A requested CLI run can
check runtime delivery, question/steering handling and cancellation in the
disposable project; it cannot exercise Ctrl/Cmd+Enter or prove native Edit,
draft replacement, attachment restoration or rendered controls. Desktop was
not requested for the current work, so those interactions remain unqualified.

## Read-only dev binding

With a fresh explicit read grant for the owned dev endpoint PID records and
NEW disposable project, use the exact full UUID/workspace from that owned run:

~~~bash
python3 -B scripts/cortex_paseo_dev.py binding --agent "$AGENT_ID" \
  --workspace "$WORKSPACE_ID" --project "$PROJECT" --timeout 10
~~~

This is a dev-only helper, not a plugin RPC. `binding` has no `--home`, endpoint
or executable override and never auto-starts the daemon, registers a plugin,
launches an engine, sends a prompt or changes settings. Observation timeout is
1–30 seconds with separately bounded owned-child cleanup; output is bounded to
8 KiB. Both private owned PID records must have valid provenance and distinct
listen/PID/serverId; only the fixed dev loopback endpoint is used. Inherited
PASEO variables are removed. Canonical project paths cannot overlap this checkout
or either Paseo home. Permissions to read external files are not implied by this
example; a refusal must not be bypassed.

Anonymous connection only: authentication-required hosts refuse; do not copy
credentials or synthesize an authenticated result. Two exact-agent fetches check
provider/workspace/cwd and the strict existing runtimeInfo wrapper. Two guarded
immutable reads check session/task/current durable owner, with canonical binding
digest validation and before/after endpoint/server checks. No unrelated agent,
workspace or latest-task enumeration occurs. Missing/ambiguous/stale/changed
bindings, busy locks, unsafe/legacy storage and nonempty WAL refuse safely.
Output retains `host`, `binding`, `binding_sha256`, `endpoint_sha256` and explicit
`provenance`; retain the observation as an identity, not a lease or acceptance.
Use its session/task/epoch in [bound task inspection](../features/observability/index.md#bound-task-inspection).

Repository client 0.9.1 fake-wire tests establish the executable helper's
positive/refusal behavior, not real installed-daemon behavior. Static inspection
of installed Paseo/client 0.10.2 found the used exact-agent refresh/current,
server-info and anonymous connection APIs; it does not prove current endpoint,
authentication or owned-agent availability. A future authorized run must observe
those facts afresh. Fixtures/exported metadata do not explain or fix the unknown
cause of an earlier short live run, or prove engine, cycle, disconnect or resource
closure. CLI is never rendered UI evidence.

### Diagnose a refused binding without bypassing it

Every failure keeps `available=false`, exit 1 and no binding.
When the guarded task inspector itself returns a complete, canonical exit-1
refusal, an additional `diagnostic` contains only `stage=task-discovery` and
an allowlisted `reason` (for example `inspection_busy`,
`inspection_uncheckpointed_wal`, `unsafe_storage`,
`legacy_storage_migration_required` or `inspection_binding_mismatch`).
That inspector diagnostic retains `error=binding_storage_refused`. Endpoint
recheck/Python child failures can retain the same outer code but now identify
`stage=endpoint-provenance` versus `task-discovery`, with only fixed local
reasons (`spawn_failed`, `subprocess_failed`, `subprocess_terminated`,
`timeout`, `output_overflow`, `invalid_output`). These are process observations,
not decoded exception messages or proof of a storage condition.

The wrapper separately identifies missing/inaccessible metadata at
`endpoint-provenance` or `endpoint-recheck` (`metadata_unavailable`), unavailable
input paths at `input-validation` (`path_unavailable`), helper startup at
`helper-spawn` (`spawn_failed`), and bounded child supervision at `helper-output`.
For example, missing PID metadata returns `binding_refused` plus
`stage=endpoint-provenance, reason=metadata_unavailable`; it does not establish
whether a daemon is stopped. A helper import failure before its main handler
can only establish `helper-output/subprocess_failed`, not its private cause.

Unknown, malformed, ambiguous, truncated or terminated output supplies no
child-authored reason; the wrapper may still report its own safe process/output
observation. Unknown exception codes remain generic. No exception text, paths, stdout/stderr
bodies, host logs or binding are exposed. This diagnoses a new observation;
it cannot recover the cause of an earlier generic refusal.
Zero child exit is not success by itself: the wrapper requires the complete
canonical envelope, exact identities and binding digest before forwarding it.

Before any dependent live qualification, run the no-paid binding/task-inspection
fixture commands in [verification](verification.md#fresh-process-isolated-targeted-launcher)
under explicitly authorized scratch/dependency access. Passing fixtures prove
helper behavior, not the old host/storage condition or a live identity.

For a later explicitly authorized observation (not an offline diagnostic task):
1. Confirm current installed CLI/bridge isolation and supported read-only commands
   as required above, and validate the checkout artifact. `binding --help` and
   fixture checks do not access a daemon or establish readiness.
2. Obtain scoped read grants for dev endpoint provenance and a NEW owned
   disposable target. With that authority, use
   `python3 -B scripts/cortex_paseo_dev.py -- status --json` to observe readiness
   of the already-running isolated dev daemon, retaining exit/completeness and
   safe identities only. Do not auto-start it; any startup needs a separate
   bounded setup decision that cannot resume unauthorized paid work.
3. Once exact owned agent/workspace/project identities are supported by that
   run, execute the guarded `binding` command above once. Retain its complete
   receipt and diagnostics; only a valid returned binding permits the bound
   metadata inspector. Stop on refusal, incomplete evidence or drift. Do not
   reuse consumed historical target consent or substitute another reader.

A later guarded observation requires explicit read authority and a justified
changed condition, not a retry for reassurance or an alternate SQLite reader:
- Busy locks or nonempty WAL: let the normal runtime connection lifecycle
  produce a checkpointed snapshot; never force a checkpoint or bypass the lock.
- Unsafe/legacy/unsupported storage: stop and obtain a scoped setup/repair
  decision; the inspector never changes modes, migrates or initializes.
- Binding mismatch: establish the correct exact owned host/session/task through
  supported observations. Never substitute a newer task or fabricate ownership.
- Generic refusal: the underlying condition remains unknown. Request narrowly
  scoped retained diagnostics/access rather than inferring sandbox, version,
  corruption or contention.

Do not repeatedly poll a refused target, restart a paid scenario just to get a
different error, or accept a missing reason as success.

## Later ten-minute CLI recipe (not current authorization)

Before the timed phase, finish static source/package/profile checks, targeted
isolated tests, link/help validation and static installed-host API/isolation
inspection. Do not prelaunch a paid task. The timed phase requires fresh owner
CLI authorization, authorized NEW disposable project and explicit read/mutation
grants, backup/digests of the final default settings and dev-only destination,
and observed dev isolation. Production remains untouched. Use the real final
`policy/model-settings-default.json`, not a simplified replacement; preserve
native parameters, ceilings and settings revision/digest bindings.

Start the clock before startup/environment work. By minute 2, all permitted
backup/config/plugin/endpoint readiness and explicit owned route evidence must
be ready to dispatch; otherwise abort before any paid call and clean up. Exercise
only the authorized scenario, retaining exact agent/workspace/session/task/epoch,
selected versus ACTIVE configuration, actual native confirmations and complete
inspection pages/command receipts. Start cleanup no later than 6:30 and finish
by minute 10, with bounded waits (at most 60 seconds each). Retain cleanup,
disconnect and resource-release observations separately: task closure or daemon
stop is not their proof. If readiness, cycle, history or cleanup evidence is
missing, report the run incomplete rather than extending the budget or passing
an absent signal. Restore only the owned dev changes against retained backups
and matching revisions; never touch production or another task's resources.

## Opt-in private wire tracing

For an explicitly authorized dev daemon launch, `--wire-trace` opts in to the
[developer-only trace](../features/observability/index.md#developer-only-wire-tracing).
It is accepted only with `start` or `daemon start` at the canonical
`~/.paseo-dev` home:

```bash
python3 -B scripts/cortex_paseo_dev.py --wire-trace -- start --timeout 60 --json
```

Start between tasks, after the existing dev-daemon stop procedure; starting an
already-running daemon cannot change its environment. The wrapper sets
`CORTEX_DEV_WIRE_TRACE=1` and `CORTEX_DEV_TRACE_HOME` only for this opt-in,
with the existing checkout-source/XDG isolation. Ordinary wrapper invocations
discard inherited trace opt-ins. The runtime refuses installed production
payloads or non-dev state even if these variables are supplied.

The gate's `.git`/`.cortex` ancestor check is scoped to the trace
directory's own subtree, from it up to and including the guarded
`~/.paseo-dev` home itself (W2); your own checkout living under `$HOME`
being a Git repository never disables tracing, but a `.git`/`.cortex`
marker *inside* `~/.paseo-dev` itself still refuses. `--wire-trace start`
preflights this same gate before launching Paseo and fails visibly
(`cortex_paseo_dev: --wire-trace preflight refused (...)`, exit 2) instead
of starting a daemon whose tracing can never activate.

`scripts/cortex_trace_view.py` (the read-only viewer invoked below) shares
that same `_ancestors_up_to` scoping (W3): it only treats a `.git`/`.cortex`
marker inside the guarded dev home as unsafe, never a Git checkout at
`$HOME` itself or above it.

Traces are confined to
`~/.paseo-dev/cortex-xdg/state/cortex/wire-traces/`, outside Git and project
storage, with private ownership/modes and capped rotation. Existing permissive
or symlinked targets stop tracing rather than being changed. The read-only
`python3 -B scripts/cortex_trace_view.py --task "$TASK" --pretty` command
filters surviving records. Read the trace guide for redaction, missing IDs,
rotation and completeness limits. Do not publish raw traces: redaction does
not make private prompts, file/report bodies or thought updates public.

Disable by stopping only the dev daemon and starting it without
`--wire-trace`. No production launch, daemon configuration, plugin install,
paid prompt or GUI qualification is authorized by enabling logging.
