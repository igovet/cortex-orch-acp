# Real-client observation runbook (Zed, AionUI)

This runbook is a reproducible, manual (or later scripted) GUI check list
for qualifying Cortex against the two real ACP clients selected in
`.cortex/acp-migration/T002/` (AionUI 2.2.2 supersedes codeg as the second
client; see `aionui-custom-agent.md`). **Every row below is `NOT RUN`.** This task
(T011) prepares the runbook and the underlying non-paid harness/config
tooling; it does not itself perform live GUI qualification. Owner of every
row: **T035**.

Mock/harness output (`scripts/cortex_acp_smoke.py`, the pytest ACP suite)
is real automated evidence for the protocol/launch surface, but it is
**not** GUI evidence and must never be cited as satisfying a row below.

**Known harness limitation (F5, T011 review):** `scripts/cortex_acp_smoke.py`
cannot detect a stray write to the real stdout that happens *before*
`cortex_runtime.acp.agent.main` redirects `sys.stdout` to `sys.stderr` --
by the time the harness's reader thread starts consuming stdout, any such
write already landed ahead of the first protocol frame, and the harness has
no way to distinguish that from the client itself misbehaving. The
"stdout-clean verification via logs" row below (and, independently, a
manual `python3 -B plugins/cortex/scripts/cortex_acp.py < /dev/null | head
-c 200 | xxd` byte-level check) is the authoritative check for this
specific failure mode, not the automated harness.

## Preconditions for T035

- Zed 1.20.1 (or the then-current qualified version — re-verify and record
  the exact version at run time) with Cortex registered via
  `scripts/cortex_acp_client_config.py --apply --i-consent-to-write` or the
  manual snippet in [zed-custom-agent.md](zed-custom-agent.md).
- AionUI 2.2.2 (or the then-current version) with Cortex registered per
  [aionui-custom-agent.md](aionui-custom-agent.md) via Settings → Agent
  Management → Custom Agents → Add Custom Agent.
- At least one real, user-approved, non-fake connection configured through
  Cortex's own onboarding for the "selector rendering" and "connection
  repair" rows; a paid model call still requires separate, explicit user
  authorization per row (see `INVARIANTS.md`: "Install with pinned versions
  and explicit consent; diagnostics is non-paid and full probes are
  opt-in").

## Observation table

| Client | Version | Step | Expected | Observed | Evidence path | Status |
| --- | --- | --- | --- | --- | --- | --- |
| Zed | (record exact) | Cortex appears as a separate agent in the Agent panel | "Cortex" listed distinctly from built-in providers and other custom agents; no auto-activation from `.cortex` presence alone | | | NOT RUN |
| Zed | (record exact) | Start a session with Cortex | Session opens; ACP `initialize`/`session/new` succeed; no stdout protocol pollution (verify via Zed's own agent log, not just this harness) | | | NOT RUN |
| Zed | (record exact) | Selector rendering, no connection configured | No Model/Thought-level selector shown (never an empty/fake one) | | | NOT RUN |
| Zed | (record exact) | Selector rendering, one connection configured | Model selector shown with real route id(s)/labels; Thought level shown iff the route reports one | | | NOT RUN |
| Zed | (record exact) | Command fallback | `/cortex-status`, `/cortex-model`, `/cortex-effort`, `/cortex-setup` work as slash commands even if Zed's own config-option UI is used instead | | | NOT RUN |
| Zed | (record exact) | Progress coherence | Streamed `agent_message_chunk` updates render incrementally and coherently; final `stopReason` matches what the UI shows as "done"/"cancelled" | | | NOT RUN |
| Zed | (record exact) | Setup/repair messaging | `/cortex-setup` guidance is legible in the UI and never shows a secret; a broken/unauthorized connection shows a truthful repair message, not a silent failure | | | NOT RUN |
| Zed | (record exact) | Uninstall | Removing the `agent_servers` entry removes Cortex from the panel; no Cortex connection/credential state is affected | | | NOT RUN |
| AionUI | (record exact) | Cortex appears as a separate agent | Same as the Zed row, in AionUI's own Custom Agents list UI | | | NOT RUN |
| AionUI | (record exact) | Start a session with Cortex | Session opens; ACP `initialize`/`session/new` succeed; if AionUI injects its own `mcpServers` into `session/new` params (UNKNOWN — see `aionui-custom-agent.md`), confirm it is present but never trusted/routed by Cortex | | | NOT RUN |
| AionUI | (record exact) | Selector rendering, no connection configured | Same as Zed row | | | NOT RUN |
| AionUI | (record exact) | Selector rendering, one connection configured | Same as Zed row | | | NOT RUN |
| AionUI | (record exact) | Command fallback | Same as Zed row (also resolves the UNKNOWN: whether AionUI supports slash commands at all) | | | NOT RUN |
| AionUI | (record exact) | Progress coherence | Same as Zed row | | | NOT RUN |
| AionUI | (record exact) | Setup/repair messaging | Same as Zed row | | | NOT RUN |
| AionUI | (record exact) | stdout-clean verification via logs | AionUI's own process/log view shows no non-protocol stdout from the Cortex subprocess | | | NOT RUN |
| AionUI | (record exact) | Uninstall | Same as Zed row | | | NOT RUN |

## Recording instructions for T035

For each row: fill in the exact client version, the actual observation,
a path to retained evidence (screenshot, log excerpt with secrets
redacted, or a saved transcript), and a status of `PASS`, `FAIL`, or
`BLOCKED` (never leave a row `NOT RUN` while claiming the row passed).
Cite the paid-probe/recipient approval reference separately if any row
required one (see `INVARIANTS.md`).
