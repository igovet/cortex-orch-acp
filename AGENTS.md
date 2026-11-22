# Cortex repository development

The product is Rust and contains no Python. Use the pinned Rust toolchain (`rust-toolchain.toml`) through `~/.cargo/bin/cargo` and run builds and tests under `umask 022`. Runtime behavior belongs entirely below `./`.
Do not activate orchestration merely because this repository is named Cortex;
only an explicit user selection activates the bundled orchestrator skill.

Cortex is an incoming ACP v1 stdio agent (the Rust binary `cortex-acp`,
`crates/cortex-acp`), not a plugin hosted inside another
agent's process. See `docs/project/architecture.md` for the full section map.
The Codex-plugin/gateway harness (the MITM gateway, the pinned-dependency
installer, native Codex profile registration, the lifecycle hook dispatcher, the
Codex-native `_meta` adapter, and the `.codex-plugin/` manifest) is not part of
the product. `SECURITY.md`'s "Current ACP boundary" describes the current security model.
Treat `docs/project/index.md`, `docs/features/index.md` and `SECURITY.md` as the current product references.
Documentation never references tasks, assignments or task reports.

The runtime owns the task lifecycle, dependency graph, scheduling, engine routes,
permissions, mandatory independent review, verification and completion. Models
propose plans, analyze evidence and submit reports/results. Keep tool argument
contracts in advertised schemas and property descriptions, never in skills or
profile instructions. One narrow exception: the `codex-acp` tool cheat sheet
(`compose` in `crates/workspace-service/src/native_instructions.rs`) may restate required
and optional fields, enum values, array item fields, schema-derived numeric/length
bounds and an example call, because it is rendered mechanically from the live
channel tool schemas (`schema::catalogue` in `crates/cortex-engine/src/schema.rs`) at session start and appended only to `codex-acp`
sessions, whose code mode lists nested tools by name and description only. It
is derived output, not authored prose: never hand-edit contract text into it,
and it does not reintroduce model-driven delegation. Preserve all 23 profiles (22 specialists and the consultant)
and their shared report protocol. Do not reintroduce model-driven delegation,
acceptance or compatibility routes. Model-originated mutations require dispatched,
channel-verified MCP tools/call; runtime-owned state transitions and events commit
transactionally. Release readiness is decided only by the checks listed in
`docs/project/verification.md`; do not treat a checkout as releasable without them.

Throughout design, implementation, qualification, and live work, use only
capabilities that are actually supported and observable in the currently installed
host and engine version. Before relying on a capability, verify that it exists in
that current host. Do not require hypothetical or future host APIs, typed contexts,
receipts, hooks, enforcement, models, tools, or transports. If a needed capability
is absent, redesign within current-host mechanisms or report the work genuinely
impossible; do not wait for or request a new host, synthesize evidence, claim a
host enforced a boundary it did not, or mark an attempt accepted without real
evidence. Tests and acceptance scenarios must themselves be executable today.

Tasks finish without self-made blockers. The runtime records host facts at
task start (GitHub CLI and native logins by exit code, passwordless or
password-needed sudo, free space, display, Docker, Git state) into the
coordinator's capsule; plan around them from the first turn instead of
discovering them late, and treat a gap as a fact to work around, not a stop.
Project rule files (`AGENTS.md`, `CLAUDE.md`, `CLAUDE.local.md`, `GEMINI.md`,
`.agents/`, `.claude/`, `docs/project/verification.md`) are write-protected for
every task unless the owner's task or message names the file or the owner sends
`allow-write: <path>`; propose rule changes in the result instead. Reuse built
packages; `build-deb` keeps only the newest two builds and caches per channel
(`scripts/cortex-xtask.sh clean-builds`), and `target/` is cleaned only with an
explicit `cargo clean`.

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


Use one real `pipeline.md` per task, newest edition first with older editions
below. Other reports are immutable real Markdown files. Both published task
documents and the SQLite metadata/relations index live under the one
project-local `.cortex` store (`<project>/.cortex/cortex.sqlite3`, published
document bodies under `<project>/.cortex/` alongside it); there is no
`.codex/cortex` anywhere. The store layer (`crates/cortex-core/src/store`) keeps exactly one
`.cortex/cortex.sqlite3` per project in the default/release channel. Workspace Dev uses the separate `.cortex-dev/cortex.sqlite3` beside it: exactly one store per logical project per channel, including linked worktrees. Dev never imports, reads or migrates the release store. Both the newest-first catalogue and
report content use bounded cursor reads. Workers select relevant reports rather
than reading everything.

