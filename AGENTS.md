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

The server stores tasks, advisory governance and Markdown reports. The model
owns the pipeline, delegated engine sessions, steering, evidence interpretation
and completion. Keep tool argument contracts in the advertised schemas and
property descriptions only, never in skills or profile instructions. Preserve
all 23 role profiles (22 specialists plus the consultant) and their shared
report protocol. Do not introduce
compatibility routes, mandatory stages or approval machines. There is no
lifecycle hook dispatcher in the packaged product (removed at T033): no
`PreToolUse`/`UserPromptSubmit`/`Stop` handler and no host trust prompt for one.
Every mutation reaches the store only through a dispatched, channel-verified
MCP `tools/call`; local source-memory, observation and registered-file
integrity work happens inside that same dispatch path, never through a
separate hook, and nothing there chooses agents or decides task acceptance.

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
report content use bounded cursor reads.
Workers select relevant reports rather than reading everything.

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
without printing credentials before relying on either engine. Real engine
prompts still follow the paid-call policy below.

Default live/test model policy (`gpt-5.6-luna`, medium/high effort) still
applies to real engine work under the ACP path — see
`docs/acp/codex-acp-adapter.md`'s T035 live evidence for `gpt-5.6-luna`
used through the production `cortex_acp.py` entry point. Heavy models stay
prohibited in routine live qualification. Any subagent assigned to edit
prompts or skills must still use only `gpt-5.6-sol` at `medium` reasoning
effort, recorded as an explicit user-requested override in the assignment;
this does not change the Luna-only policy for real live-qualification
workloads.

The no-paid default automated suite
(`PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q`, see
`docs/acp/test-matrix-and-ci.md`) cannot reach a paid engine and is not a
substitute for live qualification. The only opt-in paid probes are
`scripts/cortex_codex_probe.py` and `scripts/cortex_claude_probe.py`, and
both refuse to run without explicit `--i-authorize-paid-call` and
`--approval-ref`. `scripts/cortex_acp_smoke.py` is the no-paid startup smoke
harness (schema-validated launch/protocol surface only, not GUI evidence).

Real-client qualification uses the manual
observation table in `docs/acp/real-client-runbook.md` (Zed now; codeg
deferred), scoped by `docs/acp/test-matrix-and-ci.md`'s 45 live-only
acceptance cases. As of this writing every row in that runbook is `NOT RUN`;
this file does not know of a completed real-host qualification and must not
be edited to claim one. Do not invent a substitute procedure or claim an unobserved runbook row passed.

This release-verification obligation belongs to the owner of the Cortex
change. Run live workloads against a separate isolated project, never
against this checkout itself.
