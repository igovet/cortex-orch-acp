# Runtime orchestration rewrite — implementation and qualification

The 2026-09-21 architecture replaces model-owned orchestration. The runtime owns
lifecycle, dependency scheduling, routes, scopes, permission checks, independent
review, verification, recovery and completion. Models submit bounded proposals and
immutable reports. All 23 specialist/consultant profiles remain available.

Implemented production paths:

- Transactional task state, plan revisions, validated DAG, original goal and event
  history in the single project-local store; four concurrent non-overlapping workers.
- Closed plan/result/question/scope/replan/escalation/synthesis tools. No public
  delegate, decide, integrate, acceptance or model/effort selection API.
- Fresh mandatory review, scoped fixes, three review cycles, contained verification
  and completion bound to report identity and the reviewed project revision.
- Pre-execution gateway checks for files, commands, drafts and external MCP tools;
  native tools bounded by the engine jail in both supported pinned adapters. Unknown execution
  effects remain unresolved rather than replaying a possible mutation.
- Durable user questions, exact answer resume, steering before renewed execution,
  cancellation fencing, bounded lost-worker retries and total/idle deadlines.
- Runtime context restoration, selective report reads, capsule delivery and
  ownership fencing. Old model-owned controller/advisory entry points are removed.
- Observed tool inputs, outputs, locations and failures are retained with secret
  redaction, except file/skill read bodies omitted from new display history. Paseo shows readable tool names while hiding thinking and protocol
  bookkeeping in chat and worker panels, including replay.
- Informational questions have a read-only direct-answer path before any plan or
  worker exists. Planned work retains mandatory review and verification.
- Workers receive English-only communication instructions on every turn; the
  coordinator receives the selected message language after its context.

## Installed capabilities and the engine jail

Local inspection verified the production adapter’s bundled Codex CLI 0.154.0,
the separately installed CLI 0.155.1, codex-acp 1.12.0 and the production
pinned claude-agent-acp 0.78.0 (SDK 0.3.270). The installed hosts expose no
universal native-action approval callback, so native tools are not gated per
call. Each engine process instead runs in the engine jail, which makes only the
assignment's scopes writable; native subagents, user MCP, hooks and plugins stay
disabled. Unsupported versions or changed patch shapes fail closed.

`tests/test_engine_native_gateway.py`, explicitly enabled with
`CORTEX_PINNED_CODEX_BINARY`, runs the installed CLI through the production
wrapper inside the engine jail with a loopback fake Responses provider. On
2026-09-22 it observed MCP gateway dispatch, native `apply_patch` creating a file
in the granted scope, and the same tool failing to write outside it. The
installed Claude adapter was started in the jail and created a session without a
model turn. This is executable installed-CLI evidence, not a paid-model or ACP
GUI qualification.

## Verification boundary

The no-paid suite covers real storage, authenticated MCP subprocesses, both engine
protocol paths, complete plan → worker → reviewer → verification → synthesis runs,
Linux namespaces, cancellation/recovery races and transcript retention. Model
sessions are scripted unless a test explicitly states otherwise. The final Python
suite passed **2,354 tests with no skips** (146.92 seconds), including installed
adapter and CLI checks. TypeScript and **16 interface tests** pass; the final
UI/package changes also passed 37 targeted Python checks, followed by 20
worker-history/renderer checks after the final status correction. Package and all 23
generated profiles validate. Startup
smokes cover both empty configuration and a no-paid fixture catalogue.

Real Codex Luna and Claude Haiku workloads ran in isolated Paseo. Both final projects
reached runtime COMPLETED with mandatory code review and three passing tests. Claude
also retained a blocked attempt and required steering/replanning within its second
task; those failures are not erased. See the [exact live observations and limitations](evidence/paseo-2026-09-21/runtime-rewrite.md).

The campaign does not qualify every live-only matrix case or Zed/codeg. No absent
signal is counted as a pass. The [runbook](real-client-runbook.md) remains the manual
authority for outstanding observations; this is not an immutable release-artifact
certification. See [test matrix](test-matrix-and-ci.md).

## Production MCP update follow-up

The first production update exposed missing explicit grants in the owner's
pre-gateway MCP registry, which isolated live projects had not covered. The
registry and installer were repaired, real installed codebase and browser tools
passed contained calls, and `/cortex-status` completed through production Paseo.
The full follow-up Python suite passed 2,356 tests with no skips (146.90 seconds).
See [production repair evidence](evidence/paseo-2026-09-21/production-mcp-repair.md)
for the exact checks and their limits.

## Paseo presentation and informational requests follow-up

The follow-up hides thinking/bookkeeping, humanizes Cortex tool rows, bounds worker
card titles, restores English worker instructions and adds the read-only direct
answer path. The full no-paid suite passed 2,367 tests with no skips (147.70
seconds). Existing production history and two newly authorized isolated tasks were
inspected in Paseo: direct overview without workers and a completed Luna/Haiku
implementation/review cycle. Read bodies are omitted from new display history,
read rows never expand, and raw data is hidden in both chat and worker panels. See the [observations and
limits](evidence/paseo-2026-09-21/chat-presentation-follow-up.md).
