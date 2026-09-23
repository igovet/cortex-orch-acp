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

## 2026-09-23 observed dev instance

A read-consistent clone was created at ~/.paseo-dev; a separate Electron profile
directory exists at ~/.config/Paseo-dev. Its local daemon started successfully at
127.0.0.1:16767. The cloned cortex-acp record identifies the local repository
directory and reports enabled/running. Production remained running at its original
endpoint.

The requested dev log query for agent
6956c56d-b9f4-4a35-a572-b97659773b80 initially exited 1 because its archived
working directory was unavailable. Paseo 0.9.1 has no supported CLI option to
change an agent working directory. Following the user-selected clone-only
procedure, the archived record was backed up in ~/.paseo-dev and its two stored
working-directory fields were changed to the existing disposable directory
~/.paseo-dev-workdirs/6956c56. The dev daemon then inspected the remapped record
with that directory. The log query still exited 1, now with
"Failed to get logs: Unknown sessionId"; the archived provider session is absent
from the clone's Cortex store. This is a retained failure, not successful
history recovery.

The query for edb2bdc7-c125-4790-b159-1c2324f5d9e0 exited 0 and returned its
final retained entry. The returned history is private and is intentionally not
reproduced here. Neither command is GUI confirmation.

The installed desktop accepts the documented
PASEO_ELECTRON_USER_DATA_DIR environment control; it started under the dedicated
~/.config/Paseo-dev profile while reporting only the dev daemon at
127.0.0.1:16767. The supported `agent open` request for 6956c56 returned
"opened". The rendered session history could not be bound to that archive and
the capture was not retained because it included unrelated desktop content.
Therefore GUI recovery for 6956c56 is **NOT RUN**, rather than inferred from
the open acknowledgement. The dev GUI and dev daemon were stopped afterwards.

The cloned configuration retains the production relay/server identity. The installed
CLI observations establish separate local homes and TCP endpoints, but do not
establish a supported way to create an independent relay identity. Relay and
remote pairing were not used. No prompt or paid model call was made.


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