When the Rust store (`crates/cortex-core`) opens a project-local format-11
database with `Store::open(.., OpenMode::Initialize)`, it automatically performs
the verified format-11 to format-12 migration under the exclusive access lock
and retains a private backup in `.migration-backups/`. An
`OpenMode::InspectImmutable` (or `LiveReadOnly`) open must remain non-mutating
and report `legacy_storage_migration_required`; format 10 and unknown formats
remain fail-closed (`unsupported_storage`). No separate migration or rollback
command ships. Initialize never opens an existing database file with a plain
descriptor (closing it would drop the process's POSIX locks and let another
process delete the live WAL/SHM); see `create_private_database` in
`crates/cortex-core/src/store/locks.rs`.

Read relevant `docs/project/` and `docs/features/` pages before nontrivial work; they define the current
product contract. Also read applicable `docs/project/` and `docs/features/`
pages as supplementary current summaries. The paths in the knowledge-harvest
skill also define output conventions for a harvested target project.
After behavior changes update README.md, SECURITY.md and affected documentation;
check links and commands against source. Keep secrets, private reports and raw
host logs out of repository documents and diagnostics. Report unrun checks.

For this release use semantic version 1.0.6. After any package payload
edit, regenerate its complete content hash with
`scripts/cortex-xtask.sh package stamp` before release-sensitive checks.
Run package, profile-generation and test checks sequentially in the same
checkout. Before handing any desktop .deb to the owner, require the exact archive to pass the installed-package real-input gate in docs/project/verification.md, sequentially on disposable virtual screens, never in the owner’s session. Use the smallest meaningful tests first. Source-only profile checks use
`scripts/cortex-xtask.sh profiles check` (it takes one positional action,
`check` or `write`; `check-generated` runs the contract, instruction and profile
checks together, and `package check` validates the stamp).

## Live and model-policy rules

Real-client qualification uses a throwaway Zed profile, isolated Zed
`HOME`/XDG directories, and a separate disposable project. Do not change the
user's stable Zed settings or session. Codex and Claude use the user's existing
production native accounts: the test Zed's Cortex agent entry overrides only
its child process's `HOME` with the real user home while the Zed process and
Cortex XDG config/state remain disposable. Do not create test engine accounts,
copy credentials, or run an isolated engine login. Check native login status
without printing credentials before relying on either engine.

Default verification is no-paid: targeted `cargo test` runs, the Paseo plugin type-check
and `npm --prefix integrations/paseo run test:*` scripts, and the no-paid smoke
harness. No standing approval for paid live checks exists. Run one only on an
explicit owner request for the current work naming the mode and the surface,
and record an approval reference naming that request. Without such a request,
do not launch a paid model call or a live Paseo run; report each applicable
live check as not run with the blocker "paid live check not requested by the
owner". A request covers only the named work, mode and surface.

- **Paseo:** `cli` (guarded headless CLI) or `desktop` (Electron), only through
  the permanent isolated `~/.paseo-dev` daemon, separate
  `~/.config/Paseo-dev` Electron profile, a disposable project, and the Cortex
  runtime launched by that daemon from the explicit absolute checkout plugin
  path. Never use production `~/.paseo` or the production Cortex agent.
