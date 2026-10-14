# Token and provider plan usage

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed.


Cortex records one `usage_records` row for each engine `session/prompt` call, including coordinator continuations, worker turns, retries, and calls that fail after starting. A row is tied to its exact task, attempt, and engine session. Cortex persists the last token report from that call's `PromptResponse.usage` once. What that report covers depends on the adapter: Claude reports the whole turn, Codex only the turn's **last model request** (see [adapter token semantics](#adapter-token-semantics)). Intermediate `usage_update.used/size` is current context occupancy and is never added to a task token total. It can change within a turn and after compaction.

The Pipeline **Task tokens** figure sums reported calls across the task, including the coordinator and every worker attempt. **Coordinator** is the subset whose runtime attempt ID starts with `co_`. A worker card sums reported calls bound to its displayed execution attempt and any audited predecessor route hops for that attempt. These are token counts, not a bill, account quota, or context-window percentage. `totalTokens` is authoritative when present; otherwise the display adds uncached input, output, cache reads, and cache writes. The separate cache figures are already part of the total. Claude and Codex both report cache reads separately from uncached input, so a card can show a small `↑` value beside a large cache-read value.

A call with no recognized token report is **unavailable**, not zero. The UI displays the number of such calls beside known token totals; if every call is unknown, it says **Tokens unavailable**. The observability summary also counts unknown calls separately. The exception to
this UI alarm is a runtime release after an accepted result: an unknown row with
`failed=0` and `stop_reason='released_after_result'` is omitted from the UI's
known/unknown call counts, while diagnostics retain a separate
`released_after_result_calls` count. Real reported figures are kept; genuine
unknown/failed calls still show unavailable. This uses existing store columns
without changing CHECK constraints or format, and never means zero/free usage. It aggregates the full task; its public per-call list remains bounded. Replayed UI events do not create usage records. Repeated real engine prompts, including retries, each create their own record. The installed Claude adapter supplies per-turn `PromptResponse.usage` on a settled prompt, with uncached input, output, cache reads, cache writes and total tokens. If a prompt is interrupted before that response arrives, its partial `usage_update.used/size` still describes context occupancy and cannot be converted into a token total; the call remains explicitly unavailable.

The Paseo plugin displays per-worker context occupancy and token counts on worker cards. It does not display provider subscription-window percentages: Paseo's provider schema exposes context and token-count fields, but no plan-quota field or API for replacing the host's native Context window indicator.

## Coverage and interrupted turns

Both installed adapters expose token counts only on the settled
`session/prompt` response; their top-level `usage_update.used/size` reports
context occupancy, not token expenditure. A worker released immediately after
its accepted result may never deliver that final response. No context count,
quota percentage or missing response can stand in for its tokens.

After an accepted worker or reviewer result, `EngineSessionManagerAdapter.finish_owned`
lets the exact in-flight prompt finish and persist its usage before ordinary
teardown, for at most **15 seconds** (`_RESULT_SETTLE_SECONDS`). The turn that
called `result_submit` usually still writes a closing message. In live dev qualification runs such turns ended 2.5-4.9 s after their result,
and 4 of 25 were cut by the former 5 s bound, losing their only token report. A turn that ends sooner
settles at once, so the bound only limits a turn that does not end. The
reviewer's turn end still precedes verification and completion, so a long
closing message delays them by at most that bound. This runs on an owned cleanup thread rather
than blocking the scheduler's dispatch loop. The result remains accepted while
banking finishes. Capacity and the project writer lock are **not** released early:
a dependent engine may still queue behind those existing safety boundaries until
process exit is confirmed. Explicit cancellation and failure cleanup still use
immediate `terminate_owned`, without this grace period. A replacement manager
cannot be terminated by the previous manager's cleanup. Engines without
`finish_owned` keep the original fence: a result-bearing attempt is not torn
down while its turn is still in flight.

Result acceptance revokes the attempt's worker MCP channel in the same
transaction, while the turn that called `result_submit` is still running. The
jailed engine's 1 s authority watchdog therefore gives a revocation caused by the
attempt's own current `worker_result` a bounded grace of **20 seconds** from its
first revoked observation instead of terminating the engine at once; settlement
normally performs the teardown first (the grace exceeds the settlement bound by
more than one watchdog poll plus controller latency). Terminating at the revocation itself would
end the engine within a second of its accepted result, before the final
`PromptResponse.usage` exists, leaving the call unknown with
`released_after_result`. A revocation without such a result, a superseded
result, an unprovable result lookup, a coordinator channel and an expired channel
get no grace. Completion still requires every worker engine to be gone, so the
last reviewer's turn end (bounded by the settlement) precedes completion.

Runtime shutdown interrupts a pending settlement wait (re-checked every
50 ms) and performs ordinary teardown at once; no settlement diagnostic is
recorded for it, and unrecorded counts stay unknown. `finish_owned` called after
shutdown tears down directly. Shutdown waits at most **5 seconds** in total for
settlement teardown already in progress. A settlement owns teardown of its exact
manager, so shutdown does not invoke it a second time if exit remains unconfirmed.
Thread registration/start is serialized with shutdown; a replacement manager is
not covered by its predecessor's settlement. Unconfirmed managers, capacity and
writer locks remain quarantined for a later explicit retry. A teardown stuck
beyond the join bound is left to
the existing retained-registry recovery rather than awaited indefinitely.

The existing prompt consumer records one row against its original task, attempt
and engine session, then signals cleanup. A `usage_settlement` progress event
records `prompt_recorded` (not a claim that tokens were present) or `timeout`
with reason `post_result_prompt_settlement_timeout` and
`usage_coverage: "unavailable"`. At timeout, ordinary teardown SIGTERMs the
engine and closes the transport; neither installed adapter can send usage after
that, because its final `PromptResponse` is the only token carrier. The usage
row stays `unknown` with `released_after_result` (the closed basis vocabulary
is unchanged); the settlement event is the specific reason for that call. This does not
backfill historical rows. Historical stores without a retained prompt response
cannot distinguish an engine that omitted token usage from a response preempted
by teardown; `released_after_result` alone is not wire-emission evidence.

During the existing bounded abandonment cleanup, `engine.session_manager`
retains recognized nonnegative integer token fields from a final response that
actually arrived for that exact call. `delegation.live_adapters` consumes those
counts once before recording the call; final counts replace, rather than add
to, an earlier report for the same inner turn. A cleanup timeout or transport
failure without a response remains unknown. An inner turn with only context
updates also makes a multi-turn call unknown, even if a later turn reports
tokens. Abandonment itself keeps its original bound; post-result settlement is
the separate bounded cleanup described above. Result acceptance is unchanged.

The read-only `scripts/cortex_timeline.py --project <project> --aggregate --json`
output includes `token_coverage` by audited engine and role (coordinator,
worker, review, or an explicit unattributed bucket). It counts reported,
unknown and post-result release calls separately and sums only reported fields.
An empty bucket list means no retained records, not zero consumption. The
existing cohort `unknown_token_ratio` remains an execution-worker metric;
coordinator coverage cannot make missing worker cost usable. These are call
coverage ratios, not the proportion of actual tokens observed, and known
subtotals are not the cost of the entire verified result. Historical unknown
rows cannot be repaired without their exact original response.

## Adapter token semantics

`engine.session_manager._token_values` copies the recognized ACP fields
(`inputTokens`, `outputTokens`, `cachedReadTokens`, `cachedWriteTokens`,
`thoughtTokens`, `totalTokens`) from the settled `PromptResponse.usage`
unchanged. Each adapter decides what those fields cover (read from the installed
adapter bundles):

| Adapter | Scope of `PromptResponse.usage` | Field meaning |
| --- | --- | --- |
| claude-agent-acp 0.78.0 | The whole turn: the adapter sums every model result of the turn into a per-turn accumulator (`sessionUsage`). | `inputTokens` excludes cache; `cachedReadTokens`/`cachedWriteTokens` are cache reads/writes; `totalTokens` is the sum of the four; no `thoughtTokens` (thinking is billed inside output). |
| codex-acp 1.12.0 | Only the turn's **last model request**: `buildPromptUsage(sessionState.lastTokenUsage)`, where `lastTokenUsage` is Codex's `thread/tokenUsage/updated` `tokenUsage.last`. The cumulative `tokenUsage.total` is kept only for the adapter's `/status` text and never reaches ACP. | `inputTokens` = Codex input minus cached input; `cachedReadTokens` = cached input; `outputTokens` and `thoughtTokens` (reasoning output) of that one request; `totalTokens` as Codex reports it. |

The Codex figure is therefore a lower bound for a turn with several model
requests: its input side approximates the final context size and its output is
only the closing message (often a few dozen tokens even when earlier requests
wrote long tool arguments). `cachedReadTokens: 0` with a large `inputTokens` is
a last request that missed the provider cache, not a mapping error. Cortex
cannot recover the per-turn sum from the ACP surface: Codex's streaming
`usage_update` carries only `used`/`size` (the last request's total against the
context window), so summing it would be an estimate, never an engine report.
Codex rows keep basis `engine_reported` because the engine did report them;
compare Codex and Claude token totals only with this scope difference in mind.

## Claude plan usage: the local `/usage` CLI

`connections.quota` learns a Claude connection's plan windows (`five_hour`, `seven_day`, `seven_day_<model>`) from two independent sources, newest observation per window winning:

- The adapter's own `rate_limit_event` (`usage_update._meta["_claude/rateLimit"]`, via `quota.observe`, called from `engine.session_manager`). This rarely carries `utilization`, so on its own Claude usage is almost always unknown.
- A lazy read of the installed Claude Code CLI's own local `/usage` command: `claude -p /usage --output-format json --restricted --strict-mcp-config`. Its subprocess environment comes from `connections.environment.build_process_env` -- the same helper `connections.account_identity` uses for its own CLI spawn -- which re-verifies the connection against the on-disk registry before trusting its `credential_ref` and sets `CLAUDE_CONFIG_DIR` to the connection's own credential directory (never the ambient one); `TZ=UTC` (so the printed reset times are parseable without a per-user timezone table) is applied on top, and the command runs from an empty temporary working directory with a bounded timeout. A registry-verification rejection is treated like any other CLI failure: the window stays unknown, negative-cached for the TTL. `--restricted --strict-mcp-config` are installed-CLI flags (verified via `claude --help`) that drop project/user settings and MCP servers for this one call without touching native-account auth.

`/usage` is a documented local command (`type: "local"`, `supportsNonInteractive: true`): it never starts a model turn. Its result is only ever trusted when the parsed JSON has `is_error: false`, `num_turns: 0` and `total_cost_usd: 0` (verified live on installed Claude Code 2.1.280); anything else is discarded and the affected window stays whatever it already was. The refresh is at most once per connection per 5 minutes (a failure is cached negatively for the same window, never falls back to a stale value beyond a window's own `resets_at` expiry), serialized by a per-connection file lock so concurrent readers never spawn it twice at once. The result text's `Current session`/`Current week (all models)`/`Current week (<Model>)` lines map to `five_hour`/`seven_day`/`seven_day_<model>`; an unparseable reset time leaves that window's `resets_at` unknown, never `0`.

The default (no-paid) pytest suite never spawns the real CLI: every call passes through `connections.quota._run_claude_usage_subprocess`, which `tests/conftest.py`'s `_forbid_real_claude_usage_cli` autouse fixture replaces with a failing stub for every test by default; a test that needs this path (see `tests/test_usage_accounting.py`) monkeypatches its own fake runner instead.

The automated tests verify per-call aggregation, cache-inclusive fallback totals, and unknown-call disclosure. They do not establish rendered Paseo behavior; that requires an isolated real-client observation.


## Model applicability is owner data

Reported window labels do not imply a model family. Routing consumers apply exact `quota_windows` associations from the winning routing file. Unmapped labels conservatively apply to every model; unknown numeric evidence is never zero. Shared and scoped windows keep their original timestamps, expiry and latest-observation/cache behavior. A window whose `resets_at` has passed no longer counts as used: it is unknown until the engine reports its replacement, and it is never treated as zero. Raw status lists all reported live windows without estimating model headroom. See [exact quota-window associations](../../configuration.md#exact-quota-window-associations). The implementation and fake CLI/cache tests do not establish a current live quota observation or production adoption.
