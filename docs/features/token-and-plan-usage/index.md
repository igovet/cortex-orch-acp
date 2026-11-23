# Token and provider plan usage

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed. The former Python runtime was removed on 2026-11-16: references to its modules and tests were dropped, and statements that cited them describe behavior carried from that implementation that has not been re-verified against the Rust runtime.


Cortex attempts one immutable `usage_records` insert per successfully sent,
task-bound engine ACP `session/prompt` frame. Coordinator and worker prompts,
continuations, and real retries are separate calls. Each row is tied to the
task, execution attempt, and configured session, with a sequence starting at 1
for that session. Launch/configuration without a sent prompt, taskless short
text, and model requests internal to an adapter are not counted.

Workspace task spend sums the reported token counts by configured engine and
model. It counts a settled prompt once even when that prompt also has a usage
row; a sent prompt without a settle event is counted from its row. A missing
or invalid report is **unknown**; an explicit reported zero is **0**. The
total uses `totalTokens` when reported, otherwise it adds the reported input,
output, cache-read and cache-write counters. Reasoning tokens are not added
again. A known subtotal does not establish total consumption or cost.

The Paseo plugin displays per-worker context occupancy (labelled **Context**) and token counts on worker cards. Without a valid context snapshot it says **Context unavailable**, independently of token totals. OpenCode workers instead explain **Context unknown until turn end**: the inspected OpenCode 2.2.0 adapter reports context only when the prompt finishes and its provider-error path skips that update. The explanation remains after an error without a snapshot; real context replaces it with the normal meter. Missing context is never zero or inferred from cumulative tokens.

The custom worker card and worker page also display **plan usage N% (at route selection)** from the retained routing decision, not live provider subscription windows. Unknown selection usage stays explicit; for OpenCode the text explains that provider plan usage is not reported to Cortex and suggests checking the provider's account usage page. No quota reader, credential access or network request is added by this display. Paseo's native provider schema exposes context and token counts but no plan-quota field or API for replacing the host's native Context window indicator. Source: `integrations/paseo/client/usage-meter.tsx`, `client/worker-display.ts` and `server/worker-history.ts` under the same plugin directory.

## Coverage and interrupted turns

The driver records only the normalized token report from the exact final
response it observes. Intermediate `usage_update.used/size` values describe
context occupancy, not expenditure. If the transport closes or the prompt is
interrupted without that response, the row has basis `unknown`, no token data
and no stop reason. A refused prompt with a valid final report still contributes
only one call. A worker can publish an accepted result before its adapter's final
response; losing the response records an unknown prompt, not a failed result.

Usage recording is best effort on a separate store transaction. An accounting
error logs a sanitized diagnostic and may leave a missing row; it does not
change settlement, re-dispatch a prompt, or roll back an accepted result.
The Rust driver adds no post-result settlement wait. Its existing active-worker
revocation loop polls every second, allows 10 seconds after first observing
revocation before requesting cancellation, then waits up to 25 seconds for the
prompt to end. Neither this accounting nor a missing response changes those
bounds, capacity release, or completion. Historical rows are not rewritten or
backfilled. Paseo's older reader still recognizes legacy non-failed
`released_after_result` rows; new interrupted prompts are not assigned that
legacy marker.

## Adapter token semantics

The runtime keeps recognized nonnegative, exactly representable integer
counters (`inputTokens`, `outputTokens`, `cachedReadTokens`,
`cachedWriteTokens`, `thoughtTokens`, `totalTokens`) from the final
`PromptResponse.usage` only. Unrecognized fields such as cost and cumulative
metadata do not enter the record. A recognized but invalid counter (including
a numeric string) invalidates the report. A reasoning counter on its own
does not establish a prompt total. What the reported counters cover is
adapter-specific. The following are historical bundle observations for the
named versions, not a claim of coverage for every installed adapter:

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

