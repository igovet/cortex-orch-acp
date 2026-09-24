# Persistent isolated dev Paseo environment

## Verified Paseo 0.9.1 boundary

The installed Linux CLI is /opt/Paseo/resources/bin/paseo, version 0.9.1.
Its help documents --home <path> for the local daemon home and --host <host>
for an explicit endpoint. The supported daemon config set, start, status,
plugin install, and logs commands all accept --home.

Production is ~/.paseo on 127.0.0.1:6767. The permanent development home is
~/.paseo-dev; its daemon is configured for 127.0.0.1:16767. Use the separate
Electron profile directory ~/.config/Paseo-dev for any desktop launch. Do not use
the normal Paseo desktop profile for development validation.

## Mandatory safety rule

Never update, reinstall, stop, restart, lock, or otherwise modify the production
Paseo home, daemon, or cortex-acp plugin. All Paseo and plugin work must use
~/.paseo-dev, ~/.config/Paseo-dev, and a disposable project. Do not use
scripts/update-paseo-plugin.sh: it does not establish a dev-only daemon boundary.

Run Paseo through [scripts/cortex_paseo_dev.py](../../scripts/cortex_paseo_dev.py).
It defaults to ~/.paseo-dev, rejects the resolved production home, and rejects
per-command --home and --host overrides. It makes no model call.

## Create or refresh the persistent clone

Run this only while the dev daemon is stopped. It reads production and writes only
the dev home:

~~~bash
python3 -B scripts/cortex_paseo_clone.py
~~~

The clone utility permits only ~/.paseo as source and ~/.paseo-dev as target,
skips sockets, PID files, and locks, copies normal files to a staging directory, and
uses SQLite's backup API for SQLite databases. It atomically replaces the dev home
and preserves the previous dev home as a timestamped rollback directory. It never
writes to the production home.

After every refresh, set the distinct endpoint and enable plugins before starting:

~~~bash
python3 -B scripts/cortex_paseo_dev.py -- \
  daemon config set daemon.listen 127.0.0.1:16767 --string
python3 -B scripts/cortex_paseo_dev.py -- daemon config set pluginsEnabled true
python3 -B scripts/cortex_paseo_dev.py -- start --timeout 60 --json
python3 -B scripts/cortex_paseo_dev.py -- \
  plugin install "$PWD/integrations/paseo" --json
python3 -B scripts/cortex_paseo_dev.py -- plugin ls --json
python3 -B scripts/cortex_paseo_dev.py -- status --json
~~~

If the clone already contains the local cortex-acp directory-plugin record,
plugin install returns “already configured”; inspect plugin ls --json and
continue only when it identifies $PWD/integrations/paseo, is enabled, and has
status: "running". Never install into ~/.paseo.

## Dev instance setup and verification

The persistent dev home at ~/.paseo-dev and separate Electron profile directory
at ~/.config/Paseo-dev enable GUI testing that mirrors the production Paseo experience.
The local daemon listens on 127.0.0.1:16767 via WebSocket. Paseo 0.9.1 does not serve
a local HTTP web UI; the Electron Desktop application is the primary GUI method for
developer testing.

Before dev testing, ensure integrations/paseo dev dependencies are installed and
type-check passes:

~~~bash
npm --prefix integrations/paseo ci
npm --prefix integrations/paseo run typecheck
~~~

Type-check failures block developer testing; resolve them before starting the GUI.

**The plugin path must always be given explicitly when installing into the dev daemon**
because the plugin is edited in isolated git worktrees and its path changes between
sessions. Use the full absolute path from the current worktree root.
The guarded wrapper also passes that root as `CORTEX_PASEO_SOURCE_ROOT` to its
isolated Paseo process. Paseo 0.9.1 evaluates server plugins as CommonJS bundles,
so the provider cannot derive its source path from `import.meta.url`. A Git-source
installation uses its private launch record before this development fallback.

**Critical environment variables for isolation:**

The Paseo daemon manager (app.asar) defaults PASEO_HOME to ~/.paseo (production) when unset.
To guarantee isolation, always set these three variables.

Start the daemon first:

~~~bash
python3 -B scripts/cortex_paseo_dev.py -- start --timeout 60 --json
python3 -B scripts/cortex_paseo_dev.py -- plugin install "$PWD/integrations/paseo" --json
~~~

