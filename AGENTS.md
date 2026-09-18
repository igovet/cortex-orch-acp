# Cortex repository development

Use `python3`. Runtime behavior belongs entirely below `plugins/cortex/`.
Do not activate orchestration merely because this repository is named Cortex;
only an explicit user selection activates the bundled orchestrator skill.

Cortex is an incoming ACP v1 stdio agent (`plugins/cortex/scripts/cortex_acp.py`,
`plugins/cortex/scripts/cortex_runtime/acp/`), not a plugin hosted inside another
agent's process. See `docs/acp/index.md` for the full section map. T033 removed
the entire Codex-plugin/gateway harness (the MITM gateway, the pinned-dependency
installer, native Codex profile registration, the lifecycle hook dispatcher, the
Codex-native `_meta` adapter, and the `.codex-plugin/` manifest) — see
`docs/acp/packaging-and-onboarding.md`'s "What was Codex-plugin-harness-era and
was removed" and `SECURITY.md`'s "Source capture" section for the exact list.
Anything below describing that removed harness as though it were current is
wrong; where this file and `docs/acp/*`/`SECURITY.md` disagree, those are right.

The server stores tasks, advisory governance and Markdown reports. The model
owns the pipeline, delegated engine sessions, steering, evidence interpretation
and completion. Keep tool argument contracts in the advertised schemas and
property descriptions only, never in skills or profile instructions. Preserve
all 22 specialist profiles and their shared report protocol. Do not introduce
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

## Evidence discipline carried forward from the removed Codex-plugin audit harness

The paragraphs below this point, up to "Historical: Codex-plugin-harness
qualification audit contract", describe an automated current-host audit tool
(`observational_accept`/`host_enforcement_state` labels, `mcp_first_bootstrap`
and `worker_assignment_policy` attestations, exit-code contract) that was part
of the removed Codex-plugin harness and is retained here only as a historical
record — it is not runnable against the ACP product. The general principles it
encoded still apply and are carried forward as plain rules for any qualification
evidence, ACP or otherwise:

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

## Historical: Codex-plugin-harness qualification audit contract (superseded, kept verbatim for record only — not runnable against the ACP product)

For the explicit current-host observational qualification mode, the absence of a
signal the installed host cannot emit is `not_applicable` or a retained
diagnostic, never by itself a BLOCK: this includes bootstrap attestation,
assignment-policy attestation, a separate server event, and a process self-exit
marker. The exact assigned worker-SKILL receipt remains mandatory.
`observational_accept` still requires complete, non-truncated
capture/calls/events/audit plus one exact successful public task binding, one
completed native worker, one task-bound worker report, and exact coordinator
artifact reconciliation. BLOCK only for an actual task failure; pending or
incomplete work; malformed, mismatched, ambiguous, replayed, or truncated
evidence; artifact/check failure; or observed unsafe private/project mutation.
When the installed host omits a public task ID or native-result body, the outcome
may use only one expected root, one parent-bound native worker/report, and one
exact non-replayed artifact reconciliation as opaque observational binding;
missing, duplicate, or mismatched links remain BLOCK.
For an interactive CLI without a self-exit marker, verified idle composer,
empty owned sessions, completed native outcome, and exact owned cleanup receipt
are the alternative terminal proof. This is not host enforcement and must retain
`host_enforcement_state=unverified`.
In that same mode, a redundant worker-final report-ID omission is diagnostic when
exactly one task/parent/profile-correlated worker publication and coordinator
artifact reconciliation prove the report identity. One completed, non-mutating
`read_report` `invalid_arguments` lookup is likewise diagnostic when the valid
publication and reconciliation are independently present. Neither exception
permits missing/ambiguous publication, a failed write, a mismatch, replay or
truncation, mutation, or an unsafe/private target.
One or more exact-equivalent failures while inspecting a declared public skill or
reference leaf are diagnostics only after that same complete outcome proof and a
separate successful mandatory worker-`SKILL.md` receipt. They must share
worker/profile/manifest-bound read identity; any differing, replayed,
private/project, mutating, dispatched, truncated, or ambiguous read remains blocking
and none credits a skill receipt. The model may choose any supported ordinary
read-only means. The observer validates the declared literal leaf and the operation's
read-only outcome without requiring a particular shell command or quoting. Nested
shells, extra operations, paths other than the declared leaf, directories, globs,
and every write remain forbidden access.
In `current_host_mcp_first` only,
`worker_assignment_policy_unverified` and `mcp_first_bootstrap_unverified` are
always printed `unsupported_by_current_host`, `not_applicable` diagnostics. The
installed host cannot emit those attestations; they never become integrity
invalidators or hard blockers, grant no acceptance evidence or score, and retain
`host_enforcement_state=unverified`. All supported observable outcome, safety,
completion, artifact, replay, truncation, ambiguity, pending/open, and mutation
requirements remain fail-closed.
The final current-host audit boundary reapplies this exact two-label rule after
all parser and classifier contributions, so an earlier representation cannot
reintroduce either unavailable attestation as an invalidator. It changes no
other policy row or supported-evidence requirement.
Generic `pre_binding_host_action` is never demoted by the outcome partition because
it has no strict actor/profile/path proof and may describe an unsafe dispatch. The
only coordinator setup exception is an observer-proven, manifest-bound, bounded,
read-only literal `skills/orchestrator/SKILL.md` or its declared instruction-reference
read with its active-skill marker and exact installed-candidate identity;
the same exact read-only proof covers its bundled communication, tool-discipline,
content-safety and recovery companions. Up to four literal instruction leaves may
be inspected in one bounded read-only operation; no arbitrary skill or cache path qualifies.
the separate registered-worker static-SKILL predicate can retain independently
complete, read-only denied records as diagnostics. Neither exception admits a
directory, glob, different skill, unknown provenance, private/project target,
mutation, dispatch, truncation, replay, or ambiguity.
They remain printed and do not waive any task/report/artifact/pending failure or
any other policy label; outside that mode normal policy handling applies.
The current-host audit exits 0 only when it prints `observational_accept` with
valid evidence and every remaining finding is an explicit retained diagnostic or
not-applicable signal. It exits 1 for every hard failure, policy row, pending or
open state, mismatch, mutation, truncation, ambiguity, replay, or missing outcome
proof; diagnostics remain printed in either case.

