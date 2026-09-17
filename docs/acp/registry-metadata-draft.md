# ACP Registry metadata draft

**Status: not submitted, not published.** This page prepares the metadata
fields a future, separate Registry submission task would need. It is not
itself a submission, and Custom Agent installation (see
[zed-custom-agent.md](zed-custom-agent.md),
[codeg-custom-agent.md](codeg-custom-agent.md)) never depends on it — the
whole point of T011 is that Custom Agent registration works without any
Registry approval.

## Fields needed later (draft values, all placeholders)

| Field | Draft value / source | Notes |
| --- | --- | --- |
| Agent name | `Cortex` | Matches the `agent_servers`/custom-agent display name used in this task. |
| Package identifier | (not decided) | Depends on the packaging approach chosen in a later task (T031-adjacent); no npm/PyPI package exists yet. |
| Version | Plugin's own `1.0.0+sha256.<hash>` stamp (`plugins/cortex/.codex-plugin/plugin.json`) | Whether Registry versioning reuses this exact string or a separate scheme is undecided. |
| Launch command | `python3 -B <abs>/plugins/cortex/scripts/cortex_acp.py` | Real, already-working launch command (this task); Registry distribution channel (binary/npx/uvx) not decided. |
| Protocol version | `1` (`cortex_runtime/acp/protocol.py: PROTOCOL_VERSION`) | |
| Capabilities summary | `loadSession: false`; text-only prompt content; no HTTP/SSE MCP transport | From `protocol.initialize_result`; must stay truthful/in sync with the actual implementation at submission time. |
| Short description | "Cortex: an incoming ACP agent for Cortex orchestration" (draft wording, not final) | |
| Homepage / repository | (not decided — depends on where this repository is eventually published) | |
| Maintainer contact | (not decided) | |
| License | (not decided — depends on repository licensing, out of this task's scope) | |
| Icon / branding asset | (none prepared) | |

## Explicit non-claims

- No Registry account, submission, or review has been initiated.
- No package has been published to any registry, npm, PyPI, or otherwise.
- This draft does not authorize a future task to submit without a separate,
  explicit decision; it only avoids re-deriving these facts from scratch
  later.
