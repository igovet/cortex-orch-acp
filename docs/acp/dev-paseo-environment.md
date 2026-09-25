# Persistent isolated dev Paseo environment

## Verified Paseo 0.9.1 boundary

The installed Linux CLI is `/opt/Paseo/resources/bin/paseo`, version 0.9.1.
Its help documents `--home <path>` for the local daemon home and `--host <host>`
for an explicit endpoint. The supported daemon config set, start, status,
plugin install, and logs commands accept `--home`.

Production is `~/.paseo` on `127.0.0.1:6767`. The permanent development home is
`~/.paseo-dev`; its daemon is configured for `127.0.0.1:16767`. Use the separate
Electron profile directory `~/.config/Paseo-dev` for desktop validation. Do not use
the normal Paseo desktop profile for development testing.

Paseo 0.9.1 provides no supported browser client or local HTTP UI. The daemon
uses WebSocket; its CLI help lists daemon and plugin commands, but no web-client
launcher. Use the Electron Desktop application as the standard GUI method.

## Mandatory safety rule

Never update, reinstall, stop, restart, lock, or otherwise modify the production
Paseo home, daemon, or cortex-acp plugin. All Paseo and plugin work must use
`~/.paseo-dev`, `~/.config/Paseo-dev`, and a disposable project. Do not use
`scripts/update-paseo-plugin.sh`: it does not establish a dev-only daemon boundary.

Run Paseo through [scripts/cortex_paseo_dev.py](../../scripts/cortex_paseo_dev.py).
It defaults to `~/.paseo-dev`, rejects the resolved production home, and rejects
per-command `--home` and `--host` overrides. It makes no model call.

## Standing approval for paid live testing

The owner has granted standing approval for paid model calls required by live
verification. Run those calls through the isolated dev Paseo instance below:
the `~/.paseo-dev` daemon, its separate Electron profile, a disposable project,
and the Cortex runtime launched by that daemon from the explicit absolute
checkout plugin path. Never send paid prompts through production `~/.paseo`
or the production Cortex agent.

The existing production Codex and Claude native accounts remain the approved
engine accounts. Do not create test accounts, copy credentials, or run isolated
engine login. Before relying on an engine, check its native login status without
printing credentials. Default live model is `gpt-5.6-luna` at medium/high effort;
heavy models remain prohibited in routine live qualification.

For the guarded paid probe scripts, supply both required flags and the standing
reference `owner-standing-2026-09-24`:

~~~bash
scripts/cortex_codex_probe.py --i-authorize-paid-call --approval-ref owner-standing-2026-09-24
scripts/cortex_claude_probe.py --i-authorize-paid-call --approval-ref owner-standing-2026-09-24
~~~

This approval does not add paid calls to the default automated test suite, which
remains no-paid. It does not change the evidence standard: every live result must
be observed and retained for the exact surface being checked.

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
After a GUI check, stop the isolated daemon:

~~~bash
python3 -B scripts/cortex_paseo_dev.py -- daemon stop --timeout 15 --json
~~~

In Paseo Desktop inspect the rendered timeline and the specific behavior under
test. CLI output, logs and CDP inspection alone do not establish a rendered GUI
result.

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
error. Paseo 0.9.1 documents `--tail`, `--filter` and `--since` for logs, but
no pagination or full-history flag. If completeness cannot be independently
established, mark the check unverified. Do not store message bodies or raw logs
in repository documents or reports.
