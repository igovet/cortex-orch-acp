# Persistent isolated dev Paseo environment

## Installed Paseo 0.10.0 boundary

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

Historical 0.9.2 observation only: a plugin reload with an active worker closed the provider connection, the shim forwarded SIGTERM, Cortex recorded `host_disconnected` with `trigger: sigterm`, and Paseo did not reload the session until the next prompt. This recovery behavior has not been requalified for 0.10.0; do not reproduce it against active work without a separately authorized, isolated check. Start the daemon and the `gui` helper outside the Cortex command sandbox (a Cortex worker uses `outside_command`), because the sandboxed runner cannot write the dev home. The `gui` helper blocks until Electron exits, so run it detached; SIGTERM to the helper stops its Electron and Xvfb.

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
printing credentials. Default live model is `gpt-5.6-luna` at medium/high effort;
heavy models remain prohibited in routine live qualification.

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
Start the daemon and check the plugin as in the setup section, then, from a
disposable project directory outside this checkout:

~~~bash
W="python3 -B $CHECKOUT/scripts/cortex_paseo_dev.py --"
$W run --provider cortex --model gpt-5.6-luna --thinking medium \
  --cwd "$PROJECT" --background --json "<scenario prompt>"
$W permit ls --json                      # pending permission requests
$W permit allow <agent-id> <request-id>  # or: permit deny
$W send <agent-id> --prompt "<steering message>" --no-wait
$W wait <agent-id> --timeout 1800 --json
$W inspect <agent-id> --json
~~~

Check outcomes against the disposable project's `.cortex/cortex.sqlite3`
store, opened read-only: tasks, reports, review and verification records.
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
skips sockets, PID files, and locks, copies normal files to a staging directory, and
uses SQLite's backup API for databases. It atomically replaces the dev home and
preserves the previous dev home as a timestamped rollback directory. It never
writes to production.

After each refresh, set the distinct endpoint and enable plugins before starting:

~~~bash
python3 -B scripts/cortex_paseo_dev.py -- \
  daemon config set daemon.listen 127.0.0.1:16767 --string
python3 -B scripts/cortex_paseo_dev.py -- daemon config set pluginsEnabled true
~~~

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
