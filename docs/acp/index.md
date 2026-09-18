# ACP client integration (T011)

Cortex is an incoming ACP v1 stdio agent (`src/cortex_acp.py`,
implemented in `src/cortex_runtime/acp/`). This section
covers registering Cortex as a selectable Custom Agent in a real ACP client,
without any Registry submission dependency, and the no-paid startup smoke
harness that validates the launch/protocol surface before any live client
check.

- [Zed 1.20.1 Custom Agent setup](zed-custom-agent.md)
- [codeg 0.30.8 custom-agent recipe (partly unverified)](codeg-custom-agent.md)
- [Real-client observation runbook (live rows NOT RUN, owner T035)](real-client-runbook.md)
- [Registry metadata draft (not submitted, not published)](registry-metadata-draft.md)
- [codex-acp outgoing adapter (T013): pinned install, env/option mapping,
  evidence levels, opt-in probe](codex-acp-adapter.md)
- [Permission/filesystem/terminal bridge (T014): binding, invalidation,
  containment, and UX-vs-actual-containment limitations](permissions-and-containment.md)
- [Portable role instructions and delegation provisioning (T015): canonical
  sources, native markers, instruction-delivery qualification, delegation
  operations, and recoverable provisioning](roles-and-delegation.md)
- [Model-owned coordination and managed workers (T016): process boundary,
  event types/states, waiting without paid polling, explicit acceptance,
  and what's deferred to T017/T020/T025](orchestration.md)
- [Completion, waiting, and resource lifecycle (T017): turn-outcome
  handling, completion gating, claimed-vs-observed evidence, durable
  waiting/resumability, capacity ledger, and what's deferred to
  T020/T021/T023/T028](completion-and-lifecycle.md)
- [Durable input history and replay (T018): event identity/delivery
  ledger, crash recovery, attachment availability, trust labels, bounded
  reads, display-only session/load, and what's deferred to
  T020/T021/T022/T023](input-history-and-replay.md)
- [Recoverable publications and storage consistency (T019): orphan
  quarantine/reconcile, publish_result idempotency and report_ref binding,
  fault-injection-tested crash recovery, observed artifact revision
  binding, and entrypoint immutability/scope
  audit](publications-and-consistency.md)
- [Steering, urgent stop, questions, and coherent conversation (T020):
  cancel vs stop vs steering vs new task vs answers, delivery boundaries,
  question lifecycle and elicitation fallback, language policy and
  heuristic limits, noise policy, and what's deferred to
  T035](steering-and-questions.md)
- [Deterministic recovery capsules (T021): nine-section derived view,
  determinism without wall-clock time, worker scoping, trust/provenance
  and injection-resistant rendering, size bounds, the `read_capsule` MCP
  tool, and what's deferred to
  T022](recovery-capsules.md)
- [Context generations and qualified recovery delivery (T022): adapter
  compaction-support tiers with dist evidence, one compaction owner per
  session, deduplicated generation tracking, the built/transmitted/
  confirmed/continuation_observed/blocked/superseded delivery ledger, and
  fresh-generation capsule/role/tool-schema
  delivery](context-generations.md)
- [Recovery races and session replacement (T023): coordinator
  fencing/reconciliation/replacement sequence, worker-recovery
  reconciliation, cross-process engine-liveness detection, config/
  permission carry-over and authorized route change, and crash
  boundaries](recovery-and-replacement.md)
- [Summaries, weak models, and budgets (T024): summarizer trust boundary
  and adapter findings, semantic-summary validation and bounded
  deterministic fallback, per-route context budgeting, compaction-loop
  guard, persisted large outputs, and budget-selected limited-model
  presentation](summaries-and-budgets.md)
- [Parallel workspaces and safe integration (T025): isolated per-attempt
  snapshot copies, the serialized shared-root fallback, pure-Python
  three-way merge/conflict detection against a known base, unsaved-host-
  buffer preconditions, and revision-bound re-verification](workspaces-and-integration.md)
- [claude-agent-acp outgoing adapter (T026): the second, non-Codex engine —
  pinned install, env/option mapping, evidence levels, the QUALIFIED
  `session/new _meta.systemPrompt` instruction-delivery finding (0.78.0
  only, with its full dist evidence chain and wiring path), capability
  matrix vs codex-acp, and the opt-in, never-run coordinator probe
  documenting the remaining T035/T036 live-execution gate](claude-agent-acp-adapter.md)
- [Mixed-team routing and authorized fallback (T027): the per-task route
  profile authorizing which role may use which route, the per-attempt
  route/model/parameter-fingerprint audit trail (including fallback
  predecessors), pre-dispatch role/capability/attempt-budget enforcement,
  and the narrow, consent-gated conditions under which an authorized
  fallback ever opens a different route](routing-and-fallback.md)
- [Failures, cancellation, and disconnects (T028): the closed failure-class
  taxonomy distinguishing rate limits/auth loss/provider restrictions from a
  genuine engine crash, bounded reconciled-before-retry policy, session/
  cancel's permission-binding invalidation, host-disconnect handling, and
  the bounded owned-resource reaper](failures-and-cancellation.md)
- [Observability and resource attribution (T029): structured trace events
  with a basis classification, usage/cost attribution across retries and
  recovery calls, conservative call/token/concurrency caps (never a
  fabricated monetary number), declared-availability metrics and the
  `read_diagnostics` MCP read, the single redaction choke point, and
  unknown telemetry never gating execution](observability.md)
- [Security boundary audit (T030): enforced trust/credential/permission
  boundaries, real attacks attempted (stale-epoch delegation bypass and a
  principal-thread-id leak found and fixed), and accurately stated
  remaining containment limitations](security-boundaries.md)
- [Packaging, onboarding, and diagnostics (T031): standalone entrypoint
  and pinned-dependency manifest/verifier, non-destructive first-run
  setup (storage, connection, route pinning, client recipes), no-paid
  diagnostics, reversible uninstall, and what is Codex-plugin-harness-era
  (removed at T033, including the whole Codex plugin harness) vs.
  stays](packaging-and-onboarding.md)
- [Format migration, upgrade and rollback (T032): the no-new-version-needed
  compatibility conclusion (self-healing optional table groups since
  format 11), the existing format 10->11 `migrate` tool hardened with
  `plan`/`rollback`/`format_history`, crash-recovery ordering, and
  storage-lock concurrency exclusion](format-migration.md)

## Tools

- `scripts/cortex_acp_client_config.py` — non-destructive Zed `agent_servers`
  registration helper (default dry-run; `--apply --i-consent-to-write` for a
  real, backed-up, atomic write to an explicitly given settings path) and
  `--print-codeg-recipe`.
- `scripts/cortex_acp_smoke.py` — no-paid startup smoke harness: spawns the
  real `cortex_acp.py` entry point against a fresh temporary project and a
  fresh temporary `HOME`/`XDG_CONFIG_HOME`/`XDG_STATE_HOME`, validates every
  emitted message against the pinned ACP schema fixture when `jsonschema` is
  importable, and proves no hidden provider/model call is reachable
  (`session/prompt` on a fresh connection-less agent fails closed with
  `-32001 orchestration_unavailable`; see
  `cortex_runtime.acp.runtime.UnavailableRuntime`, the production entry
  point's only runtime).

## Test matrix and CI

[test-matrix-and-ci.md](test-matrix-and-ci.md) — the non-paid default test
command and its two mandatory environment requirements, what enforces that a
default run cannot start a paid campaign, the non-root CI requirement, and the
45 acceptance cases that only live qualification can close.

## Activation model

Cortex only activates when a client explicitly selects/launches it as its
own agent (`agent_servers`/custom-agent entry). No client that merely
detects a `.cortex` directory elsewhere ever activates Cortex orchestration
(see `AGENTS.md`: "Do not activate orchestration merely because this
repository is named Cortex").

## Live boundary

This task (T011) delivers the integration/harness scope only. Real client
GUI rendering (Zed/codeg) is qualified in T035 — see
[real-client-runbook.md](real-client-runbook.md), whose rows are all
`NOT RUN` here on purpose. Mock/harness output above is not GUI evidence.
