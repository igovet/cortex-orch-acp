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
- Independent review whenever the observed project differs from its execution
  baseline, or for every fix cycle and any unobservable state; unchanged state is
  recorded as `REVIEW_NOT_REQUIRED`. Scoped fixes, three review cycles, contained
  verification and completion remain bound to exact report and project digests.
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
  worker exists. Planned work receives independent review when required by the
  observed project state, plus verification.
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
installed Claude adapter was started in the jail and created a session without
a model turn. This is executable installed-CLI evidence, not a paid-model or ACP
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

The automated suite and isolated dev Paseo observations cover only the surfaces they exercise. Missing evidence is not treated as a pass. See [test matrix](test-matrix-and-ci.md) for automated coverage.

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

## Direct-driver Claude live check (no Paseo/Zed)

On 2026-09-23, a disposable-project, ad hoc ACP stdio driver (not part of this
checkout; see the evidence note) drove the real `src/cortex_acp.py` entry point
directly against the installed `claude-agent-acp` 0.81.0, with a referenced
(no-login, no-credential-copy) connection to the user's existing native Claude
account and a project-local `routing.json` pin. A one-worker plan routed by
`routing.auto_route.choose` (`route_basis: auto:pin`) to Claude `haiku`
completed end to end: implementation worker, independent code-reviewer,
runtime-executed verification commands (both exit 0) and `Runtime: COMPLETED`,
with no `WorkerModeNotConfirmed`. Separately, pinning every worker role to a
nonexistent Claude model id produced, on every one of four delegation attempts
across four coordinator-initiated plan revisions, the identical named
`pinned_route_unavailable` refusal with zero `ASSIGNMENT_RETRIED` events — the
runtime never looped — and the task ended cleanly in `WAITING_USER`, not stuck.
This did not exercise the separate "runtime moves the retry to another
available route" fallback case (would need a second failing route; Codex was
out of scope for this run), and remains a single-connection, two-task check,
not a Zed/Paseo/codeg qualification or a real quota-exhaustion trigger. See the
[exact commands, retained identities and limits](evidence/claude-live-qual-2026-09-23/direct-driver-run.md).
