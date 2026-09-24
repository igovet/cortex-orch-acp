# Cortex repository development

Use `python3`. Runtime behavior belongs entirely below `./`.
Do not activate orchestration merely because this repository is named Cortex;
only an explicit user selection activates the bundled orchestrator skill.

Cortex is an incoming ACP v1 stdio agent (`src/cortex_acp.py`,
`src/cortex_runtime/acp/`), not a plugin hosted inside another
agent's process. See `docs/acp/index.md` for the full section map. T033 removed
the entire Codex-plugin/gateway harness (the MITM gateway, the pinned-dependency
installer, native Codex profile registration, the lifecycle hook dispatcher, the
Codex-native `_meta` adapter, and the `.codex-plugin/` manifest) — see
`docs/acp/packaging-and-onboarding.md`'s "What was Codex-plugin-harness-era and
was removed" for the exact list and `SECURITY.md`'s "Current ACP boundary"
for the current security model.
Treat `docs/acp/*` and `SECURITY.md` as the current product references.

The runtime owns the task lifecycle, dependency graph, scheduling, engine routes,
permissions, mandatory independent review, verification and completion. Models
propose plans, analyze evidence and submit reports/results. Keep tool argument
contracts in advertised schemas and property descriptions, never in skills or
profile instructions. Preserve all 23 profiles (22 specialists and the consultant)
and their shared report protocol. Do not reintroduce model-driven delegation,
acceptance or compatibility routes. Model-originated mutations require dispatched,
channel-verified MCP tools/call; runtime-owned state transitions and events commit
transactionally. The runtime rewrite is incomplete: read
`docs/acp/runtime-rewrite-status.md` before treating this checkout as releasable.

Throughout design, implementation, qualification, and live work, use only
capabilities that are actually supported and observable in the currently installed
host and engine version. Before relying on a capability, verify that it exists in
that current host. Do not require hypothetical or future host APIs, typed contexts,
receipts, hooks, enforcement, models, tools, or transports. If a needed capability
is absent, redesign within current-host mechanisms or report the work genuinely
impossible; do not wait for or request a new host, synthesize evidence, claim a
host enforced a boundary it did not, or mark an attempt accepted without real
evidence. Tests and acceptance scenarios must themselves be executable today.

## Qualification evidence discipline

Apply these rules to ACP qualification evidence:

- Never invent, synthesize, or infer a passing result from an absent signal;
  an unsupported or unobservable signal is reported as such (`not_applicable`
  or a retained diagnostic), never silently treated as success.
- A hard failure, a pending/incomplete state, or mismatched/ambiguous/
  replayed/truncated evidence blocks acceptance; it is never downgraded to a
  diagnostic to let a run pass.
- Evidence must be complete and non-truncated, and must bind to one real,
  exact, non-replayed identity (task, session, artifact, or report) — no
  opaque or duplicated binding stands in for it.
- Report unrun or unverifiable checks explicitly rather than omitting them.
- No mutation, private-target access, or destructive action is ever excused
  by "it was only a diagnostic."

For the current ACP product, the equivalent automated, real-host, real-client
audit tool does not exist yet: live qualification against real ACP clients
(Zed now; codeg deferred) is `docs/acp/real-client-runbook.md`'s manual
observation table, all rows explicitly `NOT RUN` as of this writing, owned by
T035–T038 (see `docs/acp/test-matrix-and-ci.md` for what the automated test
suite can and cannot close). Do not invent a stand-in procedure or claim a row
passed without a real, retained observation; state plainly that this is
unverified where it is.

Use one real `pipeline.md` per task, newest edition first with older editions
below. Other reports are immutable real Markdown files. Both published task
documents and the SQLite metadata/relations index live under the one
project-local `.cortex` store (`<project>/.cortex/cortex.sqlite3`, published
document bodies under `<project>/.cortex/` alongside it); there is no
`.codex/cortex` anywhere. `tests/test_package.py` asserts exactly one
`.cortex/cortex.sqlite3` per project. Both the newest-first catalogue and
report content use bounded cursor reads. Workers select relevant reports rather
than reading everything.