## End of historical Codex-plugin-harness qualification audit contract

Use one real `pipeline.md` per task, newest edition first with older editions
below. Other reports are immutable real Markdown files. Both published task
documents and the SQLite metadata/relations index live under the one
project-local `.cortex` store (`<project>/.cortex/cortex.sqlite3`, published
document bodies under `<project>/.cortex/` alongside it); there is no
`.codex/cortex` anywhere. `tests/test_package.py` asserts exactly one
`.cortex/cortex.sqlite3` per project. Both the newest-first catalogue and
report content use bounded cursor reads.
Workers select relevant reports rather than reading everything.

Read relevant docs/project and docs/features pages before nontrivial work.
After behavior changes update README.md, SECURITY.md and affected documentation;
check links and commands against source. Keep secrets, private reports and raw
host logs out of repository documents and diagnostics. Report unrun checks.

For this release use semantic version 1.0.0. After any plugin payload
edit, regenerate its complete content hash with
`python3 -B scripts/cortex_package.py stamp` before release-sensitive checks.
Run package, sync and test checks sequentially in the same checkout. Use the
smallest meaningful tests first. Source-only sync uses `--check` or `--dry-run`.

## Current ACP-era live/model-policy rules

Never touch the user's real Zed (or other ACP client) session, its real
credentials, or a stable configuration outside an isolated throwaway
`HOME`/`XDG_CONFIG_HOME`/`XDG_STATE_HOME` (see
`docs/acp/test-matrix-and-ci.md`). No direct sync into a live host
configuration is allowed.

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

Real-client qualification — the actual ACP-era equivalent of what the
removed CLI-before-Desktop procedure below used to gate on — is the manual
observation table in `docs/acp/real-client-runbook.md` (Zed now; codeg
deferred), scoped by `docs/acp/test-matrix-and-ci.md`'s 45 live-only
acceptance cases. As of this writing every row in that runbook is `NOT RUN`;
this file does not know of a completed real-host qualification and must not
be edited to claim one. Do not invent a substitute procedure: if the
runbook or test matrix is silent on some part of what the old procedure
below used to check (for example, an equivalent to its exact-window-focus
or duplicate-receipt guards), say plainly that the ACP-era equivalent is
not yet established rather than restating the old mechanism as current.

This release-verification obligation belongs to the owner of the Cortex
change. Run live workloads against a separate isolated project, never
against this checkout itself.

## Historical: Codex-plugin/native-subagent CLI-before-Desktop qualification procedure (superseded, kept verbatim for record only — describes a harness and product model, Cortex hosted as a plugin inside Codex CLI/Desktop driving native Codex subagents via a `$cortex:orchestrator` token, that the ACP migration replaced; not current instruction)

Never install or update the stable user's Cortex plugin. Live preparation must
use `./scripts/cortex-dev` (or the root forwarding wrapper), which alone prepares
and installs the exact isolated `$HOME/.cortex-dev/.codex` candidate. No direct
normal sync, ad hoc cachebuster or stable configuration changes are allowed.

