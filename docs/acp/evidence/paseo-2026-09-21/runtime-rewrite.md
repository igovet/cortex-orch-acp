# Runtime rewrite: authorized real-engine and Paseo observations

Observed on 2026-09-21 with installed Paseo 0.8.0, codex-acp 1.12.0
(bundled Codex CLI 0.154.0), and claude-agent-acp 0.78.0 / SDK 0.3.270.
The user explicitly authorized up to two short tasks per engine using existing
native accounts: Codex `gpt-5.6-luna` / `medium` and Claude `haiku`.
No credentials were copied and no engine login or stable client settings changed.

The separate daemon listened on loopback port 6773, with disposable HOME/XDG
and projects under `/var/tmp/cortex-runtime-qualification-xzyb4g5e`.
The Cortex child alone used the real HOME for its existing native account.
The installed repository plugin and production `src/cortex_acp.py` were used.
The stable Paseo daemon and its active user task were not reloaded.

## Scenario and exact retained identities

Each project began with `calc.add(a,b)` returning `a-b`, and three committed tests:
positive operands, negative operands, and zero. The initial fixture failed two
of three tests. The prompt requested one implementation worker, mandatory independent
review, runtime `python3 -B -m unittest -q`, reports and synthesis after verification.

| Project | Paseo agent | Runtime task | Observed outcome |
| --- | --- | --- | --- |
| `codex` | `0d0e8423-8baf-4ddf-a2eb-34e057c0f739` | `t_ef939dc82f6d` | COMPLETED at 20:04:30 UTC |
| `claude` | `f433953e-2a4b-4585-95be-52d6a6200566` | `t_f63ff09c3907` | BLOCKED: additional QA worker claimed passed checks without receipts |
| `codex2` | `d55ccee7-4ef1-4dbe-9006-732af49fc5e9` | `t_05ff6d4ff73d` | COMPLETED at 20:13:36 UTC |
| `claude2` | `bc2262e0-4343-4e81-9099-89591d248bfc` | `t_943c075cd555` | Revision 1 BLOCKED; same task resumed through a fresh plan and revision 2 COMPLETED at 20:13:12 UTC |

The first Claude failure remains a failure. In the second task, unsupported passing
claims were rejected before publication. The consultant then submitted `unverified`
checks, correctly blocking the task. Explicit continuation of that same task requested
one implementation/validation node and left mandatory review to the runtime. The
runtime retained the original block, revoked prior work, created a new plan revision,
ran a new worker and code-reviewer, then verified and completed. No third task was run.
This demonstrates bounded recovery with steering, not unattended success of every
Haiku plan or automatic promotion of unresolved checks.

Both final projects retained the complete execution → independent code-reviewer →
verification → reconciliation → completion transitions. Their post-run unit tests
exited zero (three tests); Git diff changed only `return a - b` to `return a + b`.
The test file remained identical to the committed baseline. The review and passing
verification events retain exact report identities and reviewed project digests.

Each project's real `.cortex/cortex.sqlite3`, immutable reports and pipeline remain
at the project path above. Full task/node/event exports and post-run test/diff results
are in `<project-name>-result.json` in the disposable root. These files are private
qualification artifacts, not checked-in logs or substitute GUI observations.

## Actual browser observations

The browser displayed the isolated daemon's real bundled web client. Observed:

- Live coordinator prose and emitted Codex thought chunks, with tool cards for
  context/skill reads, rejected and accepted plan submissions and their full payloads.
- Working and completed worker cards showing actual role, engine, model and effort;
  worker navigation and the separate worker transcript.
- Claude gateway `Read`, `Edit` and `Shell` cards, the literal verification command,
  exit result and three-test output; draft/publication/result/synthesis calls remain visible.
- Final worker replay showed `Read — calc.py` and `Shell — python3 -B -m unittest -q`,
  with literal three-test output and a separate raw-data expander. The failed
  consultant retained a `failed` badge sourced from its runtime node, even though
  it had published a report.
- Independent code-reviewer activity, explicit APPROVE, runtime verification and
  `Runtime: COMPLETED` for the retained successful tasks.
- After browser reload, the retained Haiku continuation, completed worker/reviewer
  cards, verification, synthesis and completion text remained available.
- After deliberate isolated-plugin reload, Paseo appended `Provider connection closed`
  to already completed chats. This is retained as an induced client disconnect
  diagnostic, not hidden or described as a clean native-session restart. Runtime
  task completion and its evidence stayed unchanged.

This does not qualify Zed, codeg, arbitrary external MCP/browser tools, account loss,
all cancellation combinations, all 23 specialist workloads, or every live-only matrix
row. Those unobserved cases remain unverified. Raw native thought not emitted by an
adapter is unavailable; this record makes no claim to expose hidden model internals.

## Fixes found by these runs and validation boundary

The production adapter used CLI 0.154.0 rather than the separately installed 0.155.1.
Both versions now pass the actual installed-CLI, no-paid gateway dispatch/native patch
rejection test. Startup uses exact version and capability-shape checks.

Live failures led to pre-publication check validation with precise correction fields;
actual failed/unverified checks still block. Regression tests also found and fixed
idempotent result replay after a worker is complete. The presentation now projects
observed gateway fields while retaining raw event data, preserves complete tool titles,
and distinguishes a missing project path from an uncertain storage mutation.
Worker-history status now uses the exact runtime assignment outcome; publication
alone cannot display an unverified result as completed.

This campaign was iterative: it is evidence for these exact tasks and observed
components, not an immutable release-artifact certification. The final replay fix
and final presentation adjustments were checked separately by executable regression
and UI checks. See [current automated results](../../runtime-rewrite-status.md).