When a normal writable `Store(..., initialize=True)` opens a project-local
format-11 database, it automatically performs the verified format-11 to
format-12 migration under the exclusive access lock and retains a private
backup in `.migration-backups/`. A non-initializing inspection open
(`initialize=False`) must remain non-mutating and report
`legacy_storage_migration_required`; format 10 and unknown formats remain
fail-closed and require the explicit migration tool where supported.

Read relevant `docs/acp/` pages before nontrivial work; they define the current
product contract. Also read applicable `docs/project/` and `docs/features/`
pages as supplementary current summaries. The paths in the knowledge-harvest
skill also define output conventions for a harvested target project.
After behavior changes update README.md, SECURITY.md and affected documentation;
check links and commands against source. Keep secrets, private reports and raw
host logs out of repository documents and diagnostics. Report unrun checks.

For this release use semantic version 1.0.0. After any package payload
edit, regenerate its complete content hash with
`python3 -B scripts/cortex_package.py stamp` before release-sensitive checks.
Run package, profile-generation and test checks sequentially in the same
checkout. Use the smallest meaningful tests first. Source-only checks use
`--check` or `--dry-run`.

## Current ACP-era live/model-policy rules

Real-client qualification uses a throwaway Zed profile, isolated Zed
`HOME`/XDG directories, and a separate disposable project. Do not change the
user's stable Zed settings or session. Codex and Claude use the user's existing
production native accounts: the test Zed's Cortex agent entry overrides only
its child process's `HOME` with the real user home while the Zed process and
Cortex XDG config/state remain disposable. Do not create test engine accounts,
copy credentials, or run an isolated engine login. Check native login status
without printing credentials before relying on either engine.

The owner's standing approval authorizes paid model calls by default for
applicable live verification. Run live checks needing paid calls only through
the isolated dev Paseo setup: the permanent `~/.paseo-dev` daemon, its separate
Electron profile at `~/.config/Paseo-dev`, a disposable project, and the Cortex
runtime launched by that daemon from the explicit absolute checkout plugin path.
Never use production `~/.paseo` or the production Cortex agent. Preserve the
production-account rules above. Use `gpt-5.6-luna` at medium/high effort; heavy
models remain prohibited in routine live qualification. Check native login
status without printing credentials before relying on either engine.

The no-paid default automated suite
(`PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q`, see
`docs/acp/test-matrix-and-ci.md`) cannot reach a paid engine and is not a
substitute for live qualification. The only paid probes are
`scripts/cortex_codex_probe.py` and `scripts/cortex_claude_probe.py`; both
continue to require `--i-authorize-paid-call` and `--approval-ref`. The
standing approval authorizes using approval reference
`owner-standing-2026-09-24`. `scripts/cortex_acp_smoke.py` is the no-paid
startup smoke harness (schema-validated launch/protocol surface only, not GUI
evidence).

Real-client qualification uses the manual observation table in
`docs/acp/real-client-runbook.md` (Zed now; codeg deferred), scoped by
`docs/acp/test-matrix-and-ci.md`'s 45 live-only acceptance cases. As of this
writing every row in that runbook is `NOT RUN`; this file does not know of a
completed real-host qualification and must not be edited to claim one. Do not
invent a substitute procedure or claim an unobserved runbook row passed.

This release-verification obligation belongs to the owner of the Cortex
change. Run live workloads against a separate isolated project, never against
this checkout itself.

## Paseo development and plugin safety