All live-dev tests use `gpt-5.6-luna` with `high` effort for the coordinator.
Native test workers also use Luna, at medium or high effort; heavy models are
prohibited in live CLI/Desktop tests. Keep this test policy in isolated launcher
configuration, without changing stable user settings or general plugin routing.

Any subagent assigned to edit prompts or skills must use only `gpt-5.6-sol` with
`medium` reasoning effort. Record that explicit user-requested override in the
assignment. This repository-development rule does not change the Luna-only model
policy for real live-dev CLI/Desktop qualification workloads.

After any repository work, live-dev verification is mandatory in both real
environments, in this order: run a focused CLI scenario first, then run Desktop
only after the CLI is ACCEPT with a clean audit on the same unchanged isolated
candidate and payload. CLI and Desktop are both required; neither substitutes for
the other. If CLI fails, is rejected, unavailable, or unverified, end that live
attempt, retain the complete current calls/events tails and audit, stop its exact
session, and do not start Desktop as a diagnostic substitute. Report that host as
unverified and correct/requalify the candidate before a new CLI-first attempt.

This release-verification obligation belongs to the owner of the Cortex change.
Run live workloads in a separate isolated project, never in this Cortex checkout.
Keep Cortex implementation and source checks in this checkout; the disposable
project has its own source, Git history and task instructions.
An ordinary research task or generated proof note inside a live scenario is the
workload being observed, not another Cortex change: complete that deliverable
without recursively launching CLI/Desktop qualification or preparing a candidate.
The outer owner still must finish both real-host checks; this scope distinction
does not waive any check for implementation, launcher, policy or skill changes.

After a completed change run a focused ordinary interactive Codex live scenario.
Use `./scripts/cortex-live-smoke start --workdir PATH` for the exact
`cortex-markdown-smoke` session on the default tmux server. The helper creates bash,
attaches an owner-only output pipe, and enters the launcher literally. Observe
`capture` and `status`; visibly confirm any trust prompt before the one explicit
`enter`, and confirm the composer before submitting work. Check the passive MCP
initialization receipt against the isolated candidate and seven-tool catalogue.
Submit with `send --prompt-file FILE`: one literal insertion, five real seconds,
then one named Enter. The transport never decides readiness or acceptance.
Workloads begin with the actual `$cortex:orchestrator` token; remaining text is
ordinary product work without orchestration test instructions.

Use `events` to inspect bounded metadata-only MCP outcomes for coordinator and
native workers, and `calls` to inspect every host-tool invocation and observed
result. A command wrapper must expose its exit code or running session receipt; stdout
alone is unverified. Never accept hidden errors, truncation or unexplained duplicate writes as a
clean run. When rejecting a live run, capture the complete current `calls` and
`events` tails and run `audit` before stopping it; do not stop monitoring after the
first discovered fault. The ordinary helper seals a receipt-bound immutable pre-stop
bundle before cleanup; any post-stop no-session diagnostic must use its distinct
post-stop path and must never replace the sealed calls or events evidence. The coordinator owns every user steering and acceptance decision.
For resume stop only that session, then start with the same workdir and
`--resume-last`; confirm the existing task before continuing. No `codex exec`,
nested tmux, alternate sockets or stable plugin environment. Capture the exit
marker and use `stop` (or `stop --interrupt` after failure) to remove the exact
session and temporary observation streams. A normal successful stop requires the
owned bash marker; missing-marker evidence remains incomplete and preserves the
session for an explicit failure path. Never kill the tmux server.

Real Desktop uses `scripts/cortex-desktop-dev`, the same isolated candidate and
a disposable Electron profile. Before launch, compute the exact final result
artifact SHA-256 and retain it as `EXACT_RESULT_SHA256`. Start with `--prompt-file`
and both expected-result bindings: `--expected-result-path README.md` (or the
exact project-root result leaf) and `--expected-result-sha256 EXACT_RESULT_SHA256`.
Confirm the prepared composer, then use plain `send` with no `--prompt-file`;
it focuses that exact window, submits with one plain `Return`, and succeeds only
after one new task receipt appears. Wait for the task to become idle, the final
worker report to be published, and coordinator report/public-evidence reconciliation
to complete before collecting `calls` and `events` and running `audit`. Inspect the
audit exit code; while the session state still exists, run plain `stop` only after
the audit result has been captured. Use `events` for MCP outcomes and `calls` for every
argument-free coordinator/worker host-tool invocation; review every call for necessity
and correct role ownership. CLI/Desktop parity requires consecutive successful real-host
runs on one unchanged payload. State an unavailable host as unverified.

## End of historical Codex-plugin/native-subagent CLI-before-Desktop qualification procedure