The service learns a Claude connection's weekly plan windows from the installed Claude Code CLI's own local `/usage` command: `claude -p /usage --output-format json --restricted --strict-mcp-config --no-session-persistence`. The executable is the verified native Claude Code of the qualified adapter that sessions run (the one `auth status` uses). It runs with a cleared environment: the owner's real `HOME`, absolute `PATH` entries, `LANG`/`LC_ALL` when set, `CLAUDE_CONFIG_DIR` set to the connection's own verified native directory (`~/.claude` when referenced, `claude-config` under the connection's private state when owned; a symlinked, shared, non-private or unexpected directory is never used) and `TZ=UTC`, so the printed reset times are parseable without a time zone table. It runs from an empty private working directory in its own process group, with a 20-second deadline after which the group is killed, and with stdin closed. No credential is copied and nothing logs in. `--restricted --strict-mcp-config --no-session-persistence` are installed-CLI flags (checked with `claude --help`) that drop project/user settings and MCP servers for this call and keep it out of saved sessions, without touching native-account authentication.

`/usage` is a local command: it starts no model turn. Its result is trusted only when the parsed JSON has `type: "result"`, `subtype: "success"`, `is_error: false`, `local_command: "usage"`, `num_turns: 0`, `total_cost_usd: 0`, all four token counts in `usage` equal to 0, `duration_api_ms` 0 when present and an empty `modelUsage` when present (checked on installed Claude Code 2.1.323 and 2.1.325, 2026-11-17). Anything else is discarded and every window stays as it was. Only exact lines `Current week (all models): N% used · resets Mon D, H[:MM]am (UTC)` and `Current week (<Model>): …` become windows: all models is `seven_day`, `Fable` is `seven_day_fable` (another plain model name maps the same way). The percentage must be 0 to 100 and the reset must be a real UTC date in the next eight days; a line that does not match exactly reports nothing for its window, never a guessed reset or zero, and a window reported twice discards the whole result. A `Current session` line and the tips that follow are ignored. A successful read is a complete snapshot: a window an earlier read wrote that this one no longer reports is removed; windows of other sources are kept, and a newer observation of a window is never replaced by an older one.

Each connection is read at most once per 5 minutes, failures included (`<state>/cortex/quota/<id>.cli-probe.json` records the last check), serialized by a nonblocking per-connection lock so concurrent processes never run it twice at once. Results go to the private `<state>/cortex/quota/<id>.json` cache (owner-only directory and file, atomic replacement); a cache that cannot be read safely is never overwritten. The read is started in the background by a settings read and never awaited: the first read starts a service loop that makes a pass every 15 seconds, reading due connections one after another, and stops after 15 minutes without a settings read; a connection is therefore read again five minutes and at most one pass (plus the reads before it) after its last read. While its window is visible, the Workspace app times its next settings read 45 seconds after the oldest shown snapshot is due again (asking again every 30 seconds for up to two minutes when it is late, and on its first read or on returning from a hidden window once more 30 seconds later for a snapshot already overdue), so a shown limit is at most about six minutes old and the app reads settings about once per snapshot. A connection that stops reporting falls back to the plain 5-minute read, never a faster one. `CORTEX_WORKSPACE_USAGE_REFRESH=off` disables the loop. No `rate_limit_event` from a session is recorded.

Settings carry each Claude weekly window by itself (`windows`: name, used percentage, reset time and observation time), next to the existing combined row. Workspace shows the general window and the Fable window separately, each "Not reported" when absent. Legacy `five_hour` windows are excluded from quota percentages, reset times, hot/exhausted routing decisions and current window display, even if an owner association names that window. Only the remaining reported windows count; when none are usable, usage stays unavailable, never inferred hot or zero. Real engine refusals still use the existing bounded availability handling.

The default (no-paid) suite never runs a real CLI: the parser, cadence, lock and cache rules are tested with fixture outputs and fake runners (`crates/workspace-service/tests/usage_refresh.rs`).

The automated tests verify per-call aggregation, cache-inclusive fallback totals, and unknown-call disclosure. They do not establish rendered Paseo behavior; that requires an isolated real-client observation.


## Codex weekly display

Workspace reads Codex's retained rollout rate-limit windows without a model call, and also refreshes them with the Codex app server's `account/rateLimits/read` account request, which starts no thread or turn (present in installed Codex 0.166.0; checked 2026-11-17: no rollout file was created). The service runs the verified native Codex launcher of the qualified adapter as `codex app-server` with the owner's real `HOME` and the connection's own verified `CODEX_HOME`, sends only `initialize`, `initialized` and `account/rateLimits/read` (with `excludeResetCreditDetails`), and kills the process group after the answer or a 20-second deadline. Only the documented `rateLimits` shape for limit id `codex` is trusted (`usedPercent` 0 to 100, positive `windowDurationMins`, integer `resetsAt`); an error or any other shape changes nothing, and an expired window is not reported. Its `primary`/`secondary` windows are cached under the same names, cadence, lock and private cache as Claude's. For display the newer observation wins: a rollout's own line timestamp against the read time. A cached value without a recorded read is never substituted. The quota reader preserves each window's reported `window_minutes` as optional `windowMinutes` display metadata. Only a positive integer duration of 10080 minutes qualifies for the weekly display, in either the `primary` or `secondary` slot. Slot names alone are insufficient: some accounts report their weekly limit as `primary` with no secondary window. Missing duration, short windows and expired windows are not presented as weekly usage. The original slot names and routing quota associations are unchanged; routing still reads the rollout.

## Model applicability is owner data

Reported window labels do not imply a model family. Routing consumers apply exact `quota_windows` associations from the winning routing file. Unmapped labels conservatively apply to every model; unknown numeric evidence is never zero. Shared and scoped windows keep their original timestamps, expiry and latest-observation/cache behavior. A window whose `resets_at` has passed no longer counts as used: it is unknown until the engine reports its replacement, and it is never treated as zero. Raw status lists remaining reported live windows (excluding Claude's retired session window and paid overage) without estimating model headroom. See [exact quota-window associations](../../configuration.md#exact-quota-window-associations). The implementation and fake CLI/cache tests do not establish a current live quota observation or production adoption.
