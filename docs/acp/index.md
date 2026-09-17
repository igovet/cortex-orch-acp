# ACP client integration (T011)

Cortex is an incoming ACP v1 stdio agent (`plugins/cortex/scripts/cortex_acp.py`,
implemented in `plugins/cortex/scripts/cortex_runtime/acp/`). This section
covers registering Cortex as a selectable Custom Agent in a real ACP client,
without any Registry submission dependency, and the no-paid startup smoke
harness that validates the launch/protocol surface before any live client
check.

- [Zed 1.20.1 Custom Agent setup](zed-custom-agent.md)
- [AionUI 2.2.2 custom-agent recipe (partly unverified)](aionui-custom-agent.md)
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

## Tools

- `scripts/cortex_acp_client_config.py` — non-destructive Zed `agent_servers`
  registration helper (default dry-run; `--apply --i-consent-to-write` for a
  real, backed-up, atomic write to an explicitly given settings path) and
  `--print-aionui-recipe`.
- `scripts/cortex_acp_smoke.py` — no-paid startup smoke harness: spawns the
  real `cortex_acp.py` entry point against a fresh temporary project and a
  fresh temporary `HOME`/`XDG_CONFIG_HOME`/`XDG_STATE_HOME`, validates every
  emitted message against the pinned ACP schema fixture when `jsonschema` is
  importable, and proves no hidden provider/model call is reachable
  (`session/prompt` on a fresh connection-less agent fails closed with
  `-32001 orchestration_unavailable`; see
  `cortex_runtime.acp.runtime.UnavailableRuntime`, the production entry
  point's only runtime).

## Activation model

Cortex only activates when a client explicitly selects/launches it as its
own agent (`agent_servers`/custom-agent entry). No client that merely
detects a `.cortex` directory elsewhere ever activates Cortex orchestration
(see `AGENTS.md`: "Do not activate orchestration merely because this
repository is named Cortex").

## Live boundary

This task (T011) delivers the integration/harness scope only. Real client
GUI rendering (Zed/AionUI) is qualified in T035 — see
[real-client-runbook.md](real-client-runbook.md), whose rows are all
`NOT RUN` here on purpose. Mock/harness output above is not GUI evidence.