Never update, reinstall, or otherwise modify the production Paseo installation,
~/.paseo, or its cortex-acp plugin. All Paseo/plugin validation must use only the
permanent dev instance at ~/.paseo-dev, its separate Electron profile
~/.config/Paseo-dev, a distinct daemon endpoint, and a disposable project. Refresh
that dev home only with python3 -B scripts/cortex_paseo_clone.py while its dev daemon
is stopped; the production home is read-only to that utility. Before using an
isolation flag or environment variable, verify it against the installed Paseo version
and ACP bridge. Follow [the dev Paseo environment guide](docs/acp/dev-paseo-environment.md).
For verified Linux 0.9.1 commands, invoke python3 -B scripts/cortex_paseo_dev.py --
...; it defaults to ~/.paseo-dev, rejects ~/.paseo, and blocks command-level endpoint
overrides. If the installed build cannot be shown to isolate every required state and
the daemon endpoint, do not launch Paseo or install the plugin; report the missing
evidence.

### Dev Paseo GUI verification

Use the persistent isolated dev home with the Paseo Electron Desktop application as
the standard GUI validation mode. Paseo 0.9.1 does not provide a browser-based web
client or local HTTP UI; its daemon exposes a WebSocket endpoint, which is not a
browser UI. The installed `paseo --help` lists CLI commands and the plugin
reference lists directory, Git and npm plugin sources. No supported web-client
launch command or daemon-served UI was found. Use the Electron app connected to
the isolated dev daemon for rendered behavior. CLI is secondary and covers
daemon health, plugin registration and logs only.

Before testing, install plugin dev dependencies and pass type-check:

~~~bash
npm --prefix integrations/paseo ci
npm --prefix integrations/paseo run typecheck
npm --prefix integrations/paseo run test:activation
npm --prefix integrations/paseo run test:steer
~~~

Type-check failures block GUI testing. Always supply the plugin directory as a
full absolute path during testing. The plugin is edited in isolated git worktrees,
so its path changes between sessions. Resolve it from the current worktree root,
for example:

~~~bash
python3 -B scripts/cortex_paseo_dev.py -- start --timeout 60 --json
python3 -B scripts/cortex_paseo_dev.py -- plugin install "$PWD/integrations/paseo" --json
python3 -B scripts/cortex_paseo_dev.py gui
~~~

For Xvfb environments use:

~~~bash
python3 -B scripts/cortex_paseo_dev.py gui --xvfb
~~~

The `gui` helper selects ~/.paseo-dev, the separate ~/.config/Paseo-dev Electron
profile, binds remote debugging to 127.0.0.1 on a free port it selects by default
(or the explicit `--port`), and removes ELECTRON_RUN_AS_NODE before launch. A
busy explicit port fails before launch. It prints the selected DevTools URL and
private DISPLAY when using --xvfb. With `--xvfb`, it starts a private Xvfb on a free display, sets that
display only for the Electron child, and stops the Xvfb when the GUI exits; it never
inherits the user desktop display in that mode. The wrapper passes the current
worktree root to the dev daemon for the local plugin runtime. After the GUI check,
stop the dev daemon:

~~~bash
python3 -B scripts/cortex_paseo_dev.py -- daemon stop --timeout 15 --json
~~~

In Paseo Desktop, inspect the rendered timeline and the specific UI behavior under
test. CLI output, logs and CDP inspection alone do not establish a rendered GUI
result. Use CLI through the guarded wrapper for secondary diagnostics, with the
plugin path explicit when installing:

~~~bash
python3 -B scripts/cortex_paseo_dev.py -- status --json
python3 -B scripts/cortex_paseo_dev.py -- plugin ls --json
python3 -B scripts/cortex_paseo_dev.py -- logs <agent-id> --json
~~~

For retained-history checks, omit `--tail` and retain the exit status, output
byte count, structural event count, safe first/last event identities and any
explicit error. Paseo 0.9.1 documents `--tail`, `--filter` and `--since` for
logs, but no pagination or full-history flag; if completeness cannot be independently
established, mark that check unverified. Do not store message bodies or raw logs in
repository documents or reports.
