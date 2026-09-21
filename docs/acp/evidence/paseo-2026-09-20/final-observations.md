# Paseo final observations — 2026-09-20

This note records the bounded observations made against the production Paseo
plugin path. Codex and Claude used the existing native accounts. The tested
projects were disposable and no private chat contents or raw host logs are
included here.

## Observed results

| Candidate | Scenario | Observation | Status |
| --- | --- | --- | --- |
| `c705…` | Bare `/harvest` | A new documentation task ran with eight Luna/high worker attempts, created seven source-backed project documentation pages, and recorded a durable task acceptance. | PASS |
| `2c477…` | Bare `/harvest-refresh` | Existing documentation was refreshed and the runtime recorded durable acceptance. The final UI retained a stale wait reason, so clean Paseo completion is not qualified. | FAIL |
| `4b3b…` | Native chat persistence | A newly created native Paseo chat restored both the user and assistant transcript after reload when exercised through the Paseo CLI. A full GUI/Paseo restart observation was not run for this candidate. | PASS (CLI reload only) |
| `4b3b…` | Terminal-cancel fixture | A no-paid ACP fixture replayed the terminal worker cancellation card after reload, including the terminal state. This is protocol/fixture evidence and does not qualify a live paid client. | PASS (fixture) |
| `4b3b…` | Plugin updater | The production updater smoke completed with exit code 0 and refreshed the plugin launch configuration. | PASS |

The package identifiers above are intentionally recorded as the retained
release prefixes used by the qualification run. They are not claims that
unobserved GUI scenarios passed. In particular, the full Paseo restart path,
question UI, image prompt, MCP forwarding, slash-command rendering, model and
effort selector consistency, and the remaining mixed-engine cases remain
unverified unless recorded elsewhere in this runbook.

## Automated boundary

The final no-paid repository suite recorded **2306 passed and 3 skipped** for
this source state. Automated tests validate the protocol and adapter surface;
they do not replace real-client GUI observations. The full package stamp and
release gate are intentionally performed after this evidence note is merged
into the working tree.