**Option 1: Using the helper subcommand (recommended for safety)**

The `cortex_paseo_dev.py gui` subcommand sets all required environment variables automatically
and refuses production homes:

~~~bash
python3 -B scripts/cortex_paseo_dev.py gui
~~~

For Xvfb environments:

~~~bash
python3 -B scripts/cortex_paseo_dev.py gui --xvfb
~~~

**Option 2: Manual environment setup**

If the calling shell inherited `ELECTRON_RUN_AS_NODE` (e.g. from a Node-based
tool), unset it first; Option 1's `gui` subcommand already does this for you,
but a manual launch that leaves it set makes the Paseo executable reject its
own flags as Node arguments and exit non-zero:

~~~bash
unset ELECTRON_RUN_AS_NODE
PASEO_HOME=$HOME/.paseo-dev PASEO_ELECTRON_USER_DATA_DIR=$HOME/.config/Paseo-dev PASEO_ELECTRON_FLAGS="--remote-debugging-address=127.0.0.1 --remote-debugging-port=9222" /opt/Paseo/Paseo
~~~

For Xvfb environments, add `--ozone-platform=x11` to PASEO_ELECTRON_FLAGS:

~~~bash
PASEO_HOME=$HOME/.paseo-dev PASEO_ELECTRON_USER_DATA_DIR=$HOME/.config/Paseo-dev PASEO_ELECTRON_FLAGS="--remote-debugging-address=127.0.0.1 --remote-debugging-port=9222 --ozone-platform=x11" /opt/Paseo/Paseo
~~~

**Important:** Do not pass direct Chromium argv to the Paseo executable; it is rejected by the app.asar daemon manager. Use PASEO_ELECTRON_FLAGS only.

The Electron Desktop application connects to the dev daemon and renders the agent
timeline, worker results, and tool presentation as users experience them. This provides
evidence for GUI behavior, tool expansion, worker status display, and form input
handling that CLI commands cannot show.

**Debugging with Chrome DevTools Protocol (CDP):**

The PASEO_ELECTRON_FLAGS setting includes `--remote-debugging-address=127.0.0.1 --remote-debugging-port=9222`, which enables Chrome DevTools Protocol inspection. Connect to `http://127.0.0.1:9222` to inspect the Paseo window, capture screenshots, and verify GUI rendering programmatically using tools like Playwright or the Chrome DevTools client.

The cloned configuration retains the production relay/server identity. Relay and remote
pairing were not used in this setup. No prompt or paid model call was made.


## Standard CLI verification mode

Use CLI verification through the persistent dev home as the standard Paseo test
mode when the result does not require a rendered-client claim. Start, inspect, and
stop only the isolated daemon through the guarded wrapper:

~~~bash
python3 -B scripts/cortex_paseo_dev.py -- start --timeout 60 --json
python3 -B scripts/cortex_paseo_dev.py -- status --json
python3 -B scripts/cortex_paseo_dev.py -- plugin ls --json
python3 -B scripts/cortex_paseo_dev.py -- logs <agent-id> --json
python3 -B scripts/cortex_paseo_dev.py -- daemon stop --timeout 15 --json
~~~

For a retained-history check, omit `--tail` and retain the command exit status,
output byte count, structural event count, safe first/last event identities, and any
explicit error. Do not put message bodies or raw logs in documentation or reports.
This establishes CLI access to the selected dev daemon and its cloned records; it
does not establish rendered history, tool expansion, visual labels, keyboard focus,
or another GUI behavior.

Paseo 0.9.1 documents `--tail`, `--filter`, and `--since` for `logs`, but no
pagination or full-history flag. If an untailed response cannot be independently
shown complete, mark the history criterion **NOT RUN** or unverified; do not infer
that it contains every stored event. Stop the dev daemon after every check.

## GUI qualification

The rendered history of either requested archive was not established. The
"Completed with issues" status across all three requested views, model-name
display across the requested surfaces, and Codex shell expansion remain
**NOT RUN** because no identified existing session was rendered and inspected
at those surfaces. CLI history retrieval is not a substitute for a rendered-client
observation.

Stop only the selected dev daemon when needed:

~~~bash
python3 -B scripts/cortex_paseo_dev.py -- daemon stop --timeout 15 --json
~~~