- **Standalone Cortex Workspace:** `desktop` only, explicitly requested for
  the standalone 1.0.6 app. Test the exact **Dev channel** archive
  (`cortex-workspace-dev`) in the owner's real environment using the
  [desktop procedure](docs/project/verification.md#standalone-cortex-workspace-desktop):
  real HOME, keyring and existing native/gh logins, private per-run Dev XDG
  roots, a private authenticated Xvfb (smoke alone may use the owner's display),
  and a separate disposable project. There is no nested sandbox lab. The Dev
  archive is tested by extraction without `sudo`; installing it is only for a
  scenario that needs the installed package. Never install, modify or restart the release package or
  service, never touch production `~/.paseo`, and never change the owner's
  projects, settings, registrations or stores. Observe the real window with XTEST input
  and screenshots or recordings.

On either route, preserve the production-account rules above: no copied
credentials, test accounts or engine login; check native login status without
printing credentials. Use only an explicit owner-selected engine/model route
supported by current settings and observed host data; do not substitute or
automatically escalate. Heavy models remain prohibited in routine live
qualification.

The no-paid default automated suite
(`scripts/cortex-xtask.sh test-fast`, see
`docs/project/test-matrix.md`) cannot reach a paid engine and is not a
substitute for live qualification. The only paid Paseo probes are
`scripts/cortex-paseo-tools.sh probe-codex` and `probe-claude`; both
continue to require `--i-authorize-paid-call` and `--approval-ref`; run them
only on an explicit owner request and pass a reference naming that request
(for example `owner-request-<date>-cli`). `scripts/cortex-paseo-tools.sh acp-smoke` is the no-paid
startup smoke harness (schema-validated launch/protocol surface only, not GUI
evidence).

The default runner runs its steps one at a time under GNU `timeout`
(`--kill-after=5`; 900 s for build and cargo, 600 s for npm, 180 s per Paseo
script), passes a 60-second per-test deadline to the Vitest suites, points
`HOME`/`XDG_*` at a scratch directory and runs cargo `--locked --offline`.
A missing prerequisite (cargo, npm, installed `node_modules`, GNU `timeout`)
fails explicitly with exit 2. Do not run an unbounded whole-workspace test
command. Targeted `cargo test` commands must also use GNU `timeout`; keep
package, profile and test checks sequential.


This release-verification obligation belongs to the owner of the Cortex
change. Run live workloads against a separate isolated project, never against
this checkout itself.

### Standalone desktop testing procedure

Use the [binding verification procedure](docs/project/verification.md#standalone-cortex-workspace-desktop).
These commands run from the checkout through `outside_command` on the real
host as the owner; Auto handles consent.

```bash
# Host facts only (display, logins, sudo, Docker, free space, tools); never fails on a gap:
bash scripts/qualify-dev-deb.sh probe
# No-paid smoke; reuse the newest local Dev archive instead of rebuilding:
bash scripts/qualify-dev-deb.sh smoke --archive newest
bash scripts/qualify-dev-deb.sh smoke
# Only with explicit authorization for the named paid scenario:
# DIR is the evidence directory of a successful private-Xvfb smoke of the same archive:
bash scripts/qualify-dev-deb.sh drive --archive newest --smoke-evidence .workspace-build/dev-evidence.XXXXXXXX -- /absolute/path/to/foreground-scenario-driver
# Only if a scenario truly needs the installed Dev package (not the default; asks sudo):
bash scripts/qualify-dev-deb.sh install --archive newest
# Delete named local Dev build pairs once they are no longer needed:
bash scripts/qualify-dev-deb.sh cleanup build-XXXXXXXX
# Disk hygiene (keeps the newest per channel; also automatic after every successful build):
scripts/cortex-xtask.sh clean-builds --dry-run
```

The default way to test a .deb needs no `sudo` and no owner action: the launcher
extracts the archive with `dpkg-deb -x` into a per-run scratch directory (no
maintainer scripts, no dpkg registration), runs its service and desktop there
under private Dev roots, and removes the scratch directory at teardown. Build an
archive once, test it by extraction, and delete builds you no longer need
(`cleanup`, `cortex-xtask.sh clean-builds`).

Drive always runs on a private authenticated Xvfb (never the owner's display);
`--real-display` is for smoke only. Xvfb, xauth, mcookie, xdpyinfo, xdotool and xprop
are required, and a missing one exits 1. View PR addresses are recorded as digests in
`opener.sha256` and never handed to a browser (`--capture-opener` is accepted but
unnecessary).

The launcher verifies the exact archive (package name, manifest SHA-256,
bundled engines), extracts it without maintainer scripts and starts its service
and desktop with private Dev XDG roots and the owner's real HOME, session bus
and gh configuration. It unsets `CORTEX_ROUTING`. Smoke sends no task: it reads
back machine settings, the observed light route and the matching portable
default over the run's own socket, clicks the window and keeps a screenshot.
Before paying, the driver repeats the settings and route/default readback on
its own socket, confirms the route in the rendered app and checks native login
by exit code without printing credentials. A desktop window does not qualify
phone-width behavior; View PR needs an accepted task with a PR, Update needs
`taskWorktrees`, and older pages need a long enough conversation.

Environment problems are facts, not stops. A missing display, tool, login or
passwordless sudo is recorded (exit 3 lists the unobserved steps) and the task
adapts: use a private Xvfb, install what is missing,
ask the owner to sign in through the question route while other work
continues, or report the step as not run. Never present an unobserved step as
a pass. Build an archive once and reuse it (`--archive newest`); do not rebuild
packages repeatedly, and never write isolation infrastructure for a run. Do not
edit this file or `docs/project/verification.md` unless the owner's task names
them.

## Paseo development and plugin safety

Never update, reinstall, or otherwise modify the production Paseo installation,
~/.paseo, or its cortex-acp plugin. All Paseo/plugin validation must use only the
permanent dev instance at ~/.paseo-dev, its separate Electron profile
~/.config/Paseo-dev, a distinct daemon endpoint, and a disposable project. Refresh
that dev home only with `scripts/cortex-paseo-tools.sh paseo-clone` while its dev daemon
is stopped; the production home is read-only to that utility. Before using an
isolation flag or environment variable, verify it against the installed Paseo version
and ACP bridge. Follow [the dev Paseo environment guide](docs/project/dev-environment.md).
A read-only `/opt/Paseo/resources/bin/paseo --version` check on 2026-10-14 reported
installed Paseo 0.11.2; CLI/isolation observations recorded for 0.10.x and 0.9.x are
historical and do not qualify 0.11.2.
A read-only `--version` check on 2026-10-16 reported installed Paseo 0.11.4, and its guarded CLI help matched the `run`, `send`, `wait` and `inspect` options recorded in the dated note of docs/project/dev-environment.md. This does not requalify 0.11.2 or the historical 0.10.x and 0.9.x observations.
A read-only `--version` check on 2026-10-23 reported installed Paseo 0.12.0, and the guarded wrapper's `run`, `send`, `permit`, `wait` and `inspect` help each exited 0 with the options recorded in the dated note of docs/project/dev-environment.md. An owner-requested paid `cli` qualification that day, through the isolated `~/.paseo-dev` daemon and a disposable project, passed the lifecycle/review/verification, model-remark consent and question/answer resume scenarios. This qualifies CLI runtime/protocol behavior on 0.12.0 only: rendered desktop/GUI behavior was not run, and it does not requalify the 0.11.x, 0.10.x or 0.9.x observations.
A read-only `--version` check on 2026-11-03 reported installed Paseo 0.15.0, and the guarded wrapper's `run`, `send`, `permit`, `wait` and `inspect` help each exited 0 with the options recorded in the dated note of docs/project/dev-environment.md (`send` also advertises `--behavior`, and `permit allow` supports `--input`). An owner-requested paid `cli` qualification that day, through the isolated `~/.paseo-dev` daemon and disposable projects, passed the lifecycle/review/verification, model-remark consent (on a Git-initialized fixture; an earlier non-Git fixture failure is recorded in the dated note) and question/answer resume scenarios. This qualifies CLI runtime/protocol behavior on 0.15.0 only: rendered desktop/GUI behavior was not run, and it does not requalify the 0.12.0, 0.11.x, 0.10.x or 0.9.x observations.
Recheck the installed CLI/bridge and guarded wrapper before live use. Invoke `scripts/cortex-paseo-tools.sh paseo-dev --
...`; it defaults to ~/.paseo-dev, rejects ~/.paseo, and blocks command-level endpoint
overrides. If the installed build cannot be shown to isolate every required state and
the daemon endpoint, do not launch Paseo or install the plugin; report the missing
evidence.

### Requested Paseo live modes: `cli` and `desktop`

Both modes run only on an explicit owner request naming the mode (see above).
In `cli` mode, drive a Cortex agent headlessly through the guarded wrapper
(`run`, `send`, `permit`, `wait`, `inspect`) and check outcomes against the
disposable project's `.cortex` store; it establishes runtime and protocol
behavior, not rendered GUI behavior. In `desktop` mode, use the Electron app as
described below. See the dev Paseo environment guide for the `cli` commands.

### Dev Paseo GUI verification

For a requested `desktop` check, use the persistent isolated dev home with the
Paseo Electron Desktop application. The historically inspected Paseo 0.10.0 CLI help listed CLI commands but no web-client launch
command. This does not establish whether a daemon-served UI exists. Use the
Electron app connected to the isolated dev daemon for rendered behavior unless
a supported alternative is independently verified. Until recovery is qualified for the deployed version, apply plugin updates between active tasks. A host SIGKILL without SIGTERM cannot be recorded; native engine actions have no retained command receipt. Outside a
requested `cli` check, CLI use is secondary and covers daemon health, plugin
registration and logs only.

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
scripts/cortex-paseo-tools.sh paseo-dev -- start --timeout 60 --json
scripts/cortex-paseo-tools.sh paseo-dev -- plugin install "$PWD/integrations/paseo" --json
scripts/cortex-paseo-tools.sh paseo-dev gui
~~~

For Xvfb environments use:

~~~bash
scripts/cortex-paseo-tools.sh paseo-dev gui --xvfb
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
scripts/cortex-paseo-tools.sh paseo-dev -- daemon stop --timeout 15 --json
~~~

In Paseo Desktop, inspect the rendered timeline and the specific UI behavior under
test. CLI output, logs and CDP inspection alone do not establish a rendered GUI
result. Use CLI through the guarded wrapper for secondary diagnostics, with the
plugin path explicit when installing:

~~~bash
scripts/cortex-paseo-tools.sh paseo-dev -- status --json
scripts/cortex-paseo-tools.sh paseo-dev -- plugin ls --json
scripts/cortex-paseo-tools.sh paseo-dev -- logs <agent-id> --json
~~~

For retained-history checks, omit `--tail` and retain the exit status, output
byte count, structural event count, safe first/last event identities and any
explicit error. The currently installed Paseo's `logs --help` and retained-history behavior must be checked before
relying on historical 0.9.2 observations; if completeness cannot be independently
established, mark that check unverified. Do not store message bodies or raw logs in
repository documents or reports.
